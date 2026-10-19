<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright (C) 2026 Prodigy75000
-->

# FamiRust

A clean-room NES / Famicom emulator core in Rust, shipped as a
[libretro](https://www.libretro.com/) core. Accuracy comes first, and save
states are byte-identical across platforms, so they work for netplay and
rollback.

## Features

- **CPU**: cycle-accurate 2A03. Passes the full
  [SingleStepTests](https://github.com/SingleStepTests/65x02) 6502 suite,
  unofficial opcodes included.
- **PPU**: dot-accurate 2C02 with mid-frame raster effects and sprite-0 splits.
- **APU**: all five channels, with DMC DMA stalls.
- **Mappers**: 0, 1, 2, 3, 4, 5, 7, 9, 11, 13, 34, 64, 65, 66, 69, 71, 79, 113,
  118, 119, 228, 232.
- **Famicom Disk System**: drive, disk writes and saves, the expansion sound
  channel, and side swapping through the libretro disk interface. No BIOS
  file is needed (see below).
- **Headers**: iNES and NES 2.0, with a correction database for bad dumps.
- **RetroAchievements**: system RAM and cartridge RAM are exposed through the
  libretro memory map.
- **Save states**: every field is serialized little-endian in a fixed order,
  with no pointers, floats or `usize`. A state saved on Android loads on
  Windows or macOS. See [`docs/SAVESTATE.md`](docs/SAVESTATE.md).

## FDS without a BIOS file

The core carries its own 8 KiB FDS BIOS, written in 6502 from the documented
RAM Adapter interface and from the real BIOS's behaviour, observed as a black
box. If the frontend supplies a `disksys.rom`, that is used instead.

The real BIOS serves as the reference. On a corpus of 507 disks, 490 are
handed the same machine state by both BIOSes at the moment the game starts.
The details are in [`docs/notes/FDS-HLE.md`](docs/notes/FDS-HLE.md).

```sh
scripts/build-fds-bios.sh          # firmware/fds-hle/src/main.s -> the image
python scripts/fds-hle-check.py    # compare both BIOSes over a corpus
```

## Building

```sh
cargo build --release   # core, libretro cdylib and dev tools
cargo test
```

The libretro core lands in `target/release/` as `nescore_libretro.dll`,
`libnescore_libretro.so` or `libnescore_libretro.dylib`.

Android (arm64) needs 16 KB page alignment for Google Play:

```sh
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=<ndk>/.../aarch64-linux-android21-clang
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo build --release -p nes-libretro --target aarch64-linux-android
```

macOS needs no extra flags. For a universal binary, build both
`x86_64-apple-darwin` and `aarch64-apple-darwin` and join them with `lipo`.

## Dev tools

`crates/nes-runner` has these headless tools:

| tool | what it does |
|---|---|
| `tomharte` | runs the CPU conformance suite |
| `nes` | renders N frames to a PNG and dumps the audio |
| `play` | runs scripted input on both pads, with RAM assertions |
| `fingerprint` | checks save-state parity |
| `fdstrace` | shows which BIOS routines an FDS game calls |
| `fdsdiff` | boots one disk under two BIOSes and compares them |

```sh
cargo run --release -p nes-runner --bin nes -- game.nes 300 out.png
cargo run -p nes-runner --bin play -- game.nes "40=start;60=right;92=right+a" out
```

[`docs/DEBUG_HARNESS.md`](docs/DEBUG_HARNESS.md) covers the rest.

## Layout

```
crates/nes-core/      the emulator (lib crate nes_core)
crates/nes-libretro/  libretro C ABI
crates/nes-runner/    dev tools
crates/nes-asm/       a small 6502 assembler
firmware/fds-hle/     the built-in FDS BIOS
roms/famirust-demo/   the demo cartridge (CC0)
roms/noisy-neighbors/ a two-player game, in progress (CC0)
docs/                 hardware notes and design docs
```

## Demo cartridge

[`roms/famirust-demo/`](roms/famirust-demo/) is an original NES cartridge,
public domain under CC0 1.0. Five screens each stress one part of the
hardware: sprites and the per-line limit, a sprite-0 scroll split, the
palette, all five sound channels, and both controllers. It also serves as a
test fixture that needs no user-supplied ROM.

```sh
scripts/build-demo-rom.sh
cargo test -p nes-core --test demo_cart
```

No copyrighted ROMs or BIOS images are included. Supply your own legally
obtained dumps.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). The cartridges in `roms/` are CC0
1.0, each with its own `LICENSE`.

"Nintendo Entertainment System", "Famicom" and "NES" are trademarks of
Nintendo. FamiRust is an independent clean-room implementation, not
affiliated with or endorsed by Nintendo.
