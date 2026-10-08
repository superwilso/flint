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
pub mod pages;
pub mod paint;
pub mod prefs;
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
    /// Show a page.
    Tab(Tab),
    /// Settings ▸ Theme.
    Theme(ThemePref),
    /// Read what is on the player: albums and who put them there, plays, likes, palettes.
    ReadPlayer,
    /// On the player ▸ take the playlists the player has changed back to the PC.
    PullPlaylists,
    /// Choose the folder of `.palette` files on this PC.
    PickPalettes,
    /// Check the PC's palettes with Cinder's own rules, against what the player holds.
    CheckPalettes,
    /// Copy the new and changed palettes to the player.
    SendPalettes,
    /// A line of text that takes typing: click to type into it.
    Field(Field),
    /// Check ▸ one verdict's card: show only the files with that verdict. Pressed again, all.
    Verdict(usize),
    /// Check ▸ forget the filter.
    ClearFilter,
    /// Settings ▸ Last.fm ▸ keep the API key and secret typed in.
    LastfmSaveKey,
    /// Settings ▸ Last.fm ▸ open the page where a key is made, in the browser.
    LastfmGetKey,
    /// SensMe ▸ open Sony's Music Center download page, which installs the analysis engine.
    GetMusicCenter,
    /// SensMe ▸ open FFmpeg's download page.
    GetFfmpeg,
    /// Settings ▸ Last.fm ▸ sign in through the browser.
    LastfmSignIn,
    LastfmSignOut,
    /// Settings ▸ Last.fm ▸ type a different key.
    LastfmChangeKey,
    /// …and go back to the one already saved.
    LastfmKeepKey,
    /// Likes & plays ▸ send the plays to Last.fm.
    Scrobble,
    /// Likes & plays ▸ work out what keeping likes in step would change. Writes nothing.
    CompareLikes,
    /// Likes & plays ▸ make those changes.
    SyncLikes,
    /// Palettes ▸ open the editor on a new palette, starting from Cinder's colours.
    NewPalette,
    /// Palettes ▸ the editor ▸ start again from this palette: 0 is Cinder, then `palette::EXAMPLES`.
    PaletteStart(usize),
    /// Palettes ▸ the editor ▸ bring an accent of its own, or offer all six of Cinder's.
    ToggleOwnAccent,
    /// Palettes ▸ the editor ▸ write it into the palettes folder as `<id>.palette`.
    SavePalette,
    /// Palettes ▸ the editor ▸ close it. What was typed is kept until the next New palette.
    ClosePalette,
    /// Palettes ▸ the editor ▸ open the shared repository's form, filled in with this palette.
    SharePalette,
    /// Palettes ▸ download the shared palettes into the folder. Never replaces a file.
    FetchPalettes,
    /// Palettes ▸ open the shared repository in the browser.
    BrowsePalettes,
    /// Palettes ▸ show the shared palettes: search them, see them, install one.
    OpenShop,
    /// Palettes ▸ the shared palettes ▸ back to the folder's table.
    CloseShop,
    /// Palettes ▸ the shared palettes ▸ read the list again from the repository.
    FetchShop,
    /// Palettes ▸ the shared palettes ▸ put this one (an index into `Shop::items`) in the folder,
    /// and on the player when its internal memory is chosen.
    InstallShared(usize),
}

/// The lines of text the window takes typing into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    ApiKey,
    ApiSecret,
    /// Check ▸ the filter over the flagged files.
    CheckFilter,
    /// Palettes ▸ the editor ▸ the name the player shows.
    PaletteName,
    /// Palettes ▸ the editor ▸ one colour, by its place in `palette::KEYS`.
    Colour(u8),
    /// Palettes ▸ the shared palettes ▸ what to look for.
    ShopSearch,
}

impl Field {
    /// Shown as dots. The secret is a password in all but name: anyone holding it and the key can
    /// act as this application.
    pub fn masked(self) -> bool {
        self == Field::ApiSecret
    }

    /// What an empty field says it is for.
    pub fn placeholder(self) -> &'static str {
        match self {
            Field::ApiKey => "Paste the API key",
            Field::ApiSecret => "Paste the shared secret",
            Field::CheckFilter => "Filter by file, folder or reason",
            Field::PaletteName => "Name it",
            Field::Colour(_) => "#rrggbb",
            Field::ShopSearch => "Search by name, light, dark or accent",
        }
    }

    /// The most it takes. A colour is `#` and six digits; a name is what the player shows.
    pub fn max(self) -> usize {
        match self {
            Field::Colour(_) => 7,
            Field::PaletteName => flint_core::palette::MAX_NAME,
            _ => Field::MAX,
        }
    }

    /// Where Tab goes from here.
    fn next(self, own_accent: bool) -> Field {
        let colours = if own_accent { 18 } else { 12 };
        match self {
            Field::ApiKey => Field::ApiSecret,
            Field::ApiSecret => Field::ApiKey,
            Field::CheckFilter => Field::CheckFilter,
            Field::ShopSearch => Field::ShopSearch,
            Field::PaletteName => Field::Colour(0),
            Field::Colour(i) if (i as usize) + 1 < colours => Field::Colour(i + 1),
            Field::Colour(_) => Field::PaletteName,
        }
    }

    /// Longer than any real key, filter or path fragment; a paste of a whole document stops here.
    pub const MAX: usize = 200;
}

/// A key the focused field acts on. Everything that is not one of these is ignored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Backspace,
    /// Ctrl+Backspace: empty the field.
    Clear,
    Enter,
    Tab,
    Escape,
}

/// Settings ▸ Last.fm, as far as the window needs to know it. Built from the credentials file by
/// [`lastfm_facts`]; the secret and the session key never leave it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Lastfm {
    /// An API key and its secret are saved.
    pub has_key: bool,
    /// Signed in: `Some(name)`, where the name is empty if Last.fm never gave one.
    pub user: Option<String>,
    /// Settings is showing the key fields over a key that is already saved.
    pub editing: bool,
}

impl Lastfm {
    pub fn signed_in(&self) -> bool {
        self.has_key && self.user.is_some()
    }

    /// Settings ▸ Last.fm ▸ Account, in words.
    pub fn account(&self) -> String {
        match (&self.user, self.has_key) {
            (_, false) => "Not set up".into(),
            (None, true) => "Not signed in".into(),
            (Some(n), true) if n.is_empty() => "Signed in".into(),
            (Some(n), true) => format!("Signed in as {n}"),
        }
    }
}

/// What the credentials file says, for the window.
pub fn lastfm_facts(c: &flint_core::lastfm::Credentials) -> Lastfm {
    let has_key = !c.api_key.is_empty() && !c.api_secret.is_empty();
    Lastfm { has_key, user: (has_key && !c.session_key.is_empty()).then(|| c.username.clone()), editing: false }
}

/// What "Compare likes" found would change.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LikesPlan {
    pub liked: usize,
    pub device_add: usize,
    pub device_remove: usize,
    pub lastfm_love: usize,
    pub lastfm_unlove: usize,
}

impl LikesPlan {
    pub fn changes(&self) -> usize {
        self.device_add + self.device_remove + self.lastfm_love + self.lastfm_unlove
    }
}

/// The window's pages, in the order the tabs are drawn — the same order in every page, so a tab
/// never moves under the pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    #[default]
    Sync,
    Player,
    Check,
    SensMe,
    Likes,
    Palettes,
    Settings,
}

impl Tab {
    /// Its place in [`Tab::ALL`].
    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub const ALL: [Tab; 7] =
        [Tab::Sync, Tab::Player, Tab::Check, Tab::SensMe, Tab::Likes, Tab::Palettes, Tab::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Sync => "Sync",
            Tab::Player => "On the player",
            Tab::Check => "Check",
            Tab::SensMe => "SensMe",
            Tab::Likes => "Likes & plays",
            Tab::Palettes => "Palettes",
            Tab::Settings => "Settings",
        }
    }
}

/// Settings ▸ Theme. System follows Windows, and keeps following it while the window is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThemePref {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::Light, ThemePref::Dark, ThemePref::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::Light => "Light",
            ThemePref::Dark => "Dark",
            ThemePref::System => "System",
        }
    }

    /// The word in `gui.conf`.
    pub fn word(self) -> &'static str {
        match self {
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
            ThemePref::System => "system",
        }
    }

    pub fn from_word(w: &str) -> ThemePref {
        match w.trim() {
            "light" => ThemePref::Light,
            "dark" => ThemePref::Dark,
            _ => ThemePref::System,
        }
    }
}

/// One file `flint check` had something to say about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckRow {
    /// LOSSY, SUSPECT, UPSAMPLED, PADDED, DAMAGED or UNSURE.
    pub verdict: String,
    /// The file, relative to the library.
    pub file: String,
    /// The evidence, in words: the cutoff, the zeroed bits, the error count.
    pub why: String,
}

/// One album folder on the player.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AlbumRow {
    /// The folder, relative to the volume.
    pub folder: String,
    /// Index into `Model::volumes`.
    pub volume: usize,
    pub files: usize,
    pub bytes: u64,
    /// FLAC, MP3, AAC… — the commonest audio format in the folder.
    pub format: String,
    /// At least one file in it is in that volume's `flint-manifest.tsv`.
    pub by_flint: bool,
    /// The stars the player shows for it: the mean of its rated tracks, `None` when none is.
    pub rating: Option<u8>,
    /// Listens the player has counted across its tracks.
    pub plays: u32,
}

/// A playlist made on the player (`cinder_playlists`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlaylistRow {
    pub name: String,
    pub tracks: usize,
    /// The player has changed it since a PC last took it.
    pub edited: bool,
}

/// One row of the player's `.scrobbler.log`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayRow {
    /// Unix time the play started.
    pub when: i64,
    pub track: String,
    pub artist: String,
    /// PLAY (listened) or SKIP.
    pub kind: String,
}

/// A `.palette` file in a volume's `cinder_palettes` folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaletteFile {
    pub volume: usize,
    pub name: String,
    pub bytes: u64,
}

/// Everything "Read the player" found. `read` is false until it has run once.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PlayerFacts {
    pub read: bool,
    pub albums: Vec<AlbumRow>,
    /// Newest first.
    pub plays: Vec<PlayRow>,
    /// Log rows that could not be parsed. They are kept on the player; this only counts them.
    pub unreadable: usize,
    /// Songs liked on the player (`cinder_loved.tsv`).
    pub likes: usize,
    pub palettes: Vec<PaletteFile>,
    /// Tracks the player holds a rating for, and tracks it has counted a listen of
    /// (`cinder_stats.tsv`).
    pub rated: usize,
    pub counted: usize,
    /// The saved views (smart playlists), as (name, its rules in a line).
    pub views: Vec<(String, String)>,
    /// The playlists made on the player.
    pub playlists: Vec<PlaylistRow>,
}

/// A job a worker thread runs. The window never runs one itself — see `win32::start`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Job {
    Plan,
    Apply,
    Scan,
    Import,
    Check,
    /// Read the player: what is on it and who put it there, plays, likes, palettes. Writes nothing.
    ReadPlayer,
    /// Copy the playlists the player has changed into the PC's playlists folder, and take their
    /// EDITED mark off on the player.
    PullPlaylists,
    /// Check the palette folder with Cinder's rules, against the player's. Writes nothing.
    CheckPalettes,
    /// Copy the new and changed palettes to the player's `cinder_palettes`.
    SendPalettes,
    /// Save the API key and secret typed into Settings.
    LastfmKey,
    /// Sign in through the browser: a token from Last.fm, its page opened, then wait for the allow.
    LastfmSignIn,
    /// Forget the session key. The API key stays.
    LastfmSignOut,
    /// Send the plays in `.scrobbler.log` to Last.fm, and take the sent ones out of it.
    Scrobble,
    /// Work out what keeping likes in step would change on each side. Writes nothing.
    CompareLikes,
    /// Carry that out, both ways.
    SyncLikes,
    /// Write the editor's palette into the palettes folder, then check the folder.
    SavePalette,
    /// Download the shared palettes into the palettes folder, then check the folder.
    FetchPalettes,
    /// Read the shared palettes' list and every file on it, for the shop. Writes nothing.
    FetchShop,
    /// Put one shared palette in the folder, and on the player, then check the folder.
    InstallShared,
}

