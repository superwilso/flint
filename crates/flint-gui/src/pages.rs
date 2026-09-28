//! The pages behind the tabs, other than Sync (which is `layout` in `lib.rs`).
//!
//! Each page is one question about the library or the player, answered from something Flint
//! already reads: the check store, the manifests, the scrobble log, the likes file, the palette
//! folder. Nothing on these pages writes MUSIC to the player — "Read the player" only reads, and
//! Palettes ▸ Send copies a few small `.palette` files — so none of them carries the accent. The
//! accent on this window means music about to be written, and that only ever comes from Sync.
//!
//! Same rule as the rest of the window: this is the only place that knows where a page's widgets
//! are, and the paint and the hit test both read it.

use crate::{
    job_of, live, thousands, AlbumRow, Field, Id, Kind, Model, Rect, Tab, ThemePref, Tone, Widget, BAND_H, BTN_H, PAD,
};

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

/// Height kept at the bottom of a page for the status line (and the bar, while its job runs).
/// Settings has no footer: its one job, signing in, reports in the Last.fm section itself.
fn footer_h(m: &Model) -> i32 {
    if m.tab == Tab::Settings {
        0
    } else if m.footer_job(m.tab).is_some() {
        46
    } else {
        30
    }
}

/// The status line — this page's job's latest word, or what is running elsewhere, or what the
/// last job said — and, while this page's own job runs, its bar and a Stop button.
fn footer(m: &Model, inner: i32, h: i32, out: &mut Vec<Widget>) {
    if m.tab == Tab::Settings {
        return;
    }
    let mut y = h - PAD - 20;
    if let Some(r) = m.footer_job(m.tab) {
        const STOP_W: i32 = 90;
        out.push(w(Id::None, Rect::new(PAD, y - 20, inner - STOP_W - 12, 8), Kind::Progress(r.progress), ""));
        out.push(w(
            Id::Stop,
            Rect::new(PAD + inner - STOP_W, y - 30, STOP_W, BTN_H),
            Kind::Button { primary: false, enabled: true },
            "Stop",
        ));
        y = h - PAD - 18;
    }
    out.push(w(Id::None, Rect::new(PAD, y, inner - 100, 20), Kind::Status, m.status_line(m.tab)));
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

/// How wide a tool button is for its label.
fn tool_w(label: &str) -> i32 {
    (label.chars().count() as i32 * 8 + 36).max(120)
}

/// A page's one action, as an outlined tool button, with a note to its right. The note gives way
/// to the reason the button is grey when something else is holding what it needs.
fn action(out: &mut Vec<Widget>, m: &Model, y: i32, inner: i32, id: Id, label: &str, note: &str) -> i32 {
    let bw = tool_w(label);
    out.push(w(id, Rect::new(PAD, y, bw, 30), Kind::Tool { enabled: live(m, id) }, label));
    let waits = job_of(id).and_then(|j| m.waits_for(j));
    let note = waits.as_deref().unwrap_or(note);
    if !note.is_empty() {
        out.push(w(Id::None, Rect::new(PAD + bw + 14, y + 6, inner - bw - 14, 18), Kind::Hint, note));
    }
    y + 30
}

/// Tool buttons left to right from `x`, each sized to its label. Returns where the next would go.
fn tools(out: &mut Vec<Widget>, m: &Model, mut x: i32, y: i32, items: &[(Id, String)]) -> i32 {
    for (id, label) in items {
        let bw = tool_w(label);
        out.push(w(*id, Rect::new(x, y, bw, 30), Kind::Tool { enabled: live(m, *id) }, label.clone()));
        x += bw + 10;
    }
    x
}

/// A line of text that takes typing.
fn field(out: &mut Vec<Widget>, m: &Model, f: Field, rect: Rect) {
    out.push(w(
        Id::Field(f),
        rect,
        Kind::Field {
            focused: m.focus == Some(f),
            masked: f.masked(),
            enabled: live(m, Id::Field(f)),
            placeholder: f.placeholder(),
        },
        m.field(f),
    ));
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
    y = action(out, m, y, inner, Id::ReadPlayer, "Read the player", note) + 16;
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
pub const VERDICTS: [(&str, &str, Tone); 5] = [
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
    y = action(out, m, y, inner, Id::Check, label, &note) + 16;

    // The verdict names ARE the column headings of this row, so they go above it. Each count is
    // also the filter for its verdict: the number says how many, a click shows which.
    let count = |v: &str| m.findings.iter().filter(|f| f.verdict == v).count();
    let n = VERDICTS.len() as i32 + 1;
    let cw = (inner - 10 * (n - 1)) / n;
    for (i, (k, _, _)) in VERDICTS.iter().enumerate() {
        out.push(w(Id::None, Rect::new(PAD + i as i32 * (cw + 10), y, cw, 16), Kind::Label, *k));
    }
    out.push(w(Id::None, Rect::new(PAD + 5 * (cw + 10), y, cw, 16), Kind::Label, "OK"));
    y += 18;
    let ok = m.checked.map(|c| c.saturating_sub(m.findings.len()));
    for i in 0..n as usize {
        let r = Rect::new(PAD + i as i32 * (cw + 10), y, cw, 64);
        let (num, word, tone) = match VERDICTS.get(i) {
            Some((k, word, tone)) => {
                let c = count(k);
                (
                    if m.checked.is_some() { thousands(c) } else { "—".into() },
                    *word,
                    if c > 0 { *tone } else { Tone::Plain },
                )
            }
            None => (
                ok.map_or_else(|| "—".into(), thousands),
                "real lossless",
                if ok.is_some() { Tone::Ok } else { Tone::Plain },
            ),
        };
        // A verdict's card is a choice once there is something to choose between.
        if i < VERDICTS.len() && !m.findings.is_empty() {
            out.push(w(Id::Verdict(i), r, Kind::Pick { on: m.check_verdict == Some(i) }, ""));
        } else {
            out.push(w(Id::None, r, Kind::Card { filled: true }, ""));
        }
        out.push(w(
            Id::None,
            Rect::new(r.x + 12, r.y + 8, r.w - 24, 28),
            Kind::Stat { tone, caption: String::new() },
            num,
        ));
        out.push(w(Id::None, Rect::new(r.x + 12, r.y + 38, r.w - 24, 16), Kind::Label, word));
    }
    y += 64 + 14;
    if m.checked.is_none() {
        return;
    }
    if m.findings.is_empty() {
        hint(out, PAD, y, inner, "Nothing to look at: every FLAC checked is the lossless audio it claims to be.");
        return;
    }

    // The filter: a verdict (the cards above) and text, both at once.
    let fw = (inner / 2).min(360);
    field(out, m, Field::CheckFilter, Rect::new(PAD, y, fw, 30));
    let rows = m.check_rows();
    let filtered = m.check_verdict.is_some() || !m.check_filter.trim().is_empty();
    let mut nx = PAD + fw + 14;
    if filtered {
        nx = tools(out, m, nx, y, &[(Id::ClearFilter, "Show all".into())]) + 4;
    }
    let note = if filtered {
        format!("{} of {} flagged files", thousands(rows.len()), thousands(m.findings.len()))
    } else {
        "Click a count above to see only that verdict.".into()
    };
    out.push(w(Id::None, Rect::new(nx, y + 6, (PAD + inner - nx).max(40), 18), Kind::Hint, note));
    y += 40;
    if rows.is_empty() {
        hint(out, PAD, y, inner, "No flagged file matches the filter.");
        return;
    }
    let rows: Vec<Vec<(String, Tone, bool)>> = rows
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
    let x = tools(out, m, PAD, y, &[(Id::Scan, "Analyse library".into()), (Id::Import, "Import Music Center".into())]);
    let note = if m.library.is_none() {
        Some("Choose the music folder on the Sync page first.".to_string())
    } else {
        m.waits_for(crate::Job::Scan)
    };
    if let Some(note) = note {
        out.push(w(Id::None, Rect::new(x + 4, y + 6, (PAD + inner - x - 4).max(40), 18), Kind::Hint, note));
    }
    y += 46;
    log_pane(out, &m.sensme_log, "What the analysis does, track by track, is listed here.", PAD, y, inner, bottom - y);
}

/// A job log, for the pages whose jobs talk a lot. Same pane the Sync page uses.
fn log_pane(out: &mut Vec<Widget>, log: &[String], empty: &str, x: i32, y: i32, width: i32, height: i32) {
    if height < 40 {
        return;
    }
    if log.is_empty() {
        out.push(w(Id::None, Rect::new(x, y, width, height), Kind::Card { filled: false }, ""));
        out.push(w(Id::None, Rect::new(x + 16, y + 14, width - 32, 20), Kind::Hint, empty));
        return;
    }
    out.push(w(Id::None, Rect::new(x, y, width, height), Kind::LogPane, ""));
    let line_h = 18;
    let visible = ((height - 16) / line_h).max(0) as usize;
    let start = log.len().saturating_sub(visible);
    for (i, line) in log[start..].iter().enumerate() {
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
    hint(
        out,
        PAD,
        y,
        inner,
        "What the player wrote down — the plays in .scrobbler.log and the songs you liked — and Last.fm's side of it.",
    );
    y += 28;
    let have_volume = m.volumes.iter().any(Option::is_some);
    let plays = m.player.plays.iter().filter(|p| p.kind == "PLAY").count();
    let send = match plays {
        0 => "Send plays".to_string(),
        1 => "Send 1 play".to_string(),
        n => format!("Send {} plays", thousands(n)),
    };
    let mut items = vec![(Id::ReadPlayer, "Read the player".to_string()), (Id::Scrobble, send)];
    items.push((Id::CompareLikes, "Compare likes".into()));
    if let Some(p) = m.likes_plan {
        items.push((
            Id::SyncLikes,
            match p.changes() {
                0 => "Nothing to change".into(),
                1 => "Make 1 change".into(),
                n => format!("Make {} changes", thousands(n)),
            },
        ));
    }
    tools(out, m, PAD, y, &items);
    y += 38;

    // One line under the buttons: what is missing, what is in the way, or what Compare found.
    let busy_with =
        [crate::Job::Scrobble, crate::Job::CompareLikes, crate::Job::ReadPlayer].iter().find_map(|j| m.waits_for(*j));
    let line = if !have_volume {
        "Choose the player on the Sync page first.".to_string()
    } else if !m.lastfm.signed_in() {
        "Sending plays and keeping likes in step need Last.fm: sign in on the Settings page.".to_string()
    } else if let Some(why) = busy_with {
        why
    } else if let Some(p) = m.likes_plan {
        format!(
            "Likes: {} to add to the player, {} to take off · {} to love on Last.fm, {} to unlove.",
            p.device_add, p.device_remove, p.lastfm_love, p.lastfm_unlove
        )
    } else if !m.player.read {
        "Read the player to see the plays; Send sends exactly the ones listed.".to_string()
    } else {
        "Compare likes shows what would change on each side before anything does.".to_string()
    };
    let mut lx = PAD;
    if have_volume && !m.lastfm.signed_in() {
        lx = tools(out, m, PAD, y - 4, &[(Id::Tab(Tab::Settings), "Set up Last.fm".into())]) + 4;
    }
    hint(out, lx, y + 2, PAD + inner - lx, &line);
    y += 30;

    // What the last Last.fm job said, when there is something to read — the plays it kept, the
    // loves that failed. It takes the bottom of the page and the table gives way.
    let log_h = if m.lastfm_log.is_empty() { 0 } else { 5 * 18 + 16 };
    let table_bottom = bottom - if log_h > 0 { log_h + 10 } else { 0 };
    if log_h > 0 {
        log_pane(out, &m.lastfm_log, "", PAD, bottom - log_h, inner, log_h);
    }
    if !m.player.read {
        return;
    }
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
        hint(out, PAD, y, inner, "No plays in the log. Cinder writes one when a track finishes.");
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
    table(out, PAD, y, inner, table_bottom, &[("When", 170), ("Track", 300), ("Artist", 250), ("Kind", 0)], &rows);
}

// ── Palettes ───────────────────────────────────────────────────────────────────────────────────

fn palettes(m: &Model, inner: i32, mut y: i32, bottom: i32, out: &mut Vec<Widget>) {
    use flint_core::palette::{to_send, State};
    heading(out, PAD, y, "Palettes");
    y += 22;
    hint(
        out,
        PAD,
        y,
        inner,
        "Colour schemes for Cinder. Each .palette file is checked with the player's own readability rules before it is sent.",
    );
    y += 20;
    hint(
        out,
        PAD,
        y,
        inner,
        "Cinder reads them from cinder_palettes on the player's internal memory, and refuses any it could not draw legibly.",
    );
    y += 28;

    // The folder on this PC.
    out.push(w(Id::None, Rect::new(PAD, y + 5, 170, 20), Kind::Label, "On this PC"));
    let (text, ph) = match &m.palette_dir {
        Some(p) => (p.display().to_string(), false),
        None => ("No folder chosen".to_string(), true),
    };
    out.push(w(
        Id::None,
        Rect::new(VALUE_X, y + 3, inner - (VALUE_X - PAD) - 110, 22),
        Kind::Value { placeholder: ph },
        text,
    ));
    out.push(w(
        Id::PickPalettes,
        Rect::new(PAD + inner - 96, y, 96, 30),
        Kind::Button { primary: false, enabled: live(m, Id::PickPalettes) },
        "Choose…",
    ));
    y += 42;

    // Check, then Send. Outlined like every tool off the Sync page: Send writes a few small files
    // into cinder_palettes, never music, and the accent on this window stays with the music copy.
    let n = to_send(&m.palette_rows).len();
    let have_internal = m.volumes[0].is_some();
    out.push(w(
        Id::CheckPalettes,
        Rect::new(PAD, y, 120, 30),
        Kind::Tool { enabled: live(m, Id::CheckPalettes) },
        "Check",
    ));
    let send_label = match n {
        0 => "Send to the player".to_string(),
        1 => "Send 1 to the player".to_string(),
        n => format!("Send {n} to the player"),
    };
    let sw = send_label.chars().count() as i32 * 8 + 36;
    out.push(w(
        Id::SendPalettes,
        Rect::new(PAD + 134, y, sw, 30),
        Kind::Tool { enabled: live(m, Id::SendPalettes) },
        send_label,
    ));
    let waits = m.waits_for(crate::Job::CheckPalettes);
    let note = if let Some(why) = waits.as_deref() {
        why
    } else if !have_internal {
        "Choose the player's internal memory on the Sync page to compare and send."
    } else if m.palette_rows.is_empty() {
        "Check compares this folder with what the player holds. It writes nothing."
    } else {
        ""
    };
    if !note.is_empty() {
        let nx = PAD + 134 + sw + 14;
        out.push(w(Id::None, Rect::new(nx, y + 6, (PAD + inner - nx).max(40), 18), Kind::Hint, note));
    }
    y += 46;
    if m.palette_rows.is_empty() {
        return;
    }

    // The table: name · colours · state · why.
    const ROW_H: i32 = 30;
    let cols = [("Palette", 180), ("Colours", 130), ("On the player", 150)];
    let why_w = inner - cols.iter().map(|c| c.1).sum::<i32>();
    out.push(w(Id::None, Rect::new(PAD, y, inner, 30), Kind::Card { filled: true }, ""));
    let mut cx = PAD + 12;
    for (title, cw) in cols.iter().chain(std::iter::once(&("", why_w))) {
        out.push(w(Id::None, Rect::new(cx, y + 6, cw - 12, 18), Kind::Label, *title));
        cx += cw;
    }
    let mut ry = y + 36;
    let mut shown = 0;
    for r in &m.palette_rows {
        if ry + ROW_H > bottom {
            break;
        }
        out.push(w(Id::None, Rect::new(PAD, ry - 2, inner, 1), Kind::Rule, ""));
        let mut cx = PAD + 12;
        out.push(w(
            Id::None,
            Rect::new(cx, ry + 5, cols[0].1 - 16, 20),
            cell(Tone::Plain, true, false),
            r.name.clone(),
        ));
        cx += cols[0].1;
        if let Some(cells) = r.cells {
            out.push(w(Id::None, Rect::new(cx, ry + 6, 100, 16), Kind::Swatch(cells), ""));
        }
        cx += cols[1].1;
        let tone = match r.state {
            State::Refused => Tone::Warn,
            State::New | State::Changed => Tone::Plain,
            State::On | State::BuiltIn => Tone::Ok,
            State::PlayerOnly => Tone::Dim,
        };
        out.push(w(Id::None, Rect::new(cx, ry + 5, cols[2].1 - 16, 20), cell(tone, false, true), r.state.word()));
        cx += cols[2].1;
        let why = if r.why.is_empty() { r.file.clone() } else { r.why.clone() };
        let wt = if r.why.is_empty() { Tone::Dim } else { Tone::Warn };
        out.push(w(Id::None, Rect::new(cx, ry + 5, (why_w - 16).max(10), 20), cell(wt, false, false), why));
        ry += ROW_H;
        shown += 1;
    }
    if shown < m.palette_rows.len() && ry + 18 <= bottom + 4 {
        hint(out, PAD + 12, ry + 2, inner - 24, &format!("…and {} more", thousands(m.palette_rows.len() - shown)));
    }
}

// ── Settings ───────────────────────────────────────────────────────────────────────────────────

/// The x of Settings' value column.
const VALUE_X: i32 = PAD + 180;

fn settings(m: &Model, inner: i32, mut y: i32, out: &mut Vec<Widget>) {
    let label =
        |out: &mut Vec<Widget>, y: i32, t: &str| out.push(w(Id::None, Rect::new(PAD, y + 5, 170, 20), Kind::Label, t));
    let value_w = inner - (VALUE_X - PAD);

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
    hint(out, VALUE_X, y, value_w, "System follows Windows, and changes with it while Flint is open.");
    y += 34;

    // ── Last.fm ──
    //
    // Two steps, each a button. A key first, because Last.fm gives every application its own and
    // Flint — open source — cannot ship one without publishing its secret. Then signing in, which
    // happens on Last.fm's own page in the browser: no password is ever typed into Flint.
    heading(out, PAD, y, "Last.fm");
    y += 26;
    let lf = &m.lastfm;
    if !lf.has_key || lf.editing {
        let fw = value_w.min(400);
        label(out, y, "API key");
        field(out, m, Field::ApiKey, Rect::new(VALUE_X, y, fw, 30));
        y += 38;
        label(out, y, "Shared secret");
        field(out, m, Field::ApiSecret, Rect::new(VALUE_X, y, fw, 30));
        y += 38;
        let mut items =
            vec![(Id::LastfmSaveKey, "Save".to_string()), (Id::LastfmGetKey, "Get a key on last.fm".into())];
        if lf.editing && lf.has_key {
            items.push((Id::LastfmKeepKey, "Keep the saved key".into()));
        }
        tools(out, m, VALUE_X, y, &items);
        y += 38;
        let note = match m.footer_job(Tab::Settings) {
            Some(r) if !r.status.is_empty() => r.status.clone(),
            _ => "Make one on last.fm (any name, no callback URL), then paste the key and the secret here.".into(),
        };
        hint(out, VALUE_X, y, value_w, &note);
        y += 28;
    } else {
        label(out, y, "Account");
        out.push(w(
            Id::None,
            Rect::new(VALUE_X, y + 3, value_w, 22),
            Kind::Value { placeholder: lf.user.is_none() },
            lf.account(),
        ));
        y += 32;
        let signing_in = m.is_running(crate::Job::LastfmSignIn);
        if signing_in {
            out.push(w(Id::Stop, Rect::new(VALUE_X, y, 150, 30), Kind::Tool { enabled: true }, "Stop waiting"));
        } else if lf.user.is_some() {
            tools(
                out,
                m,
                VALUE_X,
                y,
                &[(Id::LastfmSignOut, "Sign out".into()), (Id::LastfmChangeKey, "Change key".into())],
            );
        } else {
            tools(
                out,
                m,
                VALUE_X,
                y,
                &[(Id::LastfmSignIn, "Sign in with Last.fm".into()), (Id::LastfmChangeKey, "Change key".into())],
            );
        }
        y += 38;
        let note = match m.footer_job(Tab::Settings) {
            Some(r) if !r.status.is_empty() => r.status.clone(),
            _ if lf.user.is_some() => {
                "Flint keeps a session key, never your password. Revoke it at last.fm/settings/applications.".into()
            }
            _ => "Opens Last.fm in your browser. Allow Flint there, and this window notices by itself.".into(),
        };
        hint(out, VALUE_X, y, value_w, &note);
        y += 28;
    }
    y += 8;

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
        out.push(w(Id::None, Rect::new(VALUE_X, y + 3, value_w - 110, 22), Kind::Value { placeholder: ph }, text));
        out.push(w(
            id,
            Rect::new(PAD + inner - 96, y, 96, 30),
            Kind::Button { primary: false, enabled: live(m, id) },
            "Choose…",
        ));
        y += 38;
    }
    label(out, y, "Analysis cache");
    out.push(w(
        Id::None,
        Rect::new(VALUE_X, y + 3, value_w, 22),
        Kind::Value { placeholder: false },
        m.cache_dir.clone(),
    ));
    y += 28;
    hint(
        out,
        VALUE_X,
        y,
        value_w,
        "SensMe and check results, and the Last.fm sign-in. Deleting it only means the next run does the work again.",
    );
    y += 36;
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
