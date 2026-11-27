//! `amd_fidelityfx_loader_xbox.dll`: the upscale dispatch.
//!
//! The title's Series S mode renders below its output size and calls the
//! console's FidelityFX upscaler to make the output. The loader and the
//! upscaler are the title's own DLLs and run as they are, but the
//! upscaler's passes are compute pipelines in a container we do not
//! translate: its output stayed black. `ffxDispatch` alone is taken here:
//! an upscale dispatch becomes a plain stretch of the colour input
//! (`victory_gpu::list::upscale_stretch`); anything else, and an upscale we
//! could not stretch, goes to the title's DLL.
//!
//! The structures are the public FidelityFX API's (`ffx_api.h`,
//! `ffx_upscale.h`): a header of a type and a next pointer, and resources of
//! a pointer, a 32-byte description and a state.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};


/// The DLL whose `ffxDispatch` is taken.
pub const LOADER: &str = "amd_fidelityfx_loader_xbox.dll";

static REAL: AtomicUsize = AtomicUsize::new(0);

/// The title's own `ffxDispatch`, for what is not stretched here.
pub fn set_real_dispatch(address: usize) {
    REAL.store(address, Ordering::Relaxed);
}

#[repr(C)]
struct Header {
    kind: u64,
    next: *const Header,
}

#[repr(C)]
struct Resource {
    resource: *const c_void,
    description: [u32; 8],
    state: u32,
}

#[repr(C)]
struct Upscale {
    header: Header,
    list: *const c_void,
    colour: Resource,
    depth: Resource,
    motion: Resource,
    exposure: Resource,
    reactive: Resource,
    composition: Resource,
    output: Resource,
    jitter: [f32; 2],
    motion_scale: [f32; 2],
    render: [u32; 2],
    upscale: [u32; 2],
    // The rest of `ffxDispatchDescUpscale` (`ffx_upscale.h`), read by the
    // probe only.
    enable_sharpening: u8,
    sharpness: f32,
    frame_time_delta: f32,
    pre_exposure: f32,
    reset: u8,
    camera_near: f32,
    camera_far: f32,
    camera_fov: f32,
    view_to_meters: f32,
    flags: u32,
}

/// `VICTORY_UPSCALE_PROBE=1`: logs what the title gives each upscale (the
/// inputs a temporal upscaler such as DLSS or FSR 2 needs), now and then.
fn probe() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_UPSCALE_PROBE").is_some_and(|v| v == "1"))
}

const _: () = assert!(size_of::<Resource>() == 48);
const _: () = assert!(std::mem::offset_of!(Upscale, output) == 0x138);

/// `FFX_API_RETURN_ERROR`: what a dispatch nothing could answer returns.
const ERROR: u32 = 1;

/// `FFX_API_DISPATCH_DESC_TYPE_UPSCALE`.
const UPSCALE: u64 = 0x0001_0001;

type Dispatch = extern "system" fn(*mut *mut c_void, *const Header) -> u32;

