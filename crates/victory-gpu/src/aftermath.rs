//! Diagnostics: NVIDIA Nsight Aftermath GPU crash dumps.
//!
//! `VICTORY_AFTERMATH=<path to GFSDK_Aftermath_Lib.x64.dll>` loads NVIDIA's
//! library at run time (nothing from NVIDIA is linked or shipped) and asks
//! it for a crash dump when the host device is lost. The dump (`.nv-gpudmp`,
//! opens in Nsight Graphics), its decoded JSON and the shaders' debug
//! information go to `VICTORY_AFTERMATH_DIR` (default `aftermath` in the
//! working directory). The JSON names the faulting shader, the fault type
//! (page fault or timeout) and, with resource tracking, the resource hit.
//!
//! Only in builds with the `diagnostics` feature, on NVIDIA hosts.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// Values of the SDK's GFSDK_Aftermath_Defines.h / GpuCrashDump.h /
// GpuCrashDumpDecoding.h (SDK 2025.x). `API_VERSION` must match the DLL.
const API_VERSION: u32 = 0x0000_0217;
const WATCHED_API_DX: u32 = 0x1;
const FEATURE_DEFER_DEBUG_INFO: u32 = 0x1;
const DX12_ENABLE_MARKERS: u32 = 0x1;
const DX12_ENABLE_RESOURCE_TRACKING: u32 = 0x2;
const DX12_GENERATE_SHADER_DEBUG_INFO: u32 = 0x8;
const DX12_ENABLE_SHADER_ERROR_REPORTING: u32 = 0x40;
const DECODER_ALL_INFO: u32 = 0xFFF;
const JSON_FORMAT_DEFAULT: u32 = 0;
const STATUS_FINISHED: u32 = 4;
const STATUS_FAILED: u32 = 2;
const RESULT_SUCCESS: u32 = 0x1;

type DumpCb = unsafe extern "C" fn(*const c_void, u32, *mut c_void);
type EnableFn = unsafe extern "C" fn(
    u32,
    u32,
    u32,
    Option<DumpCb>,
    Option<DumpCb>,
    *const c_void,
    *const c_void,
    *mut c_void,
) -> u32;
type Dx12InitFn = unsafe extern "C" fn(u32, u32, *mut c_void) -> u32;
type StatusFn = unsafe extern "C" fn(*mut u32) -> u32;
type CreateDecoderFn = unsafe extern "C" fn(u32, *const c_void, u32, *mut *mut c_void) -> u32;
type GenerateJsonFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    *const c_void,
    *const c_void,
    *const c_void,
    *mut c_void,
    *mut u32,
) -> u32;
type GetJsonFn = unsafe extern "C" fn(*mut c_void, u32, *mut u8) -> u32;
type DestroyDecoderFn = unsafe extern "C" fn(*mut c_void) -> u32;

struct Lib {
    dx12_init: Dx12InitFn,
    status: StatusFn,
    create_decoder: CreateDecoderFn,
    generate_json: GenerateJsonFn,
    get_json: GetJsonFn,
    destroy_decoder: DestroyDecoderFn,
    dir: PathBuf,
}

static LIB: OnceLock<Option<Lib>> = OnceLock::new();

fn lib() -> Option<&'static Lib> {
    LIB.get().and_then(Option::as_ref)
}

/// Loads the library and enables crash dumps; before the device exists.
pub(crate) fn enable() {
    LIB.get_or_init(|| {
        if !cfg!(feature = "diagnostics") {
            return None;
        }
        let path = std::env::var_os("VICTORY_AFTERMATH")?;
        let dir = std::env::var_os("VICTORY_AFTERMATH_DIR")
            .map_or_else(|| PathBuf::from("aftermath"), PathBuf::from);
        match load(&path, dir) {
            Ok(lib) => {
                tracing::info!(dir = %lib.dir.display(), "Aftermath GPU crash dumps on");
                Some(lib)
            }
            Err(e) => {
                tracing::warn!(error = %e, "VICTORY_AFTERMATH: not enabled");
                None
            }
        }
    });
}

fn load(path: &std::ffi::OsStr, dir: PathBuf) -> Result<Lib, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let wide: Vec<u16> = path.encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the library stays loaded for the run.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("cannot load {}", path.to_string_lossy()));
    }
    let sym = |name: &[u8]| {
        // SAFETY: a NUL-terminated export name of a loaded module.
        unsafe { GetProcAddress(module, name.as_ptr()) }
            .ok_or_else(|| String::from_utf8_lossy(&name[..name.len() - 1]).into_owned())
    };
    // SAFETY: the exports have these signatures (GFSDK_Aftermath headers).
    unsafe {
        let enable: EnableFn = func(sym(b"GFSDK_Aftermath_EnableGpuCrashDumps\0")?);
        let lib = Lib {
            dx12_init: func(sym(b"GFSDK_Aftermath_DX12_Initialize\0")?),
            status: func(sym(b"GFSDK_Aftermath_GetCrashDumpStatus\0")?),
            create_decoder: func(sym(b"GFSDK_Aftermath_GpuCrashDump_CreateDecoder\0")?),
            generate_json: func(sym(b"GFSDK_Aftermath_GpuCrashDump_GenerateJSON\0")?),
            get_json: func(sym(b"GFSDK_Aftermath_GpuCrashDump_GetJSON\0")?),
            destroy_decoder: func(sym(b"GFSDK_Aftermath_GpuCrashDump_DestroyDecoder\0")?),
            dir,
        };
        let r = enable(
            API_VERSION,
            WATCHED_API_DX,
            FEATURE_DEFER_DEBUG_INFO,
            Some(on_dump),
            Some(on_shader_debug_info),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if r != RESULT_SUCCESS {
            return Err(format!("GFSDK_Aftermath_EnableGpuCrashDumps: {r:#x}"));
        }
        Ok(lib)
    }
}

