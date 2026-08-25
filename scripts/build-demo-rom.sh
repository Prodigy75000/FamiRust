#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
#
# Assemble the FamiRust Demo Cart and record its hash.
#
# The ROM is committed to the repository, so this only needs running when the
# cartridge source changes. `cargo test -p nes-asm --test demo_rom_reproduces`
# is what notices if someone forgets.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

src="roms/famirust-demo/src/main.s"
out="roms/famirust-demo/famirust-demo.nes"

cargo run --quiet -p nes-asm -- "$src" -o "$out" -s

( cd roms/famirust-demo && sha256sum famirust-demo.nes > SHA256SUMS )
echo
cat roms/famirust-demo/SHA256SUMS

# Prove the committed image and the committed source still agree.
cargo test --quiet -p nes-asm --test demo_rom_reproduces
