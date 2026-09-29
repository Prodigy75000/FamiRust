// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The FDS BIOS built into the core must be exactly what the committed source
//! assembles to.
//!
//! It matters more here than for a cartridge. This image ships inside every
//! copy of the core, it replaces a piece of firmware somebody else owns, and
//! the claim that it is ours rests on it being built from the source next to
//! it. That claim should be checkable by running `cargo test`, not taken on
//! trust, so this is the test that makes it one.
//!
//! The matching half, that the image compiled INTO the core is this same file,
//! lives in `nes-core/tests/fds_hle_boot.rs`: this crate is deliberately
//! dependency-free and is not going to grow one to make an assertion that fits
//! just as well on the other side.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/nes-asm -> repository root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn assemble_bios() -> Vec<u8> {
    let src = repo_root().join("firmware/fds-hle/src/main.s");
    let opts = nes_asm::asm::Options { symbol_file: false };
    nes_asm::asm::assemble(&src, &opts)
        .unwrap_or_else(|e| panic!("the FDS BIOS no longer assembles:\n{e}"))
        .rom
}

#[test]
fn committed_image_matches_a_fresh_build_of_the_committed_source() {
    let built = assemble_bios();
    let path = repo_root().join("firmware/fds-hle/fds-hle.bin");
    let committed = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    assert_eq!(
        built.len(),
        committed.len(),
        "committed image is {} bytes, a fresh build is {}",
        committed.len(),
        built.len()
    );
    if let Some(i) = built.iter().zip(&committed).position(|(a, b)| a != b) {
        panic!(
            "committed image and a fresh build first differ at ${:04X}: \
             built ${:02X}, committed ${:02X}. Rebuild with \
             scripts/build-fds-bios.sh and commit the result.",
            0xe000 + i,
            built[i],
            committed[i]
        );
    }
}

#[test]
fn the_image_is_the_size_of_the_window_it_replaces() {
    // $E000-$FFFF and not a byte more or less: a short image would leave the
    // vectors somewhere other than $FFFA.
    assert_eq!(assemble_bios().len(), 8 * 1024);
}

#[test]
fn the_vectors_point_into_the_image() {
    // An image whose vectors were $FFFF or $0000 would still assemble, and
    // would fail on the machine rather than here.
    let rom = assemble_bios();
    for (name, off) in [("NMI", 0x1ffa), ("RESET", 0x1ffc), ("IRQ", 0x1ffe)] {
        let v = u16::from(rom[off]) | (u16::from(rom[off + 1]) << 8);
        assert!(
            v >= 0xe000,
            "{name} vector is ${v:04X}, which is not inside the BIOS window"
        );
    }
}

#[test]
fn every_entry_point_the_census_found_has_something_at_it() {
    // A game calling an address we left as fill executes the fill. Each of the
    // 40 addresses the corpus census found games entering must therefore hold
    // either a routine or a stub that says we have not written it yet; what it
    // must not hold is $FF, which is what an untouched byte of the image is.
    //
    // The list is docs/notes/FDS-CENSUS-2026-09-29.md. If the census grows, so
    // does this.
    const ENTRIES: [u16; 40] = [
        0xe149, 0xe153, 0xe161, 0xe16b, 0xe171, 0xe17e, 0xe185, 0xe18b, 0xe1b2, 0xe1c7,
        0xe1f8, 0xe237, 0xe239, 0xe305, 0xe32a, 0xe3e7, 0xe445, 0xe484, 0xe4a0, 0xe4f9,
        0xe68f, 0xe778, 0xe7bb, 0xe844, 0xe86a, 0xe8d2, 0xe8e1, 0xe997, 0xe9b1, 0xe9c8,
        0xe9d3, 0xea1f, 0xea4c, 0xea84, 0xead2, 0xeaea, 0xeafd, 0xebaf, 0xec22, 0xee24,
    ];
    let rom = assemble_bios();
    let mut bare = Vec::new();
    for a in ENTRIES {
        if rom[(a - 0xe000) as usize] == 0xff {
            bare.push(format!("${a:04X}"));
        }
    }
    assert!(
        bare.is_empty(),
        "these entry points are still bare fill: {}",
        bare.join(" ")
    );
}
