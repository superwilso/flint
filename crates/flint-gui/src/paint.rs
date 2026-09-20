//! Widgets → paint commands.
//!
//! This is the whole of the window's appearance, and it is deliberately separate from both
//! backends. A [`Cmd`] is something every 2-D drawing interface has: a filled or outlined
//! rectangle, a run of text, a polyline. GDI draws them with `FillRect`/`RoundRect`/`DrawTextW`,
//! and [`crate::svg`] writes them out as `<rect>`, `<text>` and `<polyline>` — so the SVG preview
//! is not an artist's impression of the window, it is the same command list the window paints.
//!
//! The palette is Cinder's, because Flint is a tool for Cinder's device and the two are installed
//! by the same person on the same afternoon: the ember accent (`#E0551B`) on a dark band over a
//! light body, exactly as `installer/src/gui.rs` uses it. Unlike the installer, nothing here is a
//! stock Windows control, so the light body is a choice rather than a constraint — the reason to
//! keep it is that a file path is a long line of small text and dark-on-light is the easier read.

use crate::{layout, Kind, Model, Rect};

/// A colour in the order everyone writes one: `0xRRGGBB`. The Win32 backend reverses it, because
/// `COLORREF` is `0x00BBGGRR` and that is the backend's problem, not this layer's.
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub const fn r_of(c: u32) -> u8 {
    (c >> 16) as u8
}
pub const fn g_of(c: u32) -> u8 {
    (c >> 8) as u8
}
pub const fn b_of(c: u32) -> u8 {
    c as u8
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: u32,
    pub band: u32,
    pub band_text: u32,
    pub band_dim: u32,
    pub panel: u32,
    pub panel_border: u32,
    /// The outline of a card with nothing in it yet, and of the empty log pane.
    pub empty_border: u32,
    pub text: u32,
    pub heading: u32,
    pub dim: u32,
    pub placeholder: u32,
    pub accent: u32,
    pub accent_text: u32,
    pub accent_down: u32,
    /// What a volume already holds, in a meter. Graphite: it is real, and it is not this copy.
    pub meter_have: u32,
    pub button: u32,
    pub button_border: u32,
    pub button_text: u32,
    pub check: u32,
    pub disabled: u32,
    pub disabled_text: u32,
    pub trough: u32,
    pub log_bg: u32,
    pub log_border: u32,
    pub log_text: u32,
    pub rule: u32,
}

impl Theme {
    pub const fn light() -> Theme {
        Theme {
            bg: rgb(0xF2, 0xF3, 0xF6),
            band: rgb(0x1A, 0x18, 0x18),
            band_text: rgb(0xF2, 0xEF, 0xEC),
            band_dim: rgb(0x8E, 0x88, 0x84),
            panel: rgb(0xFF, 0xFF, 0xFF),
            panel_border: rgb(0xE3, 0xE4, 0xEA),
            empty_border: rgb(0xD8, 0xD9, 0xE1),
            text: rgb(0x19, 0x1A, 0x1E),
            heading: rgb(0x3A, 0x3B, 0x44),
            dim: rgb(0x5F, 0x60, 0x69),
            placeholder: rgb(0x9A, 0x9C, 0xA6),
            accent: rgb(0xE0, 0x55, 0x1B),
            accent_text: rgb(0xFF, 0xFF, 0xFF),
            accent_down: rgb(0xB8, 0x44, 0x15),
            meter_have: rgb(0x3E, 0x3D, 0x45),
            button: rgb(0xEC, 0xEC, 0xF1),
            button_border: rgb(0xD3, 0xD4, 0xDC),
            button_text: rgb(0x19, 0x1A, 0x1E),
            check: rgb(0x2A, 0x2A, 0x31),
            disabled: rgb(0xF0, 0xF0, 0xF3),
            disabled_text: rgb(0xAF, 0xB0, 0xBA),
            trough: rgb(0xE4, 0xE5, 0xEB),
            log_bg: rgb(0x1A, 0x18, 0x18),
            log_border: rgb(0x2A, 0x27, 0x27),
            log_text: rgb(0xCF, 0xCB, 0xC7),
            rule: rgb(0xDF, 0xDF, 0xE6),
        }
    }