extern "system" fn dispatch(context: *mut *mut c_void, desc: *const Header) -> u32 {
    // SAFETY: the title passes a dispatch description, whose header says
    // what follows it.
    if let Some(header) = unsafe { desc.as_ref() } {
        static SEEN: AtomicU32 = AtomicU32::new(0);
        let first = SEEN.fetch_add(1, Ordering::Relaxed) < 4;
        if header.kind == UPSCALE {
            // SAFETY: an upscale description, by its type.
            let d = unsafe { &*desc.cast::<Upscale>() };
            if first {
                tracing::info!(
                    list = ?d.list,
                    colour = ?d.colour.resource,
                    colour_size = ?(d.colour.description[2], d.colour.description[3]),
                    output = ?d.output.resource,
                    output_size = ?(d.output.description[2], d.output.description[3]),
                    render = ?d.render,
                    upscale = ?d.upscale,
                    "upscale dispatch"
                );
            }
            #[cfg(windows)]
            if probe() {
                static N: AtomicU32 = AtomicU32::new(0);
                let n = N.fetch_add(1, Ordering::Relaxed);
                // the first 12 upscales, then one in 300
                if n < 12 || n.is_multiple_of(300) {
                    let inputs = victory_gpu::list::upscale_inputs(&[
                        d.colour.resource,
                        d.depth.resource,
                        d.motion.resource,
                        d.exposure.resource,
                        d.reactive.resource,
                        d.composition.resource,
                        d.output.resource,
                    ]);
                    tracing::info!(
                        n,
                        colour = %inputs[0],
                        depth = %inputs[1],
                        motion = %inputs[2],
                        exposure = %inputs[3],
                        reactive = %inputs[4],
                        composition = %inputs[5],
                        output = %inputs[6],
                        jitter = ?d.jitter,
                        motion_scale = ?d.motion_scale,
                        render = ?d.render,
                        upscale = ?d.upscale,
                        sharpening = d.enable_sharpening,
                        sharpness = d.sharpness,
                        frame_time_delta = d.frame_time_delta,
                        pre_exposure = d.pre_exposure,
                        reset = d.reset,
                        near = d.camera_near,
                        far = d.camera_far,
                        fov = d.camera_fov,
                        meters = d.view_to_meters,
                        flags = format_args!("{:#x}", d.flags),
                        "upscale probe"
                    );
                }
            }
            #[cfg(windows)]
            {
                let up = victory_gpu::list::Upscale {
                    colour: d.colour.resource as u64,
                    depth: d.depth.resource as u64,
                    motion: d.motion.resource as u64,
                    exposure: d.exposure.resource as u64,
                    output: d.output.resource as u64,
                    render: d.render,
                    jitter: d.jitter,
                    motion_scale: d.motion_scale,
                    reset: d.reset != 0,
                    pre_exposure: d.pre_exposure,
                    near: d.camera_near,
                    far: d.camera_far,
                    frame_ms: d.frame_time_delta,
                    dlss: std::env::var_os("VICTORY_UPSCALE").is_some_and(|v| v == "dlss"),
                };
                if victory_gpu::list::upscale(d.list, &up) {
                    return 0;
                }
            }
            if first {
                tracing::warn!("upscale dispatch not stretched: left to the title's upscaler");
            }
        } else if first {
            tracing::info!(
                kind = format_args!("{:#x}", header.kind),
                "another FidelityFX dispatch"
            );
        }
    }
    match REAL.load(Ordering::Relaxed) {
        // Neither stretched nor passed on: the output was not written, and
        // the title is told so instead of presenting it as it is.
        0 => {
            static TOLD: std::sync::Once = std::sync::Once::new();
            TOLD.call_once(|| {
                tracing::error!(
                    "a FidelityFX dispatch was not answered: no stretch was recorded and the title's own ffxDispatch was not found"
                );
            });
            ERROR
        }
        // SAFETY: the title's own `ffxDispatch`, with its own arguments.
        real => unsafe { std::mem::transmute::<usize, Dispatch>(real)(context, desc) },
    }
}

/// `FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE`.
const CREATE_UPSCALE: u64 = 0x0001_0000;

/// `ffxCreateContextDescUpscale` (`ffx_upscale.h`).
#[repr(C)]
struct CreateUpscale {
    header: Header,
    flags: u32,
    max_render: [u32; 2],
    max_upscale: [u32; 2],
    message: *const c_void,
}

/// The title's own `ffxCreateContext`.
static REAL_CREATE: AtomicUsize = AtomicUsize::new(0);

/// The title's own `ffxCreateContext`, for [`create_context`] to call.
pub fn set_real_create(address: usize) {
    REAL_CREATE.store(address, Ordering::Relaxed);
}

type CreateContext = extern "system" fn(*mut *mut c_void, *const Header, *const c_void) -> u32;

/// The output size the title's shipped FSR4 accepts (its Series S tables
/// only): "Shipping FSR4 configuration mismatch: maxUpscaleSize must be
/// 1920x1080". LOCAL (claude/dlss-1440).
const SHIPPED_UPSCALE: [u32; 2] = [1920, 1080];

