// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The committed cartridge must be exactly what the committed source
//! assembles to.
//!
//! This is the test that makes "the binary we distribute is built from the
//! source in this repository" a checkable statement rather than a promise.
//! Anyone can run `cargo test` and find out.

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

fn assemble_cart() -> Vec<u8> {
    let src = repo_root().join("roms/noisy-neighbors/src/main.s");
    let opts = nes_asm::asm::Options { symbol_file: false };
    nes_asm::asm::assemble(&src, &opts)
        .unwrap_or_else(|e| panic!("the cartridge no longer assembles:\n{e}"))
        .rom
}

#[test]
fn committed_rom_matches_a_fresh_build_of_the_committed_source() {
    let built = assemble_cart();
    let path = repo_root().join("roms/noisy-neighbors/noisy-neighbors.nes");
    let committed = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    assert_eq!(
        built.len(),
        committed.len(),
        "rebuilt image is {} bytes, the committed one is {}",
        built.len(),
        committed.len()
    );
    if let Some(at) = built.iter().zip(&committed).position(|(a, b)| a != b) {
        panic!(
            "rebuilt image differs from the committed one at offset ${at:04X}: \
             built ${:02X}, committed ${:02X}. Rebuild the ROM and commit it.",
            built[at], committed[at]
        );
    }
}

#[test]
fn assembly_is_deterministic() {
    // Two runs of the assembler over the same source must agree, or the
    // reproducibility claim above is meaningless.
    assert_eq!(assemble_cart(), assemble_cart());
}

#[test]
fn header_declares_the_cartridge_the_source_asked_for() {
    let rom = assemble_cart();
    assert_eq!(&rom[0..4], b"NES\x1a", "iNES magic");
    assert_eq!(rom[4], 2, "PRG banks (2 x 16 KB = 32 KB)");
    assert_eq!(rom[5], 1, "CHR banks (1 x 8 KB)");
    assert_eq!(rom[6] & 0x01, 1, "vertical mirroring");
    assert_eq!(rom[6] >> 4, 0, "mapper low nibble: NROM");
    assert_eq!(rom[7], 0, "mapper high nibble, and plain iNES rather than NES 2.0");
    assert_eq!(rom.len(), 16 + 32 * 1024 + 8 * 1024);
}

#[test]
fn reset_vector_points_into_the_cartridge() {
    // A ROM whose vectors are $FFFF would still "assemble". This is the
    // cheapest possible check that the vector table was actually populated.
    let rom = assemble_cart();
    let vectors = &rom[16 + 32 * 1024 - 6..16 + 32 * 1024];
    let nmi = u16::from_le_bytes([vectors[0], vectors[1]]);
    let reset = u16::from_le_bytes([vectors[2], vectors[3]]);
    let irq = u16::from_le_bytes([vectors[4], vectors[5]]);
    for (name, addr) in [("NMI", nmi), ("RESET", reset), ("IRQ", irq)] {
        assert!(
            (0x8000..0xFFFA).contains(&addr),
            "{name} vector ${addr:04X} does not point into PRG"
        );
    }
    assert_ne!(nmi, reset, "NMI and RESET should not be the same routine");
}
