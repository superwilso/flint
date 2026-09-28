//! The pages behind the tabs, other than Sync (which is `layout` in `lib.rs`).
//!
//! Each page is one question about the library or the player, answered from something Flint
//! already reads: the check store, the manifests, the scrobble log, the likes file, the palette
//! folder. Nothing on these pages writes to the player — "Read the player" only reads — so none of
//! them carries the accent. The accent on this window means bytes about to be written, and those
//! only ever come from Sync.
//!
//! Same rule as the rest of the window: this is the only place that knows where a page's widgets
//! are, and the paint and the hit test both read it.

use crate::{thousands, AlbumRow, Id, Kind, Model, Phase, Rect, Tab, ThemePref, Tone, Widget, BAND_H, BTN_H, PAD};

fn w(id: Id, rect: Rect, kind: Kind, text: impl Into<String>) -> Widget {
    Widget { id, rect, kind, text: text.into() }
}

fn heading(out: &mut Vec<Widget>, x: i32, y: i32, text: &str) {
    out.push(w(Id::None, Rect::new(x, y, 400, 18), Kind::Heading, text));
}

fn hint(out: &mut Vec<Widget>, x: i32, y: i32, width: i32, text: &str) {
    out.push(w(Id::None, Rect::new(x, y, width, 18), Kind::Hint, text));
}

fn cell(tone: Tone, strong: bool, mono: bool) -> Kind {
    Kind::Cell { tone, strong, mono }
}

/// The page on show, under the band. `out` already holds the band.
pub fn layout(m: &Model, w_: i32, h: i32, out: &mut Vec<Widget>) {
    let inner = w_ - PAD * 2;
    let y = BAND_H + 24;
    let bottom = h - PAD - footer_h(m);
    match m.tab {
        Tab::Sync => {}
        Tab::Player => player(m, inner, y, bottom, out),
        Tab::Check => check(m, inner, y, bottom, out),
        Tab::SensMe => sensme(m, inner, y, bottom, out),
        Tab::Likes => likes(m, inner, y, bottom, out),
        Tab::Palettes => palettes(m, inner, y, bottom, out),
        Tab::Settings => settings(m, inner, y, out),
    }
    footer(m, inner, h, out);
}

/// Height kept at the bottom of a page for the status line (and the bar, while a job runs).
fn footer_h(m: &Model) -> i32 {
    if m.tab == Tab::Settings {
        0
    } else if m.phase == Phase::Working {
        46
    } else {
        30
    }
}

/// The status line — what the last job said — and, while one runs, its bar and a Stop button.
fn footer(m: &Model, inner: i32, h: i32, out: &mut Vec<Widget>) {
    if m.tab == Tab::Settings {
        return;
    }
    let mut y = h - PAD - 20;
    if m.phase == Phase::Working {
        const STOP_W: i32 = 90;
        out.push(w(Id::None, Rect::new(PAD, y - 20, inner - STOP_W - 12, 8), Kind::Progress(m.progress), ""));
        out.push(w(
            Id::Stop,
            Rect::new(PAD + inner - STOP_W, y - 30, STOP_W, BTN_H),
            Kind::Button { primary: false, enabled: true },
            "Stop",
        ));
        y = h - PAD - 18;
    }
    out.push(w(Id::None, Rect::new(PAD, y, inner - 100, 20), Kind::Status, m.status.clone()));
}