/// Something a job needs to itself while it runs. Two jobs that need the same one cannot run at
/// once; any others can.
///
/// **Why not one job at a time.** Until 0.3 the window ran exactly one job and greyed out every
/// other button until it finished — and an analysis of a large library takes hours, so "Analyse
/// library" locked the whole window for an evening over nothing: checking the FLACs, reading the
/// player and sending palettes share nothing with it. What they genuinely cannot share is a file
/// two of them would both write, or a player one of them is writing to, and that is what these are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hold {
    /// The analysis cache. The scan and the import write it; a plan and a copy take analysis out
    /// of the files into it, and tag the copies from it.
    Cache,
    /// The check store, `checks.tsv`.
    Checks,
    /// The player's drives, while something writes them or must read them whole.
    Player,
    /// The palette folder and the player's `cinder_palettes`.
    Palettes,
    /// The Last.fm credentials file and the account itself.
    Lastfm,
}

impl Job {
    /// Every job, in the order `code` numbers them.
    pub const ALL: [Job; 19] = [
        Job::Plan,
        Job::Apply,
        Job::Scan,
        Job::Import,
        Job::Check,
        Job::ReadPlayer,
        Job::CheckPalettes,
        Job::SendPalettes,
        Job::LastfmKey,
        Job::LastfmSignIn,
        Job::LastfmSignOut,
        Job::Scrobble,
        Job::CompareLikes,
        Job::SyncLikes,
        Job::SavePalette,
        Job::FetchPalettes,
        Job::FetchShop,
        Job::InstallShared,
        Job::PullPlaylists,
    ];

    /// A small number for the platform layer's messages.
    pub fn code(self) -> usize {
        Job::ALL.iter().position(|j| *j == self).unwrap_or(0)
    }

    pub fn from_code(code: usize) -> Option<Job> {
        Job::ALL.get(code).copied()
    }

    pub fn holds(self) -> &'static [Hold] {
        match self {
            Job::Plan | Job::Apply => &[Hold::Cache, Hold::Player],
            Job::Scan | Job::Import => &[Hold::Cache],
            Job::Check => &[Hold::Checks],
            Job::ReadPlayer | Job::PullPlaylists => &[Hold::Player],
            // The shop's list reads the folder and the player to say what each one is there as.
            Job::CheckPalettes
            | Job::SendPalettes
            | Job::SavePalette
            | Job::FetchPalettes
            | Job::FetchShop
            | Job::InstallShared => &[Hold::Palettes],
            Job::LastfmKey | Job::LastfmSignIn | Job::LastfmSignOut => &[Hold::Lastfm],
            Job::Scrobble | Job::CompareLikes | Job::SyncLikes => &[Hold::Lastfm, Hold::Player],
        }
    }

    /// Does it read the music folder? Then the folder cannot change under it.
    pub fn reads_library(self) -> bool {
        matches!(self, Job::Plan | Job::Apply | Job::Scan | Job::Check)
    }

    /// Does it read or write the player's drives? Then they cannot change under it.
    pub fn reads_volumes(self) -> bool {
        matches!(
            self,
            Job::Plan
                | Job::Apply
                | Job::ReadPlayer
                | Job::PullPlaylists
                | Job::CheckPalettes
                | Job::SendPalettes
                | Job::SavePalette
                | Job::FetchPalettes
                | Job::FetchShop
                | Job::InstallShared
                | Job::Scrobble
                | Job::CompareLikes
                | Job::SyncLikes
        )
    }

    /// The pages whose footer shows this job's bar and its Stop.
    pub fn pages(self) -> &'static [Tab] {
        match self {
            Job::Plan | Job::Apply => &[Tab::Sync],
            Job::Scan | Job::Import => &[Tab::SensMe],
            Job::Check => &[Tab::Check],
            Job::ReadPlayer => &[Tab::Player, Tab::Likes],
            Job::PullPlaylists => &[Tab::Player],
            Job::CheckPalettes
            | Job::SendPalettes
            | Job::SavePalette
            | Job::FetchPalettes
            | Job::FetchShop
            | Job::InstallShared => &[Tab::Palettes],
            Job::LastfmKey | Job::LastfmSignIn | Job::LastfmSignOut => &[Tab::Settings],
            Job::Scrobble | Job::CompareLikes | Job::SyncLikes => &[Tab::Likes],
        }
    }

    /// What it is doing, for a page that is not its own: "…while analysing the library".
    pub fn doing(self) -> &'static str {
        match self {
            Job::Plan => "working out the sync",
            Job::Apply => "copying to the player",
            Job::Scan => "analysing the library",
            Job::Import => "importing Music Center's analysis",
            Job::Check => "checking the FLACs",
            Job::ReadPlayer => "reading the player",
            Job::PullPlaylists => "taking playlists back",
            Job::CheckPalettes => "checking palettes",
            Job::SendPalettes => "sending palettes",
            Job::SavePalette => "saving the palette",
            Job::FetchPalettes => "downloading shared palettes",
            Job::FetchShop => "reading the shared palettes",
            Job::InstallShared => "installing a palette",
            Job::LastfmKey => "saving the Last.fm key",
            Job::LastfmSignIn => "signing in to Last.fm",
            Job::LastfmSignOut => "signing out of Last.fm",
            Job::Scrobble => "sending plays to Last.fm",
            Job::CompareLikes => "comparing likes",
            Job::SyncLikes => "syncing likes",
        }
    }

    /// Which log its lines go to.
    pub fn pane(self) -> Pane {
        match self {
            Job::Scan | Job::Import => Pane::SensMe,
            Job::LastfmKey
            | Job::LastfmSignIn
            | Job::LastfmSignOut
            | Job::Scrobble
            | Job::CompareLikes
            | Job::SyncLikes => Pane::Lastfm,
            _ => Pane::Sync,
        }
    }
}

/// The logs. A job's chatter goes to the page it belongs to, so an analysis running in the
/// background does not interleave four thousand lines into a sync's plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Sync,
    SensMe,
    Lastfm,
}

/// A job that is running, and what it last said.
#[derive(Clone, PartialEq, Debug)]
pub struct Running {
    pub job: Job,
    /// 0.0..=1.0, or `None` for a job with no measurable length.
    pub progress: Option<f32>,
    /// The line under its bar.
    pub status: String,
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
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

/// A list that scrolls. Every page has at most one table and one log, so a page and one of these
/// name a list exactly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Table,
    Log,
}

/// A key that moves the page's list: the table if the page has one that scrolls, else the log.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Palettes ▸ the editor: a palette being made, as typed. Nothing here is checked until it is
/// drawn — [`Draft::problems`] runs the player's own rules over exactly the text Save would write.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Draft {
    /// The editor is showing, in place of the table.
    pub open: bool,
    pub name: String,
    /// In `palette::KEYS` order. The last six count only with `own_accent`.
    pub hex: [String; 18],
    pub own_accent: bool,
    /// The starting point last chosen: 0 is Cinder, then `palette::EXAMPLES`.
    pub start: usize,
    /// The file this draft was last saved as, which Save may replace. No other file is replaced.
    pub saved: Option<String>,
    /// A colour has been typed since the last starting point was chosen.
    pub edited: bool,
    /// A starting point clicked once over edited colours: a second click replaces them.
    pub confirm_start: Option<usize>,
}

impl Draft {
    /// Starting points, as the editor names them.
    pub const STARTS: [&'static str; 4] = ["Cinder", "Slate", "Paper", "Sony"];

    /// A fresh draft from starting point `start`, with its colours and no name.
    pub fn from_start(start: usize) -> Draft {
        use flint_core::palette::{parse, values, CINDER, EXAMPLES};
        let tokens = start
            .checked_sub(1)
            .and_then(|i| EXAMPLES.get(i))
            .and_then(|(id, body)| parse(id, body).ok())
            .map_or(CINDER, |p| p.tokens);
        let v = values(&tokens);
        Draft {
            open: true,
            hex: std::array::from_fn(|i| format!("#{:06x}", v[i])),
            own_accent: tokens.accent.is_some(),
            start: start.min(Draft::STARTS.len() - 1),
            ..Draft::default()
        }
    }

    /// The file it saves as: `my-palette.palette`. Empty id when the name has nothing usable.
    pub fn file(&self) -> String {
        format!("{}.{}", flint_core::palette::id_for(&self.name), flint_core::palette::EXTENSION)
    }

    /// What Save writes.
    pub fn body(&self) -> String {
        let n = if self.own_accent { 18 } else { 12 };
        let pairs: Vec<(&str, &str)> =
            flint_core::palette::KEYS.iter().zip(self.hex.iter()).take(n).map(|(k, v)| (*k, v.as_str())).collect();
        flint_core::palette::file_text(self.name.trim(), &pairs)
    }

    /// Why the player would refuse it, in its own words. Empty when it would load.
    pub fn problems(&self) -> Vec<String> {
        let id = flint_core::palette::id_for(&self.name);
        if id.is_empty() {
            return vec!["Give it a name with at least one letter or digit in it.".into()];
        }
        flint_core::palette::parse(&id, &self.body()).err().unwrap_or_default()
    }

    /// One colour, if what is typed is one.
    pub fn colour(&self, i: usize) -> Option<u32> {
        let h = self.hex.get(i)?.trim();
        let h = h.strip_prefix('#').unwrap_or(h);
        (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten()
    }

    /// The colours as the player would draw them, for the preview. `None` while any is not a
    /// colour yet.
    pub fn tokens(&self) -> Option<flint_core::palette::Tokens> {
        use flint_core::palette::{AccentTokens, Neutrals, Tokens};
        let c: Vec<u32> = (0..18).map(|i| self.colour(i)).collect::<Option<_>>()?;
        let n = |o: usize| Neutrals {
            bg: c[o],
            panel: c[o + 1],
            line: c[o + 2],
            ink: c[o + 3],
            dim: c[o + 4],
            faint: c[o + 5],
        };
        let a = |o: usize| AccentTokens { acc: c[o], acc_ink: c[o + 1], row_sel: c[o + 2] };
        Some(Tokens { day: n(0), night: n(6), accent: self.own_accent.then(|| (a(12), a(15))) })
    }
}

/// Palettes ▸ the shared palettes: the repository's list, as last read, and what to look for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shop {
    /// The shop is showing, in place of the table.
    pub open: bool,
    /// Every shared palette, in the index's order. Empty until the list has been read.
    pub items: Vec<flint_core::palette::SharedPalette>,
    /// The list has been read at least once this session, even if it came back empty.
    pub read: bool,
    pub search: String,
    /// The one being installed, by index into `items`.
    pub installing: Option<usize>,
}

impl Shop {
    /// The items the search leaves, by index into `items`.
    pub fn shown(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| self.items[i].matches(&self.search)).collect()
    }
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
    /// The jobs running now, oldest first.
    pub running: Vec<Running>,
    /// One line per thing that happened, newest last. The pane shows the tail. This is the Sync
    /// page's log, and the log of every job without a page of its own.
    pub log: Vec<String>,
    /// The SensMe page's log: the analysis and the import.
    pub sensme_log: Vec<String>,
    /// The Likes & plays page's log: everything that talked to Last.fm.
    pub lastfm_log: Vec<String>,
    /// The last thing a finished job said. A page with a job of its own running shows that job's
    /// line instead.
    pub status: String,
    /// True once a plan has been made and nothing has changed since, which is what makes COPY
    /// legal: this window never copies anything the user has not been shown first.
    pub planned: bool,
    /// The shown plan's key, which Copy must still match.
    pub plan_key: Option<u64>,
    /// What the library holds, once a job has read it.
    pub source: Option<LibraryFacts>,
    /// What each destination holds and what the plan would add, in the same order as `volumes`.
    pub dest: [Option<VolumeFacts>; 2],
    /// The page on show.
    pub tab: Tab,
    /// Settings ▸ Theme. Kept in `gui.conf` with the folders.
    pub theme: ThemePref,
    /// How many FLACs the last check looked at; `None` before one has run this session.
    pub checked: Option<usize>,
    /// What the last check found, one row per flagged file.
    pub findings: Vec<CheckRow>,
    /// What "Read the player" found.
    pub player: PlayerFacts,
    /// Settings ▸ Last.fm.
    pub lastfm: Lastfm,
    /// What is typed into the key fields. Cleared once it is saved.
    pub key_input: String,
    pub secret_input: String,
    /// The field typing goes to, if any.
    pub focus: Option<Field>,
    /// Check ▸ the filter: text to look for, and one verdict (an index into `pages::VERDICTS`).
    pub check_filter: String,
    pub check_verdict: Option<usize>,
    /// The last "Compare likes", until something makes it stale.
    pub likes_plan: Option<LikesPlan>,
    /// Where the analysis cache lives, for Settings.
    pub cache_dir: String,
    /// What new SensMe analysis still needs on this PC. Both false until the window has looked.
    pub no_engine: bool,
    pub no_ffmpeg: bool,
    /// The folder of `.palette` files on this PC.
    pub palette_dir: Option<PathBuf>,
    /// The last palette check: the built-in palette first, then every file by name.
    pub palette_rows: Vec<flint_core::palette::Row>,
    /// Palettes ▸ the editor.
    pub draft: Draft,
    /// Palettes ▸ the shared palettes.
    pub shop: Shop,
    /// Each page's lists, by `Tab::index` then `Area`. A table's is the first row shown; a log's
    /// is how many lines up from the newest, so 0 follows the log as it grows.
    pub scroll: [[usize; 2]; 7],
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

