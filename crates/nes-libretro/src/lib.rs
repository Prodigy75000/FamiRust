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
    (*info).library_version = c"0.1.0".as_ptr();
    (*info).valid_extensions = c"nes".as_ptr();
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
        if !s.rom.is_empty() {
            s.core = Nes::from_rom(&s.rom).ok();
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
        match Nes::from_rom(&rom) {
            Ok(core) => {
                s.rom = rom;
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
            let mut buttons = 0u8;
            for &(id, bit) in NES_MAP {
                if unsafe { input(0, RETRO_DEVICE_JOYPAD, 0, id) } != 0 {
                    buttons |= bit;
                }
            }
            core.set_buttons(0, buttons);
            s.last_buttons = buttons;
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

#[no_mangle]
pub extern "C" fn retro_get_memory_data(_id: u32) -> *mut c_void {
    ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn retro_get_memory_size(_id: u32) -> usize {
    0
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
