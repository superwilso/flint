//! Flint's window.
//!
//! **Why there is a window at all.** `docs/MUSIC_CENTER.md` §5 names it as the single biggest gap
//! against Sony's Music Center: a terminal is a fine way to move a library once you know the
//! commands, and a poor way to do it for the first time. Everything the window does goes through
//! the same `flint-core` the command line uses — there is one implementation of a sync, and the two
//! front ends only differ in how the plan is shown.
//!
//! **Why there is no toolkit.** The same reason `cinder-installer` has none: this is an executable
//! people download and run against their music, and every crate added is one more thing they have to
//! trust. Windows already ships a window, buttons and a font. So `win32.rs` talks to
//! user32/gdi32/comdlg32 directly, and everything above it is plain Rust with no dependencies.
//!
//! **The shape, and why it is this shape.** The platform layer knows nothing about Flint and Flint
//! knows nothing about Windows:
//!
//! ```text
//!   Model  ──layout()──►  Vec<Widget>  ──►  paint commands ──►  GDI   (Windows)
//!                              │                           └──►  SVG   (anywhere: the preview)
//!                              └──hit()──►  Action  ──►  a worker thread ──► flint-core
//! ```
//!
//! [`layout`] is the ONLY place that knows where anything is, and both the painting and the hit
//! test read it — the defect this avoids is a button drawn in one place and clicked in another,
//! which is the recurring one in Cinder's own UI audits. It also means the whole window is testable
//! on a machine that has never seen Windows, and can be rendered to an SVG that is exactly what the
//! window will paint.

pub mod job;
pub mod paint;
pub mod svg;

#[cfg(windows)]
pub mod win32;

use std::path::PathBuf;

/// Logical window size. Physical pixels are this times the DPI scale, which the platform layer
/// applies; nothing above it thinks about DPI.
pub const W: i32 = 900;
pub const H: i32 = 640;

/// The smallest the window may get. Below this the log pane stops being a log and the paths stop
/// being readable, so the platform layer refuses rather than reflowing into nonsense.
pub const MIN_W: i32 = 760;
pub const MIN_H: i32 = 560;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

/// What a click on a widget asks for. `None` is everything that is not a control.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Id {
    None,
    /// Choose the folder the music is in.
    PickLibrary,
    /// Choose destination `n` — the player's internal memory, then its card.
    PickVolume(usize),
    /// Forget destination `n`.
    ClearVolume(usize),
    /// Choose the folder the `.m3u8` playlists are in.
    PickPlaylists,
    ClearPlaylists,
    /// Write SensMe tags into the copies.
    ToggleSensMe,
    /// Copy cover art and lyrics that sit beside the music.
    ToggleExtras,
    /// Work out what would be copied, and show it. Writes nothing.
    Plan,
    /// Carry the plan out.
    Apply,
    /// Analyse the library into Flint's cache.
    Scan,
    /// Take the analysis Music Center has already done.
    Import,
    /// Look for FLACs that are not the lossless audio they claim to be.
    Check,
    /// Ask the running job to stop at the next file.
    Stop,
}

/// What the window is doing. The controls that would start a second job are disabled while one is
/// running, which is the whole of the state machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Working,
}

/// A job the worker thread runs. The window never runs one itself — see `win32::start`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Job {
    Plan,
    Apply,
    Scan,
    Import,
    Check,
}

/// Everything the window knows. Owned by the UI thread; the worker talks to it through the
/// platform layer's mutex.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub library: Option<PathBuf>,
    /// Up to two destinations, in the order they are offered: internal memory, then the card.
    pub volumes: [Option<PathBuf>; 2],
    pub playlists: Option<PathBuf>,
    pub sensme: bool,
    pub extras: bool,
    pub phase: Phase,
    /// 0.0..=1.0, or `None` for a job with no measurable length.
    pub progress: Option<f32>,
    /// One line per thing that happened, newest last. The pane shows the tail.
    pub log: Vec<String>,
    /// The line under the progress bar: what is happening now.
    pub status: String,
    /// True once a plan has been made and nothing has changed since, which is what makes COPY
    /// legal: this window never copies anything the user has not been shown first.
    pub planned: bool,
}

