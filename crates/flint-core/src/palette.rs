//! Cinder palettes, checked on the PC before they go to the player.
//!
//! A palette is a small text file (`slate.palette`) of colours that Cinder reads from
//! `cinder_palettes/` on the player. Cinder REFUSES a palette it would be hard to read — and until
//! now the only place that said so was a log file on the player, found after the copy, the unplug
//! and a trip through Settings. This module is Cinder's parser and readability rules, ported so
//! Flint can give the same verdict before anything is copied.
//!
//! **This is a port, and it must stay one.** The source of truth is Cinder's
//! `player/cinder-ui/src/palette.rs` (parse, `problems`) and `theme.rs` (the six accents and the
//! night dim). If the two ever disagree, the player is right and this file is the bug. The tests
//! below carry Cinder's own shipped palettes and its own calibration numbers for that reason.

/// The folder on the player's drive.
pub const DIR_NAME: &str = "cinder_palettes";
/// Compared case-insensitively.
pub const EXTENSION: &str = "palette";
/// Anything bigger is not a palette (the player reads the folder on its render thread).
pub const MAX_BYTES: u64 = 16 * 1024;
/// How many palettes the player loads.
pub const MAX_FILES: usize = 32;
/// Longest display name, in characters.
pub const MAX_NAME: usize = 16;
/// Longest id (the file name without `.palette`).
pub const MAX_ID: usize = 32;
/// The built-in palette's id. A file may not use it.
pub const BUILTIN_ID: &str = "cinder";

const NEUTRAL_KEYS: [&str; 6] = ["bg", "panel", "line", "ink", "dim", "faint"];
const ACCENT_KEYS: [&str; 6] =
    ["day.accent", "day.accent_ink", "day.row_select", "night.accent", "night.accent_ink", "night.row_select"];

/// Six neutral colours of one mode, `0xRRGGBB`. Night values are written BEFORE the night dim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neutrals {
    pub bg: u32,
    pub panel: u32,
    pub line: u32,
    pub ink: u32,
    pub dim: u32,
    pub faint: u32,
}

/// One accent in one mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccentTokens {
    pub acc: u32,
    pub acc_ink: u32,
    pub row_sel: u32,
}

/// A whole palette: day and night neutrals, and an accent of its own if it brings one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub day: Neutrals,
    pub night: Neutrals,
    pub accent: Option<(AccentTokens, AccentTokens)>,
}

/// Cinder's own palette (`theme.rs` `CINDER`). Every key a file leaves out falls back to this.
pub const CINDER: Tokens = Tokens {
    day: Neutrals { bg: 0x0d0c0b, panel: 0x13110f, line: 0x221f1b, ink: 0xece7df, dim: 0x95908a, faint: 0x5f5a52 },
    night: Neutrals { bg: 0x000000, panel: 0x0a0908, line: 0x161310, ink: 0x8d8170, dim: 0x5b5347, faint: 0x3b362d },
    accent: None,
};

