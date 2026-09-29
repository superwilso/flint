//! The window itself, on Windows, with no toolkit.
//!
//! **What this file is allowed to know.** How to make a window, how to draw a [`crate::paint::Cmd`]
//! with GDI, how to open a folder chooser, and how to run a job on another thread. It knows nothing
//! about syncing, planning or SensMe — those live in [`crate::job`], which is why they are testable
//! on a machine that has never run Windows.
//!
//! **Everything is owner-drawn.** There are no child controls at all: no buttons, no static text,
//! no edit boxes. `WM_PAINT` paints [`crate::layout`]'s widget list into a back buffer and blits it,
//! and `WM_LBUTTONUP` asks [`crate::hit`] what was under the pointer. The alternative — real child
//! controls, as `cinder-installer` uses — buys the system theme and keyboard handling, and costs a
//! second source of truth for where everything is. The installer is a wizard of stock controls and
//! wants the theme; this is one dense page whose layout must match its hit test exactly.
//!
//! **The worker threads.** A sync takes minutes and an analysis hours. Each job runs on its own
//! thread, owns nothing the UI thread touches, and posts [`WM_JOB`] to the window for every update;
//! the update itself travels through that job's mutex-guarded queue. The UI thread never blocks on
//! a worker, so the window keeps painting and STOP keeps answering while gigabytes move. Jobs that
//! share nothing run at once — which ones may is [`crate::Hold`]'s business, not this file's.
//!
//! **Typing.** Settings takes a Last.fm key and Check a filter, so the window has text fields —
//! also owner-drawn. `WM_CHAR` hands keys to [`crate::key`], Ctrl+V reads the clipboard into
//! [`crate::paste`], and that is all the editing there is: type, delete, paste, clear.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::job::{self, Settings, Update};
use crate::paint::{b_of, commands, g_of, r_of, Align, Cmd, Face, Theme};
use crate::{click, hit, layout, set_path, Id, Job, Key, Model, Nav, ThemePref, H, MIN_H, MIN_W, W};

// ── the Win32 surface, declared rather than depended on ────────────────────────────────────────

