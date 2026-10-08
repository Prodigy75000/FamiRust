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

## Where the HLE intercepts (written before it was built)

A **synthetic 8 KB image**, not CPU address traps: our own artwork where the
real BIOS keeps its graphics, a reserved opcode plus a routine id at each
published entry point, and one dispatch arm in the CPU. Save states stay
trivial, entry addresses land where games expect them, and the cycle charge is a
table sitting next to the id. The census makes this cheap, because only fourteen
titles read the image as data at all.

## The BIOS itself

`firmware/fds-hle/src/main.s`, assembled by our own `nes-asm` into the 8 KiB
image `nes-core` carries with `include_bytes!`. Build it with
`scripts/build-fds-bios.sh`; `cargo test` checks that the committed image is
what the committed source assembles to, and that the core carries that same
image rather than a stale one.

**It is 6502, not host code, and the census is why.** The plan in the previous
section was a synthetic image full of trap opcodes with the work done in Rust.
One number killed it: `$E1F8` averages 8.4 million cycles a call, and a single
Bio Miracle call took 9,911,532, which at `BYTE_CYCLES = 149` is one whole side
of a disk. A host-side routine has to invent that time, and keep inventing it
while the game's own interrupt handlers run inside the wait. Doing the work in
6502 gets the timing right by doing the work. It also makes save states free,
because the BIOS window already serializes.

### What was measured, and how

Everything below came out of `fdsdiff`, which boots a disk under two BIOSes and
compares the machine each hands the game at **handover**: the instant the BIOS
dispatches through the `$DFFC` pseudo-vector.

**The file-selection rule.** A file loads if its ID is at most the boot read
file code at byte **`$19`** of the disk-info block. Byte `$1A`, which is what a
first reading suggested, is `$FF` on every disk and would load everything; Bio
Miracle loads two of its seven files. The comparison is `<=` rather than `<`,
which Falsion settles: boot code 15, and its ID-15 file is loaded.

**The NMI vector selection is real, and `$0100` drives it.** The first version
dispatched through `$DFFA` unconditionally, because all three disks here use it.
A corpus scan found Doki Doki Panic on vector 1 and Tama & Friends on vector 2,
with all three of their vectors distinct at the moment of dispatch. Which byte
selects was then found by correlation rather than by reading the ROM: ten disks
run to their first NMI, work RAM captured at that instant, all 2048 bytes tested
for whether their value predicts the vector. Exactly one survives.

| `$0100` | N (bit 7) | V (bit 6) | vector |
|---|---|---|---|
| `$C0` | 1 | 1 | 3, `$DFFA`. The value the BIOS leaves at boot |
| `$80` | 1 | 0 | 2, `$DFF8` |
| `$40` | 0 | 1 | 1, `$DFF6` |
| `$00` | 0 | 0 | a `$E1B2` wait is parked: finish it |

The `$00` row was marked "never observed; we return" and the `rti` we had put
there was WRONG. Profiling `$E1B2` showed what a zero selector means: not "no
handler installed" but "somebody is parked in a vblank wait and the NMI owns
their exit". That arm now drops the interrupt frame and returns to whoever
called `$E1B2`, several frames up the stack. Three byte-budget predictions
agree with it and are asserted at build time: the arm begins at `$E19D` where
our branch targets already put it, it is exactly 21 bytes so it ends on
`$E1B2`, and `$E1B2`'s own prelude is 19 bytes so its spin sits at `$E1C5`,
which is the interrupted PC on the stack in every measured call and one byte
short of the IRQ entry.

The cycle counts confirm it independently. A `bit` test plus two branches plus
`jmp (ind)` costs 13 cycles for `$C0` and 14 for either of the others, which is
exactly what the real handler measures, and the branch targets land on the same
addresses its control flow visits.

**The BIOS boots with horizontal mirroring.** Not a detail: a file of kind 2
loads into the nametables at the address its header names, and which physical
bank that is depends on `$4025` bit 3. With it clear, Zelda's licence screen
landed in bank 0; the real BIOS puts it in bank 1, and with it set all 224 bytes
match the oracle.