/// Cinder's six built-in accents, `(name, day, night)` — `theme.rs` `ACCENT_ROWS`. A palette without
/// an accent of its own is checked against every one, because the player offers all six.
pub const ACCENTS: [(&str, AccentTokens, AccentTokens); 6] = [
    (
        "AMBER",
        AccentTokens { acc: 0xf4651f, acc_ink: 0x1a0a02, row_sel: 0x1c1713 },
        AccentTokens { acc: 0x863810, acc_ink: 0x000000, row_sel: 0x0f0c0a },
    ),
    (
        "CRIMSON",
        AccentTokens { acc: 0xe0392f, acc_ink: 0x1a0403, row_sel: 0x1c1214 },
        AccentTokens { acc: 0x7a1f1a, acc_ink: 0x000000, row_sel: 0x0f0a0b },
    ),
    (
        "VIOLET",
        AccentTokens { acc: 0x9a6ff0, acc_ink: 0x0b0618, row_sel: 0x15141f },
        AccentTokens { acc: 0x553d84, acc_ink: 0x000000, row_sel: 0x0b0a10 },
    ),
    (
        "AZURE",
        AccentTokens { acc: 0x2f8fe0, acc_ink: 0x020a16, row_sel: 0x12161f },
        AccentTokens { acc: 0x1a4e7a, acc_ink: 0x000000, row_sel: 0x0a0b10 },
    ),
    (
        "MINT",
        AccentTokens { acc: 0x2fc98a, acc_ink: 0x02120c, row_sel: 0x121a17 },
        AccentTokens { acc: 0x1a6e4c, acc_ink: 0x000000, row_sel: 0x0a0e0c },
    ),
    (
        "BONE",
        AccentTokens { acc: 0xd8d2c8, acc_ink: 0x0d0c0b, row_sel: 0x1a1917 },
        AccentTokens { acc: 0x77736e, acc_ink: 0x000000, row_sel: 0x0e0d0c },
    ),
];

/// Night mode scales every colour but the ink drawn ON the accent by this, on the way to the panel.
pub const NIGHT_DIM_PCT: u32 = 55;

/// The colours as they reach the panel, for one mode and one accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub bg: u32,
    pub panel: u32,
    pub line: u32,
    pub ink: u32,
    pub dim: u32,
    pub faint: u32,
    pub acc: u32,
    pub acc_ink: u32,
    pub row_sel: u32,
}

fn dim_rgb(c: u32, pct: u32) -> u32 {
    let r = ((c >> 16) & 0xff) * pct / 100;
    let g = ((c >> 8) & 0xff) * pct / 100;
    let b = (c & 0xff) * pct / 100;
    (r << 16) | (g << 8) | b
}

impl Tokens {
    /// What the player draws for `night` with built-in accent `accent` (index into [`ACCENTS`]).
    pub fn shown(&self, night: bool, accent: usize) -> Shown {
        let ac = match self.accent {
            Some((d, n)) => {
                if night {
                    n
                } else {
                    d
                }
            }
            None => {
                let (_, d, n) = ACCENTS[accent.min(ACCENTS.len() - 1)];
                if night {
                    n
                } else {
                    d
                }
            }
        };
        let n = if night { &self.night } else { &self.day };
        let d = |c: u32| if night { dim_rgb(c, NIGHT_DIM_PCT) } else { c };
        Shown {
            bg: d(n.bg),
            panel: d(n.panel),
            line: d(n.line),
            ink: d(n.ink),
            dim: d(n.dim),
            faint: d(n.faint),
            acc: d(ac.acc),
            // Not dimmed: the dark half of the accent pair already.
            acc_ink: ac.acc_ink,
            row_sel: d(ac.row_sel),
        }
    }
}

/// A palette file that passed.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub id: String,
    pub name: String,
    pub tokens: Tokens,
}