    /// Is there enough here to plan a sync, and nothing in its way?
    pub fn can_plan(&self) -> bool {
        live(self, Id::Plan)
    }

    /// COPY is only ever offered for a plan the user has already seen. A change to any input
    /// clears that, so the button cannot carry over a plan made against different settings.
    pub fn can_apply(&self) -> bool {
        live(self, Id::Apply)
    }

    pub fn busy(&self) -> bool {
        !self.running.is_empty()
    }

    pub fn is_running(&self, job: Job) -> bool {
        self.running.iter().any(|r| r.job == job)
    }

    /// Is something running that holds `h`?
    pub fn held(&self, h: Hold) -> bool {
        self.running.iter().any(|r| r.job.holds().contains(&h))
    }

    /// The running job in `job`'s way, if any: the one holding something it needs.
    pub fn blocker(&self, job: Job) -> Option<Job> {
        self.running.iter().map(|r| r.job).find(|r| r.holds().iter().any(|h| job.holds().contains(h)))
    }

    pub fn can_start(&self, job: Job) -> bool {
        self.blocker(job).is_none()
    }

    /// The music folder cannot change while something is reading it.
    pub fn library_locked(&self) -> bool {
        self.running.iter().any(|r| r.job.reads_library())
    }

    /// Nor the player's drives.
    pub fn volumes_locked(&self) -> bool {
        self.running.iter().any(|r| r.job.reads_volumes())
    }

    /// Nor the playlists and the two switches, while a plan or a copy is using them.
    pub fn sync_running(&self) -> bool {
        self.is_running(Job::Plan) || self.is_running(Job::Apply)
    }

    /// The job whose bar, line and Stop the footer of `tab` shows: the newest of that page's own.
    pub fn footer_job(&self, tab: Tab) -> Option<&Running> {
        self.running.iter().rev().find(|r| r.job.pages().contains(&tab))
    }

    /// The job a Stop on `tab` stops. Settings has no footer; its Stop is "Stop waiting" beside
    /// the Last.fm sign-in.
    pub fn stop_target(&self, tab: Tab) -> Option<Job> {
        match tab {
            Tab::Settings => self.is_running(Job::LastfmSignIn).then_some(Job::LastfmSignIn),
            t => self.footer_job(t).map(|r| r.job),
        }
    }

    /// The line at the bottom of `tab`: its own job's latest word; else what a job running
    /// elsewhere is doing, so a page never looks idle while the machine is not; else the last
    /// thing any job said.
    pub fn status_line(&self, tab: Tab) -> String {
        if let Some(r) = self.footer_job(tab) {
            return if r.status.is_empty() { format!("{}…", cap(r.job.doing())) } else { r.status.clone() };
        }
        if let Some(r) = self.running.last() {
            let more =
                if self.running.len() > 1 { format!(" (and {} more)", self.running.len() - 1) } else { String::new() };
            return if r.status.is_empty() {
                format!("{}{more}…", cap(r.job.doing()))
            } else {
                format!("{}{more}: {}", cap(r.job.doing()), r.status)
            };
        }
        self.status.clone()
    }

    /// Why `job`'s button is grey when its inputs are all there: something else holds what it
    /// needs. `None` when it is free, or running itself.
    pub fn waits_for(&self, job: Job) -> Option<String> {
        let b = self.blocker(job)?;
        (b != job).then(|| format!("Available when {} finishes.", b.doing()))
    }

    pub fn field(&self, f: Field) -> &str {
        match f {
            Field::ApiKey => &self.key_input,
            Field::ApiSecret => &self.secret_input,
            Field::CheckFilter => &self.check_filter,
            Field::PaletteName => &self.draft.name,
            Field::Colour(i) => self.draft.hex.get(i as usize).map_or("", String::as_str),
            Field::ShopSearch => &self.shop.search,
        }
    }

    fn field_mut(&mut self, f: Field) -> &mut String {
        match f {
            Field::ApiKey => &mut self.key_input,
            Field::ApiSecret => &mut self.secret_input,
            Field::CheckFilter => &mut self.check_filter,
            Field::PaletteName => &mut self.draft.name,
            Field::Colour(i) => &mut self.draft.hex[(i as usize).min(17)],
            Field::ShopSearch => &mut self.shop.search,
        }
    }

    /// Something was typed into `f`: a filter starts its table at the top again, so no match is
    /// left above the rows on show; a colour marks the draft as someone's work.
    fn typed_into(&mut self, f: Field) {
        match f {
            Field::CheckFilter => self.scroll[Tab::Check.index()][Area::Table as usize] = 0,
            Field::ShopSearch => self.scroll[Tab::Palettes.index()][Area::Table as usize] = 0,
            Field::Colour(_) => {
                self.draft.edited = true;
                self.draft.confirm_start = None;
            }
            _ => {}
        }
    }

    /// Where `a` on the page on show is scrolled to, as stored (see [`Model::scroll`]).
    pub fn scrolled(&self, a: Area) -> usize {
        self.scroll[self.tab.index()][a as usize]
    }