impl Model {
    /// The state a freshly opened window is in. SensMe and the extras are on because they are what
    /// Flint is for; both are one click away from off.
    pub fn new() -> Self {
        Model {
            sensme: true,
            extras: true,
            status: "Choose a music folder and a player volume.".into(),
            ..Model::default()
        }
    }

    /// Is there enough here to plan a sync?
    pub fn can_plan(&self) -> bool {
        self.phase == Phase::Idle && self.library.is_some() && self.volumes.iter().any(Option::is_some)
    }

    /// COPY is only ever offered for a plan the user has already seen. A change to any input
    /// clears that, so the button cannot carry over a plan made against different settings.
    pub fn can_apply(&self) -> bool {
        self.can_plan() && self.planned
    }

    /// Anything that changes what a sync would do invalidates the shown plan.
    pub fn invalidate_plan(&mut self) {
        self.planned = false;
    }

    pub fn say(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.status = line.clone();
        self.log.push(line);
    }
}

/// What a widget is, for painting. Deliberately small: this window is rows of text, a few buttons
/// and a log.
#[derive(Clone, PartialEq, Debug)]
pub enum Kind {
    /// A filled panel behind a group of rows.
    Panel,
    /// The title band at the top.
    Band,
    Title,
    Subtitle,
    /// A row's left-hand caption.
    Label,
    /// A row's value — a path, or the placeholder when there is none.
    Value {
        placeholder: bool,
    },
    Button {
        primary: bool,
        enabled: bool,
    },
    Check {
        on: bool,
        enabled: bool,
    },
    /// The progress bar. `f32` is the fill, 0..=1; a job with no length draws an empty trough.
    Progress(Option<f32>),
    /// The log pane's background.
    LogPane,
    /// One line of the log, drawn in the mono face.
    LogLine,
    Status,
    Rule,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Widget {
    pub id: Id,
    pub rect: Rect,
    pub kind: Kind,
    pub text: String,
}

const PAD: i32 = 20;
const ROW_H: i32 = 34;
const BTN_H: i32 = 30;
const BAND_H: i32 = 64;

fn label(text: &str, rect: Rect) -> Widget {
    Widget { id: Id::None, rect, kind: Kind::Label, text: text.into() }
}

fn path_text(p: &Option<PathBuf>, placeholder: &str) -> (String, bool) {
    match p {
        Some(p) => (p.display().to_string(), false),
        None => (placeholder.into(), true),
    }
}

/// One button on the action rows: what it is, what it says, whether it is a primary action, whether
/// it can be pressed now, and how wide it is.
type Action = (Id, &'static str, bool, bool, i32);

/// Where everything is. The one place that knows — both the paint and the hit test read this, so a
/// control cannot be drawn in one place and clicked in another.
pub fn layout(m: &Model, w: i32, h: i32) -> Vec<Widget> {
    let w = w.max(MIN_W);
    let h = h.max(MIN_H);
    let mut out = Vec::with_capacity(32);
    let busy = m.phase == Phase::Working;

    // ── the band ───────────────────────────────────────────────────────────────────────────
    out.push(Widget { id: Id::None, rect: Rect::new(0, 0, w, BAND_H), kind: Kind::Band, text: String::new() });
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, 14, 300, 26),
        kind: Kind::Title,
        text: format!("Flint {}", env!("CARGO_PKG_VERSION")),
    });
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, 38, w - PAD * 2, 18),
        kind: Kind::Subtitle,
        text: "Put a music library on a Sony Walkman — with SensMe, without the bloat".into(),
    });

    // ── the inputs ─────────────────────────────────────────────────────────────────────────
    let panel_y = BAND_H + PAD;
    let rows = 4;
    let panel_h = rows * ROW_H + 16 + ROW_H; // rows + the options line
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, panel_y, w - PAD * 2, panel_h),
        kind: Kind::Panel,
        text: String::new(),
    });

    let btn_w = 92;
    let clear_w = 64;
    let mut y = panel_y + 8;
    let row = |y: i32| Rect::new(PAD + 14, y, 150, ROW_H);
    let value_rect = |y: i32, trailing: i32| Rect::new(PAD + 14 + 150, y, w - PAD * 2 - 28 - 150 - trailing, ROW_H);

    // Music library.
    out.push(label("Music library", row(y)));
    let (t, ph) = path_text(&m.library, "no folder chosen");
    out.push(Widget { id: Id::None, rect: value_rect(y, btn_w + 12), kind: Kind::Value { placeholder: ph }, text: t });
    out.push(Widget {
        id: Id::PickLibrary,
        rect: Rect::new(w - PAD - 14 - btn_w, y + (ROW_H - BTN_H) / 2, btn_w, BTN_H),
        kind: Kind::Button { primary: false, enabled: !busy },
        text: "Choose…".into(),
    });
    y += ROW_H;

    // The two destinations.
    for i in 0..2 {
        let caption = if i == 0 { "Player" } else { "Memory card" };
        out.push(label(caption, row(y)));
        let (t, ph) = path_text(&m.volumes[i], if i == 0 { "no volume chosen" } else { "optional" });
        out.push(Widget {
            id: Id::None,
            rect: value_rect(y, btn_w + clear_w + 20),
            kind: Kind::Value { placeholder: ph },
            text: t,
        });
        if m.volumes[i].is_some() {
            out.push(Widget {
                id: Id::ClearVolume(i),
                rect: Rect::new(w - PAD - 14 - btn_w - clear_w - 8, y + (ROW_H - BTN_H) / 2, clear_w, BTN_H),
                kind: Kind::Button { primary: false, enabled: !busy },
                text: "Clear".into(),
            });
        }
        out.push(Widget {
            id: Id::PickVolume(i),
            rect: Rect::new(w - PAD - 14 - btn_w, y + (ROW_H - BTN_H) / 2, btn_w, BTN_H),
            kind: Kind::Button { primary: false, enabled: !busy },
            text: "Choose…".into(),
        });
        y += ROW_H;
    }

    // Playlists.
    out.push(label("Playlists", row(y)));
    let (t, ph) = path_text(&m.playlists, "optional — a folder of .m3u8 files");
    out.push(Widget {
        id: Id::None,
        rect: value_rect(y, btn_w + clear_w + 20),
        kind: Kind::Value { placeholder: ph },
        text: t,
    });
    if m.playlists.is_some() {
        out.push(Widget {
            id: Id::ClearPlaylists,
            rect: Rect::new(w - PAD - 14 - btn_w - clear_w - 8, y + (ROW_H - BTN_H) / 2, clear_w, BTN_H),
            kind: Kind::Button { primary: false, enabled: !busy },
            text: "Clear".into(),
        });
    }
    out.push(Widget {
        id: Id::PickPlaylists,
        rect: Rect::new(w - PAD - 14 - btn_w, y + (ROW_H - BTN_H) / 2, btn_w, BTN_H),
        kind: Kind::Button { primary: false, enabled: !busy },
        text: "Choose…".into(),
    });
    y += ROW_H + 8;

    // The two switches.
    out.push(Widget {
        id: Id::ToggleSensMe,
        rect: Rect::new(PAD + 14, y + 6, 320, 22),
        kind: Kind::Check { on: m.sensme, enabled: !busy },
        text: "Write SensMe tags into the copies".into(),
    });
    out.push(Widget {
        id: Id::ToggleExtras,
        rect: Rect::new(PAD + 14 + 340, y + 6, 280, 22),
        kind: Kind::Check { on: m.extras, enabled: !busy },
        text: "Copy cover art and lyrics too".into(),
    });

    // ── the actions ────────────────────────────────────────────────────────────────────────
    //
    // TWO ROWS, and they are two rows because they are two different things: the top row is the
    // transfer — look at it, then do it — and the bottom row is the library tools, which change
    // nothing on the player. A single row of five buttons invites pressing the wrong one, and the
    // first version of this laid out five and let them wrap, which put "Check FLACs" on a line of
    // its own by accident. A row still wraps if the window is narrow enough to need it.
    //
    // STOP's place at the right-hand end of the first row is reserved whether or not a job is
    // running, so the row does not reflow the moment one starts — a button that moves out from
    // under the finger that just pressed it is how the next click lands on the wrong thing.
    let act_y = panel_y + panel_h + PAD;
    let act_h = BTN_H + 6;
    const STOP_W: i32 = 90;
    let idle = m.phase == Phase::Idle;
    let rows: [&[Action]; 2] = [
        &[
            (Id::Plan, "Show what would happen", true, m.can_plan(), 210),
            (Id::Apply, "Copy to the player", true, m.can_apply(), 170),
        ],
        &[
            (Id::Scan, "Analyse library", false, idle && m.library.is_some(), 150),
            (Id::Import, "Import Music Center", false, idle, 180),
            (Id::Check, "Check FLACs", false, idle && m.library.is_some(), 130),
        ],
    ];
    let mut row_y = act_y;
    for (n, row) in rows.iter().enumerate() {
        // Only the first row has to leave room for STOP; the second is free of it.
        let avail = if n == 0 { w - PAD * 2 - STOP_W - 12 } else { w - PAD * 2 };
        let mut x = PAD;
        for &(id, text, primary, enabled, width) in row.iter() {
            if x > PAD && x + width > PAD + avail {
                x = PAD;
                row_y += act_h + 8;
            }
            out.push(Widget {
                id,
                rect: Rect::new(x, row_y, width, act_h),
                kind: Kind::Button { primary, enabled },
                text: text.into(),
            });
            x += width + 10;
        }
        if n == 0 && busy {
            out.push(Widget {
                id: Id::Stop,
                rect: Rect::new(w - PAD - STOP_W, row_y, STOP_W, act_h),
                kind: Kind::Button { primary: false, enabled: true },
                text: "Stop".into(),
            });
        }
        row_y += act_h + 8;
    }
    let act_bottom = row_y - 8;

    // ── progress, status, log ──────────────────────────────────────────────────────────────
    let prog_y = act_bottom + PAD;
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, prog_y, w - PAD * 2, 8),
        kind: Kind::Progress(if busy { m.progress } else { None }),
        text: String::new(),
    });
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, prog_y + 14, w - PAD * 2, 20),
        kind: Kind::Status,
        text: m.status.clone(),
    });

    let log_y = prog_y + 44;
    let log_h = h - log_y - PAD;
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, log_y, w - PAD * 2, log_h),
        kind: Kind::LogPane,
        text: String::new(),
    });
    let line_h = 18;
    let visible = ((log_h - 16) / line_h).max(0) as usize;
    let start = m.log.len().saturating_sub(visible);
    for (i, line) in m.log[start..].iter().enumerate() {
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD + 10, log_y + 8 + i as i32 * line_h, w - PAD * 2 - 20, line_h),
            kind: Kind::LogLine,
            text: line.clone(),
        });
    }
    out
}