/// `slate.palette` → `slate`. Hidden files (macOS `._slate.palette`) are not palettes.
pub fn palette_stem(file: &str) -> Option<&str> {
    let (stem, ext) = file.rsplit_once('.')?;
    (ext.eq_ignore_ascii_case(EXTENSION) && !stem.is_empty() && !stem.starts_with('.')).then_some(stem)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn parse_colour(v: &str) -> Option<u32> {
    let hex = v.strip_prefix('#').unwrap_or(v);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Parse and check one palette file, exactly as the player does. `id` is the file name without its
/// extension, lowercased. Every problem is reported, not just the first.
pub fn parse(id: &str, body: &str) -> Result<Palette, Vec<String>> {
    let mut errs = Vec::new();
    if !valid_id(id) {
        errs.push(format!(
            "`{id}` cannot be a palette name: use lowercase letters, digits, - and _ (at most {MAX_ID})"
        ));
    } else if id == BUILTIN_ID {
        errs.push(format!("`{BUILTIN_ID}` is the built-in palette — rename the file"));
    }
    let mut tokens = CINDER;
    let mut name = None;
    let mut seen: Vec<&str> = Vec::new();
    let mut accent: [Option<u32>; 6] = [None; 6];
    for (i, raw) in body.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            errs.push(format!("line {n}: expected `key = value`"));
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if seen.contains(&k) {
            errs.push(format!("line {n}: `{k}` is set twice"));
            continue;
        }
        seen.push(k);
        if k == "name" {
            let len = v.chars().count();
            if len == 0 || len > MAX_NAME || v.chars().any(char::is_control) {
                errs.push(format!("line {n}: `name` must be 1 to {MAX_NAME} characters"));
            } else {
                name = Some(v.to_string());
            }
            continue;
        }
        let slot = if let Some(i) = ACCENT_KEYS.iter().position(|a| *a == k) {
            Some((None, i))
        } else {
            k.split_once('.').and_then(|(mode, field)| {
                let night = match mode {
                    "day" => false,
                    "night" => true,
                    _ => return None,
                };
                NEUTRAL_KEYS.iter().position(|f| *f == field).map(|f| (Some(night), f))
            })
        };
        let Some(slot) = slot else {
            errs.push(format!("line {n}: unknown key `{k}`"));
            continue;
        };
        let Some(colour) = parse_colour(v) else {
            errs.push(format!("line {n}: `{k}` wants a colour like #1a2b3c, got `{v}`"));
            continue;
        };
        match slot {
            (Some(night), field) => {
                let set = if night { &mut tokens.night } else { &mut tokens.day };
                *match field {
                    0 => &mut set.bg,
                    1 => &mut set.panel,
                    2 => &mut set.line,
                    3 => &mut set.ink,
                    4 => &mut set.dim,
                    _ => &mut set.faint,
                } = colour;
            }
            (None, i) => accent[i] = Some(colour),
        }
    }
    match accent {
        [Some(da), Some(di), Some(ds), Some(na), Some(ni), Some(ns)] => {
            tokens.accent = Some((
                AccentTokens { acc: da, acc_ink: di, row_sel: ds },
                AccentTokens { acc: na, acc_ink: ni, row_sel: ns },
            ));
        }
        [None, None, None, None, None, None] => {}
        _ => {
            let missing: Vec<&str> =
                ACCENT_KEYS.iter().zip(accent.iter()).filter(|(_, v)| v.is_none()).map(|(k, _)| *k).collect();
            errs.push(format!("an accent of its own needs all six accent keys — missing {}", missing.join(", ")));
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let problems = problems(&tokens);
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(Palette { id: id.to_string(), name: name.unwrap_or_else(|| id.to_string()), tokens })
}

/// Contrast floors for one mode (WCAG ratios, 1.0 to 21.0).
#[derive(Clone, Copy, Debug)]
struct Floors {
    ink: f32,
    dim: f32,
    faint: f32,
    accent: f32,
}
const DAY: Floors = Floors { ink: 4.5, dim: 3.0, faint: 1.8, accent: 3.0 };
const NIGHT: Floors = Floors { ink: 1.9, dim: 1.35, faint: 1.12, accent: 1.2 };
const NIGHT_BG_MAX: f32 = 0.02;

/// Everything about `t` that would make the player hard to read, one sentence each. Empty = fine.
pub fn problems(t: &Tokens) -> Vec<String> {
    let mut out = Vec::new();
    for night in [false, true] {
        let mode = if night { "night" } else { "day" };
        let f = if night { NIGHT } else { DAY };
        let th = t.shown(night, 0);
        let ink = contrast(th.ink, th.bg);
        let dim = contrast(th.dim, th.bg);
        let faint = contrast(th.faint, th.bg);
        floor(&mut out, format!("{mode}.ink on {mode}.bg"), ink, f.ink);
        floor(&mut out, format!("{mode}.dim on {mode}.bg"), dim, f.dim);
        floor(&mut out, format!("{mode}.faint on {mode}.bg"), faint, f.faint);
        floor(&mut out, format!("{mode}.ink on {mode}.panel"), contrast(th.ink, th.panel), f.ink);
        if dim >= ink {
            out.push(format!(
                "{mode}.dim stands out as much as {mode}.ink ({dim:.2} vs {ink:.2}) — dim is for secondary text"
            ));
        }
        if faint >= dim {
            out.push(format!("{mode}.faint stands out as much as {mode}.dim ({faint:.2} vs {dim:.2})"));
        }
        if night && luminance(th.bg) > NIGHT_BG_MAX {
            out.push(format!(
                "night.bg is too bright for night mode (luminance {:.3}, at most {NIGHT_BG_MAX})",
                luminance(th.bg)
            ));
        }
        let pinned = t.accent.is_some();
        let accents: Vec<usize> = if pinned { vec![0] } else { (0..ACCENTS.len()).collect() };
        type Measure = fn(&Shown) -> f32;
        let checks: [(&str, &str, Measure); 3] = [
            ("accent", "bg", |s| contrast(s.acc, s.bg)),
            ("accent_ink", "accent", |s| contrast(s.acc_ink, s.acc)),
            ("accent", "row_select", |s| contrast(s.acc, s.row_sel)),
        ];
        for (what, on, measure) in checks {
            let worst = accents.iter().map(|&a| (a, measure(&t.shown(night, a)))).min_by(|x, y| x.1.total_cmp(&y.1));
            let Some((a, got)) = worst else { continue };
            if got >= f.accent {
                continue;
            }
            if pinned {
                out.push(format!("{mode}.{what} on {mode}.{on}: contrast {got:.2}, needs at least {:.2}", f.accent));
            } else {
                out.push(format!(
                    "{mode}: the built-in {} accent measures {got:.2} for {what} on {on} (needs {:.2}) — \
                     give the palette an accent of its own (all six accent keys)",
                    ACCENTS[a].0, f.accent
                ));
            }
        }
    }
    out
}

fn floor(out: &mut Vec<String>, what: String, got: f32, need: f32) {
    if got < need {
        out.push(format!("{what}: contrast {got:.2}, needs at least {need:.2}"));
    }
}

fn channel(c: u32) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance of `0xRRGGBB`.
pub fn luminance(c: u32) -> f32 {
    0.2126 * channel((c >> 16) & 0xff) + 0.7152 * channel((c >> 8) & 0xff) + 0.0722 * channel(c & 0xff)
}

/// WCAG contrast ratio, 1.0 to 21.0, in either order.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// What the player will make of one file: its id, and the palette or the reasons it is refused.
pub fn check_file(file_name: &str, body: &str) -> Option<(String, Result<Palette, Vec<String>>)> {
    let id = palette_stem(file_name)?.to_ascii_lowercase();
    let verdict = parse(&id, body);
    Some((id, verdict))
}

// ── A PC folder against the player's ──────────────────────────────────────────────────────────

/// Where one palette stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Cinder's own palette: always on the player, never a file.
    BuiltIn,
    /// On this PC and not on the player yet: Send copies it.
    New,
    /// On both, but the player's copy differs: Send replaces it.
    Changed,
    /// On both, the same.
    On,
    /// On the player only.
    PlayerOnly,
    /// The player would refuse it. `Row::why` says why; Send never copies it.
    Refused,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::BuiltIn => "BUILT IN",
            State::New => "NEW",
            State::Changed => "CHANGED",
            State::On => "ON",
            State::PlayerOnly => "ON THE PLAYER",
            State::Refused => "REFUSED",
        }
    }
}

