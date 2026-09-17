//! The COM plumbing, by hand: raw vtables and `extern "system"` imports, no crates.

use std::ffi::{c_void, OsStr};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::time::Instant;

type HRESULT = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

const CLSID_MUSIC_ANALYSIS2: Guid = Guid(0x8DE9_ED2D, 0x8CFB, 0x4B33, [0xB8, 0xE3, 0xCE, 0xE0, 0x6F, 0x29, 0x03, 0xE3]);
const IID_IMUSIC_ANALYSIS2: Guid = Guid(0x0959_C485, 0xA191, 0x4508, [0xA3, 0x38, 0x9E, 0x2B, 0x38, 0x13, 0xA1, 0x66]);
const IID_ICLASS_FACTORY: Guid = Guid(0x0000_0001, 0x0000, 0x0000, [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46]);

const VT_I2: u16 = 2;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;
/// The engine returns its integer results as VT_INT, not VT_I4.
const VT_INT: u16 = 22;
const VT_UI1_ARRAY: u16 = 0x2011;

/// VARIANT on 32-bit Windows: 8 bytes of type and reserved words, 8 bytes of value.
#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    val: [u32; 2],
}

impl Variant {
    fn empty() -> Self {
        Variant { vt: 0, r1: 0, r2: 0, r3: 0, val: [0; 2] }
    }
}

#[repr(C)]
struct SafeArray {
    dims: u16,
    features: u16,
    elem_size: u32,
    locks: u32,
    data: *mut c_void,
    count: u32,
    lower: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, model: u32) -> HRESULT;
}

#[link(name = "oleaut32")]
extern "system" {
    fn VariantClear(v: *mut Variant) -> HRESULT;
    fn SafeArrayAccessData(sa: *mut SafeArray, data: *mut *mut c_void) -> HRESULT;
    fn SafeArrayUnaccessData(sa: *mut SafeArray) -> HRESULT;
    fn SysStringLen(s: *const u16) -> u32;
}

type DllGetClassObject = unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> HRESULT;

/// The slot `n` function pointer of a COM object's vtable.
unsafe fn slot(obj: *mut c_void, n: usize) -> *const c_void {
    let vtbl = *(obj as *const *const *const c_void);
    *vtbl.add(n)
}

/// IUnknown::Release, through the object's OWN vtable (slot 2 on every COM interface).
unsafe fn release(obj: *mut c_void) {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(slot(obj, 2));
    f(obj);
}

// IClassFactory
const CF_CREATE_INSTANCE: usize = 3;
// IMusicAnalysis2 (dual interface: 0-2 IUnknown, 3-6 IDispatch)
const MA_RUN: usize = 7;
const MA_SET_PARAMETER: usize = 13;
const MA_GET_PARAMETER: usize = 14;
const MA_INPUT_PCM: usize = 15;
const MA_SET_INPUT_PCM_FORMAT: usize = 16;
const MA_GET_RESULT: usize = 17;
// IAnalysisResult2
const AR_GET_RESULT_BY_ID: usize = 8;

/// The engine's integer results run 0..=88; 89 is the SMFMF blob, 90 its STMM chunk alone.
const LAST_INT_RESULT: i32 = 88;
const RESULT_SMFMF: i32 = 89;

/// What InputPCM is fed per call — the size the probe used on real tracks.
const FEED_BYTES: usize = 16 * 1024;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn fail(code: i32, msg: String) -> i32 {
    eprintln!("error={msg}");
    code
}

pub fn run() -> i32 {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(dll) = args.first() else {
        return fail(2, "usage: sensme-helper <MMLib11.dll> [id=value ...] < pcm > smfmf".into());
    };
    let mut params = Vec::new();
    for a in &args[1..] {
        let parsed = a
            .to_str()
            .and_then(|s| s.split_once('='))
            .and_then(|(k, v)| Some((k.parse::<i32>().ok()?, v.parse::<i32>().ok()?)));
        match parsed {
            Some(p) => params.push(p),
            None => return fail(2, format!("bad parameter {:?}; expected id=value", a)),
        }
    }
    unsafe { analyse(dll, &params) }
}

