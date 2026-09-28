# Changelog

## Unreleased (0.2)

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
- Tests cover every page at three window sizes: every control answers at its centre, none overlap,
  nothing is drawn off the window, and only Sync carries the accent.

## 0.1.0 — 2026-09-21

First release: `flint sync`, `scan`, `import`, `check`, `inspect`, `tag-copy`, `analyse`, Last.fm
(`lastfm`, `scrobble`, `likes`), and the window.
