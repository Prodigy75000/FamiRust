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

## `$EC22`: measured 2026-10-10, one field short of writable

A metasprite blitter, and the largest routine left in the queue by a wide
margin: 1072 memory accesses for one call against about 200 for the whole of
`$E8D2`. It blocks Druid and Youkai Yashiki.

**It takes a pointer in `$00/$01` to a 12-byte parameter block** and writes an
N by M grid of OAM entries into the sprite page. It touches NO hardware at
all, in either caller: no `$20xx`, no `$40xx`. It borrows `$02-$0C`.

| field | Druid | Youkai | what it is |
|---|---|---|---|
| +00 | `$00` | `$00` | read, use not established |
| +01 | `$00` | `$70` | goes to `$08`, the Y origin |
| +03 | `$82` | `$E0` | goes to `$0A`, the X origin |
| +05 | `$0B` | `$00` | frame stride, added once PER SPRITE |
| +06 | `$00` | `$78` | tile source, high byte |
| +07 | `$8A` | `$5B` | tile source, low byte |
| +08 | `$00` | `$00` | attribute base |
| +09 | `$03` | `$02` | added to +08 to make the attribute |
| +0A | `$21` | `$64` | rows in the high nibble, columns in the low |
| +0B | `$38` | `$00` | OAM offset within page 2 |

Each of those was forced one at a time on Youkai rather than inferred:

  * `+0A` is a product and the nibbles are NOT interchangeable. `$11` writes 4
    bytes and reads one tile; `$12` and `$21` both write 8 and read two, but
    cost 492 and 466 cycles, so one nibble is the inner loop and one the outer.
  * `+05` moves the tile source by one whole FRAME, not one byte. Forced 0 to 1
    it moved `$785b-$7872` to `$7873-$788a`, exactly +24, which is the sprite
    count. That is why the field is read 24 times in a 24-sprite call.
  * `+08` and `+09` are SUMMED. `$00+$03` gives the `$03` Druid writes and
    `$00+$02` the `$02` Youkai writes. Either alone would have looked right.
  * `+0B` is an offset, not a flag: forced `$80` moved the block to
    `$0280-$02df` and `a` on exit moved with it.

**Two passes.** The first writes Y, attribute and X at OAM +0, +2 and +3,
walking a strip downward by 8 and then stepping the X origin on by 8 for the
next strip. The second walks back over the block writing the tile bytes at +1.
`$07` and `$08` are REUSED between the passes, which is what made this hard to
read: in the first pass `$07` is the strip counter, in the second it is the
tile source low byte. `a` on exit is `+0B + 4*sprites + 1`.

**What is not established, and it is the thing that blocks writing it.** How
the tile byte is fetched has two paths and only one is understood. With the
source high byte nonzero it is a plain `(ptr),y` read: Druid forced to `$78A0`
read `$78a0-$78a1`, two bytes for two sprites. Unforced, Druid's source is
`$00A0` and it instead reads `$0040-$0041` AND `$0140-$0141`, two parallel
tables at the same offset in pages 0 and 1, with `y` coming out at `$A2`
instead of `$02`. Youkai forced to a `$00` high byte stopped reading PRG and
read `$00b6-$00cd` instead. So the high byte selects a path, and the second
path has not been mapped.

**The third caller cannot currently be profiled.** Kaettekita Mario Bros.
enters `$EC22` 1386 times under the real BIOS, which the census rows confirm:
462 each from `$A386` by `jsr` and from `$A391` and `$A3A4` by `jmp`. All three
are game code. `fdsprof` reports zero calls because it does not press START, so
that title never leaves its title screen under the profiler. Giving `fdsprof`
an input option is the cheapest way to a third caller, and `fds-blockers.py`
already mashes START so the machinery exists to copy.

**And the reason this was promoted above `$E9D3` is dead.** The note below
guessed `$EC22` was what services or disables the FDS timer, on the strength of
the real run calling it 1386 times where ours never does. It touches no
hardware whatsoever, so that reading is wrong. The 1386 calls and the timer
difference are both real and the link between them is not. The Kaettekita jam
needs its own investigation.

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
