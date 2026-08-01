//! Headless dev harness: load a `.nes`, (eventually) run N frames, dump a PNG /
//! WAV. Mirrors the `snes` / `n64` bins in the sibling cores.
//!
//! For now it parses the ROM header and reports what the core understood, which
//! is enough to sanity-check dumps as they arrive. Frame rendering and the
//! `--png` / `--wav` output land with the PPU and APU.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: nes <rom.nes> [frames]");
        return ExitCode::FAILURE;
    };

    let rom = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    match nes_core::Nes::from_rom(&rom) {
        Ok(nes) => {
            let snap = nes.save_state();
            println!("loaded {path}: {} bytes, initial save-state {} bytes", rom.len(), snap.len());
            println!("(frame rendering not yet implemented — CPU/PPU pending)");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("failed to load {path}: {e:?}");
            ExitCode::FAILURE
        }
    }
}
