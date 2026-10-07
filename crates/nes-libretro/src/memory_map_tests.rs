// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The core publishes its address space, and publishes it again after a reset.
//!
//! FamiRust published no memory map at all until 2026-09-30. A front-end with
//! no map falls back to laying the console's regions over whatever
//! `retro_get_memory_data` returns for SYSTEM_RAM, one after another. Ours is
//! the 2 KiB of internal RAM, so every region RetroAchievements wanted past the
//! first pointed at something that is not what the region says it is.
//!
//! This is the default NES core, so that reaches every NES player. The same
//! shape of bug on PocketRustAdvance awarded the owner three achievements he
//! had not earned, and a false unlock is written to an account server-side and
//! cannot be taken back.
//!
//! These are FFI tests: they drive the exported C entry points the way a
//! front-end does, because the defect lived exactly in the gap between what the
//! core believed it sent and what a front-end actually received.
//!
//! They live INSIDE the crate rather than in `tests/` on purpose. An
//! integration test needs the crate to publish an `rlib`, and adding `rlib`
//! beside `cdylib` turns LTO off for the shipped core: measured at 1,387,793
//! bytes against 577,690 for the same code. An integration test would link
//! that rlib rather than the DLL anyway, so it exercises exactly what these do
//! and costs 800 KB of shipped core to do it. Do not move this back.

use std::ffi::c_void;
use std::sync::Mutex;

const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;

#[repr(C)]
struct Descriptor {
    flags: u64,
    ptr: *mut c_void,
    offset: usize,
    start: usize,
    select: usize,
    disconnect: usize,
    len: usize,
    addrspace: *const std::os::raw::c_char,
}

#[repr(C)]
struct Map {
    descriptors: *const Descriptor,
    num_descriptors: std::os::raw::c_uint,
}

/// What the fake front-end saw: (flags, start, len, pointer).
static SEEN: Mutex<Vec<(u64, usize, usize, usize)>> = Mutex::new(Vec::new());
/// How many times a map arrived, so a missing republish is visible.
static TIMES: Mutex<u32> = Mutex::new(0);
/// The core keeps one global machine, so these tests cannot run beside each
/// other. Rust runs tests in the same binary on parallel threads by default.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A front-end that only understands the LITERAL command number.
///
/// This is the whole point of the test and the reason the bug survived
/// elsewhere. The previous test in the sibling core matched on the core's own
/// constant, so a wrong constant looked correct in the test and was never sent
/// to anything real. A test that agrees with the bug is worse than no test: it
/// is evidence of the wrong thing. `0x10024` is written out here on purpose and
/// must not be replaced by a symbol.
unsafe extern "C" fn front_end(cmd: u32, data: *mut c_void) -> bool {
    match cmd {
        0x1_0024 => {
            let map = &*(data as *const Map);
            let descs = std::slice::from_raw_parts(map.descriptors, map.num_descriptors as usize);
            *SEEN.lock().unwrap() = descs
                .iter()
                .map(|d| (d.flags, d.start, d.len, d.ptr as usize))
                .collect();
            *TIMES.lock().unwrap() += 1;
            true
        }
        // Pixel format and anything else: accept and ignore.
        _ => true,
    }
}

/// A minimal NROM cartridge: 32 KiB of PRG, 8 KiB of CHR, vectors in range.
fn a_cartridge() -> Vec<u8> {
    let mut rom = vec![0u8; 16 + 32 * 1024 + 8 * 1024];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = 2; // 32 KiB PRG
    rom[5] = 1; // 8 KiB CHR
    // RESET vector at $FFFC points at $8000, which holds an endless loop.
    let prg = 16;
    rom[prg] = 0x4c; // jmp $8000
    rom[prg + 1] = 0x00;
    rom[prg + 2] = 0x80;
    let vectors = prg + 32 * 1024 - 6;
    rom[vectors] = 0x00; // NMI
    rom[vectors + 1] = 0x80;
    rom[vectors + 2] = 0x00; // RESET
    rom[vectors + 3] = 0x80;
    rom[vectors + 4] = 0x00; // IRQ
    rom[vectors + 5] = 0x80;
    rom
}

#[repr(C)]
struct GameInfo {
    path: *const std::os::raw::c_char,
    data: *const c_void,
    size: usize,
    meta: *const std::os::raw::c_char,
}

/// Load a cartridge through the C entry points, as a front-end would.
fn load(rom: &[u8]) {
    SEEN.lock().unwrap().clear();
    *TIMES.lock().unwrap() = 0;
    unsafe {
        crate::retro_set_environment(Some(front_end));
        crate::retro_init();
        let info = GameInfo {
            path: std::ptr::null(),
            data: rom.as_ptr() as *const c_void,
            size: rom.len(),
            meta: std::ptr::null(),
        };
        assert!(
            crate::retro_load_game(&info as *const GameInfo as *const _),
            "the test cartridge should load"
        );
    }
}

#[test]
fn the_core_publishes_a_memory_map_at_all() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    load(&a_cartridge());
    let seen = SEEN.lock().unwrap();
    assert!(
        !seen.is_empty(),
        "no memory map reached the front-end. Without one, RetroAchievements \
         lays its regions over SYSTEM_RAM and everything past the first is wrong."
    );
}

#[test]
fn the_first_region_is_the_internal_ram_at_zero() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    load(&a_cartridge());
    let seen = SEEN.lock().unwrap();
    let (flags, start, len, ptr) = seen[0];
    assert_eq!(flags, RETRO_MEMDESC_SYSTEM_RAM, "flags");
    assert_eq!(start, 0x0000, "the NES internal RAM starts at $0000");
    assert_eq!(
        len, 0x800,
        "2 KiB, not the 8 KiB the mirrors make it look like: a region four \
         times the size of the RAM behind it reads the same byte at four \
         addresses and calls them different things"
    );
    assert_ne!(ptr, 0, "the descriptor must point at real memory");
}

#[test]
fn a_cart_without_work_ram_publishes_one_descriptor_and_that_is_correct() {
    // Contra is UNROM (mapper 2), which has no PRG-RAM at $6000, so one
    // descriptor is the right answer and a second would be a lie about the
    // board. Recorded because the prediction that went to the front-end agent
    // said two, and a wrong expectation about a cartridge is exactly the kind
    // of thing that gets a correct core "fixed".
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    load(&a_cartridge());
    let seen = SEEN.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "a cartridge with no work RAM has exactly one region to describe"
    );
}

#[test]
fn the_map_is_published_again_after_a_reset() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // The pointers are into the machine and `retro_reset` builds a new one, so
    // a map published only at load points into a freed machine afterwards.
    // rc_client resets the core itself when hardcore mode is toggled, which is
    // how RetroAchievements can trigger the corruption it then reads.
    load(&a_cartridge());
    let first = SEEN.lock().unwrap().clone();
    assert_eq!(*TIMES.lock().unwrap(), 1);

    crate::retro_reset();

    assert_eq!(
        *TIMES.lock().unwrap(),
        2,
        "the map was not republished after a reset"
    );
    let second = SEEN.lock().unwrap().clone();
    assert_eq!(first.len(), second.len(), "the same regions should be described");
    assert_eq!(second[0].1, 0x0000);
    assert_eq!(second[0].2, 0x800);
    assert_ne!(second[0].3, 0, "and still point at real memory");
}