/// `ffxCreateContext` with the upscale's maximum sizes made the shipped
/// FSR4's (1920x1080 out, the render size scaled alike) while the title's
/// own call runs: on Series X (`VICTORY_FSR_UPSCALER=1`) the title asks for a
/// 3840x2160 output, which its FSR4 refuses, and the title then stops. The
/// dispatches are ours ([`dispatch`]), so FSR's own limits never apply to an
/// upscale; its queries (render size, jitter) are arithmetic on what they
/// are given. The description is put back as it was afterwards.
extern "system" fn create_context(context: *mut *mut c_void, desc: *const Header, memory: *const c_void) -> u32 {
    let real = REAL_CREATE.load(Ordering::Relaxed);
    if real == 0 {
        return ERROR;
    }
    let mut changed: Option<(*mut CreateUpscale, [u32; 2], [u32; 2])> = None;
    let mut at = desc;
    // SAFETY: the title's chain of descriptions, each naming its type.
    while let Some(h) = unsafe { at.as_ref() } {
        if h.kind == CREATE_UPSCALE {
            let up = at.cast::<CreateUpscale>().cast_mut();
            // SAFETY: an upscale description, by its type, in the title's memory.
            let d = unsafe { &mut *up };
            let (render, upscale) = (d.max_render, d.max_upscale);
            if upscale[0] > SHIPPED_UPSCALE[0] || upscale[1] > SHIPPED_UPSCALE[1] {
                let f = (SHIPPED_UPSCALE[0] as f64 / upscale[0].max(1) as f64)
                    .min(SHIPPED_UPSCALE[1] as f64 / upscale[1].max(1) as f64);
                d.max_upscale = SHIPPED_UPSCALE;
                d.max_render = [
                    ((render[0] as f64 * f).round() as u32).max(1),
                    ((render[1] as f64 * f).round() as u32).max(1),
                ];
                tracing::info!(
                    asked_render = ?render,
                    asked_upscale = ?upscale,
                    given_render = ?d.max_render,
                    given_upscale = ?d.max_upscale,
                    "FidelityFX upscale context: made with the shipped FSR4's sizes (the upscales themselves are ours)"
                );
                changed = Some((up, render, upscale));
            }
            break;
        }
        at = h.next;
    }
    // SAFETY: the title's own `ffxCreateContext`, with its own arguments.
    let r = unsafe { std::mem::transmute::<usize, CreateContext>(real)(context, desc, memory) };
    if let Some((up, render, upscale)) = changed {
        // SAFETY: the same description, still the title's.
        unsafe {
            (*up).max_render = render;
            (*up).max_upscale = upscale;
        }
    }
    if r != 0 {
        tracing::warn!(result = r, "FidelityFX context creation failed");
    }
    r
}

/// The title's own `ffxQuery`.
static REAL_QUERY: AtomicUsize = AtomicUsize::new(0);

/// The title's own `ffxQuery`, for [`query`] to pass on what it does not answer.
pub fn set_real_query(address: usize) {
    REAL_QUERY.store(address, Ordering::Relaxed);
}

type Query = extern "system" fn(*mut *mut c_void, *mut Header) -> u32;

// `ffx_upscale.h` query descriptions.
const Q_RATIO: u64 = 0x0001_0002;
const Q_RENDER_SIZE: u64 = 0x0001_0003;
const Q_PHASE_COUNT: u64 = 0x0001_0004;
const Q_JITTER: u64 = 0x0001_0005;

#[repr(C)]
struct QRatio {
    header: Header,
    quality: u32,
    out_ratio: *mut f32,
}
#[repr(C)]
struct QRenderSize {
    header: Header,
    display: [u32; 2],
    quality: u32,
    out_width: *mut u32,
    out_height: *mut u32,
}
#[repr(C)]
struct QPhaseCount {
    header: Header,
    render_width: u32,
    display_width: u32,
    out_count: *mut i32,
}
#[repr(C)]
struct QJitter {
    header: Header,
    index: i32,
    phases: i32,
    out_x: *mut f32,
    out_y: *mut f32,
}

/// FSR's upscale ratio per quality mode (native AA, quality, balanced,
/// performance, ultra performance).
fn ratio(quality: u32) -> f32 {
    match quality {
        0 => 1.0,
        1 => 1.5,
        2 => 1.7,
        3 => 2.0,
        4 => 3.0,
        _ => 1.5,
    }
}

