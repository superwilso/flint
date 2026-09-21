<!--
The release body. `{{SHA256SUMS}}` is replaced with the real checksums by
tools/render_release_notes.sh, which the release workflow runs — preview it with
`tools/render_release_notes.sh --preview`.

Everything above the first `## ` heading is dropped, so this comment does not ship.
GitHub appends its own generated "What's Changed" list below whatever this produces.
-->

## Flint

Put a music library on a Sony NW-A50-series Walkman — with SensMe, without the bloat.

### What to download

| File | For |
|---|---|
| **`flint-windows-x64.exe`** | Windows. This is the one to get. Run it with no arguments for the window, or from a terminal for the commands. |
| `sensme-helper-x86.exe` | **Put it in the same folder as `flint.exe`.** 32-bit because Sony's `MMLib11.dll` is 32-bit COM; Flint runs it as a child process to analyse a track. Without it, everything works except making *new* SensMe analysis. |
| `flint-linux-x64` | Linux. The scan, the plan, the copy, the tag writing and `flint check` all work. The window and the analysis engine are Windows-only. |

Nothing here contains, downloads or installs any Sony code. Analysis runs the `MMLib11.dll` that
is already on the machine, from a Music Center for PC install you chose to have. If you do not
have it, `flint import` can still take analysis Music Center did earlier, and a file that already
carries a SensMe tag is read as it stands.

### Getting started

```
flint sync "D:\Music" --to E:\ --to F:\            # what would happen — writes nothing
flint sync "D:\Music" --to E:\ --to F:\ --apply    # do it
```

Or run `flint-windows-x64.exe` with no arguments and use the window. It follows Windows' own
light/dark setting and changes with it while it is open.

### Last.fm, for a player that has no WiFi

The Walkman cannot reach Last.fm itself, so it writes files and Flint carries them:

```
flint lastfm key <api-key> <api-secret>     once — https://www.last.fm/api/account/create
flint lastfm login <your-username>          the password is exchanged for a session key, never stored

flint scrobble E:\ --apply                  the plays in .scrobbler.log, fifty at a time
flint likes E:\ F:\ --apply --playlist      liked songs both ways, and Liked Songs.m3u8
```

A scrobble leaves the player's log only once Last.fm has **accepted** it; a row it refused stays,
with the reason. Likes are a sync rather than a merge — Flint remembers what each side looked like
last time, so the first run is additive, an unplugged player never causes a removal, and a track
changed on both sides keeps the like. Matching folds `feat.` credits and re-issue suffixes the same
way Cinder folds them on the device, while live, remix and acoustic stay distinct.

Nothing here adds a dependency for the network: requests go out over Windows' own TLS stack
(WinHTTP), with the machine's proxy settings and certificate store.

### Verifying the download

Every file published here, with the SHA-256 the build produced. The workflow also files a
Sigstore build attestation, so provenance can be checked directly:

```
gh attestation verify flint-windows-x64.exe -R superwilso/flint
```

```
{{SHA256SUMS}}
```
