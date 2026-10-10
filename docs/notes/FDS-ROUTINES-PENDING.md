<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# FDS BIOS routines measured but not yet written

Measurements that cost real time to obtain and would cost it again. Each entry
here is ready to implement; the work left is 6502 and tests, not investigation.

Everything came from black-box observation of the real BIOS: `fdsprof` access
logs, forced registers and RAM through `--force`, and `fdsdiff --probe` control
paths. Nothing was read out of the ROM. Where something could not be measured
it says so.

---

## The PPUMASK family: WRITTEN 2026-10-07

`$E161`, `$E16B`, `$E171`, `$E17E` and `$E185` are implemented and held against
the real BIOS in `crates/nes-core/tests/fds_hle_routines.rs`. The measurement
that built them now lives beside the code in `firmware/fds-hle/src/main.s`,
which is where it belongs once there is code for it to describe.

One thing from the measuring is worth keeping here because it is still open:
the set is conspicuously missing a "sprites on" (`ora #%00010000`). If it
exists it is in the 13-byte `$E171-$E17D` gap, but no corpus title enters
anything there, so black-box observation cannot locate it and the stub stays.

---

## `$E153`: WRITTEN 2026-10-08, and the note below was wrong about carry

Implemented and held against the real BIOS. The duration measurement here was
right (`5 + 1790*Y`, `Y=0` meaning 256, decimal-proof) and is now reproduced
exactly, including at `Y=0`.

**The flag measurement was wrong, and the way it was wrong is the lesson.**
This note recorded "`N=0`, `Z=1`, `C=0` always" with an added "loose end" that
an NMI landing mid-call gave `C=1` on Lutter. Both halves were backwards. The
`C=0` readings came from `fdsprof` on a LIVE game whose calls were taking
interrupts, so the handler was altering the P it had pushed; the uninterrupted
value was the thing the note called the anomaly.

Re-measured with NMI off through `$2000`, with `I` set, and on calls proven by
the access log to make no writes at all and therefore to take no interrupt:
**carry comes out as `a >= $20`**, a clean threshold with `$1F` giving 0 and
`$20` giving 1. It does not depend on the carry coming in, on `y`, or on
decimal mode. A first correction said "always set", which was also wrong and
only looked right because that probe happened to hold `a`=$5A.

The general point for anything still on this page: **a flag measured on a live
game is a flag measured through that game's interrupt handlers.** Turn NMI off,
set `I`, and confirm from the access log that the call made no stack writes
before believing any flag it reports.

---

## `$EC22`: fully measured 2026-10-10, ready to write

A metasprite blitter, and the largest routine left in the queue: 1072 memory
accesses for one call against about 200 for the whole of `$E8D2`. It blocks
Druid and Youkai Yashiki, and Kaettekita Mario Bros. calls it 1386 times.

It touches NO hardware at all in any of the three callers: no `$20xx`, no
`$40xx`. It borrows `$02-$0C` and writes nothing else outside the sprite page.

**`$00/$01` point at a 12-byte parameter block. `a`, `x` and `y` are NOT
arguments**: forced to `$00`, `$FF` and `$05` the call came out identical. All
three callers happen to pass the block's own high byte in `a`, which is what
made it look like one.

| field | Druid | Youkai | Kaettekita | what it is |
|---|---|---|---|---|
| +00 | `$00` | `$00` | `$00` | control, see below |
| +01 | `$00` | `$70` | `$B7` | Y of the top row |
| +02 | | | | NEVER READ |
| +03 | `$82` | `$E0` | `$F5` | X of the left strip |
| +04 | | | | NEVER READ |
| +05 | `$0B` | `$00` | `$00` | frame stride |
| +06 | `$00` | `$78` | `$A5` | tile source, high |
| +07 | `$8A` | `$5B` | `$82` | tile source, low |
| +08 | `$00` | `$00` | `$00` | flips |
| +09 | `$03` | `$02` | `$02` | attribute |
| +0A | `$21` | `$64` | `$32` | rows in the high nibble, strips in the low |
| +0B | `$38` | `$00` | `$48` | OAM offset within page 2 |

