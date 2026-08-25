#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
#
# Assemble LIAR'S KEEP and record its hash.
#
# The ROM is committed to the repository, so this only needs running when the
# cartridge source changes. `cargo test -p nes-asm --test liars_keep_reproduces`
# is what notices if someone forgets.
#
# The reachability check runs first and on purpose. A game whose whole premise
# is lying to the player about the floor has exactly one thing it must not get
# wrong, and it is not something you can see by looking at a room.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

python roms/liars-keep/tools/reach.py
echo

src="roms/liars-keep/src/main.s"
out="roms/liars-keep/liars-keep.nes"

cargo run --quiet -p nes-asm -- "$src" -o "$out" -s

( cd roms/liars-keep && sha256sum liars-keep.nes > SHA256SUMS )
echo
cat roms/liars-keep/SHA256SUMS

# Prove the committed image and the committed source still agree, and that the
# cartridge still plays the way it is supposed to.
cargo test --quiet -p nes-asm --test liars_keep_reproduces
cargo test --quiet -p nes-core --test liars_keep
