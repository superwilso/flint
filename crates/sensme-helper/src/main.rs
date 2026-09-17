//! sensme-helper — run Sony's SensMe engine (`MMLib11.dll`, from Music Center for PC) over one track.
//!
//! ```text
//! sensme-helper <path\to\MMLib11.dll> [id=value ...]  < pcm  > smfmf
//! ```
//!
//! * **stdin:** raw PCM, signed 16-bit little-endian, 44.1 kHz, stereo, until EOF.
//! * **stdout:** the engine's SMFMF result (result id 89) — the bytes the Walkman's scanner parses.
//! * **stderr:** `key=value` lines: `engine`, `frames`, `feed_ms`, `bytes`, `result88`, and every
//!   integer result as `r<id>`.
//! * **exit:** 0 ok · 2 usage · 3 the DLL or its class would not load · 4 the engine failed.
//!
//! It is a separate 32-bit program because the DLL is 32-bit COM and Flint is not. No registration is
//! needed: `LoadLibrary` → `DllGetClassObject` → `IClassFactory::CreateInstance`. The interface IDs
//! and vtable slots are from the DLL's own type library (Cinder `analysis/RE_sensme_musiccenter.md`
//! §2); the call sequence is the one `tools/sensme/smfmf_probe.c` proved on real tracks (§5).

#[cfg(all(windows, target_arch = "x86"))]
mod engine;

#[cfg(all(windows, target_arch = "x86"))]
fn main() {
    std::process::exit(engine::run());
}

#[cfg(not(all(windows, target_arch = "x86")))]
fn main() {
    eprintln!("sensme-helper only runs as a 32-bit Windows program: build it for i686-pc-windows-gnu.");
    std::process::exit(2);
}