**Layout.** For each strip left to right, for each row top to bottom, one OAM
entry: `+0` = Y, `+1` = tile, `+2` = attribute, `+3` = X. Y steps by 8 down a
strip and X steps by 8 between strips. The real BIOS does it in TWO passes,
`Y`/attribute/`X` for every sprite and then the tiles, which is why `$07`
appears to change meaning half way through: in the first pass it is the strip
counter and in the second the tile source low byte. The final memory is what
matters, so one pass is fine and cheaper.

**+0A is a product and the nibbles are not interchangeable.** `$11` writes 4
bytes and reads one tile; `$12` and `$21` both write 8 and read two but cost
492 and 466 cycles, so one nibble is the inner loop and one the outer.

**+00 is a control byte, not a spare.** Every caller leaves it `$00`, so it had
to be forced:

  * `$00`: draw normally.
  * bit 7 set (`$80`, `$FF`): draw, but every sprite's Y is `$F4`, which is off
    screen. A hide flag.
  * any other nonzero (`$01`, `$08`): return IMMEDIATELY, nothing written, with
    `a` = +00 and `x` = +01.

**+06/+07 is the tile source and the HIGH BYTE PICKS THE MODE.** This is the
part that took longest and the evidence is worth keeping:

  * high byte nonzero: a pointer to a list of tile indices, one per sprite,
    read `(ptr),y`. Forcing Youkai's low byte to `$00`, `$10` and `$80` moved
    the reads to `$7800`, `$7810` and `$7880`, one for one, so there is no
    scaling. Poisoning the pointed-at bytes changed the tiles written.
  * high byte ZERO: +07 is a BASE TILE NUMBER and tiles run consecutively from
    it. Youkai forced to a `$00` high byte wrote `$5B $5C $5D $5E $5F $60`.
  * the `(ptr),y` read happens on BOTH paths and its result is discarded on the
    literal one. That is what Druid's phantom reads of `$0040` and `$0140` are:
    the discarded fetch plus its page-crossing dummy read, because on that path
    `y` carries the running tile number and was `$A0`. Proved by poisoning
    `$0040`/`$0140` with `$77` and then `$FF` and watching the tiles stay
    `$A0`/`$A1`, against the same poisoning on the pointer path moving them to
    `$77`/`$78`. The discarded read can never reach a register: with a zero high
    byte the address is at most `$01FE`, so skipping it is safe.

**+08 is flips and it REORDERS THE GRID.** bit 0 is vertical and bit 4 is
horizontal; nothing else in the byte does anything. Vertical sets attribute bit
7 and reverses the tile order WITHIN each strip; horizontal sets attribute bit
6 and reverses the order OF the strips. Positions do not move, so the hardware
flip bits do the per-tile mirroring and the routine just walks the tile list
from the other end. Measured on a 3x2 grid, all four combinations.

**+09 is the attribute and it is masked with `$23`**: palette in bits 0-1 and
priority in bit 5. `$FF` and `$E3` both give `$23`.

**A correction, because the first write-up of this had it wrong.** It said +08
and +09 were SUMMED, on the evidence that Druid's `$00+$03` gives `$03` and
Youkai's `$00+$02` gives `$02`. Both callers leave +08 at `$00`, which makes
"sum", "or" and "ignore +08 entirely" the same observation: the same
unfalsifiable shape as the mapper 228 fixture. Sweeping the two fields
independently is what separated them, and the right answer is
`attr = (+08 bit0 ? $80 : 0) | (+08 bit4 ? $40 : 0) | (+09 & $23)`. It also
retrodicts the one combined probe that had refused to fit either reading:
`+08=$0F, +09=$F0` gives `$80 | $20 = $A0`.

**Exit state.** `x` is `$00`. `a` and `$02` are both `+0B + 4*sprites + 1`,
confirmed on all three (`$38+8+1`, `$00+96+1`, `$48+24+1`). `y` is the sprite
count on the pointer path and base-tile-plus-count on the literal one. The
scratch comes out `$03`=`$02`, `$04`=rows, `$05`=strips, `$06`=`$00`,
`$07`/`$08`=the advanced tile source, `$09`=the attribute, `$0B`=`$00`,
`$0C`=rows. `$0A` is the one field whose exit value is not yet derived: it is
the X origin during the first pass and a counter in the second, coming out
`$FF` on Druid and `$00` on the other two.