    /// The flagged files the Check page's table shows: the verdict chosen, if one is, and the
    /// filter text anywhere in the verdict, the path or the reason, ignoring case.
    pub fn check_rows(&self) -> Vec<&CheckRow> {
        let needle = self.check_filter.trim().to_lowercase();
        let verdict = self.check_verdict.and_then(|i| pages::VERDICTS.get(i)).map(|v| v.0);
        self.findings
            .iter()
            .filter(|r| verdict.is_none_or(|v| r.verdict == v))
            .filter(|r| {
                needle.is_empty()
                    || r.file.to_lowercase().contains(&needle)
                    || r.why.to_lowercase().contains(&needle)
                    || r.verdict.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// The page that shows `p`'s log.
    fn pane_tab(p: Pane) -> Tab {
        match p {
            Pane::Sync => Tab::Sync,
            Pane::SensMe => Tab::SensMe,
            Pane::Lastfm => Tab::Likes,
        }
    }

    /// A line was added to `p`. A log scrolled up stays on the lines being read; one at the bottom
    /// follows the new line.
    fn log_grew(&mut self, p: Pane) {
        let off = &mut self.scroll[Model::pane_tab(p).index()][Area::Log as usize];
        if *off > 0 {
            *off += 1;
        }
    }

    /// Where a job's lines go.
    pub fn pane_mut(&mut self, p: Pane) -> &mut Vec<String> {
        match p {
            Pane::Sync => &mut self.log,
            Pane::SensMe => &mut self.sensme_log,
            Pane::Lastfm => &mut self.lastfm_log,
        }
    }

    /// The one control the window is asking for next, or `Id::None` while the sync cannot go
    /// ahead. It is the ONLY thing drawn in the accent, so the window always has exactly one place
    /// the eye is meant to land: a folder, then a volume, then the plan, then the copy. A tool
    /// that is perfectly pressable is not the next step — `Import Music Center` is available from
    /// the first frame and is still not what the window is asking for.
    pub fn next_step(&self) -> Id {
        if !self.can_start(Job::Plan) {
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
    /// A palette's colours as the player draws them by day — bg, line, dim, ink, accent.
    Swatch([u32; 5]),
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
    /// A page tab in the band. `count` is a small number after the name, or empty.
    Tab {
        active: bool,
        count: String,
    },
    /// One segment of a segmented control (Settings ▸ Theme).
    Segment {
        on: bool,
    },
    /// A count with a word under it — a check verdict, a total. `tone` colours the number.
    Stat {
        tone: Tone,
        caption: String,
    },
    /// One cell of a table row.
    Cell {
        tone: Tone,
        strong: bool,
        mono: bool,
    },
    /// A line of text that takes typing. The widget's text is what is in it.
    Field {
        focused: bool,
        masked: bool,
        enabled: bool,
        placeholder: &'static str,
    },
    /// A card that is also a choice — Check's verdict counts, which filter the table. `on` is the
    /// one chosen.
    Pick {
        on: bool,
    },
    /// The bar beside a list with more rows than fit. The widget's rect is the track; `list` is the
    /// list it scrolls, where the wheel works. Rows, not pixels: a list only ever shows whole rows,
    /// so nothing is ever drawn half inside it.
    Scrollbar {
        area: Area,
        first: usize,
        visible: usize,
        total: usize,
        list: Rect,
    },
}

/// What a number or a word is saying, which decides its colour. Never the accent: the accent on
/// this window means bytes about to be written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Plain,
    Dim,
    /// Something is wrong with this file (LOSSY, DAMAGED).
    Warn,
    /// Worth a look, not necessarily wrong (SUSPECT, UPSAMPLED, PADDED, not put there by Flint).
    Caution,
    Ok,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Widget {
    pub id: Id,
    pub rect: Rect,
    pub kind: Kind,
    pub text: String,
}

pub(crate) const PAD: i32 = 22;
pub(crate) const BTN_H: i32 = 30;
pub(crate) const BAND_H: i32 = 64;
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

/// The band: the wordmark, the player it is working with, and the page tabs, right-aligned.
fn band(m: &Model, w: i32, out: &mut Vec<Widget>) {
    out.push(Widget { id: Id::None, rect: Rect::new(0, 0, w, BAND_H), kind: Kind::Band, text: String::new() });
    out.push(Widget { id: Id::None, rect: Rect::new(PAD, 10, 120, 26), kind: Kind::Title, text: "Flint".into() });
    let tabs = tab_rects(w);
    let first_tab = tabs.first().map_or(w, |(_, r)| r.x);
    out.push(Widget {
        id: Id::None,
        rect: Rect::new(PAD, 36, (first_tab - PAD - 12).max(40), 18),
        kind: Kind::Subtitle,
        text: device_line(m),
    });
    for (tab, rect) in tabs {
        out.push(Widget {
            id: Id::Tab(tab),
            rect,
            kind: Kind::Tab { active: m.tab == tab, count: tab_count(m, tab) },
            text: tab.label().into(),
        });
    }
}

/// What the band says about the player: its drives, or that none is chosen yet.
fn device_line(m: &Model) -> String {
    let drives: Vec<String> = m.volumes.iter().flatten().map(|p| p.display().to_string()).collect();
    match drives.len() {
        0 => "No player chosen".into(),
        1 => format!("Player · {}", drives[0]),
        _ => format!("Player · {} and {}", drives[0], drives[1]),
    }
}

/// The small number after a tab's name, when there is one worth showing.
fn tab_count(m: &Model, tab: Tab) -> String {
    match tab {
        Tab::Check if !m.findings.is_empty() => thousands(m.findings.len()),
        Tab::Player if m.player.read => thousands(m.player.albums.len()),
        Tab::Likes if m.player.read => thousands(m.player.plays.len()),
        Tab::Palettes if m.palette_rows.len() > 1 => thousands(m.palette_rows.len() - 1),
        Tab::Palettes if m.player.read && !m.player.palettes.is_empty() => thousands(m.player.palettes.len()),
        _ => String::new(),
    }
}

/// Tab geometry: 34 px tall, sized to the label, right-aligned in the band with 4 px between. Pure
/// arithmetic on the label length — `paint::Face::advance` — so the hit test needs no fonts.
pub fn tab_rects(w: i32) -> Vec<(Tab, Rect)> {
    const GAP: i32 = 4;
    // The label and 20 px of air; the tabs that can carry a count get room for three digits too,
    // always, so a count arriving never moves a tab.
    let counted = |t: Tab| matches!(t, Tab::Player | Tab::Check | Tab::Likes | Tab::Palettes);
    let width = |t: Tab| {
        (t.label().chars().count() as f32 * paint::Face::Strong.advance()) as i32 + 20 + if counted(t) { 20 } else { 0 }
    };
    let total: i32 = Tab::ALL.iter().map(|t| width(*t)).sum::<i32>() + GAP * (Tab::ALL.len() as i32 - 1);
    let mut x = w.max(MIN_W) - PAD - total;
    Tab::ALL
        .iter()
        .map(|&t| {
            let r = Rect::new(x, 15, width(t), 34);
            x += r.w + GAP;
            (t, r)
        })
        .collect()
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
    let next = m.next_step();
    let on = |id: Id| live(m, id);

    // ── the band ───────────────────────────────────────────────────────────────────────────
    band(m, w, &mut out);
    if m.tab != Tab::Sync {
        pages::layout(m, w, h, &mut out);
        return out;
    }

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
        kind: Kind::Button { primary: next == Id::PickLibrary, enabled: on(Id::PickLibrary) },
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
        kind: Kind::Tool { enabled: on(Id::PickPlaylists) },
        text: "Choose…".into(),
    });
    if m.playlists.is_some() {
        px -= 62 + 8;
        out.push(Widget {
            id: Id::ClearPlaylists,
            rect: Rect::new(px, y, 62, 26),
            kind: Kind::Tool { enabled: on(Id::ClearPlaylists) },
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
                kind: Kind::Button { primary: false, enabled: on(Id::ClearVolume(i)) },
                text: "Clear".into(),
            });
        }
        out.push(Widget {
            id: Id::PickVolume(i),
            rect: Rect::new(w - PAD - CARD_PAD - CHOOSE_W, y + 8, CHOOSE_W, BTN_H),
            kind: Kind::Button { primary: next == Id::PickVolume(i), enabled: on(Id::PickVolume(i)) },
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
        kind: Kind::Check { on: m.sensme, enabled: on(Id::ToggleSensMe) },
        text: "Write SensMe tags into the copies".into(),
    });
    out.push(Widget {
        id: Id::ToggleExtras,
        rect: Rect::new(PAD + 320, y, 280, 22),
        kind: Kind::Check { on: m.extras, enabled: on(Id::ToggleExtras) },
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
    let sync_job = m.footer_job(Tab::Sync);
    if sync_job.is_some() {
        out.push(Widget {
            id: Id::Stop,
            rect: Rect::new(w - PAD - STOP_W, y, STOP_W, act_h),
            kind: Kind::Button { primary: false, enabled: true },
            text: "Stop".into(),
        });
    } else if let Some(why) = m.waits_for(Job::Plan) {
        // Grey with every input in place is a question; this is the answer. An analysis running
        // for an hour holds the cache the copy tags from, and the window should say so rather
        // than look broken.
        out.push(hint(why, Rect::new(x + 4, y + 9, (w - PAD - x - 4).max(40), 18)));
    }
    y += act_h + 10;

    // The library tools (analyse, import Music Center's work, check the FLACs) moved to the
    // SensMe and Check pages, which are about them. What is left here is the sync.
    y -= 10;

    // ── progress, status, log ──────────────────────────────────────────────────────────────
    //
    // The bar exists only while a sync job does. An empty trough at rest is a control that is not
    // controlling anything, and the space it was holding goes to the log, which is the part of
    // this window people actually read.
    y += 16;
    let status_y = if let Some(r) = sync_job {
        out.push(Widget {
            id: Id::None,
            rect: Rect::new(PAD, y, inner, 8),
            kind: Kind::Progress(r.progress),
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
        text: m.status_line(Tab::Sync),
    });

    let log_y = status_y + 28;
    let log_h = (h - log_y - PAD).max(0);
    // An empty pane is a place to say what happens next, not a black rectangle waiting to be
    // filled: `log_pane` draws it as the outline the cards use.
    pages::log_pane(
        &mut out,
        &m.log,
        "Every file Flint would copy, remove or tag is listed here first. Nothing is written until you \
         press Copy to the player.",
        (PAD, log_y, inner, log_h),
        m.scroll[Tab::Sync.index()][Area::Log as usize],
    );
    out
}

/// Which control is under `(x, y)`, if any. Disabled controls do not answer: a button that is drawn
/// grey and still acts is worse than one that is not there.
pub fn hit(m: &Model, w: i32, h: i32, x: i32, y: i32) -> Option<Id> {
    layout(m, w, h).into_iter().rev().find_map(|wid| {
        let live = match wid.kind {
            // `Tool` was missing from this list until 0.2, so every outlined button — Analyse
            // library, Import Music Center, Check FLACs and the playlist folder's Choose… and
            // Clear — was drawn, lit up under the pointer's hand cursor… and ignored the click.
            Kind::Button { enabled, .. }
            | Kind::Check { enabled, .. }
            | Kind::Tool { enabled }
            | Kind::Field { enabled, .. } => enabled,
            // A tab is always live: looking at another page never starts or stops anything.
            Kind::Tab { .. } | Kind::Segment { .. } | Kind::Pick { .. } => true,
            _ => false,
        };
        (live && wid.id != Id::None && wid.rect.contains(x, y)).then_some(wid.id)
    })
}

// ── Scrolling ──────────────────────────────────────────────────────────────────────────────────

/// The thumb in a scrollbar's track: as tall as the share of rows on show, never under 24 px.
pub fn thumb(track: Rect, first: usize, visible: usize, total: usize) -> Rect {
    let (h, total) = (track.h as i64, total.max(1) as i64);
    let th = (h * visible as i64 / total).clamp(24.min(h), h);
    let span = (total - visible as i64).max(1);
    let ty = ((h - th) * first as i64 / span).clamp(0, h - th);
    Rect::new(track.x, track.y + ty as i32, track.w, th as i32)
}

/// A scrollbar the layout drew: `(area, track, list, first, visible, total)`.
type Bar = (Area, Rect, Rect, usize, usize, usize);

fn bars(m: &Model, w: i32, h: i32) -> Vec<Bar> {
    layout(m, w, h)
        .into_iter()
        .filter_map(|wid| match wid.kind {
            Kind::Scrollbar { area, first, visible, total, list } => {
                Some((area, wid.rect, list, first, visible, total))
            }
            _ => None,
        })
        .collect()
}

/// Show row `first` of `area` at the top, clamped. True when that moved anything.
fn show_from(m: &mut Model, bar: &Bar, first: i64) -> bool {
    let (area, _, _, _, visible, total) = *bar;
    let max = total.saturating_sub(visible);
    let first = first.clamp(0, max as i64) as usize;
    let stored = match area {
        Area::Table => first,
        Area::Log => max - first,
    };
    let slot = &mut m.scroll[m.tab.index()][area as usize];
    std::mem::replace(slot, stored) != stored
}

/// The mouse wheel at `(x, y)`: `rows` down (negative is up), in whichever list is under it.
pub fn wheel(m: &mut Model, w: i32, h: i32, x: i32, y: i32, rows: i32) -> bool {
    let Some(b) = bars(m, w, h).into_iter().find(|b| b.2.contains(x, y) || b.1.contains(x, y)) else {
        return false;
    };
    show_from(m, &b, b.3 as i64 + rows as i64)
}

/// A key that moves the page's main list: its table if that scrolls, else its log.
pub fn nav(m: &mut Model, w: i32, h: i32, k: Nav) -> bool {
    let bs = bars(m, w, h);
    let Some(b) = bs.iter().find(|b| b.0 == Area::Table).or(bs.first()).copied() else { return false };
    let (first, page) = (b.3 as i64, (b.4 as i64 - 1).max(1));
    let to = match k {
        Nav::Up => first - 1,
        Nav::Down => first + 1,
        Nav::PageUp => first - page,
        Nav::PageDown => first + page,
        Nav::Home => 0,
        Nav::End => i64::MAX / 2,
    };
    show_from(m, &b, to)
}

/// A press on a scrollbar at `(x, y)`. On the thumb, it starts a drag and returns the area and
/// where on the thumb it was caught; on the track, it pages toward the press and returns `None`.
pub fn press_bar(m: &mut Model, w: i32, h: i32, x: i32, y: i32) -> Option<Option<(Area, i32)>> {
    let b = bars(m, w, h).into_iter().find(|b| b.1.contains(x, y))?;
    let t = thumb(b.1, b.3, b.4, b.5);
    if t.contains(x, y) {
        return Some(Some((b.0, y - t.y)));
    }
    let page = (b.4 as i64 - 1).max(1);
    show_from(m, &b, b.3 as i64 + if y < t.y { -page } else { page });
    Some(None)
}

/// A thumb caught `grab` px below its top, dragged to `y`.
pub fn drag_bar(m: &mut Model, w: i32, h: i32, area: Area, grab: i32, y: i32) -> bool {
    let Some(b) = bars(m, w, h).into_iter().find(|b| b.0 == area) else { return false };
    let t = thumb(b.1, b.3, b.4, b.5);
    let range = (b.1.h - t.h).max(1) as i64;
    let max = b.5.saturating_sub(b.4) as i64;
    let top = (y - grab - b.1.y) as i64;
    show_from(m, &b, (top.clamp(0, range) * max + range / 2) / range)
}

/// The job a control starts, if it starts one.
pub fn job_of(id: Id) -> Option<Job> {
    Some(match id {
        Id::Plan => Job::Plan,
        Id::Apply => Job::Apply,
        Id::Scan => Job::Scan,
        Id::Import => Job::Import,
        Id::Check => Job::Check,
        Id::ReadPlayer => Job::ReadPlayer,
        Id::PullPlaylists => Job::PullPlaylists,
        Id::CheckPalettes => Job::CheckPalettes,
        Id::SendPalettes => Job::SendPalettes,
        Id::LastfmSaveKey => Job::LastfmKey,
        Id::LastfmSignIn => Job::LastfmSignIn,
        Id::LastfmSignOut => Job::LastfmSignOut,
        Id::Scrobble => Job::Scrobble,
        Id::CompareLikes => Job::CompareLikes,
        Id::SyncLikes => Job::SyncLikes,
        Id::SavePalette => Job::SavePalette,
        Id::FetchPalettes => Job::FetchPalettes,
        Id::FetchShop => Job::FetchShop,
        Id::InstallShared(_) => Job::InstallShared,
        _ => return None,
    })
}

/// Everything `job` reads is there — whether or not something else is in its way.
fn ready(m: &Model, job: Job) -> bool {
    let volume = m.volumes.iter().any(Option::is_some);
    match job {
        Job::Plan => m.library.is_some() && volume,
        Job::Apply => m.library.is_some() && volume && m.planned,
        Job::Scan | Job::Check => m.library.is_some(),
        Job::Import => true,
        Job::ReadPlayer => volume,
        // Somewhere to put them, and something to take: a read that found a changed playlist.
        Job::PullPlaylists => {
            m.volumes[0].is_some() && m.playlists.is_some() && m.player.playlists.iter().any(|p| p.edited)
        }
        Job::CheckPalettes => m.palette_dir.is_some() || m.volumes[0].is_some(),
        Job::SendPalettes => m.volumes[0].is_some() && !flint_core::palette::to_send(&m.palette_rows).is_empty(),
        Job::LastfmKey => !m.key_input.trim().is_empty() && !m.secret_input.trim().is_empty(),
        Job::LastfmSignIn => m.lastfm.has_key,
        Job::LastfmSignOut => m.lastfm.user.is_some(),
        // What Send sends is the table the page shows, so the player has to have been read.
        Job::Scrobble => {
            m.lastfm.signed_in() && volume && m.player.read && m.player.plays.iter().any(|p| p.kind == "PLAY")
        }
        Job::CompareLikes => m.lastfm.signed_in() && volume,
        Job::SyncLikes => m.lastfm.signed_in() && volume && m.likes_plan.is_some_and(|p| p.changes() > 0),
        Job::SavePalette => m.draft.open && m.palette_dir.is_some() && m.draft.problems().is_empty(),
        Job::FetchPalettes => m.palette_dir.is_some(),
        Job::FetchShop => true,
        Job::InstallShared => m.palette_dir.is_some(),
    }
}

/// Would a click on `id` do anything now? The layout draws a control live exactly when this says
/// so and `click` acts exactly when it says so — one answer for both, so a button cannot look
/// pressable and ignore the press, or the other way round.
pub fn live(m: &Model, id: Id) -> bool {
    match id {
        Id::None => false,
        Id::Tab(_)
        | Id::Theme(_)
        | Id::Stop
        | Id::Verdict(_)
        | Id::ClearFilter
        | Id::LastfmGetKey
        | Id::GetMusicCenter
        | Id::GetFfmpeg => true,
        Id::PickLibrary => !m.library_locked(),
        Id::PickVolume(_) | Id::ClearVolume(_) => !m.volumes_locked(),
        Id::PickPlaylists | Id::ClearPlaylists | Id::ToggleSensMe | Id::ToggleExtras => !m.sync_running(),
        Id::PickPalettes => !m.held(Hold::Palettes),
        Id::Field(Field::CheckFilter) | Id::BrowsePalettes => true,
        Id::NewPalette => !m.draft.open,
        Id::Field(Field::PaletteName | Field::Colour(_))
        | Id::PaletteStart(_)
        | Id::ToggleOwnAccent
        | Id::ClosePalette => m.draft.open,
        Id::SharePalette => m.draft.open && m.draft.problems().is_empty(),
        Id::OpenShop => !m.shop.open,
        Id::CloseShop | Id::Field(Field::ShopSearch) => m.shop.open,
        Id::InstallShared(i) => {
            m.shop.open
                && m.shop.items.get(i).is_some_and(|p| installable(m, p))
                && ready(m, Job::InstallShared)
                && m.can_start(Job::InstallShared)
        }
        Id::Field(_) | Id::LastfmChangeKey | Id::LastfmKeepKey => !m.held(Hold::Lastfm),
        other => job_of(other).is_some_and(|job| ready(m, job) && m.can_start(job)),
    }
}

/// Would Install do anything for `p`? It never replaces a file of the same name that is someone's
/// own, in the folder or on the player, and there is nothing to do once it is in the folder and — when the player's internal
/// memory is chosen — on the player too.
pub fn installable(m: &Model, p: &flint_core::palette::SharedPalette) -> bool {
    use flint_core::palette::Have;
    let player_done = m.volumes[0].is_none() || p.player == Have::Same;
    p.loads() && p.folder != Have::Different && p.player != Have::Different && !(p.folder == Have::Same && player_done)
}

/// Apply a click to the model, and say which job (if any) the platform layer should start.
///
/// The file pickers are the platform's business — this returns the `Id` for those and the caller
/// opens a dialog, because a folder chooser is the one thing that cannot be pure.
pub fn click(m: &mut Model, id: Id) -> Option<Job> {
    if !live(m, id) {
        return None;
    }
    if !matches!(id, Id::Field(_)) {
        m.focus = None;
    }
    match id {
        Id::ToggleSensMe => {
            m.sensme = !m.sensme;
            m.invalidate_plan();
        }
        Id::ToggleExtras => {
            m.extras = !m.extras;
            m.invalidate_plan();
        }
        Id::ClearVolume(i) => {
            m.volumes[i] = None;
            m.invalidate_plan();
            m.likes_plan = None;
        }
        Id::ClearPlaylists => {
            m.playlists = None;
            m.invalidate_plan();
        }
        Id::Tab(t) => m.tab = t,
        Id::Theme(p) => m.theme = p,
        Id::Field(f) => m.focus = Some(f),
        Id::Verdict(i) => {
            m.check_verdict = if m.check_verdict == Some(i) { None } else { Some(i) };
            m.scroll[Tab::Check.index()][Area::Table as usize] = 0;
        }
        Id::ClearFilter => {
            m.check_verdict = None;
            m.check_filter.clear();
            m.scroll[Tab::Check.index()][Area::Table as usize] = 0;
        }
        Id::LastfmChangeKey => {
            m.lastfm.editing = true;
            m.focus = Some(Field::ApiKey);
        }
        Id::LastfmKeepKey => {
            m.lastfm.editing = false;
            m.key_input.clear();
            m.secret_input.clear();
        }
        // The shop reads its list the first time it opens; Refresh reads it again.
        Id::OpenShop => {
            m.shop.open = true;
            m.draft.open = false;
            m.scroll[Tab::Palettes.index()][Area::Table as usize] = 0;
            m.focus = Some(Field::ShopSearch);
            if !m.shop.read && live(m, Id::FetchShop) {
                return Some(Job::FetchShop);
            }
        }
        Id::CloseShop => {
            m.shop.open = false;
            m.scroll[Tab::Palettes.index()][Area::Table as usize] = 0;
        }
        Id::InstallShared(i) => {
            m.shop.installing = Some(i);
            return Some(Job::InstallShared);
        }
        // The draft closed last is still there: New palette opens it again. A fresh one starts
        // from Cinder.
        Id::NewPalette => {
            m.shop.open = false;
            if m.draft.hex[0].is_empty() {
                m.draft = Draft::from_start(0);
            }
            m.draft.open = true;
            m.draft.confirm_start = None;
            m.focus = Some(Field::PaletteName);
        }
        // A new starting point replaces the colours, and keeps the name and where it was saved.
        // Over colours someone has typed, it asks first: the first click says what it would do.
        Id::PaletteStart(i) => {
            if m.draft.edited && m.draft.confirm_start != Some(i) {
                m.draft.confirm_start = Some(i);
            } else {
                let d = Draft::from_start(i);
                m.draft = Draft { name: std::mem::take(&mut m.draft.name), saved: m.draft.saved.take(), ..d };
            }
        }
        Id::ToggleOwnAccent => {
            m.draft.own_accent = !m.draft.own_accent;
            m.draft.edited = true;
        }
        Id::ClosePalette => m.draft.open = false,
        other => return job_of(other),
    }
    None
}

/// A key pressed while a field has the focus. Returns a job when Enter means one: in the key
/// fields, Enter saves them.
pub fn key(m: &mut Model, k: Key) -> Option<Job> {
    let f = m.focus?;
    if !live(m, Id::Field(f)) {
        m.focus = None;
        return None;
    }
    match k {
        Key::Char(c) if !c.is_control() => {
            let text = m.field_mut(f);
            if text.chars().count() < f.max() {
                text.push(c);
            }
            m.typed_into(f);
        }
        Key::Char(_) => {}
        Key::Backspace => {
            m.field_mut(f).pop();
            m.typed_into(f);
        }
        Key::Clear => {
            m.field_mut(f).clear();
            m.typed_into(f);
        }
        Key::Tab => m.focus = Some(f.next(m.draft.own_accent)),
        Key::Escape => m.focus = None,
        Key::Enter => {
            if matches!(f, Field::ApiKey | Field::ApiSecret) {
                return click(m, Id::LastfmSaveKey);
            }
            m.focus = None;
        }
    }
    None
}

/// Text from the clipboard, into the focused field. One line: a key copied off a web page often
/// brings a newline or a space with it, and neither belongs in a key.
pub fn paste(m: &mut Model, text: &str) {
    let Some(f) = m.focus else { return };
    if !live(m, Id::Field(f)) {
        return;
    }
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line: String = if matches!(f, Field::ApiKey | Field::ApiSecret | Field::Colour(_)) {
        line.chars().filter(|c| !c.is_whitespace()).collect()
    } else {
        line.to_string()
    };
    let field = m.field_mut(f);
    let room = f.max().saturating_sub(field.chars().count());
    field.extend(line.chars().filter(|c| !c.is_control()).take(room));
    m.typed_into(f);
}

/// A page on the web a control opens in the browser, rather than a job.
pub fn url_for(m: &Model, id: Id) -> Option<String> {
    match id {
        Id::LastfmGetKey => Some(flint_core::lastfm::CREATE_KEY_URL.into()),
        Id::GetMusicCenter => Some(flint_core::engine::MUSIC_CENTER_URL.into()),
        Id::GetFfmpeg => Some(flint_core::engine::FFMPEG_URL.into()),
        Id::BrowsePalettes => Some(format!("{}/tree/main/palettes", flint_core::palette::SHARED_REPO)),
        Id::SharePalette if live(m, id) => Some(flint_core::palette::share_url(m.draft.name.trim(), &m.draft.body())),
        _ => None,
    }
}

/// `job` has been handed to a worker. Its pane is cleared for it and anything it makes stale is
/// forgotten.
pub fn started(m: &mut Model, job: Job) {
    m.running.retain(|r| r.job != job);
    m.running.push(Running { job, progress: None, status: String::new() });
    match job {
        // A sync's log is the plan it shows; a new one replaces it. Other jobs that log to the
        // Sync pane append, because that pane may be showing a plan someone is reading.
        Job::Plan | Job::Apply => m.log.clear(),
        Job::Scan | Job::Import => m.sensme_log.clear(),
        Job::Scrobble | Job::CompareLikes | Job::SyncLikes => m.lastfm_log.clear(),
        _ => {}
    }
    // A cleared log starts at its newest line again.
    if matches!(
        job,
        Job::Plan | Job::Apply | Job::Scan | Job::Import | Job::Scrobble | Job::CompareLikes | Job::SyncLikes
    ) {
        m.scroll[Model::pane_tab(job.pane()).index()][Area::Log as usize] = 0;
    }
    if job == Job::Check {
        m.scroll[Tab::Check.index()][Area::Table as usize] = 0;
    }
    // Anything that changes the analysis cache changes what a copy would tag, so the shown plan
    // is spent: a copy only ever carries out a plan that is still true. A copy that has been
    // carried out does not come back either — the window asks to be shown the new state of the
    // player rather than offering to copy the same plan a second time.
    if job.holds().contains(&Hold::Cache) {
        m.planned = false;
    }
    if job == Job::Check {
        m.findings.clear();
    }
    if matches!(job, Job::CompareLikes | Job::SyncLikes) {
        m.likes_plan = None;
    }
}

/// One thing a running job said.
pub fn update(m: &mut Model, job: Job, u: job::Update) {
    use job::Update;
    let pane = job.pane();
    match u {
        Update::Say(line) => {
            if let Some(r) = m.running.iter_mut().find(|r| r.job == job) {
                r.status = line.clone();
            }
            // A check's per-file progress is a status line, not a log: thousands of them would bury
            // whatever the Sync pane is showing.
            if job != Job::Check {
                m.pane_mut(pane).push(line);
                m.log_grew(pane);
            }
        }
        Update::Log(line) => {
            m.pane_mut(pane).push(line);
            m.log_grew(pane);
        }
        Update::Progress(p) => {
            if let Some(r) = m.running.iter_mut().find(|r| r.job == job) {
                r.progress = p;
            }
        }
        Update::Planned(key) => {
            m.planned = job == Job::Plan;
            m.plan_key = Some(key);
        }
        Update::Library(facts) => m.source = Some(facts),
        Update::Volume(i, facts) => {
            if let Some(slot) = m.dest.get_mut(i) {
                *slot = Some(facts);
            }
        }
        Update::Finding(row) => m.findings.push(row),
        Update::Checked(n) => m.checked = Some(n),
        // A table filled afresh starts at its top.
        Update::Player(facts) => {
            m.player = *facts;
            m.scroll[Tab::Player.index()][Area::Table as usize] = 0;
            m.scroll[Tab::Likes.index()][Area::Table as usize] = 0;
        }
        // Only the playlists changed; the album table stays where it was scrolled to.
        Update::Playlists(rows) => m.player.playlists = rows,
        // Not under the shop's table: an install checks the folder, and the list stays put.
        Update::Palettes(rows) => {
            m.palette_rows = rows;
            if !m.shop.open {
                m.scroll[Tab::Palettes.index()][Area::Table as usize] = 0;
            }
        }
        // Saved: the table comes back with the new row in it. New palette opens the draft again.
        Update::PaletteSaved(file) => {
            m.draft.saved = Some(file);
            m.draft.open = false;
        }
        Update::Shop(items) => {
            m.shop.items = items;
            m.shop.read = true;
            m.scroll[Tab::Palettes.index()][Area::Table as usize] = 0;
        }
        Update::Installed { file, on_player } => {
            use flint_core::palette::Have;
            if let Some(p) = m.shop.items.iter_mut().find(|p| p.file == file) {
                p.folder = Have::Same;
                if on_player {
                    p.player = Have::Same;
                }
            }
        }
        Update::Lastfm(facts) => {
            m.lastfm = facts;
            if !m.lastfm.editing {
                m.key_input.clear();
                m.secret_input.clear();
            }
        }
        Update::Plays { plays, unreadable } => {
            m.player.plays = plays;
            m.player.unreadable = unreadable;
        }
        Update::Likes(plan) => m.likes_plan = plan,
        // The platform opens it; there is nothing to keep.
        Update::Open(_) => {}
    }
    // The log is unbounded otherwise: a library of 40,000 tracks would hold 40,000 strings for the
    // sake of the 9 lines the pane shows.
    const KEEP: usize = 2_000;
    let log = m.pane_mut(pane);
    if log.len() > KEEP * 2 {
        log.drain(..log.len() - KEEP);
    }
}

/// `job` has finished. Returns the reason when it failed, for the platform to show in a box.
pub fn finished(m: &mut Model, job: Job, result: Result<String, String>) -> Option<String> {
    m.running.retain(|r| r.job != job);
    if job == Job::InstallShared {
        m.shop.installing = None;
    }
    let (line, err) = match result {
        Ok(word) => (word, None),
        Err(why) => (format!("stopped: {why}"), Some(why)),
    };
    m.status = line.clone();
    m.pane_mut(job.pane()).push(line);
    err
}

/// A folder the user picked, for `id`. Any of these invalidates a shown plan.
pub fn set_path(m: &mut Model, id: Id, path: PathBuf) {
    match id {
        Id::PickLibrary => m.library = Some(path),
        Id::PickVolume(i) => {
            m.volumes[i] = Some(path);
            m.likes_plan = None;
        }
        Id::PickPlaylists => m.playlists = Some(path),
        Id::PickPalettes => {
            // A new folder is a new comparison; the old rows describe the old one.
            m.palette_dir = Some(path);
            m.palette_rows.clear();
            return;
        }
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

    /// The cell texts of a table's first column on show, top to bottom.
    fn shown_files(m: &Model) -> Vec<String> {
        layout(m, W, H)
            .into_iter()
            .filter(|w| matches!(w.kind, Kind::Cell { .. }) && w.text.starts_with("file "))
            .map(|w| w.text)
            .collect()
    }

    fn bar(m: &Model) -> (Rect, Rect) {
        layout(m, W, H)
            .into_iter()
            .find_map(|w| match w.kind {
                Kind::Scrollbar { list, .. } => Some((w.rect, list)),
                _ => None,
            })
            .expect("a scrollbar")
    }

    /// A table longer than the window: every row can be reached by the wheel, the keys, the
    /// track and the thumb, and none of them scrolls past either end.
    #[test]
    fn a_long_table_scrolls_every_way_and_stops_at_its_ends() {
        let mut m = ready();
        m.tab = Tab::Check;
        m.checked = Some(500);
        m.findings = (0..100)
            .map(|i| CheckRow { verdict: "LOSSY".into(), file: format!("file {i:03}"), why: "cut at 16 kHz".into() })
            .collect();
        let first = shown_files(&m);
        let visible = first.len();
        assert!(visible > 5 && visible < 100, "{visible}");
        assert_eq!(first[0], "file 000");
        let (track, list) = bar(&m);
        let (lx, ly) = (list.x + 40, list.y + 20);

        assert!(wheel(&mut m, W, H, lx, ly, 3));
        assert_eq!(shown_files(&m)[0], "file 003");
        assert!(wheel(&mut m, W, H, lx, ly, -50));
        assert_eq!(shown_files(&m)[0], "file 000");
        assert!(!wheel(&mut m, W, H, lx, ly, -1), "nothing above the first row");
        assert!(!wheel(&mut m, W, H, 5, 5, 3), "the wheel over no list does nothing");

        assert!(nav(&mut m, W, H, Nav::End));
        assert_eq!(shown_files(&m).last().unwrap(), "file 099");
        assert_eq!(shown_files(&m).len(), visible);
        assert!(!nav(&mut m, W, H, Nav::Down), "nothing below the last row");
        nav(&mut m, W, H, Nav::PageUp);
        assert_eq!(shown_files(&m).last().unwrap(), &format!("file {:03}", 99 - (visible - 1)));
        nav(&mut m, W, H, Nav::Home);

        // The track below the thumb pages down; the thumb drags to the end.
        assert_eq!(press_bar(&mut m, W, H, track.x + 2, track.bottom() - 2), Some(None));
        assert_eq!(shown_files(&m)[0], format!("file {:03}", visible - 1));
        nav(&mut m, W, H, Nav::Home);
        let t = thumb(track, 0, visible, 100);
        let Some(Some((area, grab))) = press_bar(&mut m, W, H, t.x + 2, t.y + 5) else { panic!("the thumb") };
        assert_eq!((area, grab), (Area::Table, 5));
        drag_bar(&mut m, W, H, area, grab, track.bottom() + 200);
        assert_eq!(shown_files(&m).last().unwrap(), "file 099");
        drag_bar(&mut m, W, H, area, grab, track.y - 200);
        assert_eq!(shown_files(&m)[0], "file 000");

        // A filter that leaves fewer rows than fit shows them all, and no bar.
        m.check_filter = "file 05".into();
        assert_eq!(shown_files(&m).len(), 10);
        assert!(!layout(&m, W, H).iter().any(|w| matches!(w.kind, Kind::Scrollbar { .. })));
    }

    /// A log follows its newest line; scrolled up, it stays on the lines being read while more
    /// arrive, and End brings it back to following.
    #[test]
    fn a_log_follows_its_tail_until_it_is_scrolled_up() {
        let mut m = ready();
        m.log = (0..300).map(|i| format!("line {i}")).collect();
        let last = |m: &Model| layout(m, W, H).into_iter().rfind(|w| w.kind == Kind::LogLine).unwrap().text;
        assert_eq!(last(&m), "line 299");
        let (_, list) = bar(&m);
        wheel(&mut m, W, H, list.x + 20, list.y + 20, -5);
        assert_eq!(last(&m), "line 294");
        started(&mut m, Job::ReadPlayer); // a job that logs to this pane without clearing it
        update(&mut m, Job::ReadPlayer, job::Update::Log("line 300".into()));
        assert_eq!(last(&m), "line 294", "the lines being read stay put");
        nav(&mut m, W, H, Nav::End);
        assert_eq!(last(&m), "line 300");
        update(&mut m, Job::ReadPlayer, job::Update::Log("line 301".into()));
        assert_eq!(last(&m), "line 301", "and at the bottom it follows again");
        wheel(&mut m, W, H, list.x + 20, list.y + 20, -1000);
        assert_eq!(layout(&m, W, H).into_iter().find(|w| w.kind == Kind::LogLine).unwrap().text, "line 0");
    }

    /// The palette editor, in every state that changes its shape, at every size: on the window,
    /// every live control hittable at its centre, no two overlapping.
    #[test]
    fn the_palette_editor_fits_at_every_size() {
        let mut drafts = Vec::new();
        for start in 0..Draft::STARTS.len() {
            let mut d = Draft::from_start(start);
            d.name = "Late Night".into();
            drafts.push(d.clone());
            d.hex[3] = d.hex[0].clone(); // text the colour of the background: refused, at length
            d.hex[9] = "#zz".into();
            drafts.push(d);
        }
        for d in drafts {
            for (w, h) in [(W, H), (MIN_W, MIN_H), (1400, 900)] {
                let mut m = ready();
                m.tab = Tab::Palettes;
                m.palette_dir = Some("/palettes".into());
                m.draft = d.clone();
                m.focus = Some(Field::Colour(2));
                let widgets = layout(&m, w, h);
                for wid in &widgets {
                    assert!(wid.rect.x >= 0 && wid.rect.y >= 0, "{:?} at {:?}", wid.kind, wid.rect);
                    assert!(
                        wid.rect.right() <= w && wid.rect.bottom() <= h,
                        "{w}x{h}: {:?} at {:?}",
                        wid.kind,
                        wid.rect
                    );
                }
                let live: Vec<&Widget> = widgets.iter().filter(|x| x.id != Id::None).collect();
                assert!(live.iter().any(|x| x.id == Id::Field(Field::Colour(11))));
                assert_eq!(live.iter().any(|x| x.id == Id::Field(Field::Colour(17))), d.own_accent);
                for (i, a) in live.iter().enumerate() {
                    let (cx, cy) = (a.rect.x + a.rect.w / 2, a.rect.y + a.rect.h / 2);
                    if !matches!(a.kind, Kind::Tool { enabled: false }) {
                        assert_eq!(hit(&m, w, h, cx, cy), Some(a.id), "{w}x{h}: {:?} at {:?}", a.id, a.rect);
                    }
                    for b in &live[i + 1..] {
                        let overlap = a.rect.x < b.rect.right()
                            && b.rect.x < a.rect.right()
                            && a.rect.y < b.rect.bottom()
                            && b.rect.y < a.rect.bottom();
                        assert!(!overlap, "{w}x{h}: {:?} overlaps {:?}", a.id, b.id);
                    }
                }
            }
        }
    }

    /// A shop of every kind of row: one to install, one already installed, one whose name is
    /// someone else's file, one the player would refuse, and enough of the first kind to scroll.
    fn shop_model() -> Model {
        use flint_core::palette::{Have, SharedPalette, EXAMPLES};
        let mut m = ready();
        m.tab = Tab::Palettes;
        m.palette_dir = Some("/palettes".into());
        let mut items: Vec<SharedPalette> =
            EXAMPLES.iter().map(|(id, body)| SharedPalette::new(&format!("{id}.palette"), body)).collect();
        items[1].folder = Have::Same;
        items[1].player = Have::Same;
        items[2].folder = Have::Different;
        items.push(SharedPalette::new("broken.palette", "name = Broken\nday.bg = #000000\nday.bg = #000000\n"));
        for i in 0..30 {
            let mut p = items[0].clone();
            p.file = format!("copy-{i}.palette");
            p.name = format!("Copy {i}");
            items.push(p);
        }
        m.shop = Shop { open: true, items, read: true, ..Shop::default() };
        m
    }

    #[test]
    fn the_shop_fits_at_every_size_and_every_button_is_where_it_is_drawn() {
        for (w, h) in [(W, H), (MIN_W, MIN_H), (1400, 900)] {
            let m = shop_model();
            let widgets = layout(&m, w, h);
            for wid in &widgets {
                assert!(wid.rect.x >= 0 && wid.rect.y >= 0, "{:?} at {:?}", wid.kind, wid.rect);
                assert!(wid.rect.right() <= w && wid.rect.bottom() <= h, "{w}x{h}: {:?} at {:?}", wid.kind, wid.rect);
            }
            let live_ids: Vec<&Widget> = widgets.iter().filter(|x| x.id != Id::None).collect();
            assert!(live_ids.iter().any(|x| x.id == Id::InstallShared(0)), "{w}x{h}: the first row installs");
            for id in [Id::InstallShared(1), Id::InstallShared(2), Id::InstallShared(3)] {
                assert!(!live(&m, id), "{id:?}: installed, someone's own, refused");
                assert!(!live_ids.iter().any(|x| x.id == id), "{w}x{h}: {id:?} has no button");
            }
            for (i, a) in live_ids.iter().enumerate() {
                let (cx, cy) = (a.rect.x + a.rect.w / 2, a.rect.y + a.rect.h / 2);
                if !matches!(a.kind, Kind::Tool { enabled: false }) {
                    assert_eq!(hit(&m, w, h, cx, cy), Some(a.id), "{w}x{h}: {:?} at {:?}", a.id, a.rect);
                }
                for b in &live_ids[i + 1..] {
                    assert!(
                        !(a.rect.x < b.rect.right()
                            && b.rect.x < a.rect.right()
                            && a.rect.y < b.rect.bottom()
                            && b.rect.y < a.rect.bottom()),
                        "{w}x{h}: {:?} overlaps {:?}",
                        a.id,
                        b.id
                    );
                }
            }
            assert!(widgets.iter().any(|x| matches!(x.kind, Kind::Scrollbar { .. })), "{w}x{h}: 34 rows scroll");
        }
    }

    #[test]
    fn the_shop_opens_searches_installs_and_never_replaces_anyones_file() {
        let mut m = shop_model();
        m.shop = Shop::default();
        // The first open reads the list; the next does not read it again.
        assert_eq!(click(&mut m, Id::OpenShop), Some(Job::FetchShop));
        assert_eq!(m.focus, Some(Field::ShopSearch));
        let items = shop_model().shop.items;
        update(&mut m, Job::FetchShop, job::Update::Shop(items));
        click(&mut m, Id::CloseShop);
        assert_eq!(click(&mut m, Id::OpenShop), None);

        // A search starts the list at the top and leaves only what matches.
        m.scroll[Tab::Palettes.index()][Area::Table as usize] = 9;
        paste(&mut m, "light");
        assert_eq!(m.scroll[Tab::Palettes.index()][Area::Table as usize], 0);
        let names: Vec<&str> = m.shop.shown().iter().map(|&i| m.shop.items[i].name.as_str()).collect();
        assert_eq!(names, ["Paper"]);
        let paper = m.shop.shown()[0];
        assert!(!live(&m, Id::InstallShared(paper)), "Paper's name is someone's own file here");
        key(&mut m, Key::Clear);
        assert_eq!(m.shop.shown().len(), m.shop.items.len());

        // Install: a job for that one, then it says where it is and offers nothing more.
        assert_eq!(click(&mut m, Id::InstallShared(0)), Some(Job::InstallShared));
        assert_eq!(m.shop.installing, Some(0));
        started(&mut m, Job::InstallShared);
        assert!(!live(&m, Id::InstallShared(4)), "one install at a time");
        let file = m.shop.items[0].file.clone();
        update(&mut m, Job::InstallShared, job::Update::Installed { file, on_player: true });
        finished(&mut m, Job::InstallShared, Ok("installed".into()));
        assert_eq!(m.shop.installing, None);
        assert!(!live(&m, Id::InstallShared(0)));
        assert!(live(&m, Id::InstallShared(4)));

        // Without the player's memory chosen, in the folder is done.
        m.volumes[0] = None;
        m.shop.items[4].folder = flint_core::palette::Have::Same;
        assert!(!live(&m, Id::InstallShared(4)));
        // A different file of that name on the player is someone's too.
        m.volumes[0] = Some("/player".into());
        m.shop.items[4].player = flint_core::palette::Have::Different;
        assert!(!live(&m, Id::InstallShared(4)));

        // New palette takes the page; the shop keeps its list for next time.
        click(&mut m, Id::NewPalette);
        assert!(!m.shop.open && m.draft.open && m.shop.read);
        click(&mut m, Id::ClosePalette);
        click(&mut m, Id::OpenShop);
        assert!(m.shop.open && !m.draft.open);
    }

    /// Typing a palette: New opens on Cinder's colours with the name focused, Tab walks the
    /// colours, a colour takes seven characters, a paste loses its spaces, and Save and Share
    /// wait for a palette the player would load.
    #[test]
    fn a_palette_is_typed_checked_and_saved() {
        let mut m = ready();
        m.tab = Tab::Palettes;
        assert!(!live(&m, Id::SavePalette));
        click(&mut m, Id::NewPalette);
        assert!(m.draft.open && !live(&m, Id::NewPalette));
        assert_eq!(m.focus, Some(Field::PaletteName));
        assert_eq!(m.draft.problems(), vec!["Give it a name with at least one letter or digit in it.".to_string()]);
        for c in "Late Night".chars() {
            key(&mut m, Key::Char(c));
        }
        assert!(m.draft.problems().is_empty(), "{:?}", m.draft.problems());
        assert!(live(&m, Id::SharePalette));
        assert!(!live(&m, Id::SavePalette), "no folder to save into");
        m.palette_dir = Some("/palettes".into());
        assert!(live(&m, Id::SavePalette));
        assert_eq!(m.draft.file(), "late-night.palette");

        key(&mut m, Key::Tab);
        assert_eq!(m.focus, Some(Field::Colour(0)));
        key(&mut m, Key::Clear);
        paste(&mut m, " #14 1820 99\n");
        assert_eq!(m.draft.hex[0], "#141820", "seven characters, no spaces");
        for _ in 0..11 {
            key(&mut m, Key::Tab);
        }
        assert_eq!(m.focus, Some(Field::Colour(11)));
        key(&mut m, Key::Tab);
        assert_eq!(m.focus, Some(Field::PaletteName), "without an accent of its own, Tab skips its keys");

        // Text the colour of the background: the player's reason, and nothing to save or share.
        m.draft.hex[3] = m.draft.hex[0].clone();
        assert!(!m.draft.problems().is_empty());
        assert!(!live(&m, Id::SavePalette) && !live(&m, Id::SharePalette));
        assert_eq!(url_for(&m, Id::SharePalette), None);

        // A new starting point keeps the name; Paper brings its accent. Colours were typed, so
        // it takes a second click.
        click(&mut m, Id::PaletteStart(2));
        click(&mut m, Id::PaletteStart(2));
        assert_eq!(m.draft.name, "Late Night");
        assert!(m.draft.own_accent && m.draft.problems().is_empty());
        let url = url_for(&m, Id::SharePalette).expect("a share link");
        assert!(url.contains("day.accent%20%3D%20%23c4471a"), "{url}");
        click(&mut m, Id::ClosePalette);
        assert!(!m.draft.open && live(&m, Id::NewPalette));

        // Closing is not discarding: New palette opens the same draft again.
        click(&mut m, Id::NewPalette);
        assert_eq!(m.draft.name, "Late Night");
        assert_eq!(m.draft.start, 2);

        // Over typed colours, a starting point asks first; a second click replaces them.
        m.focus = Some(Field::Colour(0));
        key(&mut m, Key::Clear);
        paste(&mut m, "#101010");
        click(&mut m, Id::PaletteStart(1));
        assert_eq!(m.draft.hex[0], "#101010", "nothing replaced on the first click");
        assert_eq!(m.draft.confirm_start, Some(1));
        assert!(layout(&m, W, H).iter().any(|w| w.text.contains("Click Slate again")));
        click(&mut m, Id::PaletteStart(1));
        assert_eq!(m.draft.hex[0], "#0e1116");
        assert!(!m.draft.edited && m.draft.confirm_start.is_none());

        // Saved: the editor closes so the table shows the new row.
        update(&mut m, Job::SavePalette, job::Update::PaletteSaved("late-night.palette".into()));
        assert!(!m.draft.open && m.draft.saved.is_some());
    }

    /// A filter typed while the table is scrolled down starts it at the top again, so no match
    /// hides above the rows on show.
    #[test]
    fn a_new_filter_starts_the_table_at_the_top() {
        let mut m = ready();
        m.tab = Tab::Check;
        m.checked = Some(500);
        m.findings = (0..100)
            .map(|i| CheckRow { verdict: "LOSSY".into(), file: format!("file {i:03}"), why: "x".into() })
            .collect();
        nav(&mut m, W, H, Nav::End);
        click(&mut m, Id::Field(Field::CheckFilter));
        for c in "file 0".chars() {
            key(&mut m, Key::Char(c));
        }
        assert_eq!(shown_files(&m)[0], "file 000");
        nav(&mut m, W, H, Nav::End);
        click(&mut m, Id::Verdict(0));
        assert_eq!(shown_files(&m)[0], "file 000");
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

    /// With a copy running, the jobs that share anything with it are dead and STOP appears. A
    /// check shares nothing with a copy — it reads the library, which a copy never writes — so it
    /// may still start.
    #[test]
    fn a_running_job_disables_the_starters_and_offers_stop() {
        let mut m = ready();
        m.planned = true;
        started(&mut m, Job::Apply);
        for id in [Id::Plan, Id::Apply, Id::Scan, Id::Import, Id::ReadPlayer] {
            assert_eq!(click(&mut m, id), None, "{id:?} started beside a copy");
        }
        let stop = layout(&m, W, H).into_iter().find(|w| w.id == Id::Stop);
        let stop = stop.expect("a running job offers a way to stop it");
        assert_eq!(hit(&m, W, H, stop.rect.x + 4, stop.rect.y + 4), Some(Id::Stop));
        assert_eq!(m.stop_target(Tab::Sync), Some(Job::Apply));
        // …and the pickers are dead too, so a path cannot change under a running job.
        assert!(hit(&m, W, H, 0, 0).is_none());
        let pick = layout(&m, W, H).into_iter().find(|w| w.id == Id::PickLibrary).unwrap();
        assert_eq!(hit(&m, W, H, pick.rect.x + 4, pick.rect.y + 4), None);
        assert_eq!(click(&mut m, Id::Check), Some(Job::Check));
    }

    /// The question that started this: an analysis runs for hours, and it holds only the analysis
    /// cache. Everything that does not touch the cache stays live while it runs; the two that do
    /// (a plan tags copies from it) say what they are waiting for.
    #[test]
    fn a_scan_leaves_everything_it_does_not_share_live() {
        let mut m = ready();
        m.lastfm = Lastfm { has_key: true, user: None, editing: false };
        started(&mut m, Job::Scan);
        for id in [
            Id::Check,
            Id::ReadPlayer,
            Id::CheckPalettes,
            Id::LastfmSignIn,
            Id::PickVolume(1),
            Id::PickPlaylists,
            Id::ToggleSensMe,
            Id::Tab(Tab::Likes),
        ] {
            assert!(live(&m, id), "{id:?} is dead during a scan");
        }
        for id in [Id::Plan, Id::Scan, Id::Import, Id::PickLibrary] {
            assert!(!live(&m, id), "{id:?} is live during a scan");
        }
        assert_eq!(m.waits_for(Job::Plan).as_deref(), Some("Available when analysing the library finishes."));
        assert_eq!(m.waits_for(Job::Check), None);
        // Two at once: the check starts, and each job keeps its own line.
        assert_eq!(click(&mut m, Id::Check), Some(Job::Check));
        started(&mut m, Job::Check);
        update(&mut m, Job::Scan, job::Update::Say("[3/456] Says.flac".into()));
        update(&mut m, Job::Check, job::Update::Say("12/3184 checked".into()));
        assert_eq!(m.status_line(Tab::SensMe), "[3/456] Says.flac");
        assert_eq!(m.status_line(Tab::Check), "12/3184 checked");
        assert_eq!(m.sensme_log, vec!["[3/456] Says.flac".to_string()], "a check's progress is not logged");
        assert!(m.log.is_empty(), "a scan's lines do not land in the Sync log");
        assert!(m.status_line(Tab::Sync).contains("(and 1 more)"), "{}", m.status_line(Tab::Sync));
        // The scan finishing frees the plan, and leaves the check running.
        assert_eq!(finished(&mut m, Job::Scan, Ok("Analysed 456 tracks.".into())), None);
        assert!(live(&m, Id::Plan));
        assert!(m.is_running(Job::Check) && !m.is_running(Job::Scan));
        assert_eq!(m.sensme_log.last().map(String::as_str), Some("Analysed 456 tracks."));
        assert_eq!(finished(&mut m, Job::Check, Err("ffmpeg is missing".into())).as_deref(), Some("ffmpeg is missing"));
        assert!(!m.busy());
    }

    /// Settings has no footer; its Stop is the one beside a sign-in that is waiting.
    #[test]
    fn stop_on_settings_stops_the_sign_in() {
        let mut m = ready();
        m.lastfm = Lastfm { has_key: true, user: None, editing: false };
        started(&mut m, Job::Scan);
        m.tab = Tab::Settings;
        assert_eq!(m.stop_target(Tab::Settings), None, "Stop on Settings must not stop a scan");
        started(&mut m, Job::LastfmSignIn);
        assert_eq!(m.stop_target(Tab::Settings), Some(Job::LastfmSignIn));
        assert!(layout(&m, W, H).iter().any(|w| w.id == Id::Stop && w.text == "Stop waiting"));
        assert_eq!(m.stop_target(Tab::SensMe), Some(Job::Scan));
    }

    /// Typing: a field takes characters while it has the focus, Tab moves between the key fields,
    /// Enter in them saves, and a paste brings one line with no stray whitespace.
    #[test]
    fn fields_take_typing_and_pastes() {
        let mut m = ready();
        m.tab = Tab::Settings;
        assert_eq!(key(&mut m, Key::Char('a')), None);
        assert!(m.key_input.is_empty(), "nothing has the focus, so nothing is typed");
        click(&mut m, Id::Field(Field::ApiKey));
        for c in "ab1".chars() {
            key(&mut m, Key::Char(c));
        }
        key(&mut m, Key::Backspace);
        assert_eq!(m.key_input, "ab");
        key(&mut m, Key::Tab);
        assert_eq!(m.focus, Some(Field::ApiSecret));
        paste(&mut m, "\n  s3 cr et \nsecond line");
        assert_eq!(m.secret_input, "s3cret", "a key keeps no whitespace and no second line");
        assert_eq!(key(&mut m, Key::Enter), Some(Job::LastfmKey));
        assert_eq!(m.focus, None, "saving takes the caret out of the field");
        click(&mut m, Id::Field(Field::ApiSecret));
        key(&mut m, Key::Clear);
        assert!(m.secret_input.is_empty());
        assert_eq!(key(&mut m, Key::Enter), None, "Save needs both halves");
        // A click anywhere else takes the focus away.
        click(&mut m, Id::Tab(Tab::Check));
        assert_eq!(m.focus, None);
        // The filter keeps its spaces.
        click(&mut m, Id::Field(Field::CheckFilter));
        paste(&mut m, "Pink Moon");
        assert_eq!(m.check_filter, "Pink Moon");
        for _ in 0..(Field::MAX + 20) {
            key(&mut m, Key::Char('x'));
        }
        assert_eq!(m.check_filter.chars().count(), Field::MAX);
        key(&mut m, Key::Escape);
        assert_eq!(m.focus, None);
    }

    /// The Check page's filter: a verdict card, text anywhere in the row, both, and Show all.
    #[test]
    fn the_check_filter_narrows_the_table() {
        let mut m = ready();
        m.tab = Tab::Check;
        let row = |v: &str, f: &str, why: &str| CheckRow { verdict: v.into(), file: f.into(), why: why.into() };
        m.findings = vec![
            row("LOSSY", "Burial/Untrue/02 Archangel.flac", "stops at 16.0 kHz"),
            row("LOSSY", "Radiohead/Kid A/01.flac", "stops at 16.0 kHz"),
            row("PADDED", "Burial/Untrue/05 Near Dark.flac", "24-bit file holding 16-bit samples"),
        ];
        let files = |m: &Model| m.check_rows().iter().map(|r| r.file.clone()).collect::<Vec<_>>();
        assert_eq!(files(&m).len(), 3);
        m.check_filter = "BURIAL".into();
        assert_eq!(files(&m).len(), 2, "the text ignores case");
        let lossy = pages::VERDICTS.iter().position(|v| v.0 == "LOSSY").unwrap();
        click(&mut m, Id::Verdict(lossy));
        assert_eq!(files(&m), vec!["Burial/Untrue/02 Archangel.flac".to_string()]);
        m.check_filter = "16-bit".into();
        assert!(files(&m).is_empty(), "the reason is searched too, and the verdict still applies");
        click(&mut m, Id::Verdict(lossy));
        assert_eq!(m.check_verdict, None, "a second click on the card lets it go");
        assert_eq!(files(&m).len(), 1);
        click(&mut m, Id::ClearFilter);
        assert_eq!(files(&m).len(), 3);
        assert!(m.check_filter.is_empty());
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
        started(&mut m, Job::Apply);
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

    /// A model with something on every page: findings, a read player, a log.
    fn full() -> Model {
        let mut m = ready();
        m.volumes[1] = Some("/card".into());
        m.checked = Some(40);
        m.findings = (0..30)
            .map(|i| CheckRow { verdict: "LOSSY".into(), file: format!("a/{i}.flac"), why: "cutoff".into() })
            .collect();
        m.player = PlayerFacts {
            read: true,
            albums: (0..40)
                .map(|i| AlbumRow {
                    folder: format!("Artist/Album {i}"),
                    volume: i % 2,
                    files: 10,
                    bytes: 1 << 28,
                    format: "FLAC".into(),
                    by_flint: i % 3 != 0,
                    rating: (i % 4 == 0).then_some((i % 5 + 1) as u8),
                    plays: (i * 3) as u32,
                })
                .collect(),
            plays: (0..40)
                .map(|i| PlayRow {
                    when: 1_700_000_000 + i,
                    track: "T".into(),
                    artist: "A".into(),
                    kind: "PLAY".into(),
                })
                .collect(),
            unreadable: 1,
            likes: 3,
            palettes: vec![PaletteFile { volume: 0, name: "moss.palette".into(), bytes: 600 }],
            rated: 12,
            counted: 80,
            views: vec![("Late favourites".into(), "4 stars and up · most played first".into())],
            playlists: vec![
                PlaylistRow { name: "Late Night".into(), tracks: 14, edited: true },
                PlaylistRow { name: "Walk".into(), tracks: 9, edited: false },
            ],
        };
        m.log = (0..60).map(|i| format!("line {i}")).collect();
        m
    }

    /// Every page: each live control answers a click at its own centre, no two overlap, and nothing
    /// is drawn off the window — at the default size, the smallest and a large one.
    #[test]
    fn every_page_is_hittable_and_on_the_window() {
        for tab in Tab::ALL {
            for (w, h) in [(W, H), (MIN_W, MIN_H), (1400, 900)] {
                let mut m = full();
                m.tab = tab;
                let widgets = layout(&m, w, h);
                for wid in &widgets {
                    assert!(wid.rect.x >= 0 && wid.rect.y >= 0, "{tab:?}: {:?} at {:?}", wid.kind, wid.rect);
                    assert!(
                        wid.rect.right() <= w && wid.rect.bottom() <= h,
                        "{tab:?} {w}x{h}: {:?} off the window at {:?}",
                        wid.kind,
                        wid.rect
                    );
                }
                let live: Vec<&Widget> = widgets.iter().filter(|x| x.id != Id::None).collect();
                for wid in &live {
                    let enabled = !matches!(
                        wid.kind,
                        Kind::Button { enabled: false, .. }
                            | Kind::Tool { enabled: false }
                            | Kind::Check { enabled: false, .. }
                    );
                    if enabled {
                        let (cx, cy) = (wid.rect.x + wid.rect.w / 2, wid.rect.y + wid.rect.h / 2);
                        assert_eq!(hit(&m, w, h, cx, cy), Some(wid.id), "{tab:?}: {:?} at {:?}", wid.id, wid.rect);
                    }
                }
                for (i, a) in live.iter().enumerate() {
                    for b in &live[i + 1..] {
                        let overlap = a.rect.x < b.rect.right()
                            && b.rect.x < a.rect.right()
                            && a.rect.y < b.rect.bottom()
                            && b.rect.y < a.rect.bottom();
                        assert!(!overlap, "{tab:?}: {:?} overlaps {:?}", a.id, b.id);
                    }
                }
            }
        }
    }

    /// The outlined tool buttons answer clicks. They did not, before 0.2: `hit` only knew filled
    /// buttons and checkboxes, so Analyse library, Import Music Center, Check FLACs and the
    /// playlist folder's Choose… were drawn and dead.
    #[test]
    fn tool_buttons_answer_clicks() {
        let mut m = ready();
        m.playlists = Some("/lists".into());
        for (tab, id) in [
            (Tab::Sync, Id::PickPlaylists),
            (Tab::Sync, Id::ClearPlaylists),
            (Tab::SensMe, Id::Scan),
            (Tab::SensMe, Id::Import),
            (Tab::Check, Id::Check),
            (Tab::Player, Id::ReadPlayer),
        ] {
            m.tab = tab;
            let wid =
                layout(&m, W, H).into_iter().find(|w| w.id == id).unwrap_or_else(|| panic!("{id:?} not on {tab:?}"));
            assert_eq!(hit(&m, W, H, wid.rect.x + 5, wid.rect.y + 5), Some(id), "{id:?} on {tab:?} is dead");
        }
        assert_eq!(click(&mut m, Id::ReadPlayer), Some(Job::ReadPlayer));
    }

    /// Tabs switch pages, stay where they are whatever the counts say, and only Sync ever carries
    /// the accent — nothing on the other pages writes to the player.
    #[test]
    fn tabs_switch_pages_and_only_sync_is_accented() {
        let mut m = ready();
        let before: Vec<Rect> =
            layout(&m, W, H).iter().filter(|w| matches!(w.kind, Kind::Tab { .. })).map(|w| w.rect).collect();
        assert_eq!(before.len(), Tab::ALL.len());
        for tab in Tab::ALL {
            let r = tab_rects(W).into_iter().find(|(t, _)| *t == tab).unwrap().1;
            assert_eq!(hit(&m, W, H, r.x + r.w / 2, r.y + r.h / 2), Some(Id::Tab(tab)));
            click(&mut m, Id::Tab(tab));
            assert_eq!(m.tab, tab);
            let accented =
                layout(&m, W, H).into_iter().filter(|w| matches!(w.kind, Kind::Button { primary: true, .. })).count();
            assert_eq!(accented, usize::from(tab == Tab::Sync), "{tab:?} has {accented} accented controls");
        }
        let mut f = full();
        f.tab = Tab::Sync;
        let after: Vec<Rect> =
            layout(&f, W, H).iter().filter(|w| matches!(w.kind, Kind::Tab { .. })).map(|w| w.rect).collect();
        assert_eq!(before, after, "a count appearing moved a tab");
        // Choosing a theme is a click like any other, and starts nothing.
        assert_eq!(click(&mut m, Id::Theme(ThemePref::Dark)), None);
        assert_eq!(m.theme, ThemePref::Dark);
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
