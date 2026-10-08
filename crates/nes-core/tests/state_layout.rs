// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! `STATE_VERSION` has to move when the serialized layout moves, and no reader
//! outside this repo can check that.
//!
//! The savestate format is a binding byte-identical contract across every
//! in-house core: a core accepts any state at or below its own version, so a
//! layout that changes without a version bump produces states that LOAD and are
//! WRONG rather than states that are refused. That is the dangerous direction.
//!
//! `STATE_VERSION` is a hand-maintained number, and the failure mode is one of
//! omission: somebody adds a field to a device's `save`/`load` pair and does
//! not bump it. The number by itself cannot catch that, because the thing that
//! went missing is the edit nobody made.
//!
//! TrophyHubAndroid's release gate snapshots the value, which catches a bump
//! between cuts (the signal that the desktop core needs rebuilding in the same
//! pass). It cannot catch the omission: it reads the number we declare, and a
//! number that should have moved and did not looks identical to one that
//! correctly stayed put. So the check has to live here, pinned to the layout
//! itself.
//!
//! The pin is the serialized LENGTH plus a digest of a freshly built machine.
//! Length is the layout proxy and is immune to emulation changes: adding or
//! removing a serialized field moves it, while a PPU timing fix does not. The
//! digest is the stronger tripwire, catching a reorder or a retype that keeps
//! the size. Both are taken from a machine that has NOT been stepped, so this
//! does not become a test of emulation behaviour that fires every time someone
//! corrects a cycle.

use nes_core::{Nes, STATE_VERSION};

/// FNV-1a, so this file needs no dependency to hold a digest.
fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// A minimal iNES image for `mapper`, with every PRG byte distinct enough that
/// a mapper's power-on bank choice shows up in the digest.
fn ines(mapper: u8, prg_banks: u8, chr_banks: u8) -> Vec<u8> {
    let mut v = vec![0u8; 16];
    v[0..4].copy_from_slice(b"NES\x1a");
    v[4] = prg_banks;
    v[5] = chr_banks;
    v[6] = (mapper & 0x0f) << 4;
    v[7] = mapper & 0xf0;
    for b in 0..prg_banks {
        v.extend(std::iter::repeat(b ^ 0xa5).take(16 * 1024));
    }
    for b in 0..chr_banks {
        v.extend(std::iter::repeat(b ^ 0x5a).take(8 * 1024));
    }
    // A reset vector inside the last bank, so from_rom's power-on reset lands
    // somewhere defined rather than wherever the fill happens to point.
    let n = v.len();
    v[n - 4] = 0x00;
    v[n - 3] = 0x80;
    v
}

/// One row per mapper whose layout this pins: name, mapper, PRG banks, CHR
/// banks, serialized length, digest of a fresh machine's state.
///
/// If a row below fails, exactly one of two things has happened.
///
/// 1. You changed the serialized field set or its order. That is a format
///    change: **bump `STATE_VERSION`**, note what moved in its doc comment,
///    rebuild the shipped artifacts for every platform in the same pass, and
///    then update these numbers.
/// 2. You changed a device's power-on values without changing the layout. Old
///    states still load, so no bump is needed. Update the digest alone, and
///    leave the length as it was.
///
/// Updating the numbers to make the test green WITHOUT deciding which of those
/// it was is the one response that defeats the point.
const LAYOUT: &[(&str, u8, u8, u8, usize, u64)] = &[
    ("NROM", 0, 2, 1, 12825, 0x0c502e1c7aff7952),
    ("MMC1", 1, 4, 2, 12830, 0x8072598470ebb960),
    ("UxROM", 2, 4, 0, 12826, 0xaa060fd5b3b9593a),
    ("CNROM", 3, 2, 4, 4634, 0xf6228affee5ab266),
    ("MMC3", 4, 8, 8, 12841, 0x15e2042145304844),
    ("AxROM", 7, 4, 0, 12827, 0xc1291da0f182c042),
    ("Action 52", 228, 96, 64, 4638, 0x9d0bbd1e16612446),
];

#[test]
fn the_serialized_layout_has_not_moved_without_a_version_bump() {
    assert_eq!(STATE_VERSION, 4, "if you bumped this, read LAYOUT's doc comment");
    let mut wrong = Vec::new();
    for &(name, mapper, prg, chr, want_len, want_digest) in LAYOUT {
        let nes = Nes::from_rom(&ines(mapper, prg, chr)).expect("synthetic cart should load");
        let state = nes.save_state();
        let (got_len, got_digest) = (state.len(), digest(&state));
        if got_len != want_len || got_digest != want_digest {
            wrong.push(format!(
                "    (\"{name}\", {mapper}, {prg}, {chr}, {got_len}, 0x{got_digest:016x}),"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the savestate layout has moved. Decide WHICH kind of change this was \
         (see the doc comment on LAYOUT) before pasting these in:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn a_state_round_trips_and_re_saves_byte_identically() {
    // The contract is byte-identical serialization, so a state that loads must
    // produce the same bytes again. Without this the layout pin above could be
    // satisfied by a save/load pair that disagree with each other.
    let rom = ines(4, 8, 8);
    let mut nes = Nes::from_rom(&rom).unwrap();
    for _ in 0..3 {
        nes.step_frame();
    }
    let first = nes.save_state();

    let mut other = Nes::from_rom(&rom).unwrap();
    other.load_state(&first).expect("a state should load on the same ROM");
    assert_eq!(
        first,
        other.save_state(),
        "re-saving a loaded state produced different bytes"
    );
}

#[test]
fn a_newer_state_version_is_refused_rather_than_guessed_at() {
    // The whole reason the version exists. An older build must refuse a newer
    // state cleanly, because the alternative is reading a layout it does not
    // know with the field offsets it does know.
    let rom = ines(0, 2, 1);
    let mut nes = Nes::from_rom(&rom).unwrap();
    let mut state = nes.save_state();
    state[8] = 0xff; // format_version low byte, straight after the 8-byte magic
    state[9] = 0xff;
    assert!(
        nes.load_state(&state).is_err(),
        "a state from the future must be refused, not interpreted"
    );
}
