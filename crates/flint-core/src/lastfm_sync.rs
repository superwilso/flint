//! What `flint scrobble` and `flint likes` do, for the command line and the window alike.
//!
//! Both front ends call these two functions, so there is one implementation of "which rows come out
//! of the log" and "who wins when the two sides disagree". Each line worth reading is handed to
//! `say`: the command line prints it, the window logs it. A run that is not told to `apply` reads
//! both sides, says what it would do, and writes nothing — the same dry run the terminal has always
//! had.

use std::path::{Path, PathBuf};

use crate::{lastfm, likes, scrobblelog};

/// What a scrobble run found and did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScrobbleReport {
    /// Plays waiting in the logs, across every volume.
    pub found: usize,
    pub accepted: usize,
    pub ignored: usize,
    /// Plays Last.fm refused for now (code 4, too new, and anything else). They stay in the log.
    pub kept: usize,
    /// Plays Last.fm refused for good (code 3, more than 14 days old): moved out of the log into
    /// `.scrobbler.refused.log` beside it, with the reason, rather than re-sent on every run.
    pub refused: usize,
    /// Rows taken out of the logs because Last.fm has them now.
    pub removed: usize,
    /// Why the run stopped early, when it did. What was accepted before that is still removed.
    pub stopped: Option<String>,
}

/// Send the plays in each volume's `.scrobbler.log` to Last.fm, and take the ones it accepted out
/// of the file. `cancel` is asked between batches of fifty.
pub fn scrobble(
    roots: &[PathBuf],
    creds: lastfm::Credentials,
    apply: bool,
    cancel: &dyn Fn() -> bool,
    say: &mut dyn FnMut(String),
) -> Result<ScrobbleReport, String> {
    let mut report = ScrobbleReport::default();
    let mut found = Vec::new();
    for root in roots {
        let path = root.join(".scrobbler.log");
        if !path.is_file() {
            say(format!("{}: no .scrobbler.log", root.display()));
            continue;
        }
        let body = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let log = scrobblelog::parse(&body);
        let plays = log.plays_to_send();
        say(format!(
            "{}: {} play(s) to send{}{}",
            path.display(),
            thousands(plays.len()),
            if log.entries.len() > log.plays().count() {
                format!(", {} skipped-track row(s) left alone", log.entries.len() - log.plays().count())
            } else {
                String::new()
            },
            if log.unreadable.is_empty() {
                String::new()
            } else {
                format!(", {} row(s) this version cannot read (kept)", log.unreadable.len())
            }
        ));
        for (line, _, why) in &log.unreadable {
            say(format!("    line {line}: {why}"));
        }
        if log.local_time() && !plays.is_empty() {
            say("    times are the player's clock (#TZ/UNKNOWN): sent as UTC in this PC's time zone".into());
        }
        for entry in plays.iter().take(3) {
            say(format!("    {} — {}", entry.artist, entry.track));
        }
        if plays.len() > 3 {
            say(format!("    …and {} more", thousands(plays.len() - 3)));
        }
        report.found += plays.len();
        found.push((path, log, plays));
    }

    if report.found == 0 {
        say("nothing to send.".into());
        return Ok(report);
    }
    if !apply {
        return Ok(report);
    }

    let mut client = lastfm::Client::new(creds).map_err(|e| e.to_string())?;
    'volumes: for (path, log, plays) in &found {
        let mut accepted_rows: Vec<scrobblelog::Entry> = Vec::new();
        let mut refused_rows: Vec<(scrobblelog::Entry, String)> = Vec::new();
        let (mut accepted, mut ignored) = (0usize, 0usize);
        for batch in plays.chunks(lastfm::BATCH) {
            if cancel() {
                report.stopped = Some("stopped before the next batch".into());
                rewrite(path, log, &accepted_rows, &refused_rows, &mut report, say)?;
                break 'volumes;
            }
            // What goes over the wire carries real UTC; what comes out of the file is found by
            // the row as it was written. The two differ only in the timestamp, so a refusal is
            // matched against the converted copy and the original row stays in the log.
            let wire = to_send(log, batch);
            match client.scrobble(&wire) {
                Ok(result) => {
                    accepted += result.accepted;
                    ignored += result.ignored;
                    for (entry, why, code) in &result.rejected {
                        let what = if *code == TOO_OLD { "moved to the refused log" } else { "kept" };
                        say(format!("    {what}: {} — {} ({why})", entry.artist, entry.track));
                    }
                    // What Last.fm took comes out of the file, and so does what it can never take,
                    // into the refused log; the rest stays to be sent again.
                    for (original, sent) in batch.iter().zip(&wire) {
                        let refusal = result.rejected.iter().find(|(e, _, _)| e.identity() == sent.identity());
                        match refusal {
                            None => accepted_rows.push(original.clone()),
                            Some((_, why, code)) if *code == TOO_OLD => {
                                refused_rows.push((original.clone(), why.clone()));
                                report.refused += 1;
                            }
                            _ => report.kept += 1,
                        }
                    }
                }
                Err(e) => {
                    // Stop at the first batch that fails: the file is rewritten with whatever was
                    // accepted so far, so nothing is lost and a re-run carries on where this left off.
                    say(format!("    stopped: {e}"));
                    report.stopped = Some(e.to_string());
                    break;
                }
            }
        }
        report.accepted += accepted;
        report.ignored += ignored;
        say(format!("{}: {} accepted, {} ignored by Last.fm", path.display(), thousands(accepted), ignored));
        rewrite(path, log, &accepted_rows, &refused_rows, &mut report, say)?;
        if report.stopped.is_some() {
            break;
        }
    }
    Ok(report)
}

