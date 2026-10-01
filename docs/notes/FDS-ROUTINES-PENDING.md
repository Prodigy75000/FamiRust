<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# FDS BIOS routines measured but not yet written

Measurements that cost real time to obtain and would cost it again. Each entry
here is ready to implement; the work left is 6502 and tests, not investigation.

Everything came from black-box observation of the real BIOS: `fdsprof` access
logs, forced registers and RAM through `--force`, and `fdsdiff --probe` control
paths. Nothing was read out of the ROM. Where something could not be measured
it says so.

---

## The PPUMASK family: `$E161`, `$E16B`, `$E171`, `$E17E`, `$E185`

Five entry points that are one routine with five masks. **`$E161` alone blocks
10 titles** and is top of the queue; the rest come nearly free once it is in.

All five have the identical shape. They read the PPUMASK shadow at `$FE`,
transform it, and write the result to `$FE` **and then** `$2001`, shadow first.
They read `$2002` never, so they are callable mid-frame and what a mid-frame
`$2001` write does to the picture is the caller's problem.

| entry | `$FE`=`$00` gives | `$FF` gives | transform | what it is | cycles |
|---|---|---|---|---|---|
| `$E161` | `$00` | `$E7` | `and #%11100111` | screen off | **18** |
| `$E16B` | `$18` | `$FF` | `ora #%00011000` | screen on | **21** |
| `$E171` | `$00` | `$EF` | `and #%11101111` | sprites off | **21** |
| `$E17E` | `$00` | `$F7` | `and #%11110111` | background off | **21** |
| `$E185` | `$08` | `$FF` | `ora #%00001000` | background on | **21** |

Emphasis bits (5-7) and left-column bits (0-2) pass through untouched. Bubble
Bobble's natural `$26` comes back `$26`, which is what first proved these are
masks rather than constants.

**Interface, identical across all five.** No register arguments: forced
`a=$FF x=$AA y=$55` changed nothing, and the 13 callers of `$E161` enter with
freely varying registers. No inline arguments; `rts` to `jsr+3`. `A` = the
masked result, always clobbered. `X` and `Y` preserved. `N` and `Z` from the
result; **`C`, `V`, `D` and `I` all preserved** (`p=$61` in came back `$61`;
`p=$E9` with a zero result came back `$6B`).

**No stack writes at all.** Unlike `$E149` these do not even `pha`, so the
bytes below the returned stack pointer come back exactly as the caller left
them. The complete bus activity of one call is: read `$FE`, write `$FE`, write
`$2001`, then the `rts` reads.

Cycles are min=max across every caller and every forced state including `D=1`.

**Layout.** `$E161`'s body is ten bytes and `$E16B` is ten bytes away, so it
fits flush with no trampoline. The 21-cycle shape is `2 + 3 + 16`, i.e.
`lda #mask` then `jmp` to a shared tail, which also fits the byte budget:
`$E16B` has only six bytes before `$E171`, and `$E185` only six before the NMI
handler at `$E18B`. Two shared tails are needed, one `ora $FE` and one
`and $FE`, each `sta $FE` / `sta PPUMASK` / `rts` and each 16 cycles.

**Not determined.** The set is conspicuously missing a "sprites on"
(`ora #%00010000`). If it exists it is in the 13-byte `$E171-$E17D` gap, but no
corpus title enters anything there, so it cannot be located black-box and the
stub can stay.

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
