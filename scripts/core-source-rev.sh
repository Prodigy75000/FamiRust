#!/usr/bin/env bash
#
# What is the last commit that could have changed the shipped core binary?
#
#   scripts/core-source-rev.sh          # the commit: "<short> <date> <subject>"
#   scripts/core-source-rev.sh --paths  # the authoritative path list, one per line
#
# This exists because the obvious query is wrong in two directions at once, and
# both were live. The deploy script used to ask
#
#   git log -1 -- crates/nes-core/src crates/nes-libretro/src
#
# which on 2026-10-08 answered 84aac06, a TEST-ONLY commit, while missing
# 9f5f74b and 3956f85, which both changed firmware/fds-hle/fds-hle.bin. That
# file is compiled into the core by `include_bytes!` in nes-core/src/fds.rs, so
# a BIOS change is a binary change that no amount of looking at crates/ can
# see. TrophyHubAndroid's release gate had the identical blind spot in its own
# path list and reported the core clean while two BIOS commits sat newer than
# the artifact. Hence one list, here, that both can read.
#
# INCLUDED, and why each earns its place:
#
#   crates/nes-core/src, crates/nes-libretro/src   the code itself
#   crates/{nes-core,nes-libretro}/Cargo.toml      crate-type and deps. Not
#       theoretical: adding "rlib" beside "cdylib" turned LTO off and more than
#       doubled the shipped core, with no source change at all (270a793).
#   firmware/fds-hle/fds-hle.bin                   embedded by include_bytes!
#   Cargo.toml (workspace root)                    [profile.release]: lto,
#       opt-level and codegen-units all change the binary
#
# DELIBERATELY NOT INCLUDED:
#
#   crates/*/tests/**        integration tests cannot reach a release binary
#   crates/*/src/*_tests.rs  unit-test modules that live under src/ because an
#       integration test would need an rlib (see the LTO note above). They are
#       declared `#[cfg(test)] mod foo_tests;` in lib.rs, so a scanner reading
#       the FILE finds no #[cfg(test)] in it and takes the whole thing for
#       production code. The `_tests.rs` suffix is the convention that makes
#       them identifiable from the path alone; keep to it.
#   firmware/fds-hle/src/main.s   the SOURCE of the embedded image rather than
#       the image. Tracking the .bin is both sufficient and more precise: a
#       main.s edit that was never assembled does not change the binary, and
#       `cargo test` already refuses that case
#       (nes-asm/tests/fds_hle_bios_reproduces.rs asserts the committed image
#       is what the committed source assembles to).
#   dumps/, out/, docs/, scripts/   never compiled in.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PATHS=(
  "crates/nes-core/src"
  "crates/nes-core/Cargo.toml"
  "crates/nes-libretro/src"
  "crates/nes-libretro/Cargo.toml"
  "firmware/fds-hle/fds-hle.bin"
  "Cargo.toml"
)
EXCLUDE=":(exclude)crates/*/src/*_tests.rs"

if [ "${1:-}" = "--paths" ]; then
  printf '%s\n' "${PATHS[@]}"
  exit 0
fi

cd "$REPO"
git log -1 --format='%h %ad %s' --date=short -- "${PATHS[@]}" "$EXCLUDE"