/// Which control is under `(x, y)`, if any. Disabled controls do not answer: a button that is drawn
/// grey and still acts is worse than one that is not there.
pub fn hit(m: &Model, w: i32, h: i32, x: i32, y: i32) -> Option<Id> {
    layout(m, w, h).into_iter().rev().find_map(|wid| {
        let live = match wid.kind {
            Kind::Button { enabled, .. } | Kind::Check { enabled, .. } => enabled,
            _ => false,
        };
        (live && wid.id != Id::None && wid.rect.contains(x, y)).then_some(wid.id)
    })
}

/// Apply a click to the model, and say which job (if any) the platform layer should start.
///
/// The file pickers are the platform's business — this returns the `Id` for those and the caller
/// opens a dialog, because a folder chooser is the one thing that cannot be pure.
pub fn click(m: &mut Model, id: Id) -> Option<Job> {
    match id {
        Id::ToggleSensMe => {
            m.sensme = !m.sensme;
            m.invalidate_plan();
            None
        }
        Id::ToggleExtras => {
            m.extras = !m.extras;
            m.invalidate_plan();
            None
        }
        Id::ClearVolume(i) => {
            m.volumes[i] = None;
            m.invalidate_plan();
            None
        }
        Id::ClearPlaylists => {
            m.playlists = None;
            m.invalidate_plan();
            None
        }
        Id::Plan => m.can_plan().then_some(Job::Plan),
        Id::Apply => m.can_apply().then_some(Job::Apply),
        Id::Scan => (m.phase == Phase::Idle && m.library.is_some()).then_some(Job::Scan),
        Id::Import => (m.phase == Phase::Idle).then_some(Job::Import),
        Id::Check => (m.phase == Phase::Idle && m.library.is_some()).then_some(Job::Check),
        _ => None,
    }
}

