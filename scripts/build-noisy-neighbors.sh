#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
#
# Assemble NOISY NEIGHBORS and record its hash.
#
# The ROM is committed to the repository, so this only needs running when the
# cartridge source changes. `cargo test -p nes-asm --test
# noisy_neighbors_reproduces` is what notices if someone forgets.
#
# There is no reachability check here yet. There will be: a two-player room has
# more that can go quietly wrong than a one-player room did, and none of it is
# visible by looking. See tools/reach2.py when it lands.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

python roms/noisy-neighbors/tools/genart.py
python roms/noisy-neighbors/tools/genmusic.py
echo

src="roms/noisy-neighbors/src/main.s"
out="roms/noisy-neighbors/noisy-neighbors.nes"

cargo run --quiet -p nes-asm -- "$src" -o "$out" -s

( cd roms/noisy-neighbors && sha256sum noisy-neighbors.nes > SHA256SUMS )
echo
cat roms/noisy-neighbors/SHA256SUMS

# Prove the committed image and the committed source still agree, and that the
# cartridge still enforces the one rule it exists for.
cargo test --quiet -p nes-asm --test noisy_neighbors_reproduces
cargo test --quiet -p nes-core --test noisy_neighbors
