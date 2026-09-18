# Flint against Music Center for PC

*Written 2026-09-18, against Music Center for PC 2.7.3 and Flint at `1cfe26f`+.*

Sony's Music Center for PC is the official way to get music onto an NW-A50-series Walkman. Flint is
a command-line tool that does the transfer part, plus a few things Music Center does not. This
document is the honest map of the two: what Flint matches, what it deliberately does not do, and
what is missing and worth building.

**Read the verdicts as written.** "Parity" means a Walkman owner can do the job with Flint. It does
not mean the two work the same way, and where Flint is better it says so with a reason rather than
a claim.

---

## 1. Getting music onto the player

| | Music Center | Flint | Verdict |
|---|---|---|---|
| Copy a library to the player | Drag and drop, or a sync | `flint sync <library> --to E:\ [--to F:\]` | **Parity** |
| Internal memory **and** a card | Two destinations, chosen by hand per album | Planned: an album is never split, albums that share a playlist stay on one volume, and an album already on a volume stays there | **Flint ahead** — a 4,000-track library is not a drag-and-drop job |
| Space management | Shows free space | Each volume gets what is free less 512 MB of headroom, or `--gb N` | Parity |
| Remove what no longer belongs | Delete by hand on the device page | Anything the plan does not place is swept, and the empty folders it leaves are pruned | **Flint ahead** |
| Don't re-copy what is already there | Compares the file | A per-volume manifest records what each copy was made from, because a SensMe-tagged copy is larger than its source and size alone would re-copy the library every run | **Flint ahead** (it has to be) |
| An interrupted transfer | Leaves a partial file | Every copy is written to a temporary name and renamed, so the old file or the new one survives — never half of one | **Flint ahead** |
| Playlists | Transfers them | `--playlists <folder>`; a playlist never spans two volumes, and one whose tracks all landed elsewhere is not written | Parity |
| Cover art and lyrics beside the music | Transfers artwork | `.jpg/.jpeg/.png/.lrc` in an album folder travel with it, and are **never swept** | Parity (and Cinder reads `.lrc`) |
| Convert on transfer (FLAC → AAC/MP3 to fit more) | Yes | **No** | **Gap — see §5** |
| Transfer to anything but a Walkman volume | Several Sony devices | Any folder, so any mounted player | — |

## 2. SensMe, and analysis

| | Music Center | Flint | Verdict |
|---|---|---|---|
| Sony's analysis engine | Bundled (`MMLib11.dll`) | Runs Sony's own DLL, from the Music Center install — never shipped, never downloaded | Parity, without redistributing Sony's code |
| Where the result goes | Into your PC library's files, reported at nearly a megabyte each | Into the **copy on the player** only, about 6 KB, and never into a PC file | **Flint ahead** — the reason it exists |
| Reusing analysis you already have | — | `flint import` takes Music Center's own per-track cache; `scan` and `sync` take a tag already inside a file | **Flint ahead** |
| Trimming a tag that is already fat | — | Chunks the player's scanner never reads are dropped from the copy | **Flint ahead** |
| Channels on the player | Written for Sony's own player | The same tag, so stock reads it — and Cinder reads it too (`CINDER_SENSME`) | Parity |
| Works without Music Center installed | It **is** Music Center | Everything except analysis. The engine is Sony's; no engine, no new analysis | — |
| macOS / Linux | Windows only | Windows for analysis; the rest cross-compiles and runs anywhere | Flint ahead, partially |

## 3. What Flint has and Music Center does not

* **`flint check` — finding fake FLACs.** Lossy audio re-encoded as FLAC, upsampled files and
  16-bit padded to 24. Measured against transcodes of four albums; the README has the hit rates,
  including the cases it cannot catch.
* **A transfer you can read before it happens.** `flint sync` without `--apply` prints the plan.
* **Nothing is ever written to the PC library.** Music Center writes analysis into your files;
  Flint's only writes are the copies on the player, its own cache, and the per-volume manifest.
* **One executable, no installer, no service, no account.**

## 4. What Music Center does that Flint does not, and will not

Out of scope, not backlog. Each is a large piece of software in its own right, and none of it is
about getting music onto a Walkman:

* **Playing music on the PC**, with its own equaliser and DSEE HX.
* **Ripping and burning CDs.**
* **Gracenote lookups** — track titles, artist names and cover art fetched from a licensed
  database. Flint reads the tags your files already carry.
* **Editing your library's metadata.** Flint never writes to a PC file; a tag editor is a different
  tool, and better ones exist.
* **Importing an iTunes or Media Go library.** Flint takes a folder of music.

## 5. The gaps worth closing, in order

1. **A window.** The single biggest difference for anyone who does not live in a terminal, and the
   only reason to reach for Music Center for the transfer itself. The design is already decided:
   Cinder's installer draws a real Win32 window with no toolkit and no dependencies
   (`installer/src/gui.rs`, 1,624 lines), and the same layer fits a two-pane "library → player"
   view with the plan in the middle. Until it exists, `flint sync` without `--apply` is the preview
   and the terminal is the UI.
2. **Convert on transfer.** A 64 GB card holds about 150 FLAC albums or 600 at AAC 256. Music Center
   does this and it is the one transfer feature Flint genuinely lacks. FFmpeg is already a
   dependency of `scan` and `check`; the work is a per-format rule (`--convert flac=aac:256`), the
   manifest recording what a copy was made from and at what setting, and keeping the SensMe tag
   across the re-encode — the analysis is of the audio, not of the file, so it survives.
3. **Likes and scrobbles**, carried over from the Python `Sony-sync` this replaces: the three-way
   liked-songs sync (`docs/LIKES_SYNC.md` in Cinder) and collecting `.scrobbler.log` off the player.
   Music Center has no equivalent; this is parity with Flint's own predecessor, not with Sony.
4. **M4A/MP4 SensMe tags.** FLAC and MP3 are done and device-proven. Sony's own writer handles MP4
   through `OmgMp4LibWrapper.dll`; the container work is understood and untested.
5. **A device page** — what is on the player, how full it is, what Flint put there and when. Today
   that reads out of `flint-manifest.tsv` with a text editor.

## 6. Where the facts come from

* Music Center's engine, its cache layout and the tag containers: Cinder's
  [`analysis/RE_sensme_musiccenter.md`](https://github.com/superwilso/Cinder/blob/main/analysis/RE_sensme_musiccenter.md),
  §1–§11 — including the two device proofs that the player turns these tags into channels.
* The "almost a megabyte" figure is Wampy's `MAKING_OF.md`, reporting what Music Center did to a
  library. Flint has not measured it directly; what is measured is the engine's own output, which
  is about 6 KB and does not grow with track length.
* Music Center's id → file mapping (used by `flint import`) is **not verified**: it lives in its
  NeDB store and nobody here has confirmed the field names. `crates/flint-core/src/musiccenter.rs`
  therefore assumes no schema, and every import is checked against the file it claims to describe.
