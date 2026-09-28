# Changelog

## Unreleased

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