/// One row of the Palettes page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The file name, as found; empty for the built-in palette.
    pub file: String,
    pub name: String,
    pub state: State,
    /// The first reason the player would refuse it — the one to fix first. Empty otherwise.
    pub why: String,
    /// bg, line, dim, ink, accent as the player draws them by day. `None` for a refused file.
    pub cells: Option<[u32; 5]>,
}

fn cells(t: &Tokens) -> [u32; 5] {
    let s = t.shown(false, 0);
    [s.bg, s.line, s.dim, s.ink, s.acc]
}

/// Compare the palette files in a PC folder with the ones on the player, both as `(file name,
/// contents)`. The built-in palette leads, then every palette by name. A file refused on the PC is
/// REFUSED whatever the player holds.
pub fn compare(pc: &[(String, String)], player: &[(String, String)]) -> Vec<Row> {
    let key = |f: &str| palette_stem(f).map(str::to_ascii_lowercase);
    let mut rows = vec![Row {
        file: String::new(),
        name: "Cinder".into(),
        state: State::BuiltIn,
        why: String::new(),
        cells: Some(cells(&CINDER)),
    }];
    let mut rest: Vec<Row> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (file, body) in pc {
        let Some(id) = key(file) else { continue };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        let theirs = player.iter().find(|(f, _)| key(f).as_deref() == Some(id.as_str()));
        rest.push(match parse(&id, body) {
            Err(errs) => Row {
                file: file.clone(),
                name: id.clone(),
                state: State::Refused,
                why: errs.first().cloned().unwrap_or_default(),
                cells: None,
            },
            Ok(p) => Row {
                file: file.clone(),
                name: p.name.clone(),
                state: match theirs {
                    None => State::New,
                    Some((_, b)) if b == body => State::On,
                    Some(_) => State::Changed,
                },
                why: String::new(),
                cells: Some(cells(&p.tokens)),
            },
        });
    }
    for (file, body) in player {
        let Some(id) = key(file) else { continue };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        rest.push(match parse(&id, body) {
            Err(errs) => Row {
                file: file.clone(),
                name: id,
                state: State::Refused,
                why: format!("on the player: {}", errs.first().cloned().unwrap_or_default()),
                cells: None,
            },
            Ok(p) => Row {
                file: file.clone(),
                name: p.name.clone(),
                state: State::PlayerOnly,
                why: String::new(),
                cells: Some(cells(&p.tokens)),
            },
        });
    }
    rest.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.file.cmp(&b.file)));
    rows.extend(rest);
    rows
}