/// A table: a header row, then as many rows as fit above `bottom`. `cols` are
/// `(title, width, right-aligned-number?)`; the last column takes what is left. Returns how many
/// rows were drawn.
fn table(
    out: &mut Vec<Widget>,
    x: i32,
    y: i32,
    width: i32,
    bottom: i32,
    cols: &[(&str, i32)],
    rows: &[Vec<(String, Tone, bool)>],
) -> usize {
    const ROW_H: i32 = 26;
    let widths: Vec<i32> = cols
        .iter()
        .enumerate()
        .map(|(i, (_, cw))| if i + 1 == cols.len() { width - cols[..i].iter().map(|c| c.1).sum::<i32>() } else { *cw })
        .collect();
    out.push(w(Id::None, Rect::new(x, y, width, 30), Kind::Card { filled: true }, ""));
    let mut cx = x + 12;
    for (i, (title, _)) in cols.iter().enumerate() {
        out.push(w(Id::None, Rect::new(cx, y + 6, widths[i] - 12, 18), Kind::Label, *title));
        cx += widths[i];
    }
    let mut ry = y + 36;
    let mut shown = 0;
    for row in rows {
        if ry + ROW_H > bottom {
            break;
        }
        out.push(w(Id::None, Rect::new(x, ry - 2, width, 1), Kind::Rule, ""));
        let mut cx = x + 12;
        for (i, (text, tone, strong)) in row.iter().enumerate() {
            let cw = widths.get(i).copied().unwrap_or(80);
            out.push(w(
                Id::None,
                Rect::new(cx, ry + 3, (cw - 16).max(10), 20),
                cell(*tone, *strong, false),
                text.clone(),
            ));
            cx += cw;
        }
        ry += ROW_H;
        shown += 1;
    }
    if shown < rows.len() && ry + 18 <= bottom + 4 {
        hint(out, x + 12, ry + 2, width - 24, &format!("…and {} more", thousands(rows.len() - shown)));
    }
    shown
}

/// A row of equal stat cells: `(number, word under it, tone)`.
fn stats(out: &mut Vec<Widget>, x: i32, y: i32, width: i32, items: &[(String, &str, Tone)]) -> i32 {
    const GAP: i32 = 10;
    const H: i32 = 64;
    let n = items.len().max(1) as i32;
    let cw = (width - GAP * (n - 1)) / n;
    for (i, (num, word, tone)) in items.iter().enumerate() {
        let r = Rect::new(x + i as i32 * (cw + GAP), y, cw, H);
        out.push(w(Id::None, r, Kind::Card { filled: true }, ""));
        out.push(w(
            Id::None,
            Rect::new(r.x + 12, r.y + 8, r.w - 24, 28),
            Kind::Stat { tone: *tone, caption: String::new() },
            num.clone(),
        ));
        out.push(w(Id::None, Rect::new(r.x + 12, r.y + 38, r.w - 24, 16), Kind::Label, *word));
    }
    y + H
}

/// A page's one action, as an outlined tool button, with a note to its right.
#[allow(clippy::too_many_arguments)]
fn action(out: &mut Vec<Widget>, m: &Model, y: i32, inner: i32, id: Id, label: &str, enabled: bool, note: &str) -> i32 {
    let bw = (label.chars().count() as i32 * 8 + 36).max(120);
    out.push(w(id, Rect::new(PAD, y, bw, 30), Kind::Tool { enabled: enabled && m.phase == Phase::Idle }, label));
    if !note.is_empty() {
        out.push(w(Id::None, Rect::new(PAD + bw + 14, y + 6, inner - bw - 14, 18), Kind::Hint, note));
    }
    y + 30
}

fn drive(m: &Model, i: usize) -> String {
    m.volumes.get(i).and_then(|v| v.as_ref()).map_or_else(|| format!("#{}", i + 1), |p| p.display().to_string())
}

// ── On the player ──────────────────────────────────────────────────────────────────────────────