/// Markers, resource tracking and shader debug information on `device`.
pub(crate) fn attach(device: &windows::Win32::Graphics::Direct3D12::ID3D12Device) {
    let Some(lib) = lib() else { return };
    let flags = DX12_ENABLE_MARKERS
        | DX12_ENABLE_RESOURCE_TRACKING
        | DX12_GENERATE_SHADER_DEBUG_INFO
        | DX12_ENABLE_SHADER_ERROR_REPORTING;
    // SAFETY: a live device; Aftermath keeps its own reference.
    let r =
        unsafe { (lib.dx12_init)(API_VERSION, flags, windows::core::Interface::as_raw(device)) };
    if r != RESULT_SUCCESS {
        tracing::warn!(
            result = format_args!("{r:#x}"),
            "GFSDK_Aftermath_DX12_Initialize failed"
        );
    }
}

/// After the device is lost: waits (up to 30 s) until the dump is written.
pub(crate) fn wait_for_dump() {
    let Some(lib) = lib() else { return };
    let start = Instant::now();
    loop {
        let mut status = 0;
        // SAFETY: a plain query.
        unsafe { (lib.status)(&mut status) };
        if status == STATUS_FINISHED || status == STATUS_FAILED {
            tracing::info!(
                status,
                ms = start.elapsed().as_millis(),
                "Aftermath crash dump done"
            );
            return;
        }
        if start.elapsed() > Duration::from_secs(30) {
            tracing::warn!(status, "Aftermath crash dump not finished after 30 s");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// An export as its typed function pointer.
///
/// # Safety
/// `F` must be the export's real signature, a function pointer type.
unsafe fn func<F: Copy>(p: unsafe extern "system" fn() -> isize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>()) };
    // SAFETY: same-sized function pointers; the caller names the signature.
    unsafe { std::mem::transmute_copy(&p) }
}

fn stamp() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", t.as_secs(), std::process::id())
}

unsafe extern "C" fn on_dump(data: *const c_void, size: u32, _: *mut c_void) {
    let Some(lib) = lib() else { return };
    // SAFETY: Aftermath passes `size` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) };
    let base = lib.dir.join(format!("victory-{}", stamp()));
    let dump = base.with_extension("nv-gpudmp");
    if let Err(e) = std::fs::write(&dump, bytes) {
        tracing::warn!(error = %e, "Aftermath dump not written");
        return;
    }
    tracing::error!(path = %dump.display(), "Aftermath GPU crash dump written");
    // SAFETY: the decoder reads the dump bytes, which outlive it.
    let json = unsafe { decode(lib, data, size) };
    match json {
        Ok(json) => {
            let path = base.with_extension("json");
            let _ = std::fs::write(&path, &json);
            tracing::error!(path = %path.display(), "Aftermath crash dump decoded");
        }
        Err(e) => tracing::warn!(error = %e, "Aftermath crash dump not decoded"),
    }
}

unsafe fn decode(lib: &Lib, data: *const c_void, size: u32) -> Result<Vec<u8>, String> {
    let mut decoder = std::ptr::null_mut();
    // SAFETY: the caller's dump bytes; the decoder is destroyed below.
    unsafe {
        let r = (lib.create_decoder)(API_VERSION, data, size, &mut decoder);
        if r != RESULT_SUCCESS {
            return Err(format!("CreateDecoder: {r:#x}"));
        }
        let mut len = 0u32;
        let r = (lib.generate_json)(
            decoder,
            DECODER_ALL_INFO,
            JSON_FORMAT_DEFAULT,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut len,
        );
        let out = if r == RESULT_SUCCESS {
            let mut buf = vec![0u8; len as usize];
            let r = (lib.get_json)(decoder, len, buf.as_mut_ptr());
            if r == RESULT_SUCCESS {
                while buf.last() == Some(&0) {
                    buf.pop();
                }
                Ok(buf)
            } else {
                Err(format!("GetJSON: {r:#x}"))
            }
        } else {
            Err(format!("GenerateJSON: {r:#x}"))
        };
        (lib.destroy_decoder)(decoder);
        out
    }
}

unsafe extern "C" fn on_shader_debug_info(data: *const c_void, size: u32, _: *mut c_void) {
    let Some(lib) = lib() else { return };
    // SAFETY: Aftermath passes `size` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) };
    let path = lib
        .dir
        .join(format!("shader-{}-{}.nvdbg", stamp(), bytes.len()));
    let _ = std::fs::write(path, bytes);
}
