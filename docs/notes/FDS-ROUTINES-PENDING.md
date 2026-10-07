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
