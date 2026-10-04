# Changelog

## Unreleased

### Play counts from the player's history

- **`flint stats <drive> [--to <sd card>] [--apply]`** gives tracks the player has never counted
  their plays from the scrobble log on the player. Cinder starts every count at zero and does not
  read its own log; this fills that gap once. A count or a rating the player already has is never
  changed, and running it twice changes nothing the second time.
- Plays are matched on artist and title. A play whose artist and title belong to two files on the
  player is reported and not placed.
- The history is what is still in `.scrobbler.log`. Plays Flint has already sent to Last.fm and
  removed from the log are not counted, so seed before the first scrobble.
- New in `flint-core`: `stats`, which reads and writes Cinder's `cinder_stats.tsv` byte for byte.
  *Command line only; the window does not show ratings or counts yet.*

### Make and share palettes

- **Palettes ▸ New palette** opens an editor. Start from Cinder, Slate, Paper or Sony, give the
  palette a name and change its twelve colours, plus six more if it has an accent of its own. A
  preview shows day and night as the panel draws them. Under it, the player's verdict updates with
  every key, in the player's own words.
- **Save to folder** writes `<name>.palette` into the palettes folder and checks the folder. It
  only ever replaces the file it saved last. Any other file with that name is left alone.
- **Close** keeps the draft. **Your palette** opens it again.
- Choosing another starting point after you have typed colours asks for a second click before it
  replaces them.
- The player's reasons name colours the way the editor labels them: *Day dim text on Day
  background*.
