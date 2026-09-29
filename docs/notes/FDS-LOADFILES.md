<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# `$E1F8` LoadFiles, measured

The general "load files from disk" entry point: 41 of the 114 corpus titles
call it, the joint most-used routine after the NMI handler, and the one a game
uses to pull in a level or a new bank after boot. It is not yet implemented.
This is the measured interface, written down so the measuring does not have to
be done twice.

Everything here came from black-box observation of the real BIOS under an
access log. Nothing was read out of the ROM. Where something could not be
measured it says so.

## The call

Two little-endian pointer words sit **inline after the `jsr`**, and the routine
rewrites the stacked return address to step over them, so it returns to
`jsr + 5`.

It has to be a genuine pop, read, push-back-plus-four. Topple Zip enters by
`jmp` with a word it arranged on the stack itself, so a version that peeked at
`sp+1`/`sp+2` without rewriting would break it.

**First word: a 10-byte disk-ID template**, compared byte for byte against
block-1 offsets `$0F-$18` (maker, game name, type, revision, side, disk number,
disk type, `$18`). A template byte of `$FF` is a per-byte wildcard, verified at
positions 0 and 5: patched to `$FF` the call succeeds, patched to a wrong value
it errors.

**Second word: a file-ID list, one byte each, terminated by `$FF`.** For every
file header on the side the list is rescanned from the start; if the header's
ID matches any entry the body loads, otherwise it is read and thrown away.

- Duplicate IDs all load. Bio Miracle's list is `1f ff` and two files carry ID
  `$1F`, so it returns 2.
- `$00` is an ordinary ID, not a terminator.
- Order in the list does not matter; files load in disk order.
- **A list whose first byte is `$FF` means the boot set**: every file with ID at
  most the boot read file code at disk-info byte `$19`, the same rule our boot
  loader already implements. Proved by relocating Adian's list to a bare `ff`
  and getting exactly its six ID-below-16 files; `ff 10 ff` behaves identically,
  so the rest of a list after a leading `$FF` is never consulted.

The list is re-read from RAM at **every** header, so a file that loads over the
list changes the selection for later files. That is real behaviour, not an
artefact, and it bit the measuring: a patched list at `$68D3` was half restored
mid-call by a 16 KB file landing on `$6000-$9FFF`.

There is **no destination override**. Where a file goes and whether it goes to
CPU or PPU space is entirely the header's business.

## What comes back

| | |
|---|---|
| `A` | status: `$00` success, otherwise the error number in **BCD** |
| `X` | the same as `A` |
| `Y` | how many file bodies actually loaded, also left in `$0E` |
| flags | `N=0`, `V=1`, `C=0` on both paths, `Z` per `A`, and **`I=0`** |

Carry is not a status; `beq` is how a caller checks success. Interrupts are
enabled on exit even when the caller entered with `I` set, so the routine does
its own `cli`.

**A file that is not on the side is not an error.** A list naming only an absent
ID returns success with `Y=0`, so a game that cares has to look at `Y`.

Disk-ID mismatch error codes, by which template byte failed: byte 0 gives `$04`,
bytes 1-4 give `$05`, then `$06`, `$07`, `$08`, `$09`, and byte 9 gives `$10`.
That last one is what proves the codes are BCD: `$08` holds `$0A` in binary at
the same moment. The compare stops at the first bad byte.

## What it does to the machine

It **always rewinds**, every call: stop the motor, a counted delay of about
916,500 cycles with nothing polled, restart, a second delay of about 268,500,
write `$FF` to `$4026` and to its shadow `$F9`, read `$4033` once for the
battery, a second stop/start, then poll `$4032`. First transfer arm at about
1,672,766 cycles.

It **never stops early**. Block 1, block 2, then exactly block 2's file count of
header and body pairs, reading every byte of skipped bodies through `$4031` as
well. A list wanting only file 5 of 10 still costs the full 9,915,551 cycles.
The 8.4M average is structural: roughly 1.67M of prelude, then the side's whole
length at 149 cycles a byte, about 9,030 cycles of gap per block, and about 100
of epilogue.

Mechanically it is **IRQ-driven rather than polled**: `$4025` gets `$6D` to arm
and `$ED` to arm with the byte IRQ, and one interrupt per byte reads `$4031`.
CRC is checked on the info block, the count block, every header and every
**loaded** body, but not on skipped ones.

Zero page `$00-$0E` is its working set, and the exit values are part of what a
game can see:

| | |
|---|---|
| `$00/$01` | the template pointer, as passed |
| `$02/$03` | the list pointer |
| `$04` | entry `SP` minus 3 |
| `$05` | `$02` on success, `$00` on error |
| `$06` | files left to walk, ends `$00` |
| `$07` | current block type, ends `$04`; on error the type that failed |
| `$08` | the boot read file code from byte `$19` |
| `$09` | `$FF` if the last file examined was skipped, `$00` if loaded |
| `$0A/$0B` | destination pointer, left past the end of the last loaded file |
| `$0C/$0D` | bytes-remaining countdown, ends `$FFFF` |
| `$0E` | files loaded, the same as `Y` |

Outside zero page: `$F9` is the `$4026` shadow, `$FA` is the `$4025` shadow and
**every** `$4025` value is built from it, preserving the mirroring bit, and it
is left as `(entry & $0F) | $23`. `$0101`, the IRQ vector selector, is read at
entry, used throughout as the transfer-IRQ state and **restored** on exit.
`$0100`, `$0102/$0103` and `$FF` are untouched.

When and only when a body is about to stream to the PPU it blanks the screen,
once per PPU file, writing `$2001` and the shadow `$FE`, and it does **not**
put the screen back; games do that themselves.

## Not measured, and how to measure it

Stated rather than guessed, because each of these is a place a plausible
implementation could be quietly wrong:

- The error codes for a corrupted `*NINTENDO-HVC*` string, a CRC failure, a
  truncated side, no disk, and a flat battery. None can be reached from a disk
  that also boots, so settling them needs fault injection in `fds.rs`: a debug
  hook that corrupts a byte or fails a CRC after boot.
- Whether the screen blank is `and #%11100111` or a constant `$06`. Every
  observed shadow masks to `$06`, so the two are indistinguishable here. A
  probe that sets a caller's `$FE` to `$1E` before the call would separate them.
- Whether a caller can watch `$0101` from its own NMI handler mid-call. None of
  the profiled callers runs game code inside the call, so a polled
  implementation is observably equivalent for them, but it is an assumption
  about the other 36.
- The home of the retry counter, and the error-path zero-page leftovers, which
  were sampled on one title only.

## Implementing it

Our boot loader already has the drive protocol: `read_byte`, `next_block`, and
the block walk in `load_disk`. What it does not have is the right working set.
The boot helpers stage through `$E0-$E8` and `$07F0`, which is boot scratch and
is **game RAM** after handover, so this routine has to work in `$00-$0E` where
the real one does. Either stream the headers rather than buffering them, or move
the boot loader onto the same cells, which costs nothing because boot wipes its
scratch before handover anyway.

Polling instead of taking the byte IRQ is fine on the evidence above, provided
`$0101` is saved and restored and the exit does its `cli`.
