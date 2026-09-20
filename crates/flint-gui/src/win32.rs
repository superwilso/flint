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
//! **The worker thread.** A sync takes minutes. It runs on its own thread, owns nothing the UI
//! thread touches, and posts [`WM_JOB`] to the window for every update; the update itself travels
//! through a mutex-guarded queue. The UI thread never blocks on the worker, so the window keeps
//! painting and STOP keeps answering while gigabytes move.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::job::{self, Settings, Update};
use crate::paint::{b_of, commands, g_of, r_of, Align, Cmd, Face, Theme};
use crate::{click, hit, layout, set_path, Id, Job, Model, Phase, H, MIN_H, MIN_W, W};

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
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(p: *mut c_void);
    fn CoInitialize(reserved: *mut c_void) -> i32;
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
/// The worker thread has put something in the queue.
const WM_JOB: u32 = 0x8000 + 1;
/// The worker thread has finished.
const WM_JOB_DONE: u32 = 0x8000 + 2;

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

/// GDI wants `0x00BBGGRR`; [`crate::paint`] speaks `0xRRGGBB`. The whole of the conversion.
fn colorref(c: u32) -> u32 {
    (r_of(c) as u32) | ((g_of(c) as u32) << 8) | ((b_of(c) as u32) << 16)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ── the window's state ─────────────────────────────────────────────────────────────────────────

/// Shared between the UI thread and the worker. The worker only ever pushes to `queue` and sets
/// `done`; everything else is the UI thread's.
struct Shared {
    queue: Mutex<Vec<Update>>,
    /// The worker's last word: `Ok(summary)` or `Err(reason)`.
    result: Mutex<Option<Result<String, String>>>,
    cancel: AtomicBool,
}

struct State {
    model: Model,
    theme: Theme,
    /// Logical (96-DPI) size; the DPI scale is applied at paint time.
    size: (i32, i32),
    dpi: u32,
    /// What the pointer is over, so a button can light up under it.
    hot: Option<Id>,
    /// What the pointer went down on, so a press that slides off does not fire.
    down: Option<Id>,
    shared: Arc<Shared>,
    fonts: Vec<(Face, Hgdi)>,
    settings_cache: Option<PathBuf>,
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

    fn settings(&mut self) -> Option<Settings> {
        let library = self.model.library.clone()?;
        let mut s = Settings::new(library);
        s.volumes = self.model.volumes.iter().flatten().cloned().collect();
        s.playlists = self.model.playlists.clone();
        s.sensme = self.model.sensme;
        s.extras = self.model.extras;
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
            const DT_VCENTER: u32 = 0x0004;
            const DT_SINGLELINE: u32 = 0x0020;
            const DT_END_ELLIPSIS: u32 = 0x8000;
            const DT_NOPREFIX: u32 = 0x0800;
            let flags = DT_VCENTER
                | DT_SINGLELINE
                | DT_END_ELLIPSIS
                | DT_NOPREFIX
                | if *align == Align::Center { DT_CENTER } else { DT_LEFT };
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

// ── the worker ─────────────────────────────────────────────────────────────────────────────────

fn start(hwnd: Hwnd, job: Job, settings: Settings, shared: Arc<Shared>) {
    shared.cancel.store(false, Ordering::Relaxed);
    shared.queue.lock().unwrap().clear();
    *shared.result.lock().unwrap() = None;
    // `Hwnd` is a raw pointer, which is not `Send`; the handle itself is fine to use from another
    // thread (that is what `PostMessageW` is for), so it crosses as an integer.
    let hwnd = hwnd as usize;
    std::thread::spawn(move || {
        let out = job::run(job, &settings, &shared.cancel, &mut |u| {
            shared.queue.lock().unwrap().push(u);
            unsafe { PostMessageW(hwnd as Hwnd, WM_JOB, 0, 0) };
        });
        *shared.result.lock().unwrap() = Some(out);
        unsafe { PostMessageW(hwnd as Hwnd, WM_JOB_DONE, 0, 0) };
    });
}

/// Drain everything the worker has queued into the model. Called on the UI thread only.
fn drain(state: &mut State) {
    let updates: Vec<Update> = std::mem::take(&mut *state.shared.queue.lock().unwrap());
    for u in updates {
        match u {
            Update::Say(line) => state.model.say(line),
            Update::Log(line) => state.model.log.push(line),
            Update::Progress(p) => state.model.progress = p,
            Update::Planned => state.model.planned = true,
        }
    }
    // The log is unbounded otherwise: a library of 40,000 tracks would hold 40,000 strings for the
    // sake of the 9 lines the pane shows.
    const KEEP: usize = 2_000;
    if state.model.log.len() > KEEP * 2 {
        state.model.log.drain(..state.model.log.len() - KEEP);
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
            let x = (l & 0xFFFF) as i16 as i32;
            let y = ((l >> 16) & 0xFFFF) as i16 as i32;
            let mut to_pick: Option<Id> = None;
            let mut to_start: Option<(Job, Settings)> = None;
            let mut repaint = false;
            let mut stop = false;
            STATE.with(|st| {
                let mut st = st.borrow_mut();
                let Some(state) = st.as_mut() else { return };
                let (lx, ly) = (state.unscale(x), state.unscale(y));
                let (w, h) = state.size;
                let over = hit(&state.model, w, h, lx, ly);
                if over != state.hot {
                    state.hot = over;
                    repaint = true;
                }
                match msg {
                    WM_LBUTTONDOWN => state.down = over,
                    WM_LBUTTONUP => {
                        // A press that slid off its control does nothing, which is what every other
                        // button on the system does.
                        let pressed = state.down.take().filter(|d| Some(*d) == over);
                        if let Some(id) = pressed {
                            match id {
                                Id::PickLibrary | Id::PickVolume(_) | Id::PickPlaylists => to_pick = Some(id),
                                Id::Stop => stop = true,
                                _ => {
                                    if let Some(job) = click(&mut state.model, id) {
                                        if let Some(s) = state.settings() {
                                            to_start = Some((job, s));
                                        }
                                    }
                                    repaint = true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            });
            if stop {
                STATE.with(|st| {
                    if let Some(state) = st.borrow().as_ref() {
                        state.shared.cancel.store(true, Ordering::Relaxed);
                    }
                });
                STATE.with(|st| {
                    if let Some(state) = st.borrow_mut().as_mut() {
                        state.model.say("stopping…");
                    }
                });
                repaint = true;
            }
            if let Some(id) = to_pick {
                let title = match id {
                    Id::PickLibrary => "Where is your music?",
                    Id::PickPlaylists => "Where are your playlists?",
                    _ => "Which drive is the player?",
                };
                if let Some(path) = pick_folder(hwnd, title) {
                    STATE.with(|st| {
                        if let Some(state) = st.borrow_mut().as_mut() {
                            set_path(&mut state.model, id, path);
                        }
                    });
                }
                repaint = true;
            }
            if let Some((job, settings)) = to_start {
                let shared = STATE.with(|st| st.borrow().as_ref().map(|s| s.shared.clone()));
                if let Some(shared) = shared {
                    STATE.with(|st| {
                        if let Some(state) = st.borrow_mut().as_mut() {
                            state.model.phase = Phase::Working;
                            state.model.progress = None;
                            state.model.log.clear();
                        }
                    });
                    start(hwnd, job, settings, shared);
                }
                repaint = true;
            }
            if repaint {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
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
            STATE.with(|st| {
                if let Some(state) = st.borrow_mut().as_mut() {
                    drain(state);
                }
            });
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_JOB_DONE => {
            STATE.with(|st| {
                if let Some(state) = st.borrow_mut().as_mut() {
                    drain(state);
                    state.model.phase = Phase::Idle;
                    let result = state.shared.result.lock().unwrap().take();
                    match result {
                        Some(Ok(word)) => state.model.say(word),
                        Some(Err(why)) => {
                            state.model.progress = None;
                            state.model.say(format!("stopped: {why}"));
                            let text = wide(&why);
                            let cap = wide("Flint");
                            MessageBoxW(hwnd, text.as_ptr(), cap.as_ptr(), MB_ICONERROR);
                        }
                        None => {}
                    }
                }
            });
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_CLOSE => {
            // A job in flight is asked to stop and the window waits for it, rather than the process
            // exiting mid-copy. `apply` renames every copy into place, so the worst a stop leaves
            // is a file not copied yet — never a half-written one.
            let busy = STATE.with(|st| st.borrow().as_ref().is_some_and(|s| s.model.phase == Phase::Working));
            if busy {
                STATE.with(|st| {
                    if let Some(state) = st.borrow_mut().as_mut() {
                        state.shared.cancel.store(true, Ordering::Relaxed);
                        state.model.say("stopping before closing…");
                    }
                });
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
        let fonts = [Face::Title, Face::Small, Face::Body, Face::Strong, Face::Mono]
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

        STATE.with(|st| {
            *st.borrow_mut() = Some(State {
                model: Model::new(),
                theme: Theme::light(),
                size: (W, H),
                dpi,
                hot: None,
                down: None,
                shared: Arc::new(Shared {
                    queue: Mutex::new(Vec::new()),
                    result: Mutex::new(None),
                    cancel: AtomicBool::new(false),
                }),
                fonts,
                settings_cache: None,
            });
        });

        SetWindowTextW(hwnd, title.as_ptr());
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