/// The Halton sequence FSR's jitter walks.
fn halton(mut index: i32, base: i32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// `ffxQuery`: the upscale's render size, ratio and jitter are answered here,
/// with FSR's own arithmetic (`ffx_api` FSR 2/3/4: ratio by quality mode,
/// `ceil(8 * ratio²)` phases, Halton(2, 3) offsets), since the title's context
/// was made with the shipped FSR4's 1080p sizes ([`create_context`]) and its
/// FSR would refuse a 4K display in a query. Anything else goes to the
/// title's own `ffxQuery`. LOCAL (claude/dlss-1440).
extern "system" fn query(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    static SEEN: AtomicU32 = AtomicU32::new(0);
    let first = SEEN.fetch_add(1, Ordering::Relaxed) < 8;
    // SAFETY: the title's query description, whose header says what follows.
    if let Some(h) = unsafe { desc.as_ref() } {
        // SAFETY (each arm): a description of the type its header names, with
        // the title's own out pointers.
        let answered = unsafe {
            match h.kind {
                Q_RATIO => {
                    let q = &*desc.cast::<QRatio>();
                    if let Some(o) = q.out_ratio.as_mut() {
                        *o = ratio(q.quality);
                    }
                    true
                }
                Q_RENDER_SIZE => {
                    let q = &*desc.cast::<QRenderSize>();
                    let r = ratio(q.quality);
                    let w = ((q.display[0] as f32 / r) as u32).max(1);
                    let hh = ((q.display[1] as f32 / r) as u32).max(1);
                    if let Some(o) = q.out_width.as_mut() {
                        *o = w;
                    }
                    if let Some(o) = q.out_height.as_mut() {
                        *o = hh;
                    }
                    if first {
                        tracing::info!(display = ?q.display, quality = q.quality, render = ?(w, hh), "FidelityFX query: render size (answered here)");
                    }
                    true
                }
                Q_PHASE_COUNT => {
                    let q = &*desc.cast::<QPhaseCount>();
                    let r = q.display_width as f32 / q.render_width.max(1) as f32;
                    if let Some(o) = q.out_count.as_mut() {
                        *o = (8.0 * r * r).ceil() as i32;
                    }
                    true
                }
                Q_JITTER => {
                    let q = &*desc.cast::<QJitter>();
                    let n = q.phases.max(1);
                    let i = q.index.rem_euclid(n) + 1;
                    if let Some(o) = q.out_x.as_mut() {
                        *o = halton(i, 2) - 0.5;
                    }
                    if let Some(o) = q.out_y.as_mut() {
                        *o = halton(i, 3) - 0.5;
                    }
                    true
                }
                _ => false,
            }
        };
        if answered {
            return 0;
        }
        if first {
            tracing::info!(kind = format_args!("{:#x}", h.kind), "FidelityFX query passed to the title's DLL");
        }
    }
    match REAL_QUERY.load(Ordering::Relaxed) {
        0 => ERROR,
        // SAFETY: the title's own `ffxQuery`, with its own arguments.
        real => unsafe { std::mem::transmute::<usize, Query>(real)(context, desc) },
    }
}

/// What the title's import of `ffxQuery` is bound to.
pub fn query_address() -> usize {
    query as *const () as usize
}

/// What the title's import of `ffxCreateContext` is bound to.
pub fn create_context_address() -> usize {
    create_context as *const () as usize
}

/// What the title's import of `ffxDispatch` is bound to. The DLL is not an
/// HLE module: the title's own must load, for its other entry points and
/// for the dispatches passed on.
pub fn dispatch_address() -> usize {
    dispatch as *const () as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// The upscale description as the public API lays it out: what the
    /// stretch reads sits where the title wrote it.
    #[test]
    fn the_upscale_description_is_laid_out_as_the_public_api() {
        assert_eq!(size_of::<Header>(), 16);
        assert_eq!(offset_of!(Upscale, list), 0x10);
        assert_eq!(offset_of!(Upscale, colour), 0x18);
        assert_eq!(offset_of!(Upscale, depth), 0x48);
        assert_eq!(offset_of!(Upscale, motion), 0x78);
        assert_eq!(offset_of!(Upscale, output), 0x138);
        assert_eq!(offset_of!(Upscale, jitter), 0x168);
        assert_eq!(offset_of!(Upscale, motion_scale), 0x170);
        assert_eq!(offset_of!(Upscale, render), 0x178);
        assert_eq!(offset_of!(Upscale, upscale), 0x180);
        assert_eq!(offset_of!(Upscale, enable_sharpening), 0x188);
        assert_eq!(offset_of!(Upscale, sharpness), 0x18c);
        assert_eq!(offset_of!(Upscale, reset), 0x198);
        assert_eq!(offset_of!(Upscale, camera_near), 0x19c);
        assert_eq!(offset_of!(Upscale, flags), 0x1ac);
        assert_eq!(offset_of!(Resource, description), 8);
        assert_eq!(offset_of!(Resource, state), 40);
    }

    /// A dispatch that is not an upscale, with no DLL of the title's to
    /// pass it to, is an error and not a silent success.
    #[test]
    fn a_dispatch_nothing_answers_is_an_error() {
        let before = REAL.swap(0, Ordering::Relaxed);
        let other = Header {
            kind: 0x0002_0001,
            next: std::ptr::null(),
        };
        assert_eq!(dispatch(std::ptr::null_mut(), &other), ERROR);
        assert_eq!(dispatch(std::ptr::null_mut(), std::ptr::null()), ERROR);
        REAL.store(before, Ordering::Relaxed);
    }
}