- **Share…** opens the shared repository's form,
  [superwilso/cinder-themes](https://github.com/superwilso/cinder-themes), with the palette already
  filled in.
- **Shared palettes** lists every palette in the shared repository. Each row shows the palette by
  day and by night, as the player draws it, and says whether it is light or dark and whether it
  brings its own accent.
  - Type in the search box to narrow the list: part of a name, *light*, *dark* or *accent*.
  - **Install** puts one palette in the folder and, when the player's internal memory is chosen and
    plugged in, on the player too. Then pick it in *Settings ▸ Display ▸ Palette*. Without the
    player, the button says **Get** and Send takes it across later.
  - A row that needs nothing says **Installed** or **In your folder**. A file of the same name that
    is someone's own, in the folder or on the player, is never replaced, and its row says **Yours
    differs**.
  - **Get all** downloads every shared palette not already in the folder. **Open on GitHub** opens
    the gallery.
  - Every palette is checked with the player's rules before it is written.

### Lists scroll

Every table and log scrolls: Check's findings, the player's albums, the plays, the palettes, and
the Sync, SensMe and Last.fm logs.

- Scroll with the mouse wheel, drag the thumb, or click the track to move a page.
- The keyboard also works: Page Up, Page Down, Home, End and the arrow keys.
- A log follows its newest line. Scrolled up, it stays on the lines you are reading while more
  arrive, and End brings it back.
- "…and N more" is gone. Every row can be reached.

### The preview lists every removal

**Show what would happen** used to list the first 12 files a copy would delete and drop the rest
without saying so. Now it lists every removal, up to 1,500, plus the playlists it would remove, and
ends on the totals. Deleting is the one thing a sync cannot undo. `flint sync` without `--apply` had
the same gap, with 10 in place of 12: it now prints every removal, then `in all: N to copy, M to
remove`.

### Fixed (audit 2026-10-01)

- **Scrobble times were off by your time zone.** Cinder writes its log with `#TZ/UNKNOWN`: the
  times are the player's own clock, which knows the time but not the zone, and the uploader is
  meant to convert them. Flint sent them as written, so every play landed an hour early or late in
  summer in the UK, and nine hours out in Japan. Flint now converts them with this PC's time zone,
  daylight saving included, and says so in the preview. A log that says `#TZ/UTC` goes out as
  written, as before.
- **Stop during Copy now stops.** The button was shown during a copy and changed nothing except
  the closing sentence, which said "Stopped early" after every file had been copied. Now the copy
  in hand finishes, the rest are left for next time, and no playlist is written that could name a
  track that is not there yet.
- **A copy that fails no longer leaves a hidden temporary file behind.** Its name started with a
  dot, so nothing found it again, and it held the space it took — on a full volume, which is
  usually why a copy fails. Files a pulled cable left behind are now swept by the next plan.
- **Planning a big library is fast.** Deciding which volume each album stays on compared every
  album with every file on the player. 4,000 albums took 36 seconds of CPU per volume, twice per
  sync. It now takes under a tenth of a second.
- **A plan no longer reads every MP3 in full.** The SensMe check worked out each track's audio key
  by hashing the whole file, on Plan and again on Copy, when the analysis cache already knew the
  answer from the file's size and date.
- With the same room on internal memory and the card, an album now goes to internal memory, as
  the code always said. It went to the card.

## 0.2.1 — 2026-09-28

### No terminal window

`flint-windows-x64.exe` is now the window alone, built for the Windows subsystem, so it opens with
no black console box behind it. The commands moved to their own download,
`flint-cli-windows-x64.exe`, which is the same program 0.2.0 shipped. FFmpeg and the SensMe
helper are started with `CREATE_NO_WINDOW`, so a check or an analysis does not flash a console per
file either.

### Jobs run side by side

0.2.0 ran one job at a time: while an analysis ran — hours, on a big library — every other button
was grey. Now each job says what it needs (the analysis cache, the player, the check results, the
palettes, the Last.fm account), and two jobs wait for each other only when they share one.

- During an analysis you can check FLACs, read the player, compare palettes, change the player's
  drives and sign in to Last.fm.
- **Show what would happen** and **Copy** still wait for an analysis, because the copies are tagged
  from its results. The button says so: *Available when analysing the library finishes.*
- Each page's footer shows its own job's progress, line and **Stop**; a page whose job is not
  running says what is, so the window never looks idle while the machine is not.
- Closing the window stops every job and closes when the last one has.

### Last.fm from the window

- **Settings ▸ Last.fm** takes the API key and shared secret (paste with Ctrl+V), with a **Get a key
  on last.fm** button that opens the page that makes one.
- **Sign in with Last.fm** opens last.fm in the browser; after *Yes, allow access* Flint picks up
  the session by itself (it asks every three seconds, for up to ten minutes, and **Stop waiting**
  gives up). The window never asks for the password. **Sign out** and **Change key** are beside it.
- **Likes & plays** gains **Send N plays**, **Compare likes** — what a likes sync would change, in
  both directions, written nowhere — and **Make N changes**, which carries that out.
- The code behind `flint scrobble` and `flint likes` moved into `flint-core` (`lastfm_sync`), so the
  window and the commands run the same thing. The commands print what they did before.

### Check can filter

Click a verdict's count to list only those files, and type in the filter to narrow by artist,
album, file name or reason (case does not matter). **Show all** clears both.

### Fixed

- **Paths and the big figures drew in the wrong font** on Windows: the window made no GDI font for
  either face, so they took whatever font the text before them had used.
- A job that failed showed its message box from inside the window's state, and a repaint while the
  box was up could close Flint. The box now opens after the state is put down.

## 0.2.0 — 2026-09-28

### The window has pages

The window is now seven tabs — **Sync · On the player · Check · SensMe · Likes & plays · Palettes ·
Settings** — from the 2026-09 redesign shared with Cinder.

- **Sync** is the window 0.1 was: the folder, the player's drives with their capacity bars, the plan,
  the copy. It is the only page that writes music to the player, and the only one with anything
  orange on it.
- **On the player** reads each drive and lists its albums, marking the ones Flint put there (from
  `flint-manifest.tsv`) and the ones something else did, with totals for both.
- **Check** runs `flint check` and shows the verdicts as counts (LOSSY, SUSPECT, UPSAMPLED, PADDED,
  DAMAGED, OK) and as a table of files, each with the reason.
- **SensMe** holds Analyse library and Import Music Center, with their progress.
- **Likes & plays** shows the plays in the player's `.scrobbler.log`, newest first, and how many
  songs are liked on it. Sending still happens with `flint scrobble` and `flint likes`.
- **Palettes** checks a folder of `.palette` files on the PC with Cinder's own readability rules —
  the same contrast floors the player applies — and sends the ones that pass. Each palette is a row
  with its colours, where it stands (NEW, CHANGED, ON, ON THE PLAYER, BUILT IN, REFUSED) and, for a
  refused one, the first reason, which is the one to fix first. **Check** writes nothing; **Send**
  copies only what the check called new or changed, never a refused file, through a temporary name
  and in lower case (the player's id for a palette is its lowercased name).
- **Settings**: Theme (Light, Dark or System), the music and playlist folders, where the analysis
  cache is, and whether Last.fm is set up.

### Remembers where things are

The folders (the palettes folder too), the two switches and the theme are kept in `gui.conf` beside
the analysis cache, so the window opens where it was left instead of asking for the music folder and
the drive every time.

### Fixed

- **The outlined buttons did nothing.** Analyse library, Import Music Center, Check FLACs, and the
  playlist folder's Choose… and Clear were drawn, lit up under the hand cursor, and ignored the
  click: the hit test only answered filled buttons and checkboxes.

### For developers

- `flint gui-preview --state` draws every page: `player`, `check`, `sensme`, `likes`, `palettes`,
  `settings`, alongside the Sync states.
- CI draws every state, not only the five Sync ones.
- **`tools/release.sh vX.Y.Z` cuts a release**: one run prepares it (version, changelog, window
  pictures, every gate), you commit, and a second run pushes, tags, waits for the workflow and
  prints the release page. The release page's *What's new* is now the version's CHANGELOG section.
- Tests cover every page at three window sizes: every control answers at its centre, none overlap,
  nothing is drawn off the window, and only Sync carries the accent.

## 0.1.0 — 2026-09-21

First release: `flint sync`, `scan`, `import`, `check`, `inspect`, `tag-copy`, `analyse`, Last.fm
(`lastfm`, `scrobble`, `likes`), and the window.
