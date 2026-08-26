// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! libretro front-end ABI for nes-core (FamiRust).
//!
//! Exposes the standard `retro_*` C entry points so the NES core loads in any
//! libretro host (RetroArch, Trophy Hub's libretro host). Each `retro_run`
//! advances the machine one frame, presents the PPU framebuffer as XRGB8888
//! (256x240), and streams the APU's 48 kHz audio to the host. Controller 1 is a
//! RetroPad mapped to the NES pad. Save-states use the core's byte-identical,
//! platform-agnostic serialization (`retro_serialize`/`retro_unserialize`).

#![allow(non_camel_case_types)]
#![allow(clippy::missing_safety_doc)]
// The `retro_*` entry points are `pub extern "C"` but take/return the libretro
// C-ABI structs, which are intentionally module-private (the ABI is the C layout,
// not Rust visibility). Silence the private-interface lint for the whole crate.
#![allow(private_interfaces)]

use nes_core::controller::button;
use nes_core::Nes;
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void};
use std::ptr;

// --- libretro C types we need -------------------------------------------------

type retro_environment_t = Option<unsafe extern "C" fn(u32, *mut c_void) -> bool>;
type retro_video_refresh_t = Option<unsafe extern "C" fn(*const c_void, u32, u32, usize)>;
type retro_log_printf_t = Option<unsafe extern "C" fn(u32, *const c_char, ...)>;

#[repr(C)]
struct retro_log_callback {
    log: retro_log_printf_t,
}
const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: u32 = 27;
const RETRO_LOG_INFO: u32 = 1;
type retro_audio_sample_batch_t = Option<unsafe extern "C" fn(*const i16, usize) -> usize>;
type retro_input_poll_t = Option<unsafe extern "C" fn()>;
type retro_input_state_t = Option<unsafe extern "C" fn(u32, u32, u32, u32) -> i16>;

