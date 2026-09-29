<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The FamiRust FDS BIOS

`fds-hle.bin` is an 8 KiB replacement for `disksys.rom`, so the Famicom Disk
System runs with no firmware file. `nes-core` carries it with `include_bytes!`;
nothing has to be found, downloaded, or placed in a system directory.

    scripts/build-fds-bios.sh

assembles `src/main.s` with our own `nes-asm` and reruns the tests. `cargo test`
checks that the committed image is exactly what the committed source builds, and
that the core carries that image rather than a stale one, so "what ships is built
from the source next to it" is checkable rather than promised.

## Provenance

Clean-room. Written against the published RAM Adapter interface and against
behaviour measured from the real BIOS as a **black box**: booting disks under it
and comparing what came out. No part of it is derived from a disassembly of
Nintendo's ROM, and none of Nintendo's artwork or text appears in it. The boot
screen is ours.

Where a measurement decided something, the source says which measurement and
what it showed, because that record is the difference between a
reimplementation and a copy.

## What it does, and what it does not

It boots: it puts up a screen, spins the drive, reads the disk, loads the files
the boot rule selects, and hands the game control through the `$DFFC`
pseudo-vector. It dispatches NMI and IRQ. Measured against the real BIOS at
handover, all 114 disks in the corpus get the same entry address and the same
program RAM, byte for byte.

It is not a finished BIOS. The corpus census found games calling forty entry
points; three are implemented and the other 37 are stubs that put **NO ROUTINE**
on the screen and stop. A game that gets past boot and then asks for one of them
says so, instead of executing fill bytes and failing in a way that looks like a
different bug each time.

Background, measurements and running order: `docs/notes/FDS-HLE.md`.
