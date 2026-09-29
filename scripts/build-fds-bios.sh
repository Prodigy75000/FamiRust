#!/usr/bin/env bash
#
# Build the HLE FDS BIOS from source and check it against the real one.
#
#   scripts/build-fds-bios.sh
#
# Assembles firmware/fds-hle/src/main.s into the 8 KiB image the core carries,
# then rebuilds everything that embeds it and runs the tests that hold it
# against `disksys.rom`. The image is committed, so anyone can check that what
# ships is what this source builds; `cargo test` does exactly that.
#
# The corpus check is a separate, slower step and is not run here:
#
#   python scripts/fds-hle-check.py      # all 114 disks, about ten minutes
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$REPO/firmware/fds-hle/src/main.s"
OUT="$REPO/firmware/fds-hle/fds-hle.bin"

echo "[1/3] assembling $SRC"
( cd "$REPO" && cargo run -q -p nes-asm -- "$SRC" -o "$OUT" -s )

SIZE=$(wc -c < "$OUT" | tr -d ' ')
[ "$SIZE" = "8192" ] || { echo "ERROR: image is $SIZE bytes, want 8192" >&2; exit 1; }

# The image is compiled into nes-core with include_bytes!, so a rebuild is not
# optional: without it the core would go on carrying the previous one while the
# file on disk looked right.
echo "[2/3] rebuilding what embeds it"
( cd "$REPO" && cargo build -q -p nes-core -p nes-libretro )

echo "[3/3] tests"
( cd "$REPO" && cargo test -q -p nes-asm --test fds_hle_bios_reproduces )
( cd "$REPO" && cargo test -q -p nes-core --test fds_hle_boot )

echo
echo "$OUT"
if command -v md5sum >/dev/null 2>&1; then
  md5sum "$OUT"
fi
