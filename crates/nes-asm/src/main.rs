// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! `nes-asm` -- a small 6502 assembler that turns FamiRust's demo source into an
//! iNES cartridge image.
//!
//! It exists so the chain from source to `.nes` is entirely in this repository:
//! the demo ROM we distribute is assembled by our own tool, from our own source,
//! with no third-party build dependency anywhere in the path.
//!
//!   cargo run -p nes-asm -- roms/famirust-demo/src/main.s -o famirust-demo.nes

use nes_asm::asm;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut symbol_file = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--out" => output = args.next(),
            "-s" | "--symbols" => symbol_file = true,
            "-h" | "--help" => {
                eprintln!("usage: nes-asm <source.s> [-o out.nes] [-s]");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option {other}");
                return ExitCode::FAILURE;
            }
            other => input = Some(other.to_string()),
        }
    }

    let Some(input) = input else {
        eprintln!("usage: nes-asm <source.s> [-o out.nes] [-s]");
        return ExitCode::FAILURE;
    };
    let input = std::path::PathBuf::from(input);
    let output = output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| input.with_extension("nes"));

    let opts = asm::Options { symbol_file };
    let built = match asm::assemble(&input, &opts) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("nes-asm: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = std::fs::write(&output, &built.rom) {
        eprintln!("nes-asm: cannot write {}: {e}", output.display());
        return ExitCode::FAILURE;
    }
    if symbol_file {
        let sym = output.with_extension("sym");
        if let Err(e) = std::fs::write(&sym, built.symbols.as_bytes()) {
            eprintln!("nes-asm: cannot write {}: {e}", sym.display());
            return ExitCode::FAILURE;
        }
    }

    println!(
        "{} -- {} bytes, {} bytes of PRG used, {} CHR tiles drawn",
        output.display(),
        built.rom.len(),
        built.prg_used,
        built.chr_tiles_used
    );
    ExitCode::SUCCESS
}