#[repr(C)]
struct retro_system_info {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
struct retro_game_geometry {
    base_width: u32,
    base_height: u32,
    max_width: u32,
    max_height: u32,
    aspect_ratio: f32,
}

#[repr(C)]
struct retro_system_timing {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
struct retro_system_av_info {
    geometry: retro_game_geometry,
    timing: retro_system_timing,
}

#[repr(C)]
struct retro_game_info {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 10;
const RETRO_PIXEL_FORMAT_XRGB8888: i32 = 1;

/// The frontend gives us the path to its `system/` directory here; the FDS BIOS
/// (`disksys.rom`) lives there (it is copyrighted, so it is never shipped in the
/// core, exactly like every other console BIOS).
const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: u32 = 9;

/// Classic disk-control interface: how a frontend drives the FDS disk/side swap
/// (Trophy Hub's "SIDE" button). Registered only when an FDS disk is loaded.
const RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE: u32 = 13;

#[repr(C)]
struct retro_disk_control_callback {
    set_eject_state: Option<unsafe extern "C" fn(bool) -> bool>,
    get_eject_state: Option<unsafe extern "C" fn() -> bool>,
    get_image_index: Option<unsafe extern "C" fn() -> u32>,
    set_image_index: Option<unsafe extern "C" fn(u32) -> bool>,
    get_num_images: Option<unsafe extern "C" fn() -> u32>,
    replace_image_index: Option<unsafe extern "C" fn(u32, *const retro_game_info) -> bool>,
    add_image_index: Option<unsafe extern "C" fn() -> bool>,
}

const RETRO_DEVICE_JOYPAD: u32 = 1;

/// RetroPad button id -> NES pad bit (see `nes_core::controller::button`).
const NES_MAP: &[(u32, u8)] = &[
    (0, button::B),      // RetroPad B -> NES B
    (8, button::A),      // RetroPad A -> NES A
    (2, button::SELECT), // Select
    (3, button::START),  // Start
    (4, button::UP),     // Up
    (5, button::DOWN),   // Down
    (6, button::LEFT),   // Left
    (7, button::RIGHT),  // Right
];

const BASE_W: u32 = 256;
const BASE_H: u32 = 240;
const FB_LEN: usize = (BASE_W * BASE_H) as usize;

/// Gain applied to the mixer's ~0..0.25 float output before quantizing to i16.
const AUDIO_GAIN: f32 = 3.0;

// --- Core state ---------------------------------------------------------------

struct State {
    core: Option<Nes>,
    rom: Vec<u8>, // kept so retro_reset can rebuild the machine
    // FDS: the 8 KiB BIOS and disk-control state. `is_fds` selects the FDS build
    // path in retro_reset; `bios` is empty for cartridges.
    is_fds: bool,
    bios: Vec<u8>,
    disk_index: u32,
    disk_ejected: bool,
    frame: Vec<u32>,
    audio: Vec<i16>, // scratch: interleaved stereo i16 for the host
    env: retro_environment_t,
    video: retro_video_refresh_t,
    audio_batch: retro_audio_sample_batch_t,
    input_poll: retro_input_poll_t,
    input_state: retro_input_state_t,
    log: retro_log_printf_t,
    last_buttons: u8,
}

impl State {
    const fn new() -> State {
        State {
            core: None,
            rom: Vec::new(),
            is_fds: false,
            bios: Vec::new(),
            disk_index: 0,
            disk_ejected: false,
            frame: Vec::new(),
            audio: Vec::new(),
            env: None,
            video: None,
            audio_batch: None,
            input_poll: None,
            input_state: None,
            log: None,
            last_buttons: 0,
        }
    }
}

fn logline(s: &State, msg: &str) {
    if let (Some(log), Ok(c)) = (s.log, std::ffi::CString::new(msg)) {
        unsafe { log(RETRO_LOG_INFO, c"[FamiRust] %s\n".as_ptr(), c.as_ptr()) };
    }
}

/// Trophy Hub's libretro host registers callbacks on its main thread but runs
/// frames on a dedicated emulation thread, so state is one process-global,
/// mirroring how C libretro cores keep state in plain `static`s.
struct GlobalState(UnsafeCell<State>);

// SAFETY: libretro serializes every call into the core, so there is never
// concurrent access to the single STATE instance.
unsafe impl Sync for GlobalState {}

static STATE: GlobalState = GlobalState(UnsafeCell::new(State::new()));

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    // SAFETY: see `GlobalState` -- accesses are serialized by the frontend.
    unsafe { f(&mut *STATE.0.get()) }
}

/// Ask the frontend for its `system/` directory (where `disksys.rom` lives).
fn system_directory(env: retro_environment_t) -> Option<String> {
    let env = env?;
    let mut path: *const c_char = ptr::null();
    let ok = unsafe {
        env(RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY, &mut path as *mut _ as *mut c_void)
    };
    if !ok || path.is_null() {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(path) };
    s.to_str().ok().map(|s| s.to_string())
}

/// Load the FDS BIOS (`disksys.rom`) from the frontend's system directory.
fn load_fds_bios(env: retro_environment_t) -> Option<Vec<u8>> {
    let dir = system_directory(env)?;
    // Accept it at the system root or in an `fds/` subfolder; case-insensitive
    // names are left to the frontend's filesystem.
    for cand in ["disksys.rom", "fds/disksys.rom", "FDS/disksys.rom"] {
        let p = std::path::Path::new(&dir).join(cand);
        if let Ok(b) = std::fs::read(&p) {
            if b.len() >= nes_core::fds::BIOS_LEN {
                return Some(b);
            }
        }
    }
    None
}

// --- Disk control (FDS side/disk swap; Trophy Hub's "SIDE" button) -----------
// The classic interface has no user-data pointer, so the callbacks reach the
// single global STATE directly, like a C core's file-scope statics. Frontends
// swap a disk by: set_eject_state(true) -> set_image_index(n) -> set_eject_state(false).

unsafe extern "C" fn disk_set_eject_state(ejected: bool) -> bool {
    with_state(|s| {
        s.disk_ejected = ejected;
        let idx = s.disk_index;
        if let Some(core) = &mut s.core {
            if ejected {
                core.fds_eject();
            } else {
                core.fds_insert_side(idx as usize);
            }
        }
        logline(s, &format!("disk eject={ejected} -> side {idx}"));
        true
    })
}
unsafe extern "C" fn disk_get_eject_state() -> bool {
    with_state(|s| s.disk_ejected)
}
unsafe extern "C" fn disk_get_image_index() -> u32 {
    with_state(|s| s.disk_index)
}
unsafe extern "C" fn disk_set_image_index(index: u32) -> bool {
    with_state(|s| {
        let n = s.core.as_ref().map(|c| c.fds_side_count()).unwrap_or(0) as u32;
        if index >= n {
            return false;
        }
        s.disk_index = index;
        // If the disk is currently inserted, apply the new side immediately (some
        // frontends set the index without an explicit eject cycle).
        if !s.disk_ejected {
            if let Some(core) = &mut s.core {
                core.fds_insert_side(index as usize);
            }
        }
        true
    })
}
unsafe extern "C" fn disk_get_num_images() -> u32 {
    with_state(|s| s.core.as_ref().map(|c| c.fds_side_count() as u32).unwrap_or(0))
}
unsafe extern "C" fn disk_replace_image_index(_index: u32, _info: *const retro_game_info) -> bool {
    false // fixed-size disk set; we do not support hot-replacing an image
}
unsafe extern "C" fn disk_add_image_index() -> bool {
    false
}

/// Register the disk-control interface with the frontend (FDS only).
fn register_disk_control(env: retro_environment_t) {
    let Some(env) = env else { return };
    let mut cb = retro_disk_control_callback {
        set_eject_state: Some(disk_set_eject_state),
        get_eject_state: Some(disk_get_eject_state),
        get_image_index: Some(disk_get_image_index),
        set_image_index: Some(disk_set_image_index),
        get_num_images: Some(disk_get_num_images),
        replace_image_index: Some(disk_replace_image_index),
        add_image_index: Some(disk_add_image_index),
    };
    unsafe {
        env(
            RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE,
            &mut cb as *mut _ as *mut c_void,
        );
    }
}

// --- Required libretro entry points ------------------------------------------

#[no_mangle]
pub extern "C" fn retro_api_version() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn retro_init() {
    with_state(|s| s.frame = vec![0u32; FB_LEN]);
}

#[no_mangle]
pub extern "C" fn retro_deinit() {
    with_state(|s| *s = State::new());
}

#[no_mangle]
pub unsafe extern "C" fn retro_get_system_info(info: *mut retro_system_info) {
    if info.is_null() {
        return;
    }
    (*info).library_name = c"FamiRust".as_ptr();
    (*info).library_version = c"0.2.2".as_ptr();
    (*info).valid_extensions = c"nes|fds".as_ptr();
    (*info).need_fullpath = false;
    (*info).block_extract = false;
}

#[no_mangle]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut retro_system_av_info) {
    if info.is_null() {
        return;
    }
    (*info).geometry = retro_game_geometry {
        base_width: BASE_W,
        base_height: BASE_H,
        max_width: BASE_W,
        max_height: BASE_H,
        aspect_ratio: 4.0 / 3.0,
    };
    (*info).timing = retro_system_timing {
        fps: 60.0988, // NTSC NES
        sample_rate: nes_core::SAMPLE_RATE as f64,
    };
}

#[no_mangle]
pub extern "C" fn retro_set_environment(cb: retro_environment_t) {
    with_state(|s| {
        s.env = cb;
        if let Some(env) = cb {
            let mut lc = retro_log_callback { log: None };
            let ok = unsafe {
                env(RETRO_ENVIRONMENT_GET_LOG_INTERFACE, &mut lc as *mut _ as *mut c_void)
            };
            if ok {
                s.log = lc.log;
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn retro_set_video_refresh(cb: retro_video_refresh_t) {
    with_state(|s| s.video = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_audio_sample(_cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn retro_set_audio_sample_batch(cb: retro_audio_sample_batch_t) {
    with_state(|s| s.audio_batch = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_input_poll(cb: retro_input_poll_t) {
    with_state(|s| s.input_poll = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_input_state(cb: retro_input_state_t) {
    with_state(|s| s.input_state = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_controller_port_device(_port: u32, _device: u32) {}

#[no_mangle]
pub extern "C" fn retro_reset() {
    with_state(|s| {
        if s.rom.is_empty() {
            return;
        }
        s.core = if s.is_fds {
            Nes::from_fds(&s.rom, &s.bios).ok()
        } else {
            Nes::from_rom(&s.rom).ok()
        };
        // A reset re-inserts side 0 (the drive powers up on the first side).
        if s.is_fds {
            s.disk_index = 0;
            s.disk_ejected = false;
        }
    });
}

#[no_mangle]
pub unsafe extern "C" fn retro_load_game(info: *const retro_game_info) -> bool {
    if info.is_null() || (*info).data.is_null() {
        return false;
    }
    let rom = std::slice::from_raw_parts((*info).data as *const u8, (*info).size).to_vec();
    with_state(|s| {
        if let Some(env) = s.env {
            let mut fmt = RETRO_PIXEL_FORMAT_XRGB8888;
            unsafe {
                env(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, &mut fmt as *mut i32 as *mut c_void);
            }
        }
        // FDS disk image: not a cartridge. It needs the 8 KiB BIOS from the
        // frontend's system directory, and it exposes disk/side swapping.
        if Nes::is_fds(&rom) {
            let Some(bios) = load_fds_bios(s.env) else {
                logline(s, "FDS load failed: disksys.rom not found in system dir");
                return false;
            };
            return match Nes::from_fds(&rom, &bios) {
                Ok(core) => {
                    let sides = core.fds_side_count();
                    s.rom = rom;
                    s.bios = bios;
                    s.is_fds = true;
                    s.disk_index = 0;
                    s.disk_ejected = false;
                    s.core = Some(core);
                    register_disk_control(s.env);
                    logline(s, &format!("loaded FDS disk, {sides} side(s)"));
                    true
                }
                Err(e) => {
                    logline(s, &format!("FDS load failed: {e:?}"));
                    false
                }
            };
        }
        match Nes::from_rom(&rom) {
            Ok(core) => {
                s.rom = rom;
                s.is_fds = false;
                s.core = Some(core);
                logline(s, &format!("loaded ROM, {} bytes", s.rom.len()));
                true
            }
            Err(e) => {
                logline(s, &format!("load failed: {e:?}"));
                false
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn retro_unload_game() {
    with_state(|s| s.core = None);
}

#[no_mangle]
pub extern "C" fn retro_run() {
    with_state(|s| {
        if let Some(poll) = s.input_poll {
            unsafe { poll() };
        }
        if let (Some(core), Some(input)) = (&mut s.core, s.input_state) {
            // BOTH pads. The core has always had two ($4016 -> controllers[0],
            // $4017 -> controllers[1], both strobed together and both saved), but
            // this poll used to read port 0 only, so player 2 was dead in every
            // frontend -- including netplay, where the joiner's input arrives on
            // libretro port 1 and was silently discarded here.
            for port in 0..2u32 {
                let mut buttons = 0u8;
                for &(id, bit) in NES_MAP {
                    if unsafe { input(port, RETRO_DEVICE_JOYPAD, 0, id) } != 0 {
                        buttons |= bit;
                    }
                }
                core.set_buttons(port as usize, buttons);
                if port == 0 {
                    s.last_buttons = buttons;
                }
            }
        }

        if let Some(core) = &mut s.core {
            let f = core.step_frame();
            let n = f.len().min(s.frame.len());
            s.frame[..n].copy_from_slice(&f[..n]);

            // Mono APU output -> interleaved stereo i16 for the host.
            let pcm = core.take_audio();
            s.audio.clear();
            s.audio.reserve(pcm.len() * 2);
            for &sample in &pcm {
                let v = (sample * AUDIO_GAIN).clamp(-1.0, 1.0);
                let i = (v * 32767.0) as i16;
                s.audio.push(i);
                s.audio.push(i);
            }
        }

        if let (Some(batch), false) = (s.audio_batch, s.audio.is_empty()) {
            unsafe { batch(s.audio.as_ptr(), s.audio.len() / 2) };
        }
        if let Some(video) = s.video {
            unsafe {
                video(
                    s.frame.as_ptr() as *const c_void,
                    BASE_W,
                    BASE_H,
                    BASE_W as usize * 4,
                );
            }
        }
    });
}

// --- Save RAM / save-states ---------------------------------------------------

// libretro memory ids (see libretro.h). RetroAchievements reads the NES memory
// map from SYSTEM_RAM (the 2 KiB internal RAM) and SAVE_RAM (the cart work RAM).
const RETRO_MEMORY_SAVE_RAM: u32 = 0;
const RETRO_MEMORY_SYSTEM_RAM: u32 = 2;

#[no_mangle]
pub extern "C" fn retro_get_memory_data(id: u32) -> *mut c_void {
    with_state(|s| {
        let Some(core) = &mut s.core else { return ptr::null_mut() };
        match id {
            RETRO_MEMORY_SYSTEM_RAM => core.system_ram().as_mut_ptr() as *mut c_void,
            RETRO_MEMORY_SAVE_RAM => core
                .save_ram()
                .map(|r| r.as_mut_ptr() as *mut c_void)
                .unwrap_or(ptr::null_mut()),
            _ => ptr::null_mut(),
        }
    })
}
#[no_mangle]
pub extern "C" fn retro_get_memory_size(id: u32) -> usize {
    with_state(|s| {
        let Some(core) = &mut s.core else { return 0 };
        match id {
            RETRO_MEMORY_SYSTEM_RAM => core.system_ram().len(),
            RETRO_MEMORY_SAVE_RAM => core.save_ram().map(|r| r.len()).unwrap_or(0),
            _ => 0,
        }
    })
}
// --- Debug harness (see FamiRust docs/DEBUG_HARNESS.md) ----------------------
// Custom exports the app's native bridge calls (it already dlopen's the .so).
// The app owns PNG encoding / gallery / share; the core hands over decoded
// pixels + structured data. Buffers are app-allocated (sizes below).

/// Turn per-layer capture on/off. Off in normal play (zero cost); the frontend
/// enables it while the Core Debug overlay is open.
#[no_mangle]
pub extern "C" fn nes_dbg_enable(on: u32) {
    with_state(|s| {
        if let Some(c) = &mut s.core {
            c.dbg_set_capture(on != 0);
        }
    });
}
/// Composite-layer mask for live toggles: bit0=BG, bit1=OBJ (0b11 = normal).
#[no_mangle]
pub extern "C" fn nes_dbg_set_layer_mask(mask: u32) {
    with_state(|s| {
        if let Some(c) = &mut s.core {
            c.dbg_set_layer_mask(mask as u8);
        }
    });
}
/// Copy a layer of the LAST rendered frame into `out` (256*240 XRGB8888).
/// which: 0 = BG-only (backdrop fill), 1 = OBJ-only (magenta = transparent).
#[no_mangle]
pub unsafe extern "C" fn nes_dbg_layer(which: u32, out: *mut u32) {
    if out.is_null() {
        return;
    }
    with_state(|s| {
        let Some(c) = &s.core else { return };
        let src = if which == 1 { c.dbg_obj_layer() } else { c.dbg_bg_layer() };
        ptr::copy_nonoverlapping(src.as_ptr(), out, src.len());
    });
}
/// Copy the pattern-table tile sheet into `out` (128*256 XRGB8888).
#[no_mangle]
pub unsafe extern "C" fn nes_dbg_tiles(out: *mut u32) {
    if out.is_null() {
        return;
    }
    with_state(|s| {
        let Some(c) = &mut s.core else { return };
        let t = c.dbg_tiles_argb();
        ptr::copy_nonoverlapping(t.as_ptr(), out, t.len());
    });
}
/// Copy the 32 palette entries into `out` (32 XRGB8888).
#[no_mangle]
pub unsafe extern "C" fn nes_dbg_palette(out: *mut u32) {
    if out.is_null() {
        return;
    }
    with_state(|s| {
        let Some(c) = &s.core else { return };
        let p = c.dbg_palette_argb();
        ptr::copy_nonoverlapping(p.as_ptr(), out, p.len());
    });
}
/// Write OAM as a JSON array into `buf` (UTF-8, up to `cap` bytes). Returns the
/// number of bytes written (0 if no core; may truncate if `cap` too small).
#[no_mangle]
pub unsafe extern "C" fn nes_dbg_oam_json(buf: *mut u8, cap: u32) -> u32 {
    if buf.is_null() {
        return 0;
    }
    with_state(|s| {
        let Some(c) = &s.core else { return 0 };
        let j = c.dbg_oam_json();
        let n = j.len().min(cap as usize);
        ptr::copy_nonoverlapping(j.as_ptr(), buf, n);
        n as u32
    })
}

#[no_mangle]
pub extern "C" fn retro_serialize_size() -> usize {
    with_state(|s| s.core.as_ref().map(|c| c.state_size()).unwrap_or(0))
}
#[no_mangle]
pub unsafe extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    with_state(|s| {
        let Some(core) = &s.core else { return false };
        let bytes = core.save_state();
        if data.is_null() || size < bytes.len() {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
        true
    })
}
#[no_mangle]
pub unsafe extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    with_state(|s| {
        let Some(core) = &mut s.core else { return false };
        if data.is_null() {
            return false;
        }
        let slice = std::slice::from_raw_parts(data as *const u8, size);
        // The platform-agnostic loader rejects wrong-magic / newer-version /
        // truncated / oversized input cleanly; a bad state fails, no crash.
        core.load_state(slice).is_ok()
    })
}
#[no_mangle]
pub extern "C" fn retro_cheat_reset() {}
#[no_mangle]
pub extern "C" fn retro_cheat_set(_index: u32, _enabled: bool, _code: *const c_char) {}
#[no_mangle]
pub unsafe extern "C" fn retro_load_game_special(
    _game_type: u32,
    _info: *const retro_game_info,
    _num: usize,
) -> bool {
    false
}
#[no_mangle]
pub extern "C" fn retro_get_region() -> u32 {
    0 // RETRO_REGION_NTSC
}
