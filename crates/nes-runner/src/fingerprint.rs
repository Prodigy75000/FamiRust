// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Cross-platform save-state fingerprint — the offline proof behind cross-engine
//! netplay. Run the SAME rom + frame count on every build (Android .so, iOS
//! .dylib, Windows .dll, this native harness) and compare the printed size +
//! hash. Identical output == the FAMIRST1 state is byte-for-byte independent of
//! target/ABI/compiler, which is exactly what lets a state cross machines mid-
//! session. Any divergence localises to a platform, not to "the core is
//! non-deterministic".
//!
//!   cargo run --release -p nes-runner --bin fingerprint -- <rom.nes> [frames]
//!
//! Deterministic by construction: no input is applied (both pads held at 0), so
//! the run depends only on the ROM. FNV-1a keeps it dependency-free; the hash is
//! only ever compared against another run of this same tool.

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: fingerprint <rom.nes> [frames]");
            std::process::exit(2);
        }
    };
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600);

    let rom = std::fs::read(&path).expect("cannot read rom");
    let rom_fnv = fnv1a64(&rom);

    let mut nes = nes_core::Nes::from_rom(&rom).expect("failed to load rom");
    for _ in 0..frames {
        nes.set_buttons(0, 0);
        nes.set_buttons(1, 0);
        nes.step_frame();
    }
    let state = nes.save_state();
    let state_fnv = fnv1a64(&state);

    // Round-trip: a fresh core loaded from this state must re-serialize identically.
    let mut nes2 = nes_core::Nes::from_rom(&rom).expect("failed to load rom");
    nes2.load_state(&state).expect("state load failed");
    let roundtrip = nes2.save_state() == state;

    println!("rom            {}", path);
    println!("rom_fnv1a      {rom_fnv:016x}");
    println!("frames         {frames}");
    println!("state_bytes    {}", state.len());
    println!("state_fnv1a    {state_fnv:016x}");
    println!("reload_roundtrip {}", if roundtrip { "OK" } else { "MISMATCH" });
}