**The registers at handover are NOT a contract**, though they looked like one.
Zelda, Bio Miracle and Famicom Grand Prix are all handed `a=$10 x=$00 y=$FF
sp=$FF p=$20`. Xevious is handed `x=$FF y=$00 p=$21`. They are whatever the real
BIOS's last instruction left behind, so nothing can depend on them. We set a
fixed, sane set.

### Where it stands: 114 of 114

`scripts/fds-hle-check.py` boots every disk in the corpus under both BIOSes.
**All 114 are handed the machine the real BIOS hands them**: 72 of 72 originals,
40 of 40 translations, both hacks. The contract is reaching handover at the same
entry address with identical program RAM. 67 of them also match byte for byte on
pattern RAM; the rest differ only in whose boot artwork is left lying in it.

Two earlier numbers from this same check were wrong, and both were the tooling
rather than the BIOS. They are worth recording because each is easy to repeat:

- **108 of 114**, because the check required the handover REGISTERS to match and
  those vary by disk under the real BIOS, and because `fdsdiff` was calling the
  first exit from the BIOS window a handover. The real BIOS leaves NMI enabled
  while it loads, so on a disk whose files install an NMI handler early the
  first exit is an interrupt being dispatched mid-load. That made the six Namco
  titles look like they disagreed with us when they did not.
- **113 of 114**, because a mutation test overwrote `fds-hle.bin` while the
  corpus check was reading it, so one disk was measured against a deliberately
  broken BIOS. Do not mutate a shared artifact while a background job reads it.

Work RAM, leftover boot artwork and the registers are reported and not required,
for the reasons above and in the script's own header.

### `$0101` is not the mirror of `$0100`

Written down because the symmetry is inviting and it is wrong. `$0100` is a
three-way NMI vector selector tested with `bit`; `$0101` looked like the same
thing for the single IRQ vector, so the first IRQ handler here dispatched on
bit 7 and said so in a comment.

Measured by forcing the byte and watching where control goes:

| `$0101` | what the real BIOS does |
|---|---|
| `$C0` | hands the interrupt to the game through `$DFFE`, 14 cycles, a plain `jmp`. Seen live on Bio Miracle and Arumana no Kiseki |
| `$80` | 164 cycles, and it reads `$4030`, the drive's status register |
| `$40` | into the BIOS's own transfer machinery around `$E6A6`. Seen live on Akumajou Dracula |
| `$00` | 33 cycles, a third path |

So it is the state machine of the BIOS's own disk-transfer interrupt, and only
`$C0` means "this one belongs to the game". That agrees with what profiling
`$E1F8` found separately: it drives `$0101` through a load, holding `$40` per
block and counting `$08` down to `$00` in the gaps, and restores the caller's
value on exit.

**Our loader polls the drive and the timer is off at boot, so we have no
transfer to service**, and every interrupt after handover is the game's. We
therefore dispatch on bit 7 rather than on `$C0` exactly. That is a deliberate
simplification of a rule we have measured and chosen not to implement, which is
a different thing from an approximation of one we never worked out. Returning
instead would drop an interrupt a game enabled on purpose.

Open: what the `$80` path does once it reads `$4030` and finds nothing pending.
That is the case a game would actually hit, because `$80` is what boot leaves
in `$0101`. Settling it needs the transfer machinery built, which is the same
work as taking transfer interrupts in the loader.

### The routines

Twenty-five of the forty entry points are implemented. The other 15 are stubs that put
**NO ROUTINE** on the screen and stop, which is worth its 111 bytes: Zelda hands
over correctly, runs its own code, jumps to `$EA84` and would otherwise
disappear into the fill, looking like a hundred different bugs instead of one
missing routine. `fdstrace` names the address.

