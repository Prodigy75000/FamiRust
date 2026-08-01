# FamiRust

A clean-room **Nintendo Entertainment System / Famicom** emulator core, written
from scratch in Rust. Sibling to the other in-house Trophy Hub cores (PocketRust
GB/GBC, PocketRustAdvance GBA, SuperRust SNES, UltraRust N64).

## Goals, in priority order

1. **Correctness / accuracy.** A cycle-stepped Ricoh 2A03 CPU (an NMOS 6502 with
   decimal mode fused off) interleaved with a dot-accurate 2C02 PPU and the 2A03
   APU. Conformance-first: the CPU is ground to green against the TomHarte
   single-step vectors before it drives anything else.
2. **Byte-identical, deterministic save states** — a hard requirement, not a
   feature. Cross-engine netplay and rollback are only sound when two machines
   in the same logical state always serialize to the *same bytes* on every
   platform. See [`crates/nes-core/src/save.rs`](crates/nes-core/src/save.rs):
   every mutable field is serialized little-endian in a fixed order, no
   `usize`/pointer/float/hash-ordering ever enters a state, and load is the
   strict inverse (it *refuses* truncated, malformed, or over-long buffers).
3. **Clean-room.** Hardware documentation only. Third-party emulator source is
   off-limits.

## Layout

```
crates/
  nes-core/       the emulator core (lib: nes_core)
  nes-libretro/   libretro C ABI cdylib  -> libnescore_libretro.so
  nes-runner/     dev harnesses: `tomharte` (CPU conformance), `nes` (headless)
dumps/            cart images + FDS BIOS (gitignored)
tests/vendor/     vendored TomHarte 6502 vectors (gitignored)
docs/             hardware references
```

Part of the TrophyHub root Cargo workspace.

## Status

Scaffold. Save-state backbone + iNES/NES 2.0 parser + NROM mapper + register
files are in and unit-tested; the CPU/PPU/APU execution cores are next, CPU
first via the TomHarte grind.

```
cargo test  -p nes-core
cargo run   -p nes-runner --bin nes -- path/to/game.nes
```
