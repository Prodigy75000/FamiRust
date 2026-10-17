// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! No ROM image may panic the core, whatever sizes its header claims.
//!
//! A panic in the libretro core takes the whole frontend down with it, so a
//! mapper that indexes past a ROM smaller than the board it emulates is a
//! crash on the player's phone, not a wrong pixel. The merged NES set has
//! three: HES and Sachen carts with 16 KiB of PRG on boards (79, 113) built
//! for 32 KiB banks.
//!
//! Every mapper the core accepts is run with PRG and CHR sizes real dumps
//! carry, including the awkward ones. The PRG is pseudo-random, so the CPU
//! executes noise and that noise writes the mapper's registers all over; the
//! reset vectors point into it.

use std::panic::{catch_unwind, AssertUnwindSafe};

fn rom(mapper: u16, prg_16k: usize, chr_8k: usize, seed: u32) -> Vec<u8> {
    let mut v = vec![b'N', b'E', b'S', 0x1a, prg_16k as u8, chr_8k as u8];
    v.push(((mapper & 0x0f) << 4) as u8);
    v.push((mapper & 0xf0) as u8);
    v.resize(16, 0);
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(mapper as u32 + 1);
    for _ in 0..(prg_16k * 16384 + chr_8k * 8192) {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        v.push(x as u8);
    }
    // Reset and NMI vectors into the image, so the noise is what runs.
    let end = 16 + prg_16k * 16384;
    v[end - 6..end].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
    v
}

#[test]
fn no_mapper_panics_on_a_rom_of_an_odd_size() {
    let mut accepted = 0;
    let mut failures = Vec::new();
    for mapper in 0u16..256 {
        for &(prg, chr) in &[(1, 1), (1, 0), (2, 0), (2, 1), (3, 3), (8, 16)] {
            let image = rom(mapper, prg, chr, 7);
            let r = catch_unwind(AssertUnwindSafe(|| {
                let Ok(mut nes) = nes_core::Nes::from_rom(&image) else {
                    return false;
                };
                for _ in 0..8 {
                    nes.step_frame();
                }
                true
            }));
            match r {
                Ok(true) => accepted += 1,
                Ok(false) => {}
                Err(_) => failures.push(format!("mapper {mapper}, {prg}x16K PRG, {chr}x8K CHR")),
            }
        }
    }
    // The core has to have accepted something, or this test checks nothing.
    assert!(accepted > 100, "only {accepted} images loaded");
    assert!(failures.is_empty(), "panicked:\n  {}", failures.join("\n  "));
}