| entry | titles | what it is | note |
|---|---|---|---|
| `$E18B` | 107 | NMI dispatch | cycle-exact, 13/14 |
| `$E1C7` | 23 | IRQ dispatch | 11 cycles against 14 |
| `$EE24` | 6 | reset | |
| `$EA84` | 41 | fill a page of video memory | cycle-exact, 9889 |
| `$EAEA` | 32 | scroll and PPUCTRL from the zero-page shadows | cycle-exact, 31 |
| `$E9C8` | 27 | sprite DMA from page 2 | cycle-exact, 18 |
| `$EAD2` | 27 | fill whole pages of RAM | 2 cycles fast at entry |
| `$EA1F` | 26 | read both pads, work out newly-pressed | faster; exit carry not reproduced |
| `$E1F8` | 41 | load files from disk | written; polls where the original takes the byte IRQ |
| `$EBAF` | 29 | copy 16-byte units from RAM to video memory | faster: ~282 cycles a unit against 362 |
| `$E1B2` | 18 | wait for the next vblank NMI | and it corrected the NMI handler, below |
| `$E7BB` | 18 | walk a VRAM write structure the caller points at | a little language, measured field by field |
| `$E149` | 17 | delay exactly 131 cycles | cycle-exact, and the one place faster is WRONG |
| `$EA4C` | 17 | read both pads twice and believe a repeated answer | shares its read pass with `$EA1F` |
| `$E161` | 13 | PPUMASK: screen off | cycle-exact, 18 |
| `$E185` | 13 | PPUMASK: background on | cycle-exact, 21 |
| `$E16B` | 8 | PPUMASK: screen on | cycle-exact, 21 |
| `$E171` | 6 | PPUMASK: sprites off | cycle-exact, 21 |
| `$E17E` | 2 | PPUMASK: background off | cycle-exact, 21 |
| `$E9B1` | 10 | shift a zero page register right one bit, with feedback | a random number generator; access-trace-exact, 3 cycles fast |
| `$EAFD` | 11 | jump through a table of addresses written after the call | a tail call: it consumes its own frame. Cycle-exact, 45 |
| `$E86A` | 10 | flush the queued VRAM transfer buffer at `$0302` | cycle-exact; NOT `$E7BB`'s length language |
| `$E153` | 7 | delay `y` milliseconds | cycle-exact, 5 + 1790*y; the second place faster is WRONG |
| `$E8D2` | 5 | append one block to the queued VRAM transfer buffer | the producer side of `$E86A`; 182 + 28 a byte against 224 + 41 |
| `$E8E1` | 3 | append several blocks, the shape packed into the data | shares its whole tail with `$E8D2`; written with it because Druid needs both |

**Where the cycle counts do not match, ours is faster, never slower.** That is
deliberate. A game that runs one of these inside vblank has more room than it
had rather than less, so being quick is safe and being slow is not. The
exception is interrupt dispatch, where arriving early moves every raster split
on the screen, and that one is matched exactly.

Four of the seven came out cycle-exact without being aimed at, which is the
strongest evidence available that the reconstructions are right rather than
merely adequate. `$EA84` in particular lands its `rts` on `$EAD1`, filling
precisely the 78 bytes between its entry point and the next one.

### Which routine to write next, which is not what the census says

The census ranks entry points by how many titles CALL them. That is the right
order to measure in and the wrong order to implement in, because a title that
calls five routines is stopped by whichever one it reaches FIRST, and the other
four buy it nothing until that one exists.

`scripts/fds-blockers.py` runs every corpus disk on OUR BIOS and asks the other
question. Measured 2026-10-01, with ten routines written:

| entry | titles that call it | titles STUCK on it |
|---|---|---|
| `$E149` | 17 | **15** |
| `$E161` | 13 | **9** |
| `$EA4C` | 17 | **8** |
| `$E7BB` | 18 | **6** |
| `$EAFD` | 11 | **5** |
| `$E9B1` | 10 | **4** |
| `$E1B2` | 18 | **4** |

The two the census ranked joint first, `$E1B2` and `$E7BB`, blocked four and six
titles. `$E149`, ranked below both, blocked fifteen. Popularity and blocking are
different numbers and only one of them is a work queue.

Re-measured after writing those three, which is the point of a queue that
updates:

| entry | titles stuck on it now |
|---|---|
| `$E161` | **10** |

Then `$E161` was written, and with it the four other PPUMASK masks that share
its shape, which cost almost nothing once the first was in. Re-measured again:

