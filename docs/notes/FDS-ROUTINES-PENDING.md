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

## `$E153`: delay Y milliseconds

Not part of the family above, despite sitting among them.

**Duration = `5 + 1790*Y` cycles, with `Y=0` meaning 256**, plus whatever time
the caller's own interrupt handlers steal mid-delay. Forced `y=$01` gave 1795,
`y=$02` 3585, `y=$0A` 17905, exact. 1790 cycles is 1.0001 ms NTSC, so the
interface really is "delay Y milliseconds".

`A` is preserved (forced `$80` came back `$80`). **`X` and `Y` both come out
`$00`**, and `X` is clobbered even though it is not an argument.

Uninterrupted exit flags: **`N=0`, `Z=1`, `C=0` always; `V`, `D`, `I`
preserved.** Note it preserves `V` where `$E149` clears it, so the two are not
the same kind of delay. Decimal-proof: `D=1` left `y=$01` at exactly 1795.

It reads `$0000` repeatedly during the wait but **the value is irrelevant**:
forcing `$00`, `$01`, `$70`, `$80` and `$FF` all gave byte-identical
466,204-cycle calls, so those are dummy reads and ours need not reproduce them.
No writes anywhere, no stack writes, exit `rts`.

**Loose end.** Calls with an NMI landing inside came back with `C=1` on Lutter.
Uninterrupted calls are always `C=0`, so that is the game's own handler touching
its stacked `P` rather than anything the routine does. Separating it further
would mean instrumenting Lutter's NMI handler.

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
sits on.