fn player(m: &Model, inner: i32, mut y: i32, bottom: i32, out: &mut Vec<Widget>) {
    heading(out, PAD, y, "On the player");
    y += 22;
    hint(
        out,
        PAD,
        y,
        inner,
        "What is on each drive, and who put it there, from the flint-manifest.tsv Flint writes. Reading changes nothing.",
    );
    y += 28;
    let have_volume = m.volumes.iter().any(Option::is_some);
    let note = if have_volume { "" } else { "Choose the player on the Sync page first." };
    y = action(out, m, y, inner, Id::ReadPlayer, "Read the player", have_volume, note) + 16;
    if !m.player.read {
        return;
    }
    let (mut flint_n, mut flint_b, mut other_n, mut other_b) = (0usize, 0u64, 0usize, 0u64);
    for a in &m.player.albums {
        if a.by_flint {
            flint_n += 1;
            flint_b += a.bytes;
        } else {
            other_n += 1;
            other_b += a.bytes;
        }
    }
    let human = flint_core::space::human;
    y = stats(
        out,
        PAD,
        y,
        inner,
        &[
            (thousands(flint_n), "albums put there by Flint", Tone::Plain),
            (human(flint_b), "of music from Flint", Tone::Plain),
            (thousands(other_n), "albums not by Flint", if other_n > 0 { Tone::Caution } else { Tone::Plain }),
            (human(other_b), "not by Flint, left alone", Tone::Plain),
        ],
    ) + 14;
    let rows: Vec<Vec<(String, Tone, bool)>> = m
        .player
        .albums
        .iter()
        .map(|a: &AlbumRow| {
            vec![
                (a.folder.clone(), Tone::Plain, true),
                (drive(m, a.volume), Tone::Dim, false),
                (thousands(a.files), Tone::Dim, false),
                (human(a.bytes), Tone::Dim, false),
                (a.format.clone(), Tone::Dim, false),
                (
                    if a.by_flint { "Flint".into() } else { "Not by Flint".into() },
                    if a.by_flint { Tone::Dim } else { Tone::Caution },
                    false,
                ),
            ]
        })
        .collect();
    if rows.is_empty() {
        hint(out, PAD, y, inner, "No music on the player yet.");
        return;
    }
    table(
        out,
        PAD,
        y,
        inner,
        bottom,
        &[("Album", 330), ("Drive", 70), ("Files", 60), ("Size", 90), ("Format", 80), ("Put there by", 0)],
        &rows,
    );
}

// ── Check ──────────────────────────────────────────────────────────────────────────────────────

/// The verdicts in the order they are drawn, with the word under each count.
const VERDICTS: [(&str, &str, Tone); 5] = [
    ("LOSSY", "a lossy source inside", Tone::Warn),
    ("SUSPECT", "looks lossy, unsure", Tone::Caution),
    ("UPSAMPLED", "resampled from lower", Tone::Caution),
    ("PADDED", "fewer bits than declared", Tone::Caution),
    ("DAMAGED", "will not decode cleanly", Tone::Warn),
];

fn verdict_tone(v: &str) -> Tone {
    VERDICTS.iter().find(|(k, _, _)| *k == v).map_or(Tone::Dim, |(_, _, t)| *t)
}