unsafe fn analyse(dll: &OsStr, params: &[(i32, i32)]) -> i32 {
    // Apartment-threaded, as the probe ran it: the engine's synchronous InputPCM path needs no pump.
    CoInitializeEx(null_mut(), 0x2);
    let module = LoadLibraryW(wide(dll).as_ptr());
    if module.is_null() {
        return fail(3, format!("could not load {}: {}", dll.to_string_lossy(), io::Error::last_os_error()));
    }
    let gco = GetProcAddress(module, b"DllGetClassObject\0".as_ptr());
    if gco.is_null() {
        return fail(3, "the DLL has no DllGetClassObject".into());
    }
    let gco: DllGetClassObject = std::mem::transmute(gco);
    let mut factory = null_mut();
    let hr = gco(&CLSID_MUSIC_ANALYSIS2, &IID_ICLASS_FACTORY, &mut factory);
    if hr < 0 || factory.is_null() {
        return fail(3, format!("DllGetClassObject hr=0x{:08x} (not Sony's MusicAnalysis2 engine?)", hr as u32));
    }
    let create: unsafe extern "system" fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(slot(factory, CF_CREATE_INSTANCE));
    let mut ma = null_mut();
    let hr = create(factory, null_mut(), &IID_IMUSIC_ANALYSIS2, &mut ma);
    if hr < 0 || ma.is_null() {
        return fail(3, format!("CreateInstance hr=0x{:08x}", hr as u32));
    }

    let get_param: unsafe extern "system" fn(*mut c_void, i32, *mut Variant) -> HRESULT =
        std::mem::transmute(slot(ma, MA_GET_PARAMETER));
    let set_param: unsafe extern "system" fn(*mut c_void, i32, Variant) -> HRESULT =
        std::mem::transmute(slot(ma, MA_SET_PARAMETER));

    let mut version = Variant::empty();
    if get_param(ma, 3, &mut version) >= 0 && version.vt == VT_BSTR {
        let p = version.val[0] as *const u16;
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(p, SysStringLen(p) as usize));
        eprintln!("engine={text}");
    }
    VariantClear(&mut version);

    for &(id, value) in params {
        // Keep each parameter's own type; the engine's are VT_I4 or VT_BOOL.
        let mut cur = Variant::empty();
        let vt = if get_param(ma, id, &mut cur) >= 0 { cur.vt } else { VT_I4 };
        VariantClear(&mut cur);
        let mut v = Variant::empty();
        match vt {
            VT_BOOL => {
                v.vt = VT_BOOL;
                v.val[0] = if value != 0 { 0xffff } else { 0 };
            }
            VT_I2 => {
                v.vt = VT_I2;
                v.val[0] = u32::from(value as i16 as u16);
            }
            _ => {
                v.vt = VT_I4;
                v.val[0] = value as u32;
            }
        }
        let hr = set_param(ma, id, v);
        if hr < 0 {
            return fail(4, format!("SetParameter({id}, {value}) hr=0x{:08x}", hr as u32));
        }
        eprintln!("param{id}={value}");
    }

    let set_format: unsafe extern "system" fn(*mut c_void, u16, u32, u16) -> HRESULT =
        std::mem::transmute(slot(ma, MA_SET_INPUT_PCM_FORMAT));
    let run: unsafe extern "system" fn(*mut c_void) -> HRESULT = std::mem::transmute(slot(ma, MA_RUN));
    let input: unsafe extern "system" fn(*mut c_void, *const u8, u32, i32, i32) -> HRESULT =
        std::mem::transmute(slot(ma, MA_INPUT_PCM));
    let hr = set_format(ma, 2, 44100, 16);
    if hr < 0 {
        return fail(4, format!("SetInputPCMFormat hr=0x{:08x}", hr as u32));
    }
    let hr = run(ma);
    if hr < 0 {
        return fail(4, format!("Run hr=0x{:08x}", hr as u32));
    }

    // InputPCM needs to be told which call is the last, so read one buffer ahead. Whole frames only:
    // a pipe can deliver an odd byte count, and the engine counts in 4-byte stereo frames.
    let started = Instant::now();
    let mut stdin = io::stdin().lock();
    let mut pending: Vec<u8> = Vec::with_capacity(2 * FEED_BYTES);
    let mut buf = vec![0u8; FEED_BYTES];
    let mut eof = false;
    let mut total: u64 = 0;
    loop {
        while !eof && pending.len() < 2 * FEED_BYTES {
            match stdin.read(&mut buf) {
                Ok(0) => eof = true,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return fail(4, format!("reading PCM from stdin: {e}")),
            }
        }
        let last = eof && pending.len() <= FEED_BYTES;
        let take = if last { pending.len() - pending.len() % 4 } else { FEED_BYTES };
        if take == 0 && !last {
            continue;
        }
        let hr = input(ma, pending.as_ptr(), take as u32, i32::from(last), 0);
        if hr < 0 {
            // MMER_PCMTOOSHORT (2) arrives as a failure on very short input.
            return fail(4, format!("InputPCM hr=0x{:08x} after {} frames", hr as u32, total / 4));
        }
        total += take as u64;
        pending.drain(..take);
        if last {
            break;
        }
    }
    eprintln!("frames={}", total / 4);
    eprintln!("feed_ms={}", started.elapsed().as_millis());

    let get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(slot(ma, MA_GET_RESULT));
    let mut ar = null_mut();
    let hr = get_result(ma, &mut ar);
    if hr < 0 || ar.is_null() {
        return fail(4, format!("GetResult hr=0x{:08x}", hr as u32));
    }
    let by_id: unsafe extern "system" fn(*mut c_void, i32, *mut Variant) -> HRESULT =
        std::mem::transmute(slot(ar, AR_GET_RESULT_BY_ID));

    for id in 0..=LAST_INT_RESULT {
        let mut v = Variant::empty();
        if by_id(ar, id, &mut v) >= 0 && (v.vt == VT_INT || v.vt == VT_I4) {
            if id == 88 {
                eprintln!("result88={}", v.val[0] as i32);
            }
            eprintln!("r{id}={}", v.val[0] as i32);
        }
        VariantClear(&mut v);
    }

    let mut v = Variant::empty();
    let hr = by_id(ar, RESULT_SMFMF, &mut v);
    if hr < 0 || v.vt != VT_UI1_ARRAY || v.val[0] == 0 {
        return fail(4, format!("no SMFMF result (hr=0x{:08x}, vt=0x{:x})", hr as u32, v.vt));
    }
    let sa = v.val[0] as *mut SafeArray;
    let mut data = null_mut();
    if SafeArrayAccessData(sa, &mut data) < 0 {
        return fail(4, "SafeArrayAccessData failed".into());
    }
    let bytes = std::slice::from_raw_parts(data as *const u8, ((*sa).count * (*sa).elem_size) as usize).to_vec();
    SafeArrayUnaccessData(sa);
    VariantClear(&mut v);
    eprintln!("bytes={}", bytes.len());

    release(ar);
    release(ma);
    release(factory);

    let mut out = io::stdout().lock();
    if let Err(e) = out.write_all(&bytes).and_then(|_| out.flush()) {
        return fail(4, format!("writing the result: {e}"));
    }
    0
}