    /// The same window after dark. Not an inversion: the light theme's band is already near-black,
    /// so inverting it would leave the title bar lighter than the body it sits over. The band goes
    /// DEEPER than the page instead, the greys stay warm (they share their hue with the ember, and
    /// a neutral grey next to it reads blue), and the ember itself does not move — it means "the
    /// next thing to do" in both themes, and a colour that changes with the theme cannot carry a
    /// meaning.
    pub const fn dark() -> Theme {
        Theme {
            bg: rgb(0x1B, 0x1A, 0x19),
            band: rgb(0x0E, 0x0D, 0x0D),
            band_text: rgb(0xF2, 0xEF, 0xEC),
            band_dim: rgb(0x8E, 0x88, 0x84),
            panel: rgb(0x23, 0x21, 0x20),
            panel_border: rgb(0x33, 0x2F, 0x2D),
            empty_border: rgb(0x3D, 0x39, 0x36),
            text: rgb(0xED, 0xE9, 0xE5),
            heading: rgb(0xCF, 0xC9, 0xC3),
            dim: rgb(0x9A, 0x93, 0x8D),
            placeholder: rgb(0x85, 0x7E, 0x77),
            accent: rgb(0xE0, 0x55, 0x1B),
            accent_text: rgb(0xFF, 0xFF, 0xFF),
            accent_down: rgb(0xB8, 0x44, 0x15),
            meter_have: rgb(0x7A, 0x73, 0x6C),
            button: rgb(0x2B, 0x28, 0x26),
            button_border: rgb(0x3D, 0x39, 0x36),
            button_text: rgb(0xED, 0xE9, 0xE5),
            check: rgb(0xE0, 0xDB, 0xD5),
            disabled: rgb(0x20, 0x1E, 0x1D),
            disabled_text: rgb(0x5A, 0x54, 0x4F),
            trough: rgb(0x2E, 0x2B, 0x29),
            log_bg: rgb(0x12, 0x11, 0x10),
            log_border: rgb(0x2A, 0x27, 0x25),
            log_text: rgb(0xCF, 0xCB, 0xC7),
            rule: rgb(0x30, 0x2C, 0x2A),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

/// Which font a run of text is in. The backend maps these onto real faces; nothing above it names
/// a font, so the SVG and the window agree about which text is big and which is monospaced without
/// agreeing about what is installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    /// 20 px, semibold — the product name.
    Title,
    /// 22 px semibold — a number the user came here for. The largest thing on the window after
    /// the wordmark, because "how much" is what a transfer is about.
    Figure,
    /// 15 px — a path. One notch above the body text because it is the content of the window,
    /// not a description of it.
    Path,
    /// 12 px — the strapline, the captions and the quiet prose.
    Small,
    /// 14 px — everything else.
    Body,
    /// 14 px semibold — a button, a section heading, a card's name.
    Strong,
    /// 12 px monospaced — the log.
    Mono,
}

impl Face {
    /// Point size at 96 DPI.
    pub fn px(self) -> i32 {
        match self {
            Face::Title => 20,
            Face::Figure => 22,
            Face::Path => 15,
            Face::Small => 12,
            Face::Body => 14,
            Face::Strong => 14,
            Face::Mono => 12,
        }
    }

    pub fn bold(self) -> bool {
        matches!(self, Face::Title | Face::Strong | Face::Figure)
    }

    pub fn mono(self) -> bool {
        self == Face::Mono
    }

    /// Roughly how wide one character is, used only to decide where to cut a line that does not
    /// fit. It does not have to be right — GDI clips with `DT_END_ELLIPSIS` as well, and the SVG
    /// is a preview — it has to be *close*, so the preview elides in the same place the window
    /// does. Proportional faces average near 0.5 em; Consolas is 0.55 em exactly.
    pub fn advance(self) -> f32 {
        if self.mono() {
            self.px() as f32 * 0.55
        } else {
            self.px() as f32 * 0.52
        }
    }

    pub fn fits(self, width: i32) -> usize {
        (width as f32 / self.advance()).floor().max(0.0) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    /// Right-aligned, for the figures a meter is read against: numbers line up on their last
    /// digit or they cannot be compared at a glance.
    Right,
}

/// One drawing operation. Everything the window is made of is one of these three.
#[derive(Clone, PartialEq, Debug)]
pub enum Cmd {
    /// A filled and/or outlined rectangle. `radius` > 0 asks for rounded corners.
    Rect { rect: Rect, fill: Option<u32>, border: Option<u32>, radius: i32 },
    /// A run of text, vertically centred in `rect`.
    Text { rect: Rect, text: String, color: u32, face: Face, align: Align },
    /// An open polyline — the tick in a checkbox, and nothing else so far.
    Poly { points: Vec<(i32, i32)>, color: u32, width: i32 },
}

/// Cut `text` down to `width`, keeping the END. Paths are elided from the left because the part
/// that tells you which folder this is lives at the end of one; a path cut from the right elides
/// away exactly the part the user is checking.
pub fn elide_start(text: &str, face: Face, width: i32) -> String {
    let max = face.fits(width);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max || max < 2 {
        return text.to_string();
    }
    let keep = max - 1;
    format!("…{}", chars[chars.len() - keep..].iter().collect::<String>())
}

/// Cut `text` down to `width`, keeping the start. For prose, where the beginning is the meaning.
pub fn elide_end(text: &str, face: Face, width: i32) -> String {
    let max = face.fits(width);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max || max < 2 {
        return text.to_string();
    }
    format!("{}…", chars[..max - 1].iter().collect::<String>())
}

const CHECK_BOX: i32 = 16;

/// Everything the window paints, in back-to-front order.
pub fn commands(m: &Model, w: i32, h: i32, t: &Theme) -> Vec<Cmd> {
    let mut out = vec![Cmd::Rect {
        rect: Rect::new(0, 0, w.max(crate::MIN_W), h.max(crate::MIN_H)),
        fill: Some(t.bg),
        border: None,
        radius: 0,
    }];

    for wid in layout(m, w, h) {
        let r = wid.rect;
        match &wid.kind {
            Kind::Band => {
                out.push(Cmd::Rect { rect: r, fill: Some(t.band), border: None, radius: 0 });
                // The accent is a hairline under the band, which is the only place Flint's colour
                // appears when nothing is running — a whole orange bar is a warning, not a brand.
                out.push(Cmd::Rect {
                    rect: Rect::new(r.x, r.bottom() - 3, r.w, 3),
                    fill: Some(t.accent),
                    border: None,
                    radius: 0,
                });
            }
            Kind::Title => out.push(Cmd::Text {
                rect: r,
                text: wid.text.clone(),
                color: t.band_text,
                face: Face::Title,
                align: Align::Left,
            }),
            Kind::Subtitle => out.push(Cmd::Text {
                rect: r,
                text: elide_end(&wid.text, Face::Small, r.w),
                color: t.band_dim,
                face: Face::Small,
                align: Align::Left,
            }),
            Kind::Panel => {
                out.push(Cmd::Rect { rect: r, fill: Some(t.panel), border: Some(t.panel_border), radius: 6 })
            }
            // A card with a volume in it is a filled panel; one without is an outline on the page
            // background. An empty slot should look empty — a white panel with a placeholder in it
            // reads as something that failed to load.
            Kind::Card { filled } => out.push(Cmd::Rect {
                rect: r,
                fill: Some(if *filled { t.panel } else { t.bg }),
                border: Some(if *filled { t.panel_border } else { t.empty_border }),
                radius: 8,
            }),
            Kind::Version => out.push(Cmd::Text {
                rect: r,
                text: wid.text.clone(),
                color: t.band_dim,
                face: Face::Small,
                align: Align::Right,
            }),
            Kind::Heading => out.push(Cmd::Text {
                rect: r,
                text: wid.text.clone(),
                color: t.heading,
                face: Face::Strong,
                align: Align::Left,
            }),
            Kind::Caption => out.push(Cmd::Text {
                rect: r,
                text: wid.text.clone(),
                color: t.text,
                face: Face::Strong,
                align: Align::Left,
            }),
            Kind::Hint => out.push(Cmd::Text {
                rect: r,
                text: elide_end(&wid.text, Face::Small, r.w),
                color: t.dim,
                face: Face::Small,
                align: Align::Left,
            }),
            Kind::Detail => out.push(Cmd::Text {
                rect: r,
                text: elide_end(&wid.text, Face::Small, r.w),
                color: t.dim,
                face: Face::Small,
                align: Align::Right,
            }),
            Kind::Figure { warm } => out.push(Cmd::Text {
                rect: r,
                text: elide_end(&wid.text, Face::Figure, r.w),
                color: if *warm { t.accent } else { t.text },
                face: Face::Figure,
                align: Align::Left,
            }),
            // Three lengths on one bar: what is on the volume already, what this copy would add,
            // and what would still be free. The accent is the middle one — the only part of the
            // picture this window is about to create.
            Kind::Meter { have, add, known } => {
                out.push(Cmd::Rect { rect: r, fill: Some(t.trough), border: None, radius: 3 });
                if *known {
                    let have_w = (r.w as f32 * have.clamp(0.0, 1.0)) as i32;
                    let add_w = (r.w as f32 * add.clamp(0.0, 1.0)) as i32;
                    if have_w > 0 {
                        out.push(Cmd::Rect {
                            rect: Rect::new(r.x, r.y, have_w, r.h),
                            fill: Some(t.meter_have),
                            border: None,
                            radius: 3,
                        });
                    }
                    if add_w > 0 {
                        out.push(Cmd::Rect {
                            rect: Rect::new(r.x + have_w, r.y, add_w, r.h),
                            fill: Some(t.accent),
                            border: None,
                            radius: 3,
                        });
                    }
                }
            }
            // Outlined, never filled: these change nothing on the player, and a row of filled
            // buttons next to the transfer's own would say they are the same kind of thing.
            Kind::Tool { enabled } => {
                out.push(Cmd::Rect {
                    rect: r,
                    fill: None,
                    border: Some(if *enabled { t.button_border } else { t.disabled }),
                    radius: 5,
                });
                out.push(Cmd::Text {
                    rect: r,
                    text: elide_end(&wid.text, Face::Body, r.w - 12),
                    color: if *enabled { t.button_text } else { t.disabled_text },
                    face: Face::Body,
                    align: Align::Center,
                });
            }
            Kind::Rule => {
                out.push(Cmd::Rect { rect: Rect::new(r.x, r.y, r.w, 1), fill: Some(t.rule), border: None, radius: 0 })
            }
            Kind::Label => out.push(Cmd::Text {
                rect: r,
                text: wid.text.clone(),
                color: t.dim,
                face: Face::Small,
                align: Align::Left,
            }),
            Kind::Value { placeholder } => out.push(Cmd::Text {
                rect: r,
                text: elide_start(&wid.text, Face::Path, r.w),
                color: if *placeholder { t.placeholder } else { t.text },
                face: Face::Path,
                align: Align::Left,
            }),
            Kind::Button { primary, enabled } => {
                let (fill, border, ink) = match (*primary, *enabled) {
                    (_, false) => (t.disabled, t.button_border, t.disabled_text),
                    (true, true) => (t.accent, t.accent, t.accent_text),
                    (false, true) => (t.button, t.button_border, t.button_text),
                };
                out.push(Cmd::Rect { rect: r, fill: Some(fill), border: Some(border), radius: 5 });
                out.push(Cmd::Text {
                    rect: r,
                    text: elide_end(&wid.text, Face::Strong, r.w - 12),
                    color: ink,
                    face: Face::Strong,
                    align: Align::Center,
                });
            }
            Kind::Check { on, enabled } => {
                let box_y = r.y + (r.h - CHECK_BOX) / 2;
                let b = Rect::new(r.x, box_y, CHECK_BOX, CHECK_BOX);
                // A switch that `hit()` refuses must also LOOK refused. The pair to the disabled
                // button: a live-looking control that ignores the click is the same defect as a
                // dead-looking one that acts.
                let ink = if *enabled { t.check } else { t.disabled_text };
                out.push(Cmd::Rect {
                    rect: b,
                    fill: Some(if *on { ink } else { t.panel }),
                    border: Some(if *on { ink } else { t.button_border }),
                    radius: 3,
                });
                if *on {
                    out.push(Cmd::Poly {
                        points: vec![(b.x + 4, b.y + 8), (b.x + 7, b.y + 11), (b.x + 12, b.y + 5)],
                        color: t.accent_text,
                        width: 2,
                    });
                }
                let text_x = r.x + CHECK_BOX + 8;
                out.push(Cmd::Text {
                    rect: Rect::new(text_x, r.y, r.right() - text_x, r.h),
                    text: elide_end(&wid.text, Face::Body, r.right() - text_x),
                    color: if *enabled { t.text } else { t.disabled_text },
                    face: Face::Body,
                    align: Align::Left,
                });
            }
            Kind::Progress(fill) => {
                out.push(Cmd::Rect { rect: r, fill: Some(t.trough), border: None, radius: r.h / 2 });
                // `None` is a job with no measurable length, and it leaves the trough empty
                // rather than guessing at a fraction.
                if let Some(f) = fill {
                    let done = (r.w as f32 * f.clamp(0.0, 1.0)) as i32;
                    if done > 0 {
                        out.push(Cmd::Rect {
                            rect: Rect::new(r.x, r.y, done, r.h),
                            fill: Some(t.accent),
                            border: None,
                            radius: r.h / 2,
                        });
                    }
                }
            }
            Kind::Status => out.push(Cmd::Text {
                rect: r,
                text: elide_end(&wid.text, Face::Body, r.w),
                color: t.dim,
                face: Face::Body,
                align: Align::Left,
            }),
            Kind::LogPane => {
                out.push(Cmd::Rect { rect: r, fill: Some(t.log_bg), border: Some(t.log_border), radius: 6 })
            }
            Kind::LogLine => out.push(Cmd::Text {
                rect: r,
                text: elide_start(&wid.text, Face::Mono, r.w),
                color: t.log_text,
                face: Face::Mono,
                align: Align::Left,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// The accent means "the next thing to do", so it cannot depend on the theme — and the two
    /// themes have to actually differ, or `--dark` is a no-op nobody would notice until Windows
    /// switched on its own.
    #[test]
    fn dark_keeps_the_accent_and_flips_the_page() {
        let (l, d) = (super::Theme::light(), super::Theme::dark());
        assert_eq!(l.accent, d.accent);
        assert_eq!(l.accent_down, d.accent_down);
        let lum = |c: u32| {
            (super::r_of(c) as u32 * 299 + super::g_of(c) as u32 * 587 + super::b_of(c) as u32 * 114) / 1000
        };
        assert!(lum(l.bg) > 200, "the light page is light");
        assert!(lum(d.bg) < 60, "the dark page is dark");
        // Text has to land on the right side of its own background in both.
        assert!(lum(l.text) < lum(l.bg));
        assert!(lum(d.text) > lum(d.bg));
        assert!(lum(d.dim) > lum(d.bg) + 40, "dim text stays readable on the dark page");
    }

    use super::*;
    use crate::{Id, Job, Phase, H, W};

    fn ready() -> Model {
        let mut m = Model::new();
        m.library = Some("/music".into());
        m.volumes[0] = Some("/player".into());
        m
    }

    /// Nothing is painted outside the window. The layout test says the same of the widgets; this
    /// says it of what is actually drawn, which is not the same thing — a checkbox's tick and a
    /// button's rounded border are computed here, not there.
    #[test]
    fn nothing_is_painted_off_the_window() {
        let mut m = ready();
        m.log = (0..40).map(|i| format!("line {i}")).collect();
        let t = Theme::light();
        for (w, h) in [(W, H), (crate::MIN_W, crate::MIN_H)] {
            for c in commands(&m, w, h, &t) {
                match c {
                    Cmd::Rect { rect, .. } | Cmd::Text { rect, .. } => {
                        assert!(rect.x >= 0 && rect.y >= 0, "{rect:?}");
                        assert!(rect.right() <= w && rect.bottom() <= h, "{rect:?} in {w}x{h}");
                    }
                    Cmd::Poly { points, .. } => {
                        for (x, y) in points {
                            assert!(x >= 0 && y >= 0 && x <= w && y <= h, "({x},{y})");
                        }
                    }
                }
            }
        }
    }

    /// A disabled button is drawn differently from a live one. This is the pair to the hit test:
    /// `hit()` refuses a disabled control, so it must also *look* refused.
    #[test]
    fn a_disabled_button_does_not_look_live() {
        let t = Theme::light();
        let mut m = ready();
        // COPY is disabled until a plan exists, and PLAN is live.
        let inks = |m: &Model| -> Vec<u32> {
            commands(m, W, H, &t)
                .into_iter()
                .filter_map(|c| match c {
                    Cmd::Rect { fill: Some(f), radius: 5, .. } => Some(f),
                    _ => None,
                })
                .collect()
        };
        let before = inks(&m);
        assert!(before.contains(&t.disabled), "COPY should be drawn disabled before a plan");
        m.planned = true;
        let after = inks(&m);
        assert!(after.contains(&t.accent), "a live primary button is drawn in the accent");
        assert!(
            after.iter().filter(|c| **c == t.disabled).count() < before.iter().filter(|c| **c == t.disabled).count(),
            "planning should have lit a button up"
        );
    }

    /// While a job runs, the only things drawn in the accent are the band's hairline and the
    /// progress fill. Every control is drawn refused, because every control IS refused — the pair
    /// to `hit()` returning `None` for all of them.
    #[test]
    fn a_busy_window_has_no_live_looking_control() {
        let t = Theme::light();
        let mut m = ready();
        m.planned = true;
        m.phase = Phase::Working;
        m.progress = Some(0.5);
        for c in commands(&m, W, H, &t) {
            if let Cmd::Rect { rect, fill: Some(f), radius, .. } = c {
                if f != t.accent {
                    continue;
                }
                let hairline = rect.h == 3 && rect.y < 64;
                let progress = rect.h == 8;
                assert!(hairline || progress, "something is drawn live while a job runs: {rect:?} radius {radius}");
            }
        }
        // …and every control really is refused, so the two agree.
        for wid in layout(&m, W, H) {
            if wid.id == crate::Id::Stop || wid.id == crate::Id::None {
                continue;
            }
            let cx = wid.rect.x + wid.rect.w / 2;
            let cy = wid.rect.y + wid.rect.h / 2;
            assert_eq!(crate::hit(&m, W, H, cx, cy), None, "{:?} still answers", wid.id);
        }
    }

    /// A long path keeps its tail, because the tail says which folder it is.
    #[test]
    fn a_long_path_is_elided_from_the_left() {
        let short = elide_start("/music", Face::Body, 400);
        assert_eq!(short, "/music");
        let long = elide_start("/home/someone/Music/Albums/Artist/1999 - The Record", Face::Body, 120);
        assert!(long.starts_with('…'), "{long}");
        assert!(long.ends_with("The Record"), "{long}");
        assert!(long.chars().count() <= Face::Body.fits(120));
    }

    /// The tick is inside its box, and only there when the switch is on.
    #[test]
    fn the_tick_is_inside_the_box_and_only_when_on() {
        let t = Theme::light();
        let mut m = ready();
        m.sensme = true;
        m.extras = false;
        let polys: Vec<Vec<(i32, i32)>> = commands(&m, W, H, &t)
            .into_iter()
            .filter_map(|c| match c {
                Cmd::Poly { points, .. } => Some(points),
                _ => None,
            })
            .collect();
        assert_eq!(polys.len(), 1, "one switch is on, so one tick");
        let check = layout(&m, W, H).into_iter().find(|w| w.id == Id::ToggleSensMe).unwrap();
        for (x, y) in &polys[0] {
            assert!(*x >= check.rect.x && *x <= check.rect.x + CHECK_BOX, "the tick escaped its box: {x}");
            assert!(*y >= check.rect.y && *y <= check.rect.bottom());
        }
    }

    /// A job running draws a progress bar that fills, and the trough is there either way so the
    /// window does not change height when one starts.
    #[test]
    fn progress_fills_only_while_working() {
        let t = Theme::light();
        let mut m = ready();
        let trough_and_fill = |m: &Model| {
            let mut trough = 0;
            let mut fill = 0;
            for c in commands(m, W, H, &t) {
                if let Cmd::Rect { rect, fill: Some(f), .. } = c {
                    if rect.h == 8 && f == t.trough {
                        trough += 1;
                    }
                    if rect.h == 8 && f == t.accent {
                        fill += 1;
                    }
                }
            }
            (trough, fill)
        };
        // At rest there is no bar at all: a trough with nothing in it is a control that controls
        // nothing, and the room it was holding belongs to the log.
        assert_eq!(trough_and_fill(&m), (0, 0));
        m.phase = Phase::Working;
        m.progress = Some(0.5);
        assert_eq!(trough_and_fill(&m), (1, 1));
        // A job with no measurable length leaves the trough empty rather than guessing.
        m.progress = None;
        assert_eq!(trough_and_fill(&m), (1, 0));
    }

    /// Every job the window can start is reachable from some button. If a `Job` is added and no
    /// button offers it, this fails rather than the job quietly being unreachable.
    #[test]
    fn every_job_has_a_button() {
        let mut m = ready();
        m.planned = true;
        let ids: Vec<Id> = layout(&m, W, H).into_iter().map(|w| w.id).collect();
        for (id, job) in [
            (Id::Plan, Job::Plan),
            (Id::Apply, Job::Apply),
            (Id::Scan, Job::Scan),
            (Id::Import, Job::Import),
            (Id::Check, Job::Check),
        ] {
            assert!(ids.contains(&id), "{job:?} has no button");
            let mut m2 = m.clone();
            assert_eq!(crate::click(&mut m2, id), Some(job));
        }
    }
}