/// The files Send copies: new and changed, never refused, at most as many as the player loads.
pub fn to_send(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter(|r| matches!(r.state, State::New | State::Changed))
        .map(|r| r.file.as_str())
        .take(MAX_FILES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cinder's shipped palettes, copied from `player/cinder-ui/palettes/`. They load on the player,
    // so they must load here.
    const SLATE: &str = "name = Slate\nday.bg = #0e1116\nday.panel = #141820\nday.line = #232a35\n\
        day.ink = #e6ebf2\nday.dim = #8d97a5\nday.faint = #58616e\nnight.bg = #000000\n\
        night.panel = #0a0c10\nnight.line = #151a21\nnight.ink = #8a93a0\nnight.dim = #57606c\n\
        night.faint = #373d46\n";
    const PAPER: &str = "name = Paper\nday.bg = #f4f1ea\nday.panel = #ebe6dc\nday.line = #d6d0c4\n\
        day.ink = #1c1a17\nday.dim = #5f5a52\nday.faint = #8f887d\nnight.bg = #000000\n\
        night.panel = #0a0908\nnight.line = #161310\nnight.ink = #8d8170\nnight.dim = #5b5347\n\
        night.faint = #3b362d\nday.accent = #c4471a\nday.accent_ink = #ffffff\n\
        day.row_select = #e9e2d4\nnight.accent = #863810\nnight.accent_ink = #000000\n\
        night.row_select = #0f0c0a\n";

    #[test]
    fn cinders_own_palettes_pass() {
        assert_eq!(parse("slate", SLATE).map(|p| p.name), Ok("Slate".to_string()));
        let paper = parse("paper", PAPER).expect("paper loads on the player");
        assert!(paper.tokens.accent.is_some(), "a light palette brings its own accent");
        assert!(problems(&CINDER).is_empty(), "Cinder passes its own rules with every accent");
    }

    /// Cinder's calibration numbers (`palette.rs`: day 15.9 / 6.2 / 2.9, night 2.26 / 1.54 / 1.25).
    #[test]
    fn the_contrast_maths_matches_cinders_measurements() {
        let day = CINDER.shown(false, 0);
        let night = CINDER.shown(true, 0);
        let near = |a: f32, b: f32| (a - b).abs() < 0.05;
        assert!(near(contrast(day.ink, day.bg), 15.9), "{}", contrast(day.ink, day.bg));
        assert!(near(contrast(day.dim, day.bg), 6.2));
        assert!(near(contrast(day.faint, day.bg), 2.9));
        assert!(near(contrast(night.ink, night.bg), 2.26), "{}", contrast(night.ink, night.bg));
        assert!(near(contrast(night.dim, night.bg), 1.54));
        assert!(near(contrast(night.faint, night.bg), 1.25));
    }

    #[test]
    fn unreadable_and_broken_files_are_refused_with_every_reason() {
        let errs = parse("neon", "name = Neon\nday.ink = #0e0d0c\n").unwrap_err();
        assert!(errs.iter().any(|e| e.starts_with("day.ink on day.bg: contrast")), "{errs:?}");
        let errs = parse("bad", "day.ink = pink\nwhat\nday.ink = #ffffff\nday.accent = #ff0000\n").unwrap_err();
        assert!(errs.iter().any(|e| e.contains("wants a colour")));
        assert!(errs.iter().any(|e| e.contains("expected `key = value`")));
        assert!(errs.iter().any(|e| e.contains("set twice")));
        assert!(errs.iter().any(|e| e.contains("all six accent keys")));
        assert!(parse("cinder", "").unwrap_err()[0].contains("built-in"));
        assert!(parse("Has Space", "").is_err());
        let light =
            "day.bg = #ffffff\nday.panel = #f4f4f4\nday.ink = #111111\nday.dim = #555555\nday.faint = #999999\n";
        let errs = parse("light", light).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("give the palette an accent of its own")), "{errs:?}");
    }

    #[test]
    fn a_folder_compares_with_the_player() {
        let pc = vec![
            ("Slate.palette".to_string(), SLATE.to_string()),
            ("paper.palette".to_string(), PAPER.to_string()),
            ("neon.palette".to_string(), "day.ink = #0e0d0c\n".to_string()),
            ("notes.txt".to_string(), "not a palette".to_string()),
        ];
        let player = vec![
            ("slate.palette".to_string(), SLATE.to_string()),
            ("paper.palette".to_string(), "name = Old Paper\n".to_string()),
            ("mine.palette".to_string(), "name = Mine\n".to_string()),
        ];
        let rows = compare(&pc, &player);
        let got: Vec<(&str, State)> = rows.iter().map(|r| (r.name.as_str(), r.state)).collect();
        assert_eq!(
            got,
            vec![
                ("Cinder", State::BuiltIn),
                ("Mine", State::PlayerOnly),
                ("neon", State::Refused),
                ("Paper", State::Changed),
                ("Slate", State::On),
            ]
        );
        assert!(rows.iter().find(|r| r.name == "neon").unwrap().why.starts_with("day.ink on day.bg"));
        assert_eq!(to_send(&rows), vec!["paper.palette"], "refused and identical files stay put");
    }

    #[test]
    fn only_palette_files_are_palettes() {
        assert_eq!(palette_stem("Slate.PALETTE"), Some("Slate"));
        assert_eq!(palette_stem("._slate.palette"), None);
        assert_eq!(palette_stem("slate.txt"), None);
        assert_eq!(check_file("Slate.palette", SLATE).map(|(id, v)| (id, v.is_ok())), Some(("slate".into(), true)));
    }
}
