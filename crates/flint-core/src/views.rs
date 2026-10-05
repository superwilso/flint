//! Cinder's saved views (smart playlists): `cinder_views.conf` at the top of the player's drive.
//!
//! A saved view is a name, three rules and a sort; the player shows it as a smart playlist holding
//! whatever the rules match at that moment. The songs are not stored, only the rules. The format is
//! Cinder's (`docs/TRACK_DATA.md` in the Cinder repository):
//!
//! ```text
//! [Late favourites]
//! rating=4
//! played=recent
//! format=flac
//! sort=plays
//! shuffle=settings
//! ```
//!
//! This reads the file the way the player does, so Flint shows the views the player would show,
//! and writes it the way the player does, byte for byte, so a file that went through Flint is the
//! file the player wrote. A sync never touches it: it is not music and it is not at the music root.

use std::path::Path;

pub const FILE_NAME: &str = "cinder_views.conf";
/// The player reads this many and ignores the rest.
pub const MAX_VIEWS: usize = 32;
const NAME_CHARS: usize = 48;

/// One rule's value: the word in the file, and how to say it.
macro_rules! rule {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $variant:ident => $token:literal, $label:literal;)+ }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum $name { #[default] $($(#[$vdoc])* $variant,)+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant,)+];
            /// The word in the file.
            pub fn token(self) -> &'static str { match self { $($name::$variant => $token,)+ } }
            /// How the rule reads in a sentence. Empty for the rule that matches everything.
            pub fn label(self) -> &'static str { match self { $($name::$variant => $label,)+ } }
            /// A word nobody knows is the default, as on the player.
            fn from_token(s: &str) -> $name {
                Self::ALL.iter().copied().find(|v| v.token() == s.trim()).unwrap_or_default()
            }
        }
    };
}

rule! {
    /// When the track was last listened to.
    Played {
        Any => "any", "";
        /// In the last 30 days.
        Recent => "recent", "played in the last 30 days";
        /// Played before, but not in the last 90 days.
        NotLately => "not_lately", "not played in 90 days";
        Never => "never", "never played";
    }
}

rule! {
    /// By file extension; `hires` is the database's Hi-Res flag.
    Format {
        Any => "any", "";
        Flac => "flac", "FLAC";
        Mp3 => "mp3", "MP3";
        M4a => "m4a", "M4A";
        HiRes => "hires", "Hi-Res";
    }
}

rule! {
    Sort {
        Title => "title", "A to Z";
        Plays => "plays", "most played first";
        Played => "played", "most recently played first";
        Rating => "rating", "highest rated first";
        Added => "added", "newest first";
    }
}

rule! {
    /// What the view's Shuffle band deals. `Settings` follows the player's Shuffle setting.
    Shuffle {
        Settings => "settings", "";
        Songs => "songs", "shuffles songs";
        Albums => "albums", "shuffles albums";
        Artists => "artists", "shuffles artists";
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedView {
    pub name: String,
    /// Minimum stars, 0 to 5. 0 is any, rated or not.
    pub min_rating: u8,
    pub played: Played,
    pub format: Format,
    pub sort: Sort,
    pub shuffle: Shuffle,
}

impl SavedView {
    /// The view in one line: `4 stars and up · played in the last 30 days · FLAC · most played first`.
    pub fn describe(&self) -> String {
        let rating = match self.min_rating {
            0 => String::new(),
            5 => "5 stars".to_string(),
            1 => "1 star and up".to_string(),
            n => format!("{n} stars and up"),
        };
        let mut parts: Vec<&str> =
            [rating.as_str(), self.played.label(), self.format.label()].into_iter().filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            parts.push("every track");
        }
        parts.push(self.sort.label());
        if !self.shuffle.label().is_empty() {
            parts.push(self.shuffle.label());
        }
        parts.join(" · ")
    }
}

/// A name as the player keeps it: no control characters or brackets, single spaces, 48 characters.
fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control() && *c != '[' && *c != ']').collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_CHARS).collect()
}

/// `name`, or `name 2`, `name 3`…: the first spelling no view already has, without regard to case.
fn unique_name(views: &[SavedView], name: &str) -> String {
    let taken = |n: &str| views.iter().any(|v| v.name.to_lowercase() == n.to_lowercase());
    if !taken(name) {
        return name.to_string();
    }
    (2..1000).map(|k| format!("{name} {k}")).find(|n| !taken(n)).unwrap_or_else(|| name.to_string())
}

