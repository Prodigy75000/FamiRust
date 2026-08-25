// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! A small 6502 assembler, built so that FamiRust's demo cartridge can be
//! produced from source by this repository alone, with no external toolchain
//! in the path. See `src/main.rs` for the command-line front end.

pub mod asm;
pub mod expr;
pub mod opcodes;
