# Flint

A PC companion for the Sony NW-A50-series Walkman and [Cinder](https://github.com/superwilso/Cinder):
copy a music library onto the player, keep likes and scrobbles in step, and give every track Sony's
**SensMe** mood and tempo data — without touching the files on your PC.

**Status: early rewrite.** Flint replaces the Python `Sony-sync` tool, which remains the working
version until Flint reaches it. Nothing here is released yet.

## SensMe, without the bloat

SensMe channels on the Walkman need an analysis tag in each file. Sony's Music Center writes one into
your PC library, and those files have been reported to grow by almost a megabyte each. Flint does it
differently:

* It runs **Sony's own analysis engine** (`MMLib11.dll`, installed with Music Center for PC) over each
  track once, and keeps the result in its own cache — about 6 KB per track.
* It writes that 6 KB **only into the copy it puts on the Walkman** — a FLAC `SMFM` block or an MP3
  `GEOB` frame, the same containers Sony uses. Your PC files are never modified.
* The Walkman's own scanner turns the tag into channels, the same as for Music Center's tags — so this
  works on stock firmware as well as Cinder.

Flint does not include Sony's engine. SensMe analysis needs Music Center for PC installed on the same
Windows machine; everything else in Flint works without it.

## Copying a library to the player

```
flint scan "D:\\Music"                              analyse once, into Flint's cache
flint sync "D:\\Music" --to E:\\ --to F:\\ --playlists "D:\\Playlists"     a dry run
flint sync "D:\\Music" --to E:\\ --to F:\\ --playlists "D:\\Playlists" --apply
```

Albums are the unit that moves, and an album is never split across the internal memory and the card.
Albums that share a playlist stay together, so no playlist spans two volumes, and an album already on
a volume stays there. Each volume gets whatever space is free on it, less half a gigabyte of
headroom; `--gb N` sets a budget instead. `--no-sensme` copies without tagging.

Each copy is written to a temporary file and renamed, so an interrupted transfer leaves either the
old file or the new one. Every volume carries `flint-manifest.tsv`, recording what each copy was made
from — without it, a SensMe-tagged copy is larger than its source and would be copied again on every
run, for ever.

## Checking for fake FLACs

`flint check` looks for FLACs that are not the lossless audio they claim to be. It needs FFmpeg, and
nothing else — no Music Center, no Walkman.

```
flint check "D:\Music"            every FLAC under a folder
flint check track.flac --verbose  one file, with its spectrum
```

| Verdict | What was found |
|---|---|
| `LOSSY` | The sound stops dead at an encoder's lowpass frequency, or stops at one and the top band is switched off in a third of loud frames |
| `SUSPECT` | It stops between 19.5 and 21 kHz: a high-bitrate encoder, or simply the master |
| `UPSAMPLED` | An 88.2 kHz or higher file with nothing real above CD or DAT bandwidth |
| `PADDED` | A 24-bit file whose samples only use 16 bits |
| `DAMAGED` | FFmpeg reported errors decoding the stream |

Measured against transcodes of four albums (each encoded and turned back into FLAC):

| Source | Caught |
|---|---|
| MP3 128 kbit/s | 4 of 4 |
| MP3 320 kbit/s | 4 of 4 (2 as `LOSSY`, 2 as `SUSPECT`) |
| Opus 160 kbit/s | 4 of 4 (2 as `LOSSY`, 2 as `SUSPECT`) |
| Vorbis q6 | 1 of 4 |
| LAME V0, FFmpeg AAC 256 kbit/s | 0 of 4 — these keep sound almost to 22 kHz, so there is nothing to see |
| Upsampled to 96 kHz, and 16-bit padded to 24 | 4 of 4 each |
| The four untouched originals | none flagged |

**A clean result is not proof.** It means none of the usual signs. Equally, `SUSPECT` is not an
accusation: plenty of genuine masters stop at 20 kHz. Results are cached by audio content, so a
retagged or moved file is not decoded twice.

## Layout

| Crate | What it is |
|---|---|
| `flint-core` | FLAC metadata and ID3v2 reading and writing, the SMFMF chunk format, and the decode → engine pipeline |
| `flint` | The command-line tool |
| `sensme-helper` | A 32-bit Windows helper that loads `MMLib11.dll` (the engine is 32-bit, Flint is not) |

## Building

```
cargo test
cargo build --release --target x86_64-pc-windows-gnu -p flint
cargo build --release --target i686-pc-windows-gnu -p sensme-helper
```

Both Windows targets cross-compile from Linux with mingw-w64.

## Licence

MIT. Sony's engine is not part of this repository and is not redistributed.