| entry | titles stuck on it now |
|---|---|
| `$E9B1` | **7** |
| `$EAFD` | **5** |
| `$E86A` | **5** |
| `$E8D2` | **3** |
| `$E153` | **3** |
| `$E9D3` | **2** |

Then `$E9B1`, and the queue reshuffled again rather than simply shortening,
because four of the seven titles stuck on it went all the way through while
three hit the next thing they needed:

| entry | titles stuck on it now |
|---|---|
| `$EAFD` | **7** |
| `$E86A` | **6** |
| `$E8D2` | **3** |
| `$E153` | **3** |
| `$E9D3` | **2** |

Then `$EAFD` and `$E86A`. The queue keeps reshuffling rather than simply
shortening, because a title freed from one routine stops at the next thing it
needs:

| entry | titles stuck on it now |
|---|---|
| `$E153` | **4** |
| `$E8D2` | **4** |
| `$E9D3` | **3** |

Then `$E153`:

| entry | titles stuck on it now |
|---|---|
| `$E8D2` | **4** |
| `$E9D3` | **3** |
| `$E239` | **2** |

Then `$E8D2`, and `$E8E1` with it. Writing the pair together was not tidiness:
Druid was one of the four stopped on `$E8D2` and calls `$E8E1` as well, so
shipping the first alone would have moved its wall fifteen bytes down the page
and bought it nothing. Three of the four went all the way through; Druid
stopped at `$EC22`, which was already on the list for another reason.

| entry | titles stuck on it now |
|---|---|
| `$E9D3` | **3** |
| `$E239` | **2** |
| `$EC22` | **2** |
| `$E844` | **2** |

**102 of the 114 now reach their own code without asking for a routine we have
not written**, up from 50 when the queue was first measured, in an unattended
1800-frame run with START mashed. 12 are still stopped by a missing routine,
and nothing left blocks more than three.

`$EC22` is the one to take next even though two others tie it at two titles: it
is also what the Kaettekita Mario Bros. jam points at, so it is worth three.

Ninety-nine is NOT "ninety-nine titles play correctly": thirty seconds of
unattended running reaches what it reaches, and a game that needs a routine
only when you open its menu has not asked yet.

### How a routine gets worked out

`fdsprof` puts the machine under an access log for the length of one call and
reports registers in and out, exit kind, cycles, and every address outside the
BIOS window that was read or written. A routine's interface **is** that set of
addresses. `scripts/fds-routine.py` runs it across every corpus title that calls
the routine, because one game's call shows what that game passes and six show
which registers are arguments.

`crates/nes-core/tests/fds_hle_routines.rs` is the other half: it boots a real
disk to handover under both BIOSes, plants a caller in work RAM, and compares
what comes back. That is the oracle at the level the work happens at.

And `scripts/mutate-fds-queue.py` is the third half, for the two queue
routines: it breaks the implementation fourteen different ways and checks a
test notices each time. Two things it does are the point of it rather than
incidental. It asserts the anchor matched EXACTLY once and that the assembled
image actually moved, because a mutation that failed to apply looks exactly
like a suite that caught it. And it records the one mutation that survived
along with the proof that it is a no-op: swapping `dec $06` and `sta BUF,x` in
the copy loop changes the bytes and cannot change anything observable, since
the store never touches `$0006`, sets no flags, and costs the same. An
unobservable mutation is not a hole in the tests, and saying so beats leaving
the next person to find out.

## Running order

1. ~~Dynamic tracer, then a census across the corpus.~~ Done 2026-09-29.
2. ~~Baseline smoke on the real BIOS, one ledger row per title.~~ The census
   rows and `fds-hle-check.py` are this.
3. ~~HLE boot with our own graphics and no BIOS file present.~~ Done 2026-09-29.
4. Routine by routine, each held against the real BIOS. Twenty-five of forty
   done. The queue by BLOCKING is now a tail: `$E9D3` at three, then twos and
   ones. See
   [`FDS-ROUTINES-PENDING.md`](FDS-ROUTINES-PENDING.md), which records why
   Kaettekita Mario Bros. jams, what `$EC22` has to do with it, and why a flag
   measured on a live game is a flag measured through its interrupt handlers.
5. Cycle accounting per routine, starting from the `cyc/call` column.
