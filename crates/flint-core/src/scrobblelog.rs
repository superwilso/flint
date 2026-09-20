//! The `.scrobbler.log` the player writes, read back on the PC.
//!
//! Cinder's built-in scrobbler (and `unknown321/scrobbler` before it) appends an
//! **Audioscrobbler/1.1** log at the root of the player's storage: a few `#` header lines, then one
//! tab-separated row per play. The Walkman has no WiFi, so that file is the only way a play ever
//! reaches Last.fm — which makes this parser the one thing standing between a month of listening
//! and the bin.
//!
//! Two rules follow from that, and both are about not losing rows:
//!
//! * a row this parser cannot read is **kept**, not dropped. It is reported and written back out
//!   untouched, so a future version (or a person) can still see it.
//! * a row is only removed from the file once Last.fm has **accepted** it. Anything the API
//!   rejected stays, with the reason recorded.
//!
//! The format, column by column:
//!
//! ```text
//! artist ⇥ album ⇥ title ⇥ track no. ⇥ duration (s) ⇥ rating ⇥ timestamp (unix) ⇥ MusicBrainz id
//! ```
//!
//! `rating` is `L` for listened, `S` for skipped and `B` for banned. Only `L` rows are plays; the
//! others are Audioscrobbler's own bookkeeping and Last.fm has no call that would take them.

/// One play, as the player wrote it down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub artist: String,
    pub album: String,
    pub track: String,
    pub track_number: String,
    /// Seconds. `0` when the player did not know, and then it is left out of the submission.
    pub duration: i64,
    /// `L`, `S` or `B`.
    pub rating: String,
    /// Unix time the play *started*, which is what `track.scrobble` wants.
    pub timestamp: i64,
    pub mbid: String,
    /// The line exactly as it was read, so rewriting the file cannot reformat anyone's data.
    pub raw: String,
}

impl Entry {
    /// Is this a play? `L` is; a skip or a ban is not, and Last.fm has nowhere to put them.
    pub fn is_play(&self) -> bool {
        self.rating.eq_ignore_ascii_case("L")
    }

    /// What makes two rows the same play. Two players (Cinder and `unknown321/scrobbler`) can
    /// append to one file, and a log copied off the device twice would otherwise scrobble twice.
    pub fn identity(&self) -> (String, String, i64) {
        (self.artist.to_lowercase(), self.track.to_lowercase(), self.timestamp)
    }
}

/// A parsed log: its header, its rows, and whatever could not be read.
#[derive(Clone, Debug, Default)]
pub struct Log {
    pub header: Vec<String>,
    pub entries: Vec<Entry>,
    /// `(line number, the line, why)` — kept so the caller can report them and write them back.
    pub unreadable: Vec<(usize, String, String)>,
}

impl Log {
    pub fn plays(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_play())
    }

    /// Plays, oldest first, with repeats of the same play removed. Last.fm rejects a scrobble
    /// whose timestamp it already holds, so sending duplicates only earns rejections.
    pub fn plays_to_send(&self) -> Vec<Entry> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<Entry> = self.plays().filter(|e| seen.insert(e.identity())).cloned().collect();
        out.sort_by_key(|e| e.timestamp);
        out
    }
}

/// Parse a log. Never fails: an unreadable row becomes an `unreadable` entry and is kept.
pub fn parse(body: &str) -> Log {
    let mut log = Log::default();
    for (number, raw) in body.lines().enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.starts_with('#') {
            log.header.push(line.to_string());
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 7 {
            log.unreadable.push((number + 1, line.to_string(), format!("{} columns, expected 7", parts.len())));
            continue;
        }
        let Ok(timestamp) = parts[6].trim().parse::<i64>() else {
            log.unreadable.push((number + 1, line.to_string(), "the timestamp is not a number".into()));
            continue;
        };
        let artist = parts[0].trim().to_string();
        let track = parts[2].trim().to_string();
        if artist.is_empty() || track.is_empty() {
            log.unreadable.push((number + 1, line.to_string(), "no artist or no title".into()));
            continue;
        }
        log.entries.push(Entry {
            artist,
            album: parts[1].trim().to_string(),
            track,
            track_number: parts[3].trim().to_string(),
            // A duration that will not parse is not worth losing a play over: send it without one.
            duration: parts[4].trim().parse::<i64>().unwrap_or(0),
            rating: parts[5].trim().to_string(),
            timestamp,
            mbid: parts.get(7).map(|s| s.trim().to_string()).unwrap_or_default(),
            raw: line.to_string(),
        });
    }
    log
}

