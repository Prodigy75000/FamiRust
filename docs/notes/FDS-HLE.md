<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Replacing disksys.rom

The Famicom Disk System is the one platform FamiRust runs that still demands a
firmware file. Every other core in the house has had its BIOS gate removed;
RustStation and ColecoRust did theirs in September 2026, and this is FamiRust's
version of the same job.

The method is ColecoRust's, written down in `ColecoRust/docs/notes/HLE-IS-THE-POINT.md`.
What travels between the two is the method and nothing else: a 6502 fronting a
Mitsumi Quick Disk shares no silicon with a Z80 fronting a cartridge slot.

## The rules, inherited

**A real BIOS is the oracle, never a requirement.** `dumps/fds/disksys.rom` is a
development tool. Run a title both ways and diff; when the HLE and the real BIOS
disagree, the real one is right until proven otherwise. A core that quietly
starts needing the dump has failed at the only thing this was for.

**A verdict is a ledger row with a fixed vocabulary, not prose.** Derived, not
typed. `fdstrace` emits one of `BOOT`, `BIOS-LOOP`, `MIXED`, `NOLOAD`, `HALT`
and nothing else.

**Tests must be able to fail.** Every test added for this has been mutated and
watched to fail before being trusted.

**Clean-room, against the published interface.** Never derived from a
disassembly of the ROM. The app is on two stores and the entire point is
shipping without the firmware gate; a derivative work would replace one legal
problem with a worse one.

## What was measured first, and why it had to be dynamic

A static scan of the disk images for the `JSR` opcode is **not** a census. It
returned 164 distinct targets, most of them data bytes that happen to read as
`JSR`: `$FFFF` came up eight times, and round numbers dominated the rest.

So `crates/nes-runner/src/fdstrace.rs` traces it dynamically, and the primitive
it watches is not a call opcode at all. It watches **control crossing the window
boundary**: the PC entering `$E000-$FFFF` from outside it. That is simpler than
decoding calls and strictly more complete, because it catches `JSR`, `JMP`,
`JMP ()`, an `RTS` landing in the BIOS and the NMI/IRQ/RESET vector dispatch
without decoding a single addressing mode. It is also the right definition: an
entry point *is* an address control arrives at from outside.

Reads of the window are counted per address and split by whether the CPU was
executing inside it at the time. The split is the whole point. A byte only the
BIOS reads goes away with the routine that read it, so an HLE may lay out its
internals however it likes; a byte the **game** reads out of the image is part
of the published interface and has to still be there.

## What the corpus said

`docs/notes/FDS-CENSUS-2026-09-29.md` is the full ledger. The corpus is 114
disks (72 original Japanese releases, 40 translations, 2 hacks) and lives
outside this repo at `G:\My Drive\FamiRust\Dumps`, the way ColecoRust keeps
its own. Regenerate with `scripts/fds-census.py`.

Four findings shape the design:

**Forty entry points, and twelve of them carry the library.** `$E18B` (the NMI
handler) is entered by 68 of 72 originals. `$E1F8` by 27, `$EAEA` by 22, `$EA84`
and `$EBAF` by 21 each. The tail is long and thin: sixteen entry points were
seen on exactly one original title apiece. This is a work queue with an obvious
order.

**Games barely read the BIOS as data. 100 of 114 read nothing but the interrupt
vectors.** That answers, for this machine, the question ColecoRust left open
about what a cartridge reading the BIOS area should see. The fourteen exceptions
are specific and small enough to handle one at a time: six Namco titles (Xevious,
Pac-Man, Galaxian, Galaga, Dig Dug, Dig Dug II) read an identical `$E001-$E148`,
which is one shared loader rather than six problems; Halley Wars reads
`$E000-$E800`; and the rest read a few dozen bytes or one.

That is the constraint on a synthetic image, and it is a light one. Our own
artwork can sit where the real BIOS keeps its font, because nothing outside the
BIOS looks at it.

**A fifth of the library drives the drive itself.** Twenty-two titles write
`$4024`/`$4025` in volume, up to 6182 times for Reflect World, with Metroid,
Akumajou Dracula, Zelda no Densetsu and both Super Lode Runners among them.
Those games run their own transfer loop, so the hardware transfer engine stays
load-bearing however good the HLE gets. **The HLE is strictly additive.** It
never replaces `fds.rs`.

**Loading is physical, and now there is a number for it.** `$E1F8` averages
8,410,417 CPU cycles per call. Bio Miracle Bokutte Upa spent 9,911,532 cycles in
a single call, which at `BYTE_CYCLES = 149` is 66,520 bytes, or one whole side.
An instant `LoadFiles` is not a shortcut, it is a machine that does not exist.
ColecoRust learned this late (e8825e1, 03606e6); there is no excuse for learning
it late twice.

## What makes this harder than the ColecoVision

- **Vector indirection.** NMI and IRQ go through the BIOS and dispatch via RAM
  at `$DFF6-$DFFF`. Getting it wrong breaks every game rather than one. The
  census shows the shape of it directly: `$E18B` averages **13 cycles** per stay,
  because the handler's first act is to leave through the game's RAM vector.
- **BIOS-owned RAM** in zero page, the stack page and `$DFF6-$DFFF` has to match
  byte for byte.
- **The disk is writable.** Saves land on the medium, so anything the HLE does
  to loading has to leave writing exactly as it was.

## Where the HLE should intercept

A **synthetic 8 KB image**, not CPU address traps: our own artwork where the
real BIOS keeps its graphics, a reserved opcode plus a routine id at each
published entry point, and one dispatch arm in the CPU. Save states stay
trivial, entry addresses land where games expect them, and the cycle charge is a
table sitting next to the id. The census makes this cheap, because only fourteen
titles read the image as data at all.

## Running order

1. ~~Dynamic tracer, then a census across the corpus.~~ Done 2026-09-29.
2. Baseline smoke on the real BIOS, one ledger row per title. (The census rows
   are most of this already.)
3. HLE boot with our own graphics and no BIOS file present.
4. Routine by routine, each held against the real BIOS on every corpus call.
5. Cycle accounting per routine, starting from the `cyc/call` column.