fn check(m: &Model, inner: i32, mut y: i32, bottom: i32, out: &mut Vec<Widget>) {
    heading(out, PAD, y, "Check");
    y += 22;
    hint(
        out,
        PAD,
        y,
        inner,
        "Looks inside every FLAC for audio that is not the lossless it claims to be, and says why. \
         Changes nothing.",
    );
    y += 28;
    let label = if m.checked.is_some() { "Check again" } else { "Check the library" };
    let note = match (m.library.is_some(), m.checked) {
        (false, _) => "Choose the music folder on the Sync page first.".to_string(),
        (true, Some(n)) => format!("Checked {} FLACs. Files already checked are not decoded again.", thousands(n)),
        (true, None) => "Files already checked are remembered, so a second run is quick.".to_string(),
    };
    y = action(out, m, y, inner, Id::Check, label, m.library.is_some(), &note) + 16;
    let count = |v: &str| m.findings.iter().filter(|f| f.verdict == v).count();
    let mut cells: Vec<(String, &str, Tone)> = VERDICTS
        .iter()
        .map(|(k, word, tone)| {
            let n = count(k);
            (
                if m.checked.is_some() { thousands(n) } else { "—".into() },
                *word,
                if n > 0 { *tone } else { Tone::Plain },
            )
        })
        .collect();
    let ok = m.checked.map(|n| n.saturating_sub(m.findings.len()));
    cells.push((
        ok.map_or_else(|| "—".into(), thousands),
        "real lossless",
        if ok.is_some() { Tone::Ok } else { Tone::Plain },
    ));
    // The verdict names ARE the column headings of this row, so they go above it.
    let n = cells.len() as i32;
    let cw = (inner - 10 * (n - 1)) / n;
    for (i, (k, _, _)) in VERDICTS.iter().enumerate() {
        out.push(w(Id::None, Rect::new(PAD + i as i32 * (cw + 10), y, cw, 16), Kind::Label, *k));
    }
    out.push(w(Id::None, Rect::new(PAD + 5 * (cw + 10), y, cw, 16), Kind::Label, "OK"));
    y = stats(out, PAD, y + 18, inner, &cells) + 14;
    if m.checked.is_none() {
        return;
    }
    if m.findings.is_empty() {
        hint(out, PAD, y, inner, "Nothing to look at: every FLAC checked is the lossless audio it claims to be.");
        return;
    }
    let rows: Vec<Vec<(String, Tone, bool)>> = m
        .findings
        .iter()
        .map(|f| {
            vec![
                (f.verdict.clone(), verdict_tone(&f.verdict), true),
                (f.file.clone(), Tone::Plain, false),
                (f.why.clone(), Tone::Dim, false),
            ]
        })
        .collect();
    table(out, PAD, y, inner, bottom, &[("Verdict", 110), ("File", 400), ("Why", 0)], &rows);
}

// ── SensMe ─────────────────────────────────────────────────────────────────────────────────────

fn sensme(m: &Model, inner: i32, mut y: i32, bottom: i32, out: &mut Vec<Widget>) {
    heading(out, PAD, y, "SensMe");
    y += 22;
    hint(
        out,
        PAD,
        y,
        inner,
        "Sony's mood and tempo analysis, kept in Flint's cache and written only into the copies on the player.",
    );
    y += 20;
    hint(
        out,
        PAD,
        y,
        inner,
        "New analysis needs Music Center for PC. Tracks it already analysed are imported, not redone.",
    );
    y += 28;
    let idle = m.phase == Phase::Idle;
    out.push(w(
        Id::Scan,
        Rect::new(PAD, y, 150, 30),
        Kind::Tool { enabled: idle && m.library.is_some() },
        "Analyse library",
    ));
    out.push(w(Id::Import, Rect::new(PAD + 160, y, 180, 30), Kind::Tool { enabled: idle }, "Import Music Center"));
    if m.library.is_none() {
        out.push(w(
            Id::None,
            Rect::new(PAD + 354, y + 6, inner - 354, 18),
            Kind::Hint,
            "Choose the music folder on the Sync page first.",
        ));
    }
    y += 46;
    log_pane(m, PAD, y, inner, bottom - y, out);
}

/// The job log, for the pages whose jobs talk a lot (SensMe). Same pane the Sync page uses.
fn log_pane(m: &Model, x: i32, y: i32, width: i32, height: i32, out: &mut Vec<Widget>) {
    if height < 40 {
        return;
    }
    if m.log.is_empty() {
        out.push(w(Id::None, Rect::new(x, y, width, height), Kind::Card { filled: false }, ""));
        out.push(w(
            Id::None,
            Rect::new(x + 16, y + 14, width - 32, 20),
            Kind::Hint,
            "What the analysis does, track by track, is listed here.",
        ));
        return;
    }
    out.push(w(Id::None, Rect::new(x, y, width, height), Kind::LogPane, ""));
    let line_h = 18;
    let visible = ((height - 16) / line_h).max(0) as usize;
    let start = m.log.len().saturating_sub(visible);
    for (i, line) in m.log[start..].iter().enumerate() {
        out.push(w(
            Id::None,
            Rect::new(x + 12, y + 8 + i as i32 * line_h, width - 24, line_h),
            Kind::LogLine,
            line.clone(),
        ));
    }
}