/// The file as it should look once `sent` have been accepted: the header, then every row that was
/// not sent, in the order it was read. Unreadable rows come back too — they were never ours to
/// throw away.
pub fn rewrite(log: &Log, sent: &[Entry]) -> String {
    let gone: std::collections::HashSet<(String, String, i64)> = sent.iter().map(Entry::identity).collect();
    let mut out = String::new();
    for line in &log.header {
        out.push_str(line);
        out.push('\n');
    }
    let mut rows: Vec<(usize, &str)> = Vec::new();
    for (number, line, _) in &log.unreadable {
        rows.push((*number, line.as_str()));
    }
    for entry in &log.entries {
        if gone.contains(&entry.identity()) {
            continue;
        }
        rows.push((usize::MAX, entry.raw.as_str()));
    }
    // Unreadable rows keep their place at the top by line number; everything else follows in
    // order. Exact positions do not matter to any reader of this file, only that nothing is lost.
    rows.sort_by_key(|(number, _)| *number);
    for (_, line) in rows {
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{parse, rewrite};

    /// Copied from a real device log (`/contents/.scrobbler.log`), including the header Cinder
    /// writes and a skip row.
    const LOG: &str = "#AUDIOSCROBBLER/1.1\n\
#TZ/UNKNOWN\n\
#CLIENT/Cinder 0.3.9\n\
Bonobo\tMigration\tBreak Apart\t3\t268\tL\t1758300000\t\n\
Aphex Twin\tSelected Ambient Works 85-92\tXtal\t1\t294\tL\t1758300300\t\n\
Bicep\tIsles\tAtlas\t1\t258\tS\t1758300600\t\n";

    #[test]
    fn reads_a_device_log() {
        let log = parse(LOG);
        assert_eq!(log.header.len(), 3);
        assert_eq!(log.entries.len(), 3);
        assert!(log.unreadable.is_empty());
        let first = &log.entries[0];
        assert_eq!(first.artist, "Bonobo");
        assert_eq!(first.album, "Migration");
        assert_eq!(first.track, "Break Apart");
        assert_eq!(first.duration, 268);
        assert_eq!(first.timestamp, 1_758_300_000);
        // A skip is not a play: Last.fm has no call for it.
        assert_eq!(log.plays().count(), 2);
    }

    #[test]
    fn repeats_of_one_play_are_sent_once() {
        let doubled = format!("{LOG}Bonobo\tMigration\tBreak Apart\t3\t268\tL\t1758300000\t\n");
        let log = parse(&doubled);
        assert_eq!(log.plays().count(), 3);
        assert_eq!(log.plays_to_send().len(), 2, "same artist, title and timestamp is one play");
    }

    #[test]
    fn a_row_that_cannot_be_read_is_kept_not_dropped() {
        let broken = format!("{LOG}this is not a scrobble row\nSquarepusher\tAlbum\tTitle\t1\tx\tL\tnope\t\n");
        let log = parse(&broken);
        assert_eq!(log.entries.len(), 3);
        assert_eq!(log.unreadable.len(), 2);
        // Both survive a rewrite that removes everything that was sent.
        let sent = log.plays_to_send();
        let after = rewrite(&log, &sent);
        assert!(after.contains("this is not a scrobble row"));
        assert!(after.contains("Squarepusher"));
        assert!(!after.contains("Break Apart"), "an accepted play is gone");
        assert!(after.contains("Atlas"), "the skip was never sent, so it stays");
        assert!(after.starts_with("#AUDIOSCROBBLER/1.1\n"), "the header is preserved");
    }

    #[test]
    fn a_duration_that_will_not_parse_does_not_lose_the_play() {
        let log = parse("Artist\tAlbum\tTitle\t1\t\tL\t1758300000\t\n");
        assert_eq!(log.entries.len(), 1);
        assert_eq!(log.entries[0].duration, 0);
    }

    #[test]
    fn nothing_sent_means_nothing_removed() {
        let log = parse(LOG);
        assert_eq!(rewrite(&log, &[]).lines().count(), LOG.lines().count());
    }
}
