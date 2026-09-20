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
pub const W: i32 = 920;
pub const H: i32 = 760;

/// The smallest the window may get. Below this the log pane stops being a log and the paths stop
/// being readable, so the platform layer refuses rather than reflowing into nonsense.
pub const MIN_W: i32 = 780;
pub const MIN_H: i32 = 620;

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

/// What a destination volume is holding, and what this plan would add to it.
///
/// The numbers are the ones `job::plan` already works out to decide what fits — until now they
/// were only ever written to the log as a sentence. A player has a fixed and rather small amount
/// of room, "will my library fit" is the question this window exists to answer, and a bar answers
/// it in the time it takes to look at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VolumeFacts {
    /// Bytes of music already on the volume.
    pub on_device: u64,
    /// Bytes this volume may hold: free space plus what its music already occupies, less the
    /// headroom `flint-core` keeps. Zero means "not known yet", and the meter says so.
    pub budget: u64,
    /// Bytes the shown plan would copy here. Zero once the plan is invalidated.
    pub to_copy: u64,
    /// Albums the shown plan assigns here.
    pub albums: usize,
}

/// What the library turned out to contain, once something has read it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LibraryFacts {
    pub files: usize,
    pub bytes: u64,
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
    /// What the library holds, once a job has read it.
    pub source: Option<LibraryFacts>,
    /// What each destination holds and what the plan would add, in the same order as `volumes`.
    pub dest: [Option<VolumeFacts>; 2],
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

    /// The one control the window is asking for next, or `Id::None` while a job is running. It is
    /// the ONLY thing drawn in the accent, so the window always has exactly one place the eye is
    /// meant to land: a folder, then a volume, then the plan, then the copy. A tool that is
    /// perfectly pressable is not the next step — `Import Music Center` is available from the
    /// first frame and is still not what the window is asking for.
    pub fn next_step(&self) -> Id {
        if self.phase == Phase::Working {
            Id::None
        } else if self.library.is_none() {
            Id::PickLibrary
        } else if self.volumes.iter().all(Option::is_none) {
            Id::PickVolume(0)
        } else if self.planned {
            Id::Apply
        } else {
            Id::Plan
        }
    }

    /// Anything that changes what a sync would do invalidates the shown plan — and with it the
    /// two plan-derived numbers on each meter. What is ALREADY on a volume is not plan-derived, so
    /// it stays: it was true before the change and it is true after it.
    pub fn invalidate_plan(&mut self) {
        self.planned = false;
        for d in self.dest.iter_mut().flatten() {
            d.to_copy = 0;
            d.albums = 0;
        }
    }

    pub fn say(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.status = line.clone();
        self.log.push(line);
    }
}