// ── Likes & plays ──────────────────────────────────────────────────────────────────────────────

/// `1695456840` → `23 Sep 2023 08:14` (UTC — the log does not say which zone the player was in).
pub fn when(ts: i64) -> String {
    if ts <= 0 {
        return "—".into();
    }
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = yoe + era * 400 + i64::from(mo <= 2);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {yr} {:02}:{:02}", MONTHS[(mo - 1) as usize], secs / 3600, secs % 3600 / 60)
}

fn likes(m: &Model, inner: i32, mut y: i32, bottom: i32, out: &mut Vec<Widget>) {
    heading(out, PAD, y, "Likes & plays");
    y += 22;
    hint(out, PAD, y, inner, "What the player wrote down: the plays in .scrobbler.log, and the songs you liked on it.");
    y += 20;
    hint(
        out,
        PAD,
        y,
        inner,
        "flint scrobble sends the plays to Last.fm and flint likes keeps likes in step. This is what they work from.",
    );
    y += 28;
    let have_volume = m.volumes.iter().any(Option::is_some);
    let note = if have_volume { "" } else { "Choose the player on the Sync page first." };
    y = action(out, m, y, inner, Id::ReadPlayer, "Read the player", have_volume, note) + 16;
    if !m.player.read {
        return;
    }
    let plays = m.player.plays.iter().filter(|p| p.kind == "PLAY").count();
    let skips = m.player.plays.len() - plays;
    y = stats(
        out,
        PAD,
        y,
        inner,
        &[
            (thousands(plays), "plays to send", Tone::Plain),
            (thousands(skips), "skips, never sent", Tone::Dim),
            (thousands(m.player.likes), "songs liked on the player", Tone::Plain),
            (
                thousands(m.player.unreadable),
                "log rows Flint cannot read",
                if m.player.unreadable > 0 { Tone::Caution } else { Tone::Plain },
            ),
        ],
    ) + 14;
    if m.player.plays.is_empty() {
        hint(out, PAD, y, inner, "No plays in the log yet. Cinder writes one when a track finishes.");
        return;
    }
    let rows: Vec<Vec<(String, Tone, bool)>> = m
        .player
        .plays
        .iter()
        .map(|p| {
            vec![
                (when(p.when), Tone::Dim, false),
                (p.track.clone(), Tone::Plain, true),
                (p.artist.clone(), Tone::Dim, false),
                (p.kind.clone(), if p.kind == "PLAY" { Tone::Plain } else { Tone::Dim }, false),
            ]
        })
        .collect();
    table(out, PAD, y, inner, bottom, &[("When", 170), ("Track", 300), ("Artist", 250), ("Kind", 0)], &rows);
}

// ── Palettes ───────────────────────────────────────────────────────────────────────────────────

fn palettes(m: &Model, inner: i32, mut y: i32, bottom: i32, out: &mut Vec<Widget>) {
    heading(out, PAD, y, "Palettes");
    y += 22;
    hint(
        out,
        PAD,
        y,
        inner,
        "Colour schemes for Cinder: .palette files in cinder_palettes at the top of the player's drive.",
    );
    y += 20;
    hint(
        out,
        PAD,
        y,
        inner,
        "Cinder checks each one as it loads it, and refuses any it could not draw legibly by day and by night.",
    );
    y += 28;
    let have_volume = m.volumes.iter().any(Option::is_some);
    let note = if have_volume { "" } else { "Choose the player on the Sync page first." };
    y = action(out, m, y, inner, Id::ReadPlayer, "Read the player", have_volume, note) + 16;
    if !m.player.read {
        return;
    }
    if m.player.palettes.is_empty() {
        hint(
            out,
            PAD,
            y,
            inner,
            "No palettes on the player. Copy .palette files into cinder_palettes at the top of the drive.",
        );
        return;
    }
    let rows: Vec<Vec<(String, Tone, bool)>> = m
        .player
        .palettes
        .iter()
        .map(|p| {
            vec![
                (p.name.clone(), Tone::Plain, true),
                (drive(m, p.volume), Tone::Dim, false),
                (format!("{} bytes", thousands(p.bytes as usize)), Tone::Dim, false),
            ]
        })
        .collect();
    table(out, PAD, y, inner, bottom, &[("Palette", 400), ("Drive", 120), ("Size", 0)], &rows);
}