/// Read `cinder_views.conf` as the player does: an unknown key or a bad value is that key's
/// default, a line before the first section is ignored, and a section the player would not keep
/// (no name, or past the limit) takes its keys with it.
pub fn parse(body: &str) -> Vec<SavedView> {
    let mut out: Vec<SavedView> = Vec::new();
    for line in body.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix('[') {
            let name = clean_name(rest.rsplit_once(']').map_or(rest, |(n, _)| n));
            // The player counts every section it has seen, kept or not, against the limit.
            if !name.is_empty() && out.len() < MAX_VIEWS {
                let name = unique_name(&out, &name);
                out.push(SavedView { name, ..SavedView::default() });
            } else {
                // A section that is not kept: stop its keys landing on the view before it.
                out.push(SavedView::default());
            }
            continue;
        }
        let Some(v) = out.last_mut() else { continue };
        let Some((k, val)) = l.split_once('=') else { continue };
        match k.trim() {
            "rating" => v.min_rating = val.trim().parse::<u8>().unwrap_or(0).min(5),
            "played" => v.played = Played::from_token(val),
            "format" => v.format = Format::from_token(val),
            "sort" => v.sort = Sort::from_token(val),
            "shuffle" => v.shuffle = Shuffle::from_token(val),
            _ => {}
        }
    }
    out.retain(|v| !v.name.is_empty());
    out
}

/// Write `cinder_views.conf` exactly as the player writes it: every key, defaults included.
pub fn render(views: &[SavedView]) -> String {
    let mut s =
        String::from("# Cinder saved views (smart playlists). Written by the player; see docs/TRACK_DATA.md.\n");
    for v in views {
        s.push_str(&format!(
            "\n[{}]\nrating={}\nplayed={}\nformat={}\nsort={}\nshuffle={}\n",
            clean_name(&v.name),
            v.min_rating,
            v.played.token(),
            v.format.token(),
            v.sort.token(),
            v.shuffle.token()
        ));
    }
    s
}

/// A missing file is a player with no saved views.
pub fn load(path: &Path) -> Vec<SavedView> {
    std::fs::read_to_string(path).map(|b| parse(&b)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in Cinder's `docs/TRACK_DATA.md`, as the player writes it.
    const SAMPLE: &str = "# Cinder saved views (smart playlists). Written by the player; see docs/TRACK_DATA.md.\n\
        \n[Late favourites]\nrating=4\nplayed=recent\nformat=flac\nsort=plays\nshuffle=settings\n";

    #[test]
    fn the_documented_file_reads_and_writes_back_byte_for_byte() {
        let views = parse(SAMPLE);
        assert_eq!(
            views,
            [SavedView {
                name: "Late favourites".into(),
                min_rating: 4,
                played: Played::Recent,
                format: Format::Flac,
                sort: Sort::Plays,
                shuffle: Shuffle::Settings,
            }]
        );
        assert_eq!(render(&views), SAMPLE);
        assert_eq!(views[0].describe(), "4 stars and up · played in the last 30 days · FLAC · most played first");
    }

    #[test]
    fn a_missing_key_or_an_unknown_word_is_the_default() {
        let v = &parse("[Plain]\nrating=9\nplayed=sometimes\nformat=ogg\nsort=\nshuffle=sideways\ncolour=red\n")[0];
        assert_eq!(
            (v.min_rating, v.played, v.format, v.sort, v.shuffle),
            (5, Played::Any, Format::Any, Sort::Title, Shuffle::Settings)
        );
        let v = &parse("[Old]\n")[0];
        assert_eq!(v.describe(), "every track · A to Z");
        assert_eq!(
            parse("[Deal]\nshuffle=albums\nplayed=never\n")[0].describe(),
            "never played · A to Z · shuffles albums"
        );
    }

    #[test]
    fn names_are_cleaned_and_made_unique_the_way_the_player_does() {
        let views = parse("rating=5\n[Mix]\nrating=1\n[ mix ]\n[]\nrating=3\n[A  [b]  c]\n");
        let names: Vec<&str> = views.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Mix", "mix 2", "A b c"], "a second Mix is Mix 2; brackets inside a name are dropped");
        assert_eq!(views[0].min_rating, 1, "a key before the first section belongs to nothing");
        assert_eq!(views[1].min_rating, 0, "a nameless section's keys do not land on the view before it");
    }

    #[test]
    fn only_as_many_views_as_the_player_reads() {
        let body: String = (0..40).map(|i| format!("[View {i}]\nrating=2\n")).collect();
        let views = parse(&body);
        assert_eq!(views.len(), MAX_VIEWS);
        assert!(views.iter().all(|v| v.min_rating == 2));
    }

    #[test]
    fn every_word_round_trips() {
        for &played in Played::ALL {
            for &format in Format::ALL {
                for &sort in Sort::ALL {
                    for &shuffle in Shuffle::ALL {
                        let v = SavedView { name: "V".into(), min_rating: 3, played, format, sort, shuffle };
                        assert_eq!(parse(&render(std::slice::from_ref(&v))), [v]);
                    }
                }
            }
        }
    }
}
