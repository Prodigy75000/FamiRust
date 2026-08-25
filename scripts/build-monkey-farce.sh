#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
#
# Assemble MONKEY FARCE and record its hash.
#
# The ROM is committed to the repository, so this only needs running when the
# cartridge source changes. `cargo test -p nes-asm --test monkey_farce_reproduces`
# is what notices if someone forgets.
#
# The reachability check runs first and on purpose. A game whose whole premise
# is lying to the player about the floor has exactly one thing it must not get
# wrong, and it is not something you can see by looking at a room.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

python roms/monkey-farce/tools/genart.py
python roms/monkey-farce/tools/genmusic.py
echo
python roms/monkey-farce/tools/reach.py
echo

src="roms/monkey-farce/src/main.s"
out="roms/monkey-farce/monkey-farce.nes"

cargo run --quiet -p nes-asm -- "$src" -o "$out" -s

( cd roms/monkey-farce && sha256sum monkey-farce.nes > SHA256SUMS )
echo
cat roms/monkey-farce/SHA256SUMS

# Prove the committed image and the committed source still agree, and that the
# cartridge still plays the way it is supposed to.
cargo test --quiet -p nes-asm --test monkey_farce_reproduces
cargo test --quiet -p nes-core --test monkey_farce