/// What a widget is, for painting. Deliberately small: this window is a source, two destinations,
/// a few buttons and a log.
#[derive(Clone, PartialEq, Debug)]
pub enum Kind {
    /// A filled panel behind a group of rows.
    Panel,
    /// A destination's card. `filled` is false for one with no volume chosen yet, which is drawn
    /// as an outline: an empty slot that looks empty, rather than a panel pretending to hold
    /// something.
    Card {
        filled: bool,
    },
    /// The title band at the top.
    Band,
    Title,
    /// The version, at the right-hand end of the band. Small enough to find and not to read.
    Version,
    Subtitle,
    /// The name of a section, above its content.
    Heading,
    /// A row's left-hand caption.
    Label,
    /// A destination's name, on its card.
    Caption,
    /// A row's value — a path, or the placeholder when there is none.
    Value {
        placeholder: bool,
    },
    /// Quiet prose: what a control will do, or what an empty pane is waiting for.
    Hint,
    /// The same, right-aligned: the figures a meter is read against, which sit at the far end of
    /// the line the figure starts.
    Detail,
    /// A number the user came here for, in the largest type on the window. `warm` puts it in the
    /// accent, which on this window means one thing: bytes this copy would write.
    Figure {
        warm: bool,
    },
    /// A volume's capacity, as fractions of its budget: what is already there, then what this plan
    /// would add. The rest is free space. `known` is false before anything has read the volume,
    /// and draws an empty trough rather than a full one.
    Meter {
        have: f32,
        add: f32,
        known: bool,
    },
    Button {
        primary: bool,
        enabled: bool,
    },
    /// A button for something that changes nothing on the player. Outlined, never filled: the
    /// library tools are not the transfer and should not look like it.
    Tool {
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

const PAD: i32 = 22;
const BTN_H: i32 = 30;
const BAND_H: i32 = 64;
const CHOOSE_W: i32 = 96;
const CLEAR_W: i32 = 62;
/// A destination's card: its inner padding, its height with a volume in it, and its height
/// without one. The empty card is short because an empty slot has nothing to say.
const CARD_PAD: i32 = 16;
const CARD_H: i32 = 96;
const CARD_EMPTY_H: i32 = 56;

fn heading(text: &str, rect: Rect) -> Widget {
    Widget { id: Id::None, rect, kind: Kind::Heading, text: text.into() }
}

fn hint(text: impl Into<String>, rect: Rect) -> Widget {
    Widget { id: Id::None, rect, kind: Kind::Hint, text: text.into() }
}

fn path_text(p: &Option<PathBuf>, placeholder: &str) -> (String, bool) {
    match p {
        Some(p) => (p.display().to_string(), false),
        None => (placeholder.into(), true),
    }
}

/// `n` with thousands separators. A library is tens of thousands of files and "23841" is a number
/// nobody reads at a glance.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One button on the action row: what it is, what it says, whether it is THE next step, whether it
/// can be pressed now, and how wide it is.
type Action = (Id, &'static str, bool, bool, i32);

/// Where everything is. The one place that knows — both the paint and the hit test read this, so a
/// control cannot be drawn in one place and clicked in another.
///
/// ── THE SHAPE OF THE WINDOW ─────────────────────────────────────────────────────────────────
///
/// A sync has a source, one or two destinations and a limit, and the window is laid out as that
/// sentence: the music folder at the top, a card per destination under it, and the actions below
/// both. The destinations are cards rather than rows because each one carries something a row has
/// nowhere to put — a capacity meter. A player holds 16 or 32 GB against a library of hundreds,
/// "what fits" is the question this tool exists to answer, and `job::plan` has always known the
/// answer and only ever written it to the log as a sentence.
///
/// ── WHERE THE COLOUR GOES ───────────────────────────────────────────────────────────────────
///
/// The accent means ONE thing on this window: bytes that would be written. So it is the next step
/// (one button at a time — `Show what would happen` until there is a plan, `Copy to the player`
/// once there is), the segment of a meter this copy would add, and the progress bar while it is
/// being added. A checkbox that is merely on is not an alarm and is drawn in ink.
pub fn layout(m: &Model, w: i32, h: i32) -> Vec<Widget> {
    let w = w.max(MIN_W);
    let h = h.max(MIN_H);
    let inner = w - PAD * 2;
    let mut out = Vec::with_capacity(48);
    let busy = m.phase == Phase::Working;
    let next = m.next_step();

    // ── the band ───────────────────────────────────────────────────────────────────────────
    out.push(Widget { id: Id::None, rect: Rect::new(0, 0, w, BAND_H), kind: Kind::Band, text: String::new() });
    out.push(Widget { id: Id::None, rect: Rect::new(PAD, 13, 200, 26), kind: Kind::Title, text: "Flint".into() });
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(w - PAD - 120, 15, 120, 20),
        kind: Kind::Version,
        text: env!("CARGO_PKG_VERSION").into(),
    });
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, 38, inner, 18),
        kind: Kind::Subtitle,
        text: "Put a music library on a Sony Walkman — with SensMe, without the bloat".into(),
    });

    // ── where the music is ─────────────────────────────────────────────────────────────────
    let mut y = BAND_H + 24;
    out.push(heading("Music library", Rect::new(PAD, y, 300, 16)));
    y += 20;
    let (t, ph) = path_text(&m.library, "No folder chosen");
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, y, inner - CHOOSE_W - 14, 26),
        kind: Kind::Value { placeholder: ph },
        text: t,
    });
    out.push(Widget {
        id: Id::PickLibrary,
        rect: Rect::new(w - PAD - CHOOSE_W, y - 2, CHOOSE_W, BTN_H),
        kind: Kind::Button { primary: next == Id::PickLibrary, enabled: !busy },
        text: "Choose…".into(),
    });
    y += 30;
    // The counts line is reserved whether or not they are known yet: a line that appears when a
    // job finishes would push both cards down under the pointer that is about to click one.
    // Known: what is in there. Not known yet: what Flint will do with it — which is the one thing
    // a first-time reader cannot guess, and the line is here anyway.
    let src_line = match m.source {
        Some(src) => format!("{} tracks, {}", thousands(src.files), flint_core::space::human(src.bytes)),
        None => "Everything under this folder is copied, album by album.".into(),
    };
    out.push(hint(src_line, Rect::new(PAD, y, inner, 18)));
    y += 22;

    // Playlists live here, under the library, because that is what they are: a folder of .m3u8
    // files on the PC, read from the same place the music is. They were a destination-side row
    // until now, between the player and the card, where they read as a third volume.
    let (t, ph) = path_text(&m.playlists, "No playlist folder — optional");
    out.push(Widget { id: Id::None, rect: Rect::new(PAD, y + 3, 64, 20), kind: Kind::Label, text: "Playlists".into() });
    let pl_tools = 84 + 8 + if m.playlists.is_some() { 62 + 8 } else { 0 };
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD + 68, y + 3, (inner - 68 - pl_tools).max(80), 20),
        kind: Kind::Value { placeholder: ph },
        text: t,
    });
    let mut px = w - PAD - 84;
    out.push(Widget {
        id: Id::PickPlaylists,
        rect: Rect::new(px, y, 84, 26),
        kind: Kind::Tool { enabled: !busy },
        text: "Choose…".into(),
    });
    if m.playlists.is_some() {
        px -= 62 + 8;
        out.push(Widget {
            id: Id::ClearPlaylists,
            rect: Rect::new(px, y, 62, 26),
            kind: Kind::Tool { enabled: !busy },
            text: "Clear".into(),
        });
    }
    y += 26;

    // ── where it is going ──────────────────────────────────────────────────────────────────
    y += 20;
    out.push(heading("Onto the player", Rect::new(PAD, y, 300, 16)));
    y += 20;
    for i in 0..2 {
        let chosen = m.volumes[i].is_some();
        let card_h = if chosen { CARD_H } else { CARD_EMPTY_H };
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD, y, inner, card_h),
            kind: Kind::Card { filled: chosen },
            text: String::new(),
        });
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD + CARD_PAD, y + 13, 120, 20),
            kind: Kind::Caption,
            text: if i == 0 { "Player".into() } else { "Memory card".into() },
        });
        // The path, between the name and the buttons.
        let trailing = CHOOSE_W + if chosen { CLEAR_W + 8 } else { 0 } + 14;
        let path_x = PAD + CARD_PAD + 120;
        let path_w = (w - PAD - CARD_PAD - trailing - path_x).max(80);
        let (t, ph) = path_text(
            &m.volumes[i],
            if i == 0 { "Choose the drive the player appears as" } else { "Optional — the card in the player" },
        );
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(path_x, y + 13, path_w, 20),
            kind: Kind::Value { placeholder: ph },
            text: t,
        });
        if chosen {
            out.push(Widget {
                id: Id::ClearVolume(i),
                rect: Rect::new(w - PAD - CARD_PAD - CHOOSE_W - CLEAR_W - 8, y + 8, CLEAR_W, BTN_H),
                kind: Kind::Button { primary: false, enabled: !busy },
                text: "Clear".into(),
            });
        }
        out.push(Widget {
            id: Id::PickVolume(i),
            rect: Rect::new(w - PAD - CARD_PAD - CHOOSE_W, y + 8, CHOOSE_W, BTN_H),
            kind: Kind::Button { primary: next == Id::PickVolume(i), enabled: !busy },
            text: "Choose…".into(),
        });

        // The meter, and the one number the user came for.
        if chosen {
            let facts = m.dest[i].unwrap_or_default();
            let known = facts.budget > 0;
            let (have, add) = if known {
                let b = facts.budget as f32;
                let have = (facts.on_device as f32 / b).clamp(0.0, 1.0);
                let add = (facts.to_copy as f32 / b).clamp(0.0, 1.0 - have);
                (have, add)
            } else {
                (0.0, 0.0)
            };
            out.push(Widget {
                id: Id::None,
                rect: Rect::new(PAD + CARD_PAD, y + 48, inner - CARD_PAD * 2, 10),
                kind: Kind::Meter { have, add, known },
                text: String::new(),
            });
            // The big type is for a SIZE. Before anything has read the volume there is no size,
            // and a sentence set at 22 px semibold in its place is a shout about nothing — so the
            // slot stays empty and the line below it says what would fill it.
            if known {
                let (figure, warm) = if facts.to_copy == 0 {
                    ("Nothing to copy".to_string(), false)
                } else {
                    (flint_core::space::human(facts.to_copy), true)
                };
                out.push(Widget {
                    id: Id::None,
                    rect: Rect::new(PAD + CARD_PAD, y + 62, 200, 26),
                    kind: Kind::Figure { warm },
                    text: figure,
                });
                let free = facts.budget.saturating_sub(facts.on_device + facts.to_copy);
                let albums =
                    if facts.albums > 0 { format!("{} albums, ", thousands(facts.albums)) } else { String::new() };
                out.push(Widget {
                    id: Id::None,
                    rect: Rect::new(PAD + CARD_PAD, y + 66, inner - CARD_PAD * 2, 18),
                    kind: Kind::Detail,
                    text: format!(
                        "{albums}{} already there, {} free of {}",
                        flint_core::space::human(facts.on_device),
                        flint_core::space::human(free),
                        flint_core::space::human(facts.budget)
                    ),
                });
            } else {
                // Two words rather than a sentence: the empty trough above has already said it,
                // and the same long sentence in both cards is noise with a repeat.
                out.push(Widget {
                    id: Id::None,
                    rect: Rect::new(PAD + CARD_PAD, y + 62, inner - CARD_PAD * 2, 20),
                    kind: Kind::Detail,
                    text: "Not read yet".into(),
                });
            }
        }
        y += card_h + 10;
    }

    // ── the two switches ───────────────────────────────────────────────────────────────────
    y += 6;
    out.push(Widget {
        id: Id::ToggleSensMe,
        rect: Rect::new(PAD, y, 300, 22),
        kind: Kind::Check { on: m.sensme, enabled: !busy },
        text: "Write SensMe tags into the copies".into(),
    });
    out.push(Widget {
        id: Id::ToggleExtras,
        rect: Rect::new(PAD + 320, y, 280, 22),
        kind: Kind::Check { on: m.extras, enabled: !busy },
        text: "Copy cover art and lyrics too".into(),
    });
    y += 22;

    // ── the actions ────────────────────────────────────────────────────────────────────────
    //
    // ONE accented button at a time, and which one it is IS the state of the window: look at what
    // would happen, then do it. Both are always drawn, in the same places, so neither moves out
    // from under the finger that is about to press it — only the colour moves.
    //
    // STOP's place at the right-hand end is reserved whether or not a job is running, for the same
    // reason.
    y += 18;
    let act_h = BTN_H + 6;
    const STOP_W: i32 = 90;
    let idle = m.phase == Phase::Idle;
    let acts: [Action; 2] = [
        (Id::Plan, "Show what would happen", next == Id::Plan, m.can_plan(), 214),
        (Id::Apply, "Copy to the player", next == Id::Apply, m.can_apply(), 176),
    ];
    let mut x = PAD;
    for &(id, text, primary, enabled, width) in acts.iter() {
        out.push(Widget {
            id,
            rect: Rect::new(x, y, width, act_h),
            kind: Kind::Button { primary, enabled },
            text: text.into(),
        });
        x += width + 10;
    }
    if busy {
        out.push(Widget {
            id: Id::Stop,
            rect: Rect::new(w - PAD - STOP_W, y, STOP_W, act_h),
            kind: Kind::Button { primary: false, enabled: true },
            text: "Stop".into(),
        });
    }
    y += act_h + 10;

    // The library tools change nothing on the player, so they are outlines rather than buttons.
    let tools: [Action; 3] = [
        (Id::Scan, "Analyse library", false, idle && m.library.is_some(), 138),
        (Id::Import, "Import Music Center", false, idle, 166),
        (Id::Check, "Check FLACs", false, idle && m.library.is_some(), 120),
    ];
    let mut x = PAD;
    for &(id, text, _, enabled, width) in tools.iter() {
        if x > PAD && x + width > w - PAD {
            x = PAD;
            y += 34;
        }
        out.push(Widget { id, rect: Rect::new(x, y, width, 28), kind: Kind::Tool { enabled }, text: text.into() });
        x += width + 10;
    }
    y += 28;

    // ── progress, status, log ──────────────────────────────────────────────────────────────
    //
    // The bar exists only while a job does. An empty trough at rest is a control that is not
    // controlling anything, and the space it was holding goes to the log, which is the part of
    // this window people actually read.
    y += 16;
    let status_y = if busy {
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD, y, inner, 8),
            kind: Kind::Progress(m.progress),
            text: String::new(),
        });
        y + 14
    } else {
        y
    };
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, status_y, inner, 20),
        kind: Kind::Status,
        text: m.status.clone(),
    });

    let log_y = status_y + 28;
    let log_h = (h - log_y - PAD).max(0);
    if m.log.is_empty() {
        // An empty pane is a place to say what happens next, not a black rectangle waiting to be
        // filled. It is drawn as the panel outline the cards use, so it reads as "nothing here
        // yet" rather than as a terminal that has died.
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD, log_y, inner, log_h),
            kind: Kind::Card { filled: false },
            text: String::new(),
        });
        out.push(hint(
            "Every file Flint would copy, remove or tag is listed here first. Nothing is written \
             until you press Copy to the player.",
            Rect::new(PAD + CARD_PAD, log_y + 14, inner - CARD_PAD * 2, 20),
        ));
    } else {
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD, log_y, inner, log_h),
            kind: Kind::LogPane,
            text: String::new(),
        });
        let line_h = 18;
        let visible = ((log_h - 16) / line_h).max(0) as usize;
        let start = m.log.len().saturating_sub(visible);
        for (i, line) in m.log[start..].iter().enumerate() {
            out.push(Widget {
                id: Id::None,
                rect: Rect::new(PAD + 12, log_y + 8 + i as i32 * line_h, inner - 24, line_h),
                kind: Kind::LogLine,
                text: line.clone(),
            });
        }
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

    /// ONE accented control at a time, and it is always the next thing to do. Two would be a
    /// choice where the window means to give an instruction; none, at rest, would leave a window
    /// with nothing to look at. While a job runs there is deliberately no accent on any control —
    /// the only accented thing then is the progress bar.
    #[test]
    fn exactly_one_control_is_accented_and_it_is_the_next_step() {
        let accented = |m: &Model| -> Vec<Id> {
            layout(m, W, H)
                .into_iter()
                .filter(|w| matches!(w.kind, Kind::Button { primary: true, .. }))
                .map(|w| w.id)
                .collect()
        };
        let mut m = Model::new();
        assert_eq!(accented(&m), vec![Id::PickLibrary], "an empty window asks for a folder");
        m.library = Some("/music".into());
        assert_eq!(accented(&m), vec![Id::PickVolume(0)], "…then for a volume");
        m.volumes[0] = Some("/player".into());
        assert_eq!(accented(&m), vec![Id::Plan], "…then to be shown what would happen");
        m.planned = true;
        assert_eq!(accented(&m), vec![Id::Apply], "…and only then to copy");
        m.phase = Phase::Working;
        assert!(accented(&m).is_empty(), "a running job accents no control");
    }

    /// A meter is a promise about room. `have + add` past the end of the bar would draw a volume
    /// as fitting what it cannot hold, which is the one thing this window must never say.
    #[test]
    fn a_meter_never_draws_past_the_end_of_the_volume() {
        let mut m = ready();
        m.dest[0] = Some(VolumeFacts {
            on_device: 40 * 1_000_000_000,
            budget: 50 * 1_000_000_000,
            to_copy: 30 * 1_000_000_000,
            albums: 9,
        });
        let meters: Vec<(f32, f32, bool)> = layout(&m, W, H)
            .into_iter()
            .filter_map(|w| match w.kind {
                Kind::Meter { have, add, known } => Some((have, add, known)),
                _ => None,
            })
            .collect();
        assert_eq!(meters.len(), 1, "one volume is chosen, so one meter is drawn");
        let (have, add, known) = meters[0];
        assert!(known);
        assert!(have + add <= 1.0, "have {have} + add {add} runs past the end of the bar");
        // …and a volume nothing has read yet draws an empty trough rather than a full one.
        m.dest[0] = None;
        let meter = layout(&m, W, H).into_iter().find_map(|w| match w.kind {
            Kind::Meter { known, have, .. } => Some((known, have)),
            _ => None,
        });
        assert_eq!(meter, Some((false, 0.0)));
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