**`fdsprof` needed `--mash` before any of this was possible.** Kaettekita only
calls `$EC22` once it is PLAYING, so with no input the tool reported zero calls
and the address looked dead on that title. `--mash` taps START on exactly
`fdstrace --mash`'s schedule, so a title reaches the same point it reached in
the census. The empty-result message now says so rather than implying the game
does not call it.

**The reason this was promoted above `$E9D3` was wrong.** The guess was that
`$EC22` services or disables the FDS timer, because the real run calls it 1386
times where ours never does. It touches no hardware whatsoever. The 1386 calls
are real, confirmed in the census rows at 462 each from `$A386` by `jsr` and
`$A391` and `$A3A4` by `jmp`, and the timer difference is real, and the link
between them is not. The Kaettekita jam needs its own investigation.

---

## Two notes about the address band, not routines

**`$E14A-$E15B` are not entry points.** They are addresses an `rts`/`rti`
landed on: games' NMI handlers resuming the interrupted body of `$E149` or
`$E153`. Their profiles confirm it, with `$E14D`/`$E14F` costing a variable
14-104 cycles depending on which loop phase they resumed at, `$E151` a constant
10, and `$E152` a bare 6-cycle `rts`. `scripts/fds-routine.py` used to merge
the census's entry and resumption tables and report these as popular entry
points; it now filters by kind.

**The real `$E149` body is self-contained in `$E149-$E152`, ending `pla`/`rts`.**
Ours trampolines to `$FB28`, so a game whose NMI handler inspected the stacked
PC mid-delay would see `$FB28+` where the real BIOS shows `$E149-$E152`. No
corpus title is known to do that, but it is the one observable difference our
trampoline layout creates, and it is the reason to prefer placing a body inline
when it fits.

---

## Kaettekita Mario Bros. jams, and it is not `$EAFD`

The one corpus title whose verdict is HALT rather than a clean NO ROUTINE, as
of 2026-10-07. Worth writing down because the surface symptom points at the
wrong place.

It used to stop at `$EAFD` with NO ROUTINE. With the dispatcher written it
gets further and then jams. The dispatcher is not at fault: profiled under both
BIOSes its four calls agree exactly, same registers in and out, same 45 cycles.

`fdstrace` now reports where a jam happened and the dozen addresses it came
through. For this title:

    jammed at $e3c3, reached via $e19f $e1a1 $e1a3 $e1a6 $e1a9 $e1aa $e1ab
                                 $e1ac $e1ad $e1b0 $e1b1 $e3c2

That trail is our own NMI handler's `@none` path at `$E19D`, the one that
unwinds a parked `$E1B2` wait: four `pla`s, `sta $0100`, a fifth `pla`, and an
`rts`. The `rts` at `$E1B1` returns to `$E3C2`, which is gap. So the handler
took the "a `$E1B2` wait is parked" branch when no wait was parked, and
unwound a stack frame that was not there.

**The divergence is upstream of that.** Under the real BIOS this title takes no
interrupts at all and the FDS timer IRQ is never enabled; under ours the timer
IRQ is on and `$E1C7` is entered 36,937 times in 600 frames. The real run also
calls **`$EC22` 1386 times** and ours never reaches it once, so the likeliest
reading is that `$EC22` is what services or disables that timer, and without it
the game is driven into a state the real BIOS never leaves it in.

**A separate real defect, found while reading this.** Our `$E1C7` is:

    bit $0101
    bpl @none
    jmp (VEC_IRQ)

which hands the interrupt to the game for `$0101` of `$80` as well as `$C0`.
Only `$C0` is the game's. `$80` is the real BIOS's own transfer path, measured
at 164 cycles and reading `$4030`. Fixing it properly means implementing that
path rather than just tightening the test, so it is written down here rather
than half-done.

Next step is to profile `$EC22`, which is also the last blocker Youkai Yashiki
sits on, and as of 2026-10-08 Druid as well: writing `$E8D2` and `$E8E1` freed
Druid from both and landed it here. Two titles plus this jam makes it the next
one to take even though `$E9D3` blocks three.