/// The plays as Last.fm should get them: from a `#TZ/UNKNOWN` log, with each timestamp moved
/// from the player's wall clock to UTC in this PC's time zone (see [`crate::localtime`]).
fn to_send(log: &scrobblelog::Log, batch: &[scrobblelog::Entry]) -> Vec<scrobblelog::Entry> {
    if !log.local_time() {
        return batch.to_vec();
    }
    batch
        .iter()
        .map(|e| scrobblelog::Entry { timestamp: crate::localtime::local_to_utc(e.timestamp), ..e.clone() })
        .collect()
}

/// Last.fm's ignored-message code for a timestamp more than 14 days old: it will never be taken.
const TOO_OLD: u32 = 3;

/// The file beside each `.scrobbler.log` that keeps the plays Last.fm refused for good.
pub const REFUSED_LOG: &str = ".scrobbler.refused.log";

fn rewrite(
    path: &Path,
    log: &scrobblelog::Log,
    sent: &[scrobblelog::Entry],
    refused: &[(scrobblelog::Entry, String)],
    report: &mut ScrobbleReport,
    say: &mut dyn FnMut(String),
) -> Result<(), String> {
    if sent.is_empty() && refused.is_empty() {
        return Ok(());
    }
    // The refused rows are written BEFORE they leave the log, so a pulled cable between the two
    // leaves a row in both files rather than in neither.
    if !refused.is_empty() {
        let out = path.with_file_name(REFUSED_LOG);
        let mut body = std::fs::read_to_string(&out).unwrap_or_default();
        if body.is_empty() {
            body.push_str("# Plays Last.fm refused for good (more than 14 days old), moved here by Flint.\n");
            body.push_str("# Each row is as the player wrote it, after a line saying why.\n");
        }
        for (entry, why) in refused {
            body.push_str(&format!("# {why}\n{}\n", entry.raw));
        }
        write_atomic(&out, &body).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let mut gone: Vec<scrobblelog::Entry> = sent.to_vec();
    gone.extend(refused.iter().map(|(e, _)| e.clone()));
    let body = scrobblelog::rewrite(log, &gone);
    write_atomic(path, &body).map_err(|e| format!("{}: {e}", path.display()))?;
    say(format!("    {} row(s) removed from the log", thousands(sent.len())));
    if !refused.is_empty() {
        say(format!("    {} row(s) moved to {REFUSED_LOG}", thousands(refused.len())));
    }
    report.removed += sent.len();
    Ok(())
}

/// Write through a temporary name, so a pulled cable leaves the old log rather than half a new one.
pub fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("log.tmp");
    std::fs::write(&tmp, body)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::remove_file(path).ok();
            std::fs::rename(&tmp, path)
        }
    }
}

/// What a likes run found and did.
#[derive(Clone, Debug, Default)]
pub struct LikesReport {
    pub plan: likes::Plan,
    /// The player could be read.
    pub device: bool,
    /// Last.fm could be read.
    pub lastfm: bool,
    pub loved: usize,
    pub unloved: usize,
    /// The merged list was written for the player to take in.
    pub pushed: bool,
}