// ── Settings ───────────────────────────────────────────────────────────────────────────────────

/// The x of Settings' value column.
const VALUE_X: i32 = PAD + 180;

fn settings(m: &Model, inner: i32, mut y: i32, out: &mut Vec<Widget>) {
    let busy = m.phase == Phase::Working;
    let label =
        |out: &mut Vec<Widget>, y: i32, t: &str| out.push(w(Id::None, Rect::new(PAD, y + 5, 170, 20), Kind::Label, t));

    heading(out, PAD, y, "Appearance");
    y += 26;
    label(out, y, "Theme");
    for (i, p) in ThemePref::ALL.iter().enumerate() {
        out.push(w(
            Id::Theme(*p),
            Rect::new(VALUE_X + i as i32 * 96, y, 96, 30),
            Kind::Segment { on: m.theme == *p },
            p.label(),
        ));
    }
    y += 34;
    hint(out, VALUE_X, y, inner - (VALUE_X - PAD), "System follows Windows, and changes with it while Flint is open.");
    y += 34;

    heading(out, PAD, y, "Library");
    y += 26;
    for (id, name, path, empty) in [
        (Id::PickLibrary, "Music folder", &m.library, "No folder chosen"),
        (Id::PickPlaylists, "Playlists", &m.playlists, "None — optional"),
    ] {
        label(out, y, name);
        let (text, ph) = match path {
            Some(p) => (p.display().to_string(), false),
            None => (empty.to_string(), true),
        };
        out.push(w(
            Id::None,
            Rect::new(VALUE_X, y + 3, inner - (VALUE_X - PAD) - 110, 22),
            Kind::Value { placeholder: ph },
            text,
        ));
        out.push(w(
            id,
            Rect::new(PAD + inner - 96, y, 96, 30),
            Kind::Button { primary: false, enabled: !busy },
            "Choose…",
        ));
        y += 38;
    }
    y += 10;

    heading(out, PAD, y, "Analysis cache");
    y += 26;
    label(out, y, "Kept in");
    out.push(w(
        Id::None,
        Rect::new(VALUE_X, y + 3, inner - (VALUE_X - PAD), 22),
        Kind::Value { placeholder: false },
        m.cache_dir.clone(),
    ));
    y += 30;
    hint(
        out,
        VALUE_X,
        y,
        inner - (VALUE_X - PAD),
        "SensMe results and check results. Deleting it only means the next run does the work again.",
    );
    y += 34;

    heading(out, PAD, y, "Last.fm");
    y += 26;
    label(out, y, "Account");
    out.push(w(
        Id::None,
        Rect::new(VALUE_X, y + 3, inner - (VALUE_X - PAD), 22),
        Kind::Value { placeholder: false },
        m.lastfm.clone(),
    ));
    y += 30;
    hint(
        out,
        VALUE_X,
        y,
        inner - (VALUE_X - PAD),
        "Set up once from a terminal: flint lastfm key <key> <secret>, then flint lastfm login <name>.",
    );
    y += 34;

    heading(out, PAD, y, "About");
    y += 26;
    hint(
        out,
        PAD,
        y,
        inner,
        &format!("Flint {} · MIT licence · github.com/superwilso/flint", env!("CARGO_PKG_VERSION")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_civil() {
        assert_eq!(when(0), "—");
        assert_eq!(when(86_400), "2 Jan 1970 00:00");
        assert_eq!(when(1_695_456_840), "23 Sep 2023 08:14");
        assert_eq!(when(951_782_400), "29 Feb 2000 00:00");
    }
}
