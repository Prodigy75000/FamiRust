<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright (C) 2026 Prodigy75000
-->

# FamiRust

A clean-room **Nintendo Entertainment System / Famicom** emulator core, written
from scratch in Rust — accuracy-first, with **byte-identical, platform-agnostic
save states** as a hard design constraint (for cross-engine netplay and
rollback). Sibling to the other in-house cores: PocketRust (GB/GBC),
PocketRustAdvance (GBA), SuperRust (SNES), UltraRust (N64).

It ships as a [libretro](https://www.libretro.com/) core and runs the commercial
NES library — hundreds of licensed titles boot and play.

## Design goals, in priority order

1. **Correctness / accuracy.** A cycle-stepped Ricoh 2A03 CPU (an NMOS 6502 with
   decimal mode fused off) interleaved three-dots-per-cycle with a dot-accurate
   2C02 PPU and the 2A03 APU. The CPU passes the full
   [TomHarte / SingleStepTests](https://github.com/SingleStepTests/65x02) 6502
   corpus (all 256 opcodes, 10 000 vectors each, per-cycle bus trace) — including
   every unofficial opcode — before it is allowed to drive anything else.
2. **Byte-identical, deterministic save states** — a requirement, not a feature.
   Cross-engine netplay and rollback are only sound when two machines in the same
   logical state serialize to the *same bytes* on every platform/ABI/compiler.
   See [`crates/nes-core/src/save.rs`](crates/nes-core/src/save.rs): every mutable
   field is serialized little-endian in a fixed order; no `usize`, pointer, float,
   or hash-ordering ever enters a state; load is the strict inverse and *refuses*
   truncated, malformed, or over-long buffers. A state made on the Android `.so`
   loads on the Windows `.dll`/macOS `.dylib` and vice-versa.
3. **Clean-room.** Built from hardware documentation only (the NESdev wiki
   reference pages, distilled into [`docs/notes/`](docs/notes/)). No third-party
   emulator source was consulted. Measured reference *data* — e.g. the 2C02
   master palette — is used as data, and cited where it is.

## What works

- **CPU** — cycle-accurate 2A03; full TomHarte pass incl. all illegal opcodes + JAM.
- **PPU** — dot-accurate 2C02: loopy v/t/x/w scrolling, background + sprite
  pipelines, sprite-0 hit, 8-sprite-per-line evaluation/overflow, the `$2002`
  read/NMI races, and mid-frame raster splits (sprite-0 status bars, scroll
  splits) — the tricky timing that games like *Bart vs. the Space Mutants* rely on.
- **APU** — all five channels (2 pulse, triangle, noise, DMC) with DMC DMA
  CPU stall; audio resampled for the host.
- **Mappers** — 0 (NROM), 1 (MMC1), 2 (UxROM), 3 (CNROM), 4 (MMC3), 5 (MMC5,
  incl. extended-attribute mode), 7 (AxROM), 9 (MMC2), 11 (Color Dreams),
  13 (CPROM), 34 (BNROM/NINA-001), 64 (RAMBO-1), 65 (Irem H3001), 66 (GxROM),
  69 (Sunsoft FME-7), 71 (Camerica), 79 (NINA-03), 113 (NINA-113),
  118 (TxSROM), 119 (TQROM), 232 (Camerica BF9096).
- **Headers** — iNES + NES 2.0 parsing, with a CRC32 correction DB for known-bad
  dumps (e.g. "DiskDude!"-corrupted headers).
- **RetroAchievements** — exposes system RAM + cartridge work RAM over the
  libretro memory interface.
- **Save states** — see goal #2; validated byte-identical across platforms.

## Layout

```
crates/
  nes-core/       the emulator core (lib crate: nes_core)
  nes-libretro/   libretro C ABI cdylib -> libnescore_libretro.{so,dll,dylib}
  nes-runner/     dev harnesses: `tomharte` (CPU conformance), `nes` (headless
                  render + audio dump), `fingerprint` (save-state parity check)
docs/notes/       clean-room hardware reference notes
dumps/            cart images (gitignored — bring your own)
tests/vendor/     vendored TomHarte 6502 vectors (gitignored)
```

This is a standalone Cargo workspace.

## Build & run

```sh
cargo build --release            # builds the core, libretro cdylib, and harnesses
cargo test                       # unit tests (core, save-state golden, PPU timing)

# headless: render N frames of a ROM to a PNG (+ WAV of the audio)
cargo run --release -p nes-runner --bin nes -- path/to/game.nes 300 out.png

# the shipped libretro core:
#   target/release/libnescore_libretro.so   (Linux/Android)
#   target/release/nescore_libretro.dll     (Windows)
#   target/release/libnescore_libretro.dylib (macOS)
```

**Android (arm64)** with the NDK, 16 KB-page-aligned:

```sh
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=<ndk>/.../aarch64-linux-android21-clang
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo build --release -p nes-libretro --target aarch64-linux-android
```

Nothing here includes or requires copyrighted ROMs or BIOS images — supply your
own legally obtained dumps.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

"Nintendo Entertainment System", "Famicom", and "NES" are trademarks of Nintendo.
FamiRust is an independent, clean-room reimplementation and is not affiliated with
or endorsed by Nintendo.