/// Keep the player's liked songs and Last.fm's loved tracks in step, the ways `opts` allows.
/// `state_path` is where the last run's picture of each side is kept; `leave` names removals to
/// leave alone this run, numbered as [`likes::Plan::removals`] lists them; `playlist` also writes
/// `Liked Songs.m3u8`.
#[allow(clippy::too_many_arguments)]
pub fn likes(
    roots: &[PathBuf],
    creds: lastfm::Credentials,
    state_path: &Path,
    opts: &likes::Options,
    leave: &dyn Fn(usize, likes::Side, &likes::Track) -> bool,
    apply: bool,
    playlist: bool,
    cancel: &dyn Fn() -> bool,
    say: &mut dyn FnMut(String),
) -> Result<LikesReport, String> {
    let volumes: Vec<likes::Volume> = roots
        .iter()
        .enumerate()
        .map(|(i, root)| likes::Volume::new(root.clone(), if i == 0 { "internal" } else { "card" }))
        .collect();

    // ── the player ──
    let present: Vec<&likes::Volume> = volumes.iter().filter(|v| v.present()).collect();
    let device = if present.is_empty() {
        likes::Source::missing("device", "the player is not connected — nothing read, nothing removed")
    } else {
        let mut tracks = Vec::new();
        for volume in &present {
            tracks.extend(likes::read_loved(volume));
        }
        let mut source = likes::Source::from_tracks("device", tracks);
        if present.iter().any(|v| v.import_pending()) {
            source.additive_only = true;
            source.note = "an earlier push is still waiting for the player to merge it — additive only".into();
        }
        source
    };
    say(format!(
        "player: {}",
        if device.available {
            format!("{} liked track(s)", thousands(device.tracks.len()))
        } else {
            "not connected".into()
        }
    ));

    // ── Last.fm ──
    // An account that is not set up yet is a source that is NOT AVAILABLE, not a reason to stop:
    // the run still shows what the player holds and what a first sync would do. Only the merge
    // rules care, and they already know that an absent source proves nothing.
    let mut not_configured = String::new();
    let mut client = match lastfm::Client::new(creds) {
        Ok(client) => Some(client),
        Err(why) => {
            not_configured = why.to_string();
            None
        }
    };
    let lastfm_source = match client.as_mut() {
        None => likes::Source::missing("lastfm", &not_configured),
        Some(client) => match client.loved_tracks(|page, pages, so_far| {
            if pages > 1 {
                say(format!("  Last.fm page {page}/{pages} ({so_far} so far)"));
            }
        }) {
            Ok(loved) => {
                likes::Source::from_tracks("lastfm", loved.into_iter().map(|l| likes::Track::new(&l.artist, &l.title)))
            }
            Err(e) => likes::Source::missing("lastfm", &format!("{e}")),
        },
    };
    say(format!(
        "Last.fm: {}",
        if lastfm_source.available {
            format!("{} loved track(s)", thousands(lastfm_source.tracks.len()))
        } else {
            lastfm_source.note.clone()
        }
    ));

    let state = likes::State::load(state_path);
    let mut plan = likes::plan(&state, &device, &lastfm_source, opts);
    match opts.direction {
        likes::Direction::Both => {}
        likes::Direction::ToPlayer => plan.notes.push("to the player only: Last.fm is not written".into()),
        likes::Direction::ToLastfm => plan.notes.push("to Last.fm only: the player is not written".into()),
    }
    if opts.add_only {
        plan.notes.push("add only: nothing is removed on either side".into());
    }
    for note in &plan.notes {
        say(format!("  note: {note}"));
    }
    say(format!(
        "merged: {} liked track(s) — to the player: {} to add, {} to remove; to Last.fm: {} to love, {} to unlove",
        thousands(plan.liked.len()),
        plan.device_add.len(),
        plan.device_remove.len(),
        plan.lastfm_love.len(),
        plan.lastfm_unlove.len()
    ));
    for track in plan.lastfm_love.iter().take(3) {
        say(format!("    love   {} — {}", track.artist, track.title));
    }
    // Every removal, numbered, so one can be left alone (`--leave N`).
    for (n, (side, track)) in plan.removals().enumerate() {
        let what = match side {
            likes::Side::Player => "take off the player",
            likes::Side::Lastfm => "unlove on Last.fm",
        };
        let left = if leave(n + 1, side, track) { "  (left alone)" } else { "" };
        say(format!("  {:>3}  {what}: {} — {}{left}", n + 1, track.artist, track.title));
    }
    plan.leave(leave);

    let mut report =
        LikesReport { device: device.available, lastfm: lastfm_source.available, ..LikesReport::default() };
    if !apply {
        report.plan = plan;
        return Ok(report);
    }
    if !device.available && !lastfm_source.available {
        return Err("neither side could be read — nothing to do".into());
    }

    // ── writes, Last.fm first: it is the side that can refuse ──
    if let Some(client) = client.as_mut() {
        for track in &plan.lastfm_love {
            if cancel() {
                break;
            }
            match client.love(&track.artist, &track.title) {
                Ok(()) => report.loved += 1,
                Err(e) => say(format!("    love failed for {} — {}: {e}", track.artist, track.title)),
            }
        }
        for track in &plan.lastfm_unlove {
            if cancel() {
                break;
            }
            match client.unlove(&track.artist, &track.title) {
                Ok(()) => report.unloved += 1,
                Err(e) => say(format!("    unlove failed for {} — {}: {e}", track.artist, track.title)),
            }
        }
    }
    if cancel() {
        // Stopped part way through Last.fm: the state is NOT saved, so the next run compares
        // against the last complete picture and finishes the job rather than undoing it.
        say("stopped before the player was written; run it again to finish".into());
        report.plan = plan;
        return Ok(report);
    }
    if lastfm_source.available {
        say(format!("Last.fm: {} loved, {} unloved", report.loved, report.unloved));
    }

    // ── the player: the whole list, to the internal volume (where cinder_liked.conf lives) ──
    let held = plan.held(likes::Side::Player);
    if let Some(volume) = present.first().filter(|_| opts.direction != likes::Direction::ToLastfm) {
        let tracks: Vec<likes::Track> = held.values().cloned().collect();
        likes::write_import(volume, &tracks).map_err(|e| format!("{}: {e}", volume.import_path().display()))?;
        say(format!("player: {} track(s) written to {}", thousands(tracks.len()), volume.import_path().display()));
        say("        Cinder merges it on the next start and renames it .done".into());
        report.pushed = true;
    }

    // ── the playlist, per volume: what works on ANY Cinder build, hearts or no hearts ──
    // Skipped unless asked for: it means reading the tags of every file on the player over USB,
    // and the hearts themselves need no paths at all.
    if playlist {
        for volume in &present {
            let mut seen = 0usize;
            let index = likes::index_volume(volume, |n| seen = n + 1);
            let rows = likes::playlist_rows(&held, &index);
            likes::write_playlist(volume, &rows).map_err(|e| format!("{}: {e}", volume.playlist_path().display()))?;
            say(format!(
                "{}: {} of {} liked track(s) found among {} file(s) — {}",
                volume.label,
                rows.len(),
                held.len(),
                thousands(seen),
                volume.playlist_path().display()
            ));
        }
    }

    // ── remember what each side looked like, for the next run ──
    // The device's snapshot is the list just pushed, not the one read: that IS what it will hold
    // once it merges, and recording the old one would make the push look like an unlike.
    let mut next = likes::State { last_sync: now_unix(), liked: plan.liked.clone(), ..likes::State::default() };
    if device.available {
        next.snapshots.insert("device".into(), if report.pushed { held } else { device.tracks.clone() });
    } else if let Some(old) = state.snapshots.get("device") {
        next.snapshots.insert("device".into(), old.clone());
    }
    if lastfm_source.available {
        let written = opts.direction != likes::Direction::ToPlayer;
        next.snapshots.insert(
            "lastfm".into(),
            if written { plan.held(likes::Side::Lastfm) } else { lastfm_source.tracks.clone() },
        );
    } else if let Some(old) = state.snapshots.get("lastfm") {
        next.snapshots.insert("lastfm".into(), old.clone());
    }
    next.save(state_path).map_err(|e| format!("{}: {e}", state_path.display()))?;
    say(format!("state: {}", state_path.display()));
    report.plan = plan;
    Ok(report)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn thousands(n: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flint-lastfm-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const LOG: &str = "#AUDIOSCROBBLER/1.1\n#TZ/UNKNOWN\n#CLIENT/Cinder\n\
        A\tAl\tFirst\t1\t200\tL\t1700000000\t\n\
        A\tAl\tSecond\t2\t200\tS\t1700000300\t\n";

    /// Plays Last.fm took leave the log; plays it refused for good leave it too, into the refused
    /// log with the reason, before the log is rewritten; everything else stays.
    #[test]
    fn refused_for_good_moves_to_its_own_log() {
        let vol = tmp("refused");
        let path = vol.join(".scrobbler.log");
        let body = "#AUDIOSCROBBLER/1.1\n#TZ/UTC\n\
            A\tAl\tTaken\t1\t200\tL\t1700000000\t\n\
            A\tAl\tOld\t2\t200\tL\t1600000000\t\n\
            A\tAl\tLater\t3\t200\tL\t1700000600\t\n";
        std::fs::write(&path, body).unwrap();
        let log = scrobblelog::parse(body);
        let row = |t: &str| log.entries.iter().find(|e| e.track == t).unwrap().clone();
        let mut report = ScrobbleReport::default();
        let refused = vec![(row("Old"), "Timestamp too old (code 3)".to_string())];
        rewrite(&path, &log, &[row("Taken")], &refused, &mut report, &mut |_| {}).unwrap();
        let left = std::fs::read_to_string(&path).unwrap();
        let moved = std::fs::read_to_string(vol.join(REFUSED_LOG)).unwrap();
        let _ = std::fs::remove_dir_all(&vol);
        assert!(left.contains("\tLater\t") && !left.contains("\tTaken\t") && !left.contains("\tOld\t"));
        assert!(moved.contains("# Timestamp too old (code 3)\nA\tAl\tOld\t2\t200\tL\t1600000000\t"));
        assert_eq!(report.removed, 1, "only what Last.fm took counts as sent");
    }

    /// A dry run reads the log, counts the plays and writes nothing — and needs no account.
    #[test]
    fn a_dry_scrobble_counts_and_writes_nothing() {
        let vol = tmp("dry");
        std::fs::write(vol.join(".scrobbler.log"), LOG).unwrap();
        let mut lines = Vec::new();
        let report = scrobble(std::slice::from_ref(&vol), lastfm::Credentials::default(), false, &|| false, &mut |l| {
            lines.push(l)
        })
        .unwrap();
        assert_eq!(report.found, 1, "one play; the skip is not sent");
        assert_eq!(report.removed, 0);
        assert_eq!(std::fs::read_to_string(vol.join(".scrobbler.log")).unwrap(), LOG, "a dry run wrote");
        assert!(lines.iter().any(|l| l.contains("1 play(s) to send")), "{lines:#?}");
        let _ = std::fs::remove_dir_all(&vol);
    }

    /// A `#TZ/UNKNOWN` log goes out in UTC; a `#TZ/UTC` one goes out as written. The rows sent
    /// are copies — the originals, which is what comes out of the file, are untouched.
    #[test]
    fn plays_from_a_local_clock_are_sent_in_utc() {
        let local = scrobblelog::parse(LOG);
        assert!(local.local_time());
        let plays = local.plays_to_send();
        let wire = to_send(&local, &plays);
        assert_eq!(wire[0].timestamp, crate::localtime::local_to_utc(1_700_000_000));
        assert_eq!(plays[0].timestamp, 1_700_000_000, "the row to remove keeps its written time");
        assert_eq!((wire[0].artist.as_str(), wire[0].track.as_str()), ("A", "First"));

        let utc = scrobblelog::parse(&LOG.replace("#TZ/UNKNOWN", "#TZ/UTC"));
        assert!(!utc.local_time());
        assert_eq!(to_send(&utc, &utc.plays_to_send())[0].timestamp, 1_700_000_000);
    }

    /// Sending with no key says so before touching the network or the log.
    #[test]
    fn sending_without_a_key_is_an_error_and_keeps_the_log() {
        let vol = tmp("nokey");
        std::fs::write(vol.join(".scrobbler.log"), LOG).unwrap();
        let err = scrobble(std::slice::from_ref(&vol), lastfm::Credentials::default(), true, &|| false, &mut |_| {})
            .unwrap_err();
        assert!(err.contains("API key"), "{err}");
        assert_eq!(std::fs::read_to_string(vol.join(".scrobbler.log")).unwrap(), LOG);
        let _ = std::fs::remove_dir_all(&vol);
    }

    /// Without an account, comparing likes still reads the player and plans — Last.fm is simply
    /// an absent source — and a dry run leaves no state behind.
    #[test]
    fn comparing_likes_without_an_account_reads_the_player_only() {
        let vol = tmp("likes");
        std::fs::write(vol.join("cinder_loved.tsv"), "A\tFirst\nB\tSecond\n").unwrap();
        let state = vol.join("likes-state.tsv");
        let report = likes(
            std::slice::from_ref(&vol),
            lastfm::Credentials::default(),
            &state,
            &likes::Options::default(),
            &|_, _, _| false,
            false,
            false,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert!(report.device && !report.lastfm);
        assert_eq!(report.plan.liked.len(), 2);
        assert!(!state.exists(), "a comparison saved state");
        let _ = std::fs::remove_dir_all(&vol);
    }
}