/// A folder the user picked, for `id`. Any of these invalidates a shown plan.
pub fn set_path(m: &mut Model, id: Id, path: PathBuf) {
    match id {
        Id::PickLibrary => m.library = Some(path),
        Id::PickVolume(i) => m.volumes[i] = Some(path),
        Id::PickPlaylists => m.playlists = Some(path),
        _ => return,
    }
    m.invalidate_plan();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Model {
        let mut m = Model::new();
        m.library = Some("/music".into());
        m.volumes[0] = Some("/player".into());
        m
    }

    /// Every control the layout draws is reachable by a click at its own centre, and nothing that
    /// is not a control answers one. This is the test that stops a button drifting out from under
    /// the pointer.
    #[test]
    fn every_live_control_is_hittable_where_it_is_drawn() {
        let m = ready();
        let widgets = layout(&m, W, H);
        let live: Vec<&Widget> = widgets
            .iter()
            .filter(|wid| {
                wid.id != Id::None
                    && matches!(wid.kind, Kind::Button { enabled: true, .. } | Kind::Check { enabled: true, .. })
            })
            .collect();
        assert!(live.len() >= 6, "expected the controls, got {}", live.len());
        for wid in live {
            let cx = wid.rect.x + wid.rect.w / 2;
            let cy = wid.rect.y + wid.rect.h / 2;
            assert_eq!(hit(&m, W, H, cx, cy), Some(wid.id), "{:?} at {:?}", wid.id, wid.rect);
        }
        // The band is not a control.
        assert_eq!(hit(&m, W, H, W / 2, 10), None);
    }

    /// No two controls overlap. Two buttons sharing a pixel means one of them is unreachable in
    /// that pixel and which one is an accident of ordering.
    #[test]
    fn controls_do_not_overlap() {
        let m = ready();
        let widgets = layout(&m, W, H);
        let ctrls: Vec<&Widget> = widgets
            .iter()
            .filter(|w| w.id != Id::None && !matches!(w.kind, Kind::Value { .. } | Kind::Label))
            .collect();
        for (i, a) in ctrls.iter().enumerate() {
            for b in &ctrls[i + 1..] {
                let overlap = a.rect.x < b.rect.right()
                    && b.rect.x < a.rect.right()
                    && a.rect.y < b.rect.bottom()
                    && b.rect.y < a.rect.bottom();
                assert!(!overlap, "{:?} overlaps {:?}", a.id, b.id);
            }
        }
    }

    /// Nothing is drawn outside the window, at the default size or the smallest allowed one.
    #[test]
    fn nothing_is_drawn_off_the_window() {
        for (w, h) in [(W, H), (MIN_W, MIN_H), (1400, 900)] {
            let mut m = ready();
            m.log = (0..80).map(|i| format!("line {i}")).collect();
            for wid in layout(&m, w, h) {
                assert!(wid.rect.x >= 0 && wid.rect.y >= 0, "{:?} at {:?}", wid.id, wid.rect);
                assert!(wid.rect.right() <= w, "{:?} runs past the right edge: {:?}", wid.kind, wid.rect);
                assert!(wid.rect.bottom() <= h, "{:?} runs past the bottom: {:?}", wid.kind, wid.rect);
            }
        }
    }

    /// COPY is only offered for a plan the user has been shown, and any change to the inputs takes
    /// it away again. The window must never copy against settings nobody has seen the effect of.
    #[test]
    fn copying_needs_a_plan_that_is_still_current() {
        let mut m = ready();
        assert!(m.can_plan() && !m.can_apply());
        assert_eq!(click(&mut m, Id::Apply), None, "COPY does nothing before a plan");
        m.planned = true;
        assert_eq!(click(&mut m, Id::Apply), Some(Job::Apply));
        // Any input change invalidates it.
        for id in [Id::ToggleSensMe, Id::ToggleExtras, Id::ClearPlaylists, Id::ClearVolume(1)] {
            m.planned = true;
            click(&mut m, id);
            assert!(!m.can_apply(), "{id:?} left a stale plan usable");
        }
        m.planned = true;
        set_path(&mut m, Id::PickLibrary, "/other".into());
        assert!(!m.can_apply(), "choosing a different library left a stale plan usable");
    }

    /// With a job running, the things that would start a second one are dead and STOP appears.
    #[test]
    fn a_running_job_disables_the_starters_and_offers_stop() {
        let mut m = ready();
        m.planned = true;
        m.phase = Phase::Working;
        for id in [Id::Plan, Id::Apply, Id::Scan, Id::Import, Id::Check] {
            assert_eq!(click(&mut m, id), None, "{id:?} started a second job");
        }
        let stop = layout(&m, W, H).into_iter().find(|w| w.id == Id::Stop);
        let stop = stop.expect("a running job offers a way to stop it");
        assert_eq!(hit(&m, W, H, stop.rect.x + 4, stop.rect.y + 4), Some(Id::Stop));
        // …and the pickers are dead too, so a path cannot change under a running job.
        assert!(hit(&m, W, H, 0, 0).is_none());
        let pick = layout(&m, W, H).into_iter().find(|w| w.id == Id::PickLibrary).unwrap();
        assert_eq!(hit(&m, W, H, pick.rect.x + 4, pick.rect.y + 4), None);
    }

    /// The log pane shows the TAIL: a job that prints a thousand lines must leave the newest ones
    /// on screen, not the first ones.
    #[test]
    fn the_log_pane_shows_the_newest_lines() {
        let mut m = ready();
        m.log = (0..500).map(|i| format!("line {i}")).collect();
        let lines: Vec<String> =
            layout(&m, W, H).into_iter().filter(|w| w.kind == Kind::LogLine).map(|w| w.text).collect();
        assert!(!lines.is_empty());
        assert_eq!(lines.last().unwrap(), "line 499");
        // In order, and contiguous.
        assert_eq!(lines[1], format!("line {}", 500 - lines.len() + 1));
    }

    /// A clear button only exists when there is something to clear.
    #[test]
    fn clear_appears_only_when_a_path_is_set() {
        let mut m = Model::new();
        let has = |m: &Model, id: Id| layout(m, W, H).iter().any(|w| w.id == id);
        assert!(!has(&m, Id::ClearVolume(0)) && !has(&m, Id::ClearPlaylists));
        m.volumes[0] = Some("/player".into());
        m.playlists = Some("/lists".into());
        assert!(has(&m, Id::ClearVolume(0)) && has(&m, Id::ClearPlaylists));
    }
}