type Hwnd = *mut c_void;
type Hdc = *mut c_void;
type Hgdi = *mut c_void;
type Hinstance = *mut c_void;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RectW {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct WndClassEx {
    cb_size: u32,
    style: u32,
    wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Hinstance,
    icon: Hgdi,
    cursor: Hgdi,
    background: Hgdi,
    menu_name: *const u16,
    class_name: *const u16,
    icon_sm: Hgdi,
}

#[repr(C)]
struct PaintStruct {
    hdc: Hdc,
    erase: i32,
    paint: RectW,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct MinMaxInfo {
    reserved: Point,
    max_size: Point,
    max_position: Point,
    min_track_size: Point,
    max_track_size: Point,
}

#[repr(C)]
struct BrowseInfo {
    owner: Hwnd,
    root: *const c_void,
    display_name: *mut u16,
    title: *const u16,
    flags: u32,
    callback: Option<unsafe extern "system" fn(Hwnd, u32, Lparam, Lparam) -> i32>,
    lparam: Lparam,
    image: i32,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(class: *const WndClassEx) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        window: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Hgdi,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> Lresult;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> Lresult;
    fn PostQuitMessage(code: i32);
    fn PostMessageW(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Hdc;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetClientRect(hwnd: Hwnd, r: *mut RectW) -> i32;
    fn InvalidateRect(hwnd: Hwnd, r: *const RectW, erase: i32) -> i32;
    fn LoadCursorW(instance: Hinstance, name: *const u16) -> Hgdi;
    fn SetCursor(cursor: Hgdi) -> Hgdi;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, kind: u32) -> i32;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn FillRect(hdc: Hdc, r: *const RectW, brush: Hgdi) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn SetProcessDpiAwarenessContext(ctx: isize) -> i32;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn ScreenToClient(hwnd: Hwnd, p: *mut Point) -> i32;
    fn GetKeyState(key: i32) -> i16;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> Hgdi;
    fn CreatePen(style: i32, width: i32, color: u32) -> Hgdi;
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn CreateCompatibleBitmap(hdc: Hdc, w: i32, h: i32) -> Hgdi;
    fn SelectObject(hdc: Hdc, obj: Hgdi) -> Hgdi;
    fn DeleteObject(obj: Hgdi) -> i32;
    fn DeleteDC(hdc: Hdc) -> i32;
    fn BitBlt(dst: Hdc, x: i32, y: i32, w: i32, h: i32, src: Hdc, sx: i32, sy: i32, rop: u32) -> i32;
    fn RoundRect(hdc: Hdc, l: i32, t: i32, r: i32, b: i32, ew: i32, eh: i32) -> i32;
    fn Rectangle(hdc: Hdc, l: i32, t: i32, r: i32, b: i32) -> i32;
    fn Polyline(hdc: Hdc, pts: *const Point, count: i32) -> i32;
    fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
    fn SetTextColor(hdc: Hdc, color: u32) -> u32;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> Hgdi;
    fn GetStockObject(index: i32) -> Hgdi;
}

#[link(name = "shell32")]
extern "system" {
    fn SHBrowseForFolderW(bi: *mut BrowseInfo) -> *mut c_void;
    fn SHGetPathFromIDListW(list: *mut c_void, path: *mut u16) -> i32;
    fn ShellExecuteW(
        hwnd: Hwnd,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> *mut c_void;
}

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(hwnd: Hwnd) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut c_void;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(p: *mut c_void);
    fn CoInitialize(reserved: *mut c_void) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        key: *mut c_void,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

// DWM is loaded by hand rather than linked: the dark title bar attribute does not exist before
// Windows 10 1809, and a missing export in the import table stops the whole program from starting.
// A window with a light caption on an old build is a blemish; one that will not launch is not.
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GlobalLock(mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(mem: *mut c_void) -> i32;
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_SETCURSOR: u32 = 0x0020;
const WM_CLOSE: u32 = 0x0010;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_CHAR: u32 = 0x0102;
const WM_KEYDOWN: u32 = 0x0100;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_CAPTURECHANGED: u32 = 0x0215;
const MK_LBUTTON: usize = 0x0001;
const VK_CONTROL: i32 = 0x11;
/// Rows the wheel moves a list per notch, as Windows does by default.
const WHEEL_ROWS: i32 = 3;
/// A worker thread has put something in its queue.
const WM_JOB: u32 = 0x8000 + 1;
/// A worker thread has finished. `wparam` is its job's [`Job::code`].
const WM_JOB_DONE: u32 = 0x8000 + 2;
const CF_UNICODETEXT: u32 = 13;
const SW_SHOWNORMAL: i32 = 1;

const SW_SHOW: i32 = 5;
const SRCCOPY: u32 = 0x00CC_0020;
const TRANSPARENT: i32 = 1;
const PS_SOLID: i32 = 0;
const NULL_BRUSH: i32 = 5;
const NULL_PEN: i32 = 8;
const IDC_ARROW: usize = 32512;
const IDC_HAND: usize = 32649;
const MB_ICONERROR: u32 = 0x0010;
const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
const BIF_NEWDIALOGSTYLE: u32 = 0x0040;
const DPI_AWARENESS_PER_MONITOR_V2: isize = -4;
/// Sent when Windows changes a system-wide setting; `lparam` names which one. Light/dark arrives
/// as `"ImmersiveColorSet"`.
const WM_SETTINGCHANGE: u32 = 0x001A;
const HKEY_CURRENT_USER: *mut c_void = 0x8000_0001u32 as usize as *mut c_void;
const RRF_RT_REG_DWORD: u32 = 0x0000_0018;
/// `DWMWA_USE_IMMERSIVE_DARK_MODE`. 20 since Windows 10 20H1; 19 on 1809/1903/1909, where the same
/// call with 20 is silently ignored — so both are sent and the one that applies wins.
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_USE_IMMERSIVE_DARK_MODE_OLD: u32 = 19;

/// GDI wants `0x00BBGGRR`; [`crate::paint`] speaks `0xRRGGBB`. The whole of the conversion.
fn colorref(c: u32) -> u32 {
    (r_of(c) as u32) | ((g_of(c) as u32) << 8) | ((b_of(c) as u32) << 16)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Does Windows itself want dark applications? The one place that answers it is the personalisation
/// key, which is also what File Explorer and the Settings app read. `AppsUseLightTheme` is 0 for
/// dark and 1 for light, and the value is ABSENT on a machine that has never chosen — which is the
/// light default, so every failure here reads as light.
fn system_prefers_dark() -> bool {
    const SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
    let subkey = wide(SUBKEY);
    let value = wide("AppsUseLightTheme");
    let mut data: u32 = 1;
    let mut size: u32 = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    rc == 0 && data == 0
}

/// The theme to paint with: what the person asked for on the command line, or what Windows wants.
fn theme_for(pref: Option<bool>) -> Theme {
    if pref.unwrap_or_else(system_prefers_dark) {
        Theme::dark()
    } else {
        Theme::light()
    }
}

/// Dark or light, for this window now: `--dark`/`--light` for this run first, then Settings ▸
/// Theme, then — for System — whatever Windows wants.
fn wants_dark(state: &State) -> bool {
    match (state.theme_pref, state.model.theme) {
        (Some(d), _) => d,
        (None, ThemePref::Dark) => true,
        (None, ThemePref::Light) => false,
        (None, ThemePref::System) => system_prefers_dark(),
    }
}

/// Settings ▸ Theme changed: repaint in the new theme and match the title bar to it. A choice made
/// in the window also replaces a `--dark`/`--light` given for this run — it is the newer word.
fn apply_theme(hwnd: Hwnd) {
    let dark = STATE.with(|st| {
        let mut b = st.borrow_mut();
        let state = b.as_mut()?;
        state.theme_pref = None;
        let dark = wants_dark(state);
        state.theme = if dark { Theme::dark() } else { Theme::light() };
        Some(dark)
    });
    if let Some(dark) = dark {
        set_caption_dark(hwnd, dark);
    }
}

/// Remember the folders, the switches and the theme. Best-effort: a window that cannot write its
/// preferences still works, it just asks again next time.
fn save_prefs(m: &Model) {
    let _ = crate::prefs::save(m, &crate::prefs::path());
}

/// Ask DWM for a dark title bar. Best-effort in every direction: the DLL may not be there, the
/// attribute may not exist, and the call may simply do nothing. The window is correct either way.
fn set_caption_dark(hwnd: Hwnd, dark: bool) {
    unsafe {
        let name = wide("dwmapi.dll");
        let dll = LoadLibraryW(name.as_ptr());
        if dll.is_null() {
            return;
        }
        let proc = GetProcAddress(dll, b"DwmSetWindowAttribute\0".as_ptr());
        if proc.is_null() {
            return;
        }
        let set: unsafe extern "system" fn(Hwnd, u32, *const c_void, u32) -> i32 = std::mem::transmute(proc);
        let on: i32 = i32::from(dark);
        let p: *const c_void = (&on as *const i32).cast();
        set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, p, 4);
        set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE_OLD, p, 4);
    }
}

// ── the window's state ─────────────────────────────────────────────────────────────────────────

/// Shared between the UI thread and one worker. The worker only ever pushes to `queue` and sets
/// `result`; everything else is the UI thread's.
struct Shared {
    queue: Mutex<Vec<Update>>,
    /// The worker's last word: `Ok(summary)` or `Err(reason)`.
    result: Mutex<Option<Result<String, String>>>,
    cancel: AtomicBool,
}

struct State {
    model: Model,
    theme: Theme,
    /// `Some(true)`/`Some(false)` when `--dark`/`--light` was asked for, `None` to follow Windows.
    theme_pref: Option<bool>,
    /// Logical (96-DPI) size; the DPI scale is applied at paint time.
    size: (i32, i32),
    dpi: u32,
    /// What the pointer is over, so a button can light up under it.
    hot: Option<Id>,
    /// What the pointer went down on, so a press that slides off does not fire.
    down: Option<Id>,
    /// A scrollbar thumb being dragged, and how far below its top it was caught.
    drag: Option<(crate::Area, i32)>,
    /// Wheel movement not yet a whole row: a touchpad sends it in small pieces.
    wheel: i32,
    /// One per running job.
    workers: Vec<(Job, Arc<Shared>)>,
    fonts: Vec<(Face, Hgdi)>,
    settings_cache: Option<PathBuf>,
    /// Close was pressed while jobs ran: they were asked to stop, and the window closes when the
    /// last one has.
    closing: bool,
}

impl State {
    /// Logical pixels to physical, for this window's DPI. Everything above this layer is in
    /// logical pixels; only `paint` and the hit test convert.
    fn scale(&self, v: i32) -> i32 {
        (v as i64 * self.dpi as i64 / 96) as i32
    }

    fn unscale(&self, v: i32) -> i32 {
        (v as i64 * 96 / self.dpi as i64) as i32
    }

    /// What a job needs. `None` when it needs a library and there is none; the jobs that never
    /// read the library run with an empty path they never look at.
    fn settings(&mut self, job: Job) -> Option<Settings> {
        let library = match (&self.model.library, job) {
            (Some(l), _) => l.clone(),
            (None, j) if !j.reads_library() && j != Job::Import => PathBuf::new(),
            (None, _) => return None,
        };
        let mut s = Settings::new(library);
        s.volumes = self.model.volumes.iter().flatten().cloned().collect();
        s.playlists = self.model.playlists.clone();
        s.sensme = self.model.sensme;
        s.extras = self.model.extras;
        s.internal = self.model.volumes[0].clone();
        s.palette_dir = self.model.palette_dir.clone();
        if self.model.draft.open {
            s.palette_draft = Some((self.model.draft.file(), self.model.draft.body()));
            s.palette_saved = self.model.draft.saved.clone();
        }
        if let Some(p) = self.model.shop.installing.and_then(|i| self.model.shop.items.get(i)) {
            s.shop_install = Some((p.file.clone(), p.body.clone()));
        }
        s.api_key = self.model.key_input.clone();
        s.api_secret = self.model.secret_input.clone();
        if let Some(dir) = &self.settings_cache {
            s.cache = dir.clone();
        }
        Some(s)
    }
}

// The window's state lives here rather than in `GWLP_USERDATA`, because there is exactly one
// window and a thread-local is less unsafe code than a pointer round-trip through
// `SetWindowLongPtr`.
thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

// ── painting ───────────────────────────────────────────────────────────────────────────────────

fn font_for(state: &State, face: Face) -> Hgdi {
    state.fonts.iter().find(|(f, _)| *f == face).map(|(_, h)| *h).unwrap_or(std::ptr::null_mut())
}

/// Draw one command. Every GDI object created here is selected out and deleted before returning,
/// which is the rule that stops a long-running window leaking handles until it cannot paint.
unsafe fn draw(hdc: Hdc, state: &State, cmd: &Cmd) {
    let s = |v: i32| state.scale(v);
    match cmd {
        Cmd::Rect { rect, fill, border, radius } => {
            let (l, t, r, b) = (s(rect.x), s(rect.y), s(rect.right()), s(rect.bottom()));
            let brush = match fill {
                Some(c) => CreateSolidBrush(colorref(*c)),
                None => GetStockObject(NULL_BRUSH),
            };
            let pen = match border {
                Some(c) => CreatePen(PS_SOLID, 1, colorref(*c)),
                None => GetStockObject(NULL_PEN),
            };
            let old_b = SelectObject(hdc, brush);
            let old_p = SelectObject(hdc, pen);
            if *radius > 0 {
                let d = s(*radius) * 2;
                RoundRect(hdc, l, t, r, b, d, d);
            } else if border.is_none() {
                // A plain fill: `FillRect` is exact, where `Rectangle` leaves the last row and
                // column to the pen.
                let rw = RectW { left: l, top: t, right: r, bottom: b };
                FillRect(hdc, &rw, brush);
            } else {
                Rectangle(hdc, l, t, r, b);
            }
            SelectObject(hdc, old_b);
            SelectObject(hdc, old_p);
            if fill.is_some() {
                DeleteObject(brush);
            }
            if border.is_some() {
                DeleteObject(pen);
            }
        }
        Cmd::Text { rect, text, color, face, align } => {
            let font = font_for(state, *face);
            let old = SelectObject(hdc, font);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, colorref(*color));
            let mut rw = RectW { left: s(rect.x), top: s(rect.y), right: s(rect.right()), bottom: s(rect.bottom()) };
            let w = wide(text);
            // DT_VCENTER|DT_SINGLELINE centres in the rect; DT_END_ELLIPSIS is belt and braces
            // over `paint::elide_*`, for the case where the real face is wider than the estimate.
            const DT_LEFT: u32 = 0x0000;
            const DT_CENTER: u32 = 0x0001;
            const DT_RIGHT: u32 = 0x0002;
            const DT_VCENTER: u32 = 0x0004;
            const DT_SINGLELINE: u32 = 0x0020;
            const DT_END_ELLIPSIS: u32 = 0x8000;
            const DT_NOPREFIX: u32 = 0x0800;
            let flags = DT_VCENTER
                | DT_SINGLELINE
                | DT_END_ELLIPSIS
                | DT_NOPREFIX
                | match align {
                    Align::Center => DT_CENTER,
                    Align::Right => DT_RIGHT,
                    Align::Left => DT_LEFT,
                };
            #[link(name = "user32")]
            extern "system" {
                fn DrawTextW(hdc: Hdc, text: *const u16, count: i32, r: *mut RectW, format: u32) -> i32;
            }
            DrawTextW(hdc, w.as_ptr(), -1, &mut rw, flags);
            SelectObject(hdc, old);
        }
        Cmd::Poly { points, color, width } => {
            let pen = CreatePen(PS_SOLID, s(*width).max(1), colorref(*color));
            let old = SelectObject(hdc, pen);
            let pts: Vec<Point> = points.iter().map(|(x, y)| Point { x: s(*x), y: s(*y) }).collect();
            Polyline(hdc, pts.as_ptr(), pts.len() as i32);
            SelectObject(hdc, old);
            DeleteObject(pen);
        }
    }
}

/// Paint the whole window into a back buffer and blit it. Drawing straight onto the window's DC
/// flickers badly on a page this dense, and `WM_ERASEBKGND` returning 1 (below) means nothing else
/// ever clears it.
unsafe fn paint(hwnd: Hwnd) {
    let mut ps: PaintStruct = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    STATE.with(|st| {
        let st = st.borrow();
        let Some(state) = st.as_ref() else { return };
        let (w, h) = state.size;
        let (pw, ph) = (state.scale(w), state.scale(h));
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, pw, ph);
        let old = SelectObject(mem, bmp);
        let cmds = commands(&state.model, w, h, &state.theme);
        for cmd in &cmds {
            draw(mem, state, cmd);
        }
        // The hover highlight: a hairline under the control the pointer is over. Drawn here rather
        // than in `paint::commands` because it is not part of the window's state — it is where the
        // mouse happens to be, and the SVG preview has no mouse.
        if let Some(hot) = state.hot {
            if let Some(wid) = layout(&state.model, w, h).into_iter().find(|x| x.id == hot) {
                let c = colorref(state.theme.accent);
                let pen = CreatePen(PS_SOLID, state.scale(2).max(1), c);
                let oldp = SelectObject(mem, pen);
                let y = state.scale(wid.rect.bottom());
                let pts = [Point { x: state.scale(wid.rect.x), y }, Point { x: state.scale(wid.rect.right()), y }];
                Polyline(mem, pts.as_ptr(), 2);
                SelectObject(mem, oldp);
                DeleteObject(pen);
            }
        }
        BitBlt(hdc, 0, 0, pw, ph, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
    });
    EndPaint(hwnd, &ps);
}

// ── the folder chooser ─────────────────────────────────────────────────────────────────────────

/// `SHBrowseForFolderW`, which is the one folder picker that needs no COM object graph and no
/// extra library. It is older than `IFileDialog`, and for "point at a folder" it is the same thing
/// with less code between the user and the answer.
unsafe fn pick_folder(owner: Hwnd, title: &str) -> Option<PathBuf> {
    let title = wide(title);
    let mut name = [0u16; 260];
    let mut bi = BrowseInfo {
        owner,
        root: std::ptr::null(),
        display_name: name.as_mut_ptr(),
        title: title.as_ptr(),
        flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        callback: None,
        lparam: 0,
        image: 0,
    };
    let list = SHBrowseForFolderW(&mut bi);
    if list.is_null() {
        return None;
    }
    let mut path = [0u16; 260];
    let ok = SHGetPathFromIDListW(list, path.as_mut_ptr());
    CoTaskMemFree(list);
    if ok == 0 {
        return None;
    }
    let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Some(PathBuf::from(String::from_utf16_lossy(&path[..len])))
}

// ── the workers ────────────────────────────────────────────────────────────────────────────

fn start(hwnd: Hwnd, job: Job, settings: Settings) -> Arc<Shared> {
    let shared =
        Arc::new(Shared { queue: Mutex::new(Vec::new()), result: Mutex::new(None), cancel: AtomicBool::new(false) });
    let theirs = shared.clone();
    // `Hwnd` is a raw pointer, which is not `Send`; the handle itself is fine to use from another
    // thread (that is what `PostMessageW` is for), so it crosses as an integer.
    let hwnd = hwnd as usize;
    std::thread::spawn(move || {
        let out = job::run(job, &settings, &theirs.cancel, &mut |u| {
            theirs.queue.lock().unwrap().push(u);
            unsafe { PostMessageW(hwnd as Hwnd, WM_JOB, 0, 0) };
        });
        *theirs.result.lock().unwrap() = Some(out);
        unsafe { PostMessageW(hwnd as Hwnd, WM_JOB_DONE, job.code(), 0) };
    });
    shared
}

/// Drain everything every worker has queued into the model. Called on the UI thread only. Pages
/// to open are returned rather than opened here, because this runs inside the state's borrow.
fn drain(state: &mut State) -> Vec<String> {
    let mut open = Vec::new();
    for (job, shared) in &state.workers {
        let updates: Vec<Update> = std::mem::take(&mut *shared.queue.lock().unwrap());
        for u in updates {
            if let Update::Open(url) = &u {
                open.push(url.clone());
            }
            crate::update(&mut state.model, *job, u);
        }
    }
    open
}

/// Hand `url` to whatever the person uses for web pages.
fn open_url(hwnd: Hwnd, url: &str) {
    let (op, file) = (wide("open"), wide(url));
    unsafe {
        ShellExecuteW(hwnd, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

/// The clipboard's text, if it holds any.
fn clipboard_text(hwnd: Hwnd) -> Option<String> {
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return None;
        }
        let mut out = None;
        let handle = GetClipboardData(CF_UNICODETEXT);
        if !handle.is_null() {
            let p = GlobalLock(handle) as *const u16;
            if !p.is_null() {
                let mut n = 0usize;
                while n < 65_536 && *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        out
    }
}

/// A message box with Flint's name on it.
pub fn message(text: &str) {
    let (text, cap) = (wide(text), wide("Flint"));
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), cap.as_ptr(), MB_ICONERROR) };
}

/// Start `job` if it may start, and repaint.
fn launch(hwnd: Hwnd, job: Job) {
    let settings = STATE.with(|st| {
        let mut b = st.borrow_mut();
        let state = b.as_mut()?;
        if !state.model.can_start(job) {
            return None;
        }
        let s = state.settings(job)?;
        crate::started(&mut state.model, job);
        Some(s)
    });
    if let Some(settings) = settings {
        let shared = start(hwnd, job, settings);
        STATE.with(|st| {
            if let Some(state) = st.borrow_mut().as_mut() {
                state.workers.push((job, shared));
            }
        });
    }
}

/// A click that landed on `id`, outside the state's borrow: a folder chooser and the browser both
/// run message loops of their own.
unsafe fn press(hwnd: Hwnd, id: Id) {
    let url = STATE.with(|st| st.borrow().as_ref().and_then(|state| crate::url_for(&state.model, id)));
    if let Some(url) = url {
        open_url(hwnd, &url);
        return;
    }
    match id {
        Id::PickLibrary | Id::PickVolume(_) | Id::PickPlaylists | Id::PickPalettes => {
            let title = match id {
                Id::PickLibrary => "Where is your music?",
                Id::PickPlaylists => "Where are your playlists?",
                Id::PickPalettes => "Where are your .palette files?",
                _ => "Which drive is the player?",
            };
            if let Some(path) = pick_folder(hwnd, title) {
                STATE.with(|st| {
                    if let Some(state) = st.borrow_mut().as_mut() {
                        // Nothing can start while the chooser is up, but this is one line to be sure.
                        if crate::live(&state.model, id) {
                            set_path(&mut state.model, id, path);
                            save_prefs(&state.model);
                        }
                    }
                });
            }
        }
        Id::Stop => STATE.with(|st| {
            if let Some(state) = st.borrow_mut().as_mut() {
                let Some(job) = state.model.stop_target(state.model.tab) else { return };
                for (j, shared) in &state.workers {
                    if *j == job {
                        shared.cancel.store(true, Ordering::Relaxed);
                    }
                }
                if let Some(r) = state.model.running.iter_mut().find(|r| r.job == job) {
                    r.status = "Stopping…".into();
                }
            }
        }),
        _ => {
            let (job, retheme) = STATE.with(|st| {
                let mut b = st.borrow_mut();
                let Some(state) = b.as_mut() else { return (None, false) };
                let before = state.model.theme;
                let job = click(&mut state.model, id);
                save_prefs(&state.model);
                (job, state.model.theme != before)
            });
            if retheme {
                apply_theme(hwnd);
            }
            if let Some(job) = job {
                launch(hwnd, job);
            }
        }
    }
}

// ── the window procedure ───────────────────────────────────────────────────────────────────────

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> Lresult {
    match msg {
        WM_ERASEBKGND => 1, // the back buffer covers every pixel; erasing first only flickers
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_SIZE => {
            STATE.with(|st| {
                if let Some(state) = st.borrow_mut().as_mut() {
                    let mut r = RectW::default();
                    GetClientRect(hwnd, &mut r);
                    state.dpi = GetDpiForWindow(hwnd).max(96);
                    state.size =
                        (state.unscale(r.right - r.left).max(MIN_W), state.unscale(r.bottom - r.top).max(MIN_H));
                }
            });
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_SETTINGCHANGE => {
            // Windows sends this for every system setting, so check it is ours before re-reading
            // the registry: `lparam` points at a wide string naming the change.
            let mut ours = false;
            if l != 0 {
                let p = l as *const u16;
                let mut n = 0isize;
                let mut name = String::new();
                while n < 64 && *p.offset(n) != 0 {
                    name.push(char::from_u32(*p.offset(n) as u32).unwrap_or('?'));
                    n += 1;
                }
                ours = name == "ImmersiveColorSet";
            }
            if ours {
                let changed = STATE.with(|st| {
                    let mut b = st.borrow_mut();
                    let Some(state) = b.as_mut() else { return None };
                    let dark = wants_dark(state);
                    let next = if dark { Theme::dark() } else { Theme::light() };
                    if next.bg == state.theme.bg {
                        return None;
                    }
                    state.theme = next;
                    Some(dark)
                });
                if let Some(dark) = changed {
                    set_caption_dark(hwnd, dark);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_GETMINMAXINFO => {
            let mmi = l as *mut MinMaxInfo;
            STATE.with(|st| {
                let dpi = st.borrow().as_ref().map_or(96, |s| s.dpi);
                let s = |v: i32| (v as i64 * dpi as i64 / 96) as i32;
                // Plus the frame: a client area of MIN_W needs a window a little wider. 24/48 is
                // the ordinary frame on a themed window and being a few pixels out here only means
                // the window cannot quite be made as small as it could be.
                (*mmi).min_track_size = Point { x: s(MIN_W) + 24, y: s(MIN_H) + 48 };
            });
            0
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let buttons = w;
            let x = (l & 0xFFFF) as i16 as i32;
            let y = ((l >> 16) & 0xFFFF) as i16 as i32;
            let mut pressed: Option<Id> = None;
            let mut repaint = false;
            STATE.with(|st| {
                let mut st = st.borrow_mut();
                let Some(state) = st.as_mut() else { return };
                let (lx, ly) = (state.unscale(x), state.unscale(y));
                let (w, h) = state.size;
                // A thumb being dragged owns the pointer until the button comes up.
                if let Some((area, grab)) = state.drag {
                    match msg {
                        // The button came up where this window did not hear it: the drag is over.
                        WM_MOUSEMOVE if buttons & MK_LBUTTON == 0 => {
                            state.drag = None;
                            ReleaseCapture();
                        }
                        WM_MOUSEMOVE => repaint = crate::drag_bar(&mut state.model, w, h, area, grab, ly),
                        WM_LBUTTONUP => {
                            state.drag = None;
                            ReleaseCapture();
                        }
                        _ => {}
                    }
                    return;
                }
                if msg == WM_LBUTTONDOWN {
                    if let Some(caught) = crate::press_bar(&mut state.model, w, h, lx, ly) {
                        if caught.is_some() {
                            state.drag = caught;
                            SetCapture(hwnd);
                        }
                        repaint = true;
                        return;
                    }
                }
                let over = hit(&state.model, w, h, lx, ly);
                if over != state.hot {
                    state.hot = over;
                    repaint = true;
                }
                match msg {
                    WM_LBUTTONDOWN => {
                        state.down = over;
                        // A press on nothing takes the caret out of a field, as it does anywhere else.
                        if over.is_none() && state.model.focus.take().is_some() {
                            repaint = true;
                        }
                    }
                    // A press that slid off its control does nothing, which is what every other
                    // button on the system does.
                    WM_LBUTTONUP => pressed = state.down.take().filter(|d| Some(*d) == over),
                    _ => {}
                }
            });
            if let Some(id) = pressed {
                press(hwnd, id);
                repaint = true;
            }
            if repaint {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        // Capture taken away mid-drag — Alt+Tab, or a job's message box — ends the drag.
        //
        // SetCapture and ReleaseCapture send this synchronously, from inside the state's borrow in
        // the mouse handler — which has already set `drag` itself — so a borrow that is taken is
        // left alone rather than panicking.
        WM_CAPTURECHANGED => {
            STATE.with(|st| {
                if let Ok(mut b) = st.try_borrow_mut() {
                    if let Some(state) = b.as_mut() {
                        state.drag = None;
                    }
                }
            });
            0
        }
        WM_MOUSEWHEEL => {
            // The delta is in 120ths of a notch, up positive; the point is on the screen.
            let delta = ((w >> 16) & 0xFFFF) as i16 as i32;
            let mut p = Point { x: (l & 0xFFFF) as i16 as i32, y: ((l >> 16) & 0xFFFF) as i16 as i32 };
            ScreenToClient(hwnd, &mut p);
            let moved = STATE.with(|st| {
                let mut b = st.borrow_mut();
                let Some(state) = b.as_mut() else { return false };
                let (lx, ly) = (state.unscale(p.x), state.unscale(p.y));
                // Rows in whole numbers, the remainder kept for the next message: a notch is 120,
                // and a precision touchpad sends a few units at a time.
                state.wheel += -delta * WHEEL_ROWS;
                let rows = state.wheel / 120;
                state.wheel -= rows * 120;
                let (w, h) = state.size;
                rows != 0 && crate::wheel(&mut state.model, w, h, lx, ly, rows)
            });
            if moved {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            // The keys that move a list. They make no WM_CHAR, so a field with the caret in it is
            // not in their way — except Home and End, which a field may want one day; they wait
            // until nothing is being typed into.
            let k = match w as u32 {
                0x21 => Some(Nav::PageUp),
                0x22 => Some(Nav::PageDown),
                0x23 => Some(Nav::End),
                0x24 => Some(Nav::Home),
                0x26 => Some(Nav::Up),
                0x28 => Some(Nav::Down),
                _ => None,
            };
            let ctrl = GetKeyState(VK_CONTROL) < 0;
            let moved = k.is_some_and(|k| {
                STATE.with(|st| {
                    let mut b = st.borrow_mut();
                    let Some(state) = b.as_mut() else { return false };
                    if state.model.focus.is_some() && matches!(k, Nav::Home | Nav::End) && !ctrl {
                        return false;
                    }
                    let (w, h) = state.size;
                    crate::nav(&mut state.model, w, h, k)
                })
            });
            if moved {
                InvalidateRect(hwnd, std::ptr::null(), 0);
                return 0;
            }
            DefWindowProcW(hwnd, msg, w, l)
        }
        WM_CHAR => {
            // Keys arrive as characters: `TranslateMessage` has already turned Backspace, Enter,
            // Tab, Esc and the Ctrl chords into their control codes. A character outside the basic
            // plane comes as two surrogate halves, neither a `char`, and is dropped — nothing typed
            // into a key or a filter needs one.
            let code = w as u32;
            if code == 0x16 {
                // Ctrl+V. The clipboard is read outside the state's borrow: opening it can pump
                // messages, and a message that lands here while the state is borrowed would panic.
                if let Some(text) = clipboard_text(hwnd) {
                    STATE.with(|st| {
                        if let Some(state) = st.borrow_mut().as_mut() {
                            crate::paste(&mut state.model, &text);
                        }
                    });
                }
            } else {
                let k = match code {
                    0x08 => Some(Key::Backspace),
                    0x7F => Some(Key::Clear), // Ctrl+Backspace
                    0x0D => Some(Key::Enter),
                    0x09 => Some(Key::Tab),
                    0x1B => Some(Key::Escape),
                    c if c < 0x20 => None,
                    c => char::from_u32(c).map(Key::Char),
                };
                let job = k.and_then(|k| {
                    STATE.with(|st| st.borrow_mut().as_mut().and_then(|state| crate::key(&mut state.model, k)))
                });
                if let Some(job) = job {
                    launch(hwnd, job);
                }
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_SETCURSOR => {
            // A hand over anything clickable. `WM_SETCURSOR` fires before `WM_MOUSEMOVE`, so it
            // reads `hot` from the previous move, which is a pixel behind and never wrong for long.
            let hot = STATE.with(|st| st.borrow().as_ref().and_then(|s| s.hot));
            match hot {
                Some(_) => {
                    SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND as *const u16));
                    1
                }
                None => DefWindowProcW(hwnd, msg, w, l),
            }
        }
        WM_JOB => {
            let open = STATE.with(|st| st.borrow_mut().as_mut().map(drain).unwrap_or_default());
            for url in open {
                open_url(hwnd, &url);
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_JOB_DONE => {
            let Some(job) = Job::from_code(w) else { return 0 };
            let (open, failed, close) = STATE.with(|st| {
                let mut b = st.borrow_mut();
                let Some(state) = b.as_mut() else { return (Vec::new(), None, false) };
                // Whatever it said last, before the word it finished with.
                let open = drain(state);
                let mut failed = None;
                if let Some(i) = state.workers.iter().position(|(j, _)| *j == job) {
                    let (_, shared) = state.workers.remove(i);
                    let result = shared.result.lock().unwrap().take();
                    let result = result.unwrap_or_else(|| Err("the job ended without saying how".into()));
                    failed = crate::finished(&mut state.model, job, result);
                }
                (open, failed, state.closing && state.workers.is_empty())
            });
            if close {
                DestroyWindow(hwnd);
                return 0;
            }
            for url in open {
                open_url(hwnd, &url);
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            // Outside the borrow: a message box runs its own message loop, which paints this
            // window and delivers the other jobs' updates while it is up.
            if let Some(why) = failed {
                let (text, cap) = (wide(&why), wide("Flint"));
                MessageBoxW(hwnd, text.as_ptr(), cap.as_ptr(), MB_ICONERROR);
            }
            0
        }
        WM_CLOSE => {
            // Jobs in flight are asked to stop and the window waits for the last of them, rather
            // than the process exiting mid-copy. `apply` renames every copy into place, so the
            // worst a stop leaves is a file not copied yet — never a half-written one.
            let busy = STATE.with(|st| {
                let mut b = st.borrow_mut();
                let Some(state) = b.as_mut() else { return false };
                if state.workers.is_empty() {
                    return false;
                }
                for (_, shared) in &state.workers {
                    shared.cancel.store(true, Ordering::Relaxed);
                }
                for r in &mut state.model.running {
                    r.status = "Stopping before closing…".into();
                }
                state.closing = true;
                true
            });
            if busy {
                InvalidateRect(hwnd, std::ptr::null(), 0);
                return 0;
            }
            DefWindowProcW(hwnd, msg, w, l)
        }
        WM_DESTROY => {
            STATE.with(|st| {
                if let Some(state) = st.borrow_mut().take() {
                    for (_, f) in state.fonts {
                        DeleteObject(f);
                    }
                }
            });
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

/// Open the window and run until it closes.
pub fn run() -> Result<(), String> {
    run_with(None)
}

/// `pref` is `Some(true)` for dark, `Some(false)` for light, `None` to follow Windows — and to keep
/// following it, because the window re-reads the setting on `WM_SETTINGCHANGE`.
pub fn run_with(pref: Option<bool>) -> Result<(), String> {
    unsafe {
        // Per-monitor v2 so the window is sharp on a scaled display; failing is not fatal, it just
        // means Windows stretches the 96-DPI rendering, which is blurry but usable.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_PER_MONITOR_V2);
        CoInitialize(std::ptr::null_mut());

        let class = wide("FlintWindow");
        let title = wide(&format!("Flint {}", env!("CARGO_PKG_VERSION")));
        let wc = WndClassEx {
            cb_size: std::mem::size_of::<WndClassEx>() as u32,
            style: 0x0002 | 0x0001, // CS_HREDRAW | CS_VREDRAW
            wnd_proc: Some(wnd_proc),
            cls_extra: 0,
            wnd_extra: 0,
            instance: std::ptr::null_mut(),
            icon: std::ptr::null_mut(),
            cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            icon_sm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("could not register the window class".into());
        }

        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0x8000_0000u32 as i32, // CW_USEDEFAULT
            0x8000_0000u32 as i32,
            W + 24,
            H + 48,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return Err("could not create the window".into());
        }

        let dpi = GetDpiForWindow(hwnd).max(96);
        let fonts = [Face::Title, Face::Figure, Face::Path, Face::Small, Face::Body, Face::Strong, Face::Mono]
            .into_iter()
            .map(|f| {
                let name = wide(if f.mono() { "Consolas" } else { "Segoe UI" });
                let height = -(f.px() as i64 * dpi as i64 / 96) as i32;
                let weight = if f.bold() { 600 } else { 400 };
                const CLEARTYPE_QUALITY: u32 = 5;
                let h = CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, CLEARTYPE_QUALITY, 0, name.as_ptr());
                (f, h)
            })
            .collect();

        // The window opens where it was left: folders, switches and theme from `gui.conf`.
        let mut model = Model::new();
        crate::prefs::load(&mut model, &crate::prefs::path());
        model.cache_dir = flint_core::cache::default_dir().display().to_string();
        model.lastfm = crate::lastfm_facts(&flint_core::lastfm::Credentials::load());
        let pref = pref.or(match model.theme {
            ThemePref::Dark => Some(true),
            ThemePref::Light => Some(false),
            ThemePref::System => None,
        });
        STATE.with(|st| {
            *st.borrow_mut() = Some(State {
                model,
                theme: theme_for(pref),
                theme_pref: pref,
                size: (W, H),
                dpi,
                hot: None,
                down: None,
                drag: None,
                wheel: 0,
                workers: Vec::new(),
                fonts,
                settings_cache: None,
                closing: false,
            });
        });

        SetWindowTextW(hwnd, title.as_ptr());
        set_caption_dark(hwnd, pref.unwrap_or_else(system_prefers_dark));
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
