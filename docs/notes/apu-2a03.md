# NES 2A03 APU — Clean-Room Implementation Notes (NTSC)

Cycle-accurate implementation notes for a from-scratch Rust NES APU. Synthesized from the
NESdev wiki hardware-reference pages (APU, APU_registers, APU_Pulse, APU_Sweep, APU_Triangle,
APU_Noise, APU_DMC, APU_Frame_Counter, APU_Length_Counter, APU_Envelope, APU_Mixer).

Target correctness: Blargg `apu_test`, `blargg_apu_2005.07.30`, `apu_mixer`,
`length_counter`, and `frame_counter` test ROMs; correct game audio.

All timing here is **NTSC** (CPU = 1.789773 MHz). PAL differs; not covered.

---

## 0. Clock domains and terminology

- **CPU cycle**: the base clock the 6502 runs on. Everything in the NES is measured in these.
- **APU cycle**: 1 APU cycle = 2 CPU cycles. The pulse/noise/DMC timers and the frame sequencer
  effectively advance on APU cycles (every *other* CPU cycle). The triangle timer is the exception:
  it advances **every CPU cycle**.
- The APU is physically part of the 2A03 CPU die; register writes are ordinary CPU memory writes
  to `$4000-$4017`. `$4014` is OAM DMA (a CPU/PPU feature, NOT the APU) and is out of scope here.
- **Quarter-frame clock**: clocks envelopes + triangle linear counter.
- **Half-frame clock**: clocks length counters + sweep units. (A half-frame event ALSO clocks the
  quarter-frame units, because half-frame steps are a superset — see the frame-counter table.)

Recommended tick strategy: run the APU on CPU cycles. Maintain a `cpu_cycle_parity` bit; step the
"APU-cycle" units (pulse/noise/DMC timers, frame sequencer) on the even parity, and step the
triangle timer every CPU cycle. Alternatively drive the frame sequencer off an internal cycle
counter compared against the exact CPU-cycle thresholds in §2.

---

## 1. Register map $4000–$4017

| Addr | Ch | Function |
|------|-----|----------|
| $4000 | Pulse 1 | Duty, length-halt/envelope-loop, constant-volume, volume/envelope period |
| $4001 | Pulse 1 | Sweep: enable, period, negate, shift |
| $4002 | Pulse 1 | Timer low 8 bits |
| $4003 | Pulse 1 | Length-counter load (5 bits), timer high 3 bits |
| $4004 | Pulse 2 | (same layout as $4000) |
| $4005 | Pulse 2 | (same as $4001) |
| $4006 | Pulse 2 | (same as $4002) |
| $4007 | Pulse 2 | (same as $4003) |
| $4008 | Triangle | Linear-counter control/length-halt, linear-counter reload value (7 bits) |
| $4009 | — | Unused |
| $400A | Triangle | Timer low 8 bits |
| $400B | Triangle | Length-counter load (5 bits), timer high 3 bits |
| $400C | Noise | length-halt/envelope-loop, constant-volume, volume/envelope period |
| $400D | — | Unused |
| $400E | Noise | Loop/mode flag, noise period index (4 bits) |
| $400F | Noise | Length-counter load (5 bits) |
| $4010 | DMC | IRQ enable, loop, rate index (4 bits) |
| $4011 | DMC | Direct load: 7-bit output level |
| $4012 | DMC | Sample address (high byte) |
| $4013 | DMC | Sample length |
| $4014 | — | (OAM DMA — not APU) |
| $4015 | All | Write: channel enables. Read: length/DMC status + IRQ flags |
| $4016 | — | (Controller port 1 — not APU) |
| $4017 | All | Write: frame-counter mode + IRQ inhibit. (Read side is controller port 2) |

### 1.1 Pulse registers $4000/$4004 (`DDLC VVVV`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7-6 | `DD` | Duty index (0..3), selects one of 4 waveforms (§3.1) |
| 5   | `L`  | Length-counter halt **and** envelope loop flag (shared bit) |
| 4   | `C`  | Constant volume flag. 1 = use `VVVV` directly as volume; 0 = use envelope decay level |
| 3-0 | `VVVV` | Envelope period **and** constant volume value (dual use) |

### 1.2 Pulse sweep $4001/$4005 (`EPPP NSSS`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7   | `E`  | Sweep enable |
| 6-4 | `PPP`| Sweep divider period P (actual period = P+1 half-frames) |
| 3   | `N`  | Negate flag (subtract change amount) |
| 2-0 | `SSS`| Shift count |

Writing $4001/$4005 sets the sweep **reload flag**.

### 1.3 Pulse timer low $4002/$4006 (`TTTT TTTT`)
Low 8 bits of the 11-bit timer period `t`.

### 1.4 Pulse length/timer-high $4003/$4007 (`LLLL LTTT`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7-3 | `LLLLL` | Length-counter load index (5 bits, into table §7) |
| 2-0 | `TTT`   | Timer high 3 bits |

Side effects of writing $4003/$4007:
- Length counter is loaded from the table **if the channel is enabled** in $4015.
- The pulse **sequencer phase is reset** to step 0 (restart of the duty waveform).
- The envelope **start flag is set** (restarts the envelope; decay level → 15 on next quarter clock).

### 1.5 Triangle linear/control $4008 (`CRRR RRRR`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7   | `C`  | Linear-counter control flag **and** length-counter halt (shared) |
| 6-0 | `RRRRRRR` | Linear-counter reload value (0..127) |

### 1.6 Triangle timer low/high $400A / $400B
- $400A: low 8 bits of 11-bit timer.
- $400B (`LLLL LTTT`): bits 7-3 = length-counter load index; bits 2-0 = timer high 3 bits.
- Writing $400B: loads length counter (if enabled) **and** sets the linear-counter **reload flag**.

### 1.7 Noise $400C (`--LC VVVV`)
Same as pulse $4000 but bits 7-6 unused (no duty). `L` = length halt/envelope loop, `C` = constant
volume, `VVVV` = envelope period/constant volume.

### 1.8 Noise mode/period $400E (`L--- PPPP`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7   | `L`  | Mode flag (a.k.a. "loop noise"): selects LFSR feedback tap (bit1 vs bit6) |
| 3-0 | `PPPP` | Noise period index into NTSC table (§6) |

### 1.9 Noise length $400F (`LLLL L---`)
Bits 7-3 = length-counter load index. Writing $400F also **sets the envelope start flag**.

### 1.10 DMC flags/rate $4010 (`IL-- RRRR`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7   | `I`  | IRQ enable. If 0, the DMC interrupt flag is cleared immediately on write |
| 6   | `L`  | Loop flag |
| 3-0 | `RRRR` | Rate index into NTSC rate table (§5) |

### 1.11 DMC direct load $4011 (`-DDD DDDD`)
Bits 6-0 set the 7-bit output level directly (0..127). Bit 7 ignored.

### 1.12 DMC sample address $4012 (`AAAA AAAA`)
Sample start address = `$C000 + (A << 6)` = `$C000 + A*64`.

### 1.13 DMC sample length $4013 (`LLLL LLLL`)
Sample length in bytes = `(L << 4) + 1` = `L*16 + 1`.

### 1.14 Status $4015

**Write (`---D NT21`):** channel length-counter enables.

| Bit | Field | Effect |
|-----|-------|--------|
| 4 | `D` | DMC enable. 1 = start sample if bytes-remaining is 0; 0 = clear bytes-remaining (silences after current byte). Writing $4015 **always clears the DMC interrupt flag**. |
| 3 | `N` | Noise length-counter enable. 0 → force noise length counter to 0. |
| 2 | `T` | Triangle length-counter enable. 0 → force to 0. |
| 1 | `2` | Pulse 2 length-counter enable. 0 → force to 0. |
| 0 | `1` | Pulse 1 length-counter enable. 0 → force to 0. |

When an enable bit is **cleared**, that channel's length counter is forced to 0 and cannot be
reloaded until the enable bit is set again. When set, it does NOT reload the length counter by
itself — it just permits future loads.

**Read (`IF-D NT21`):** status + IRQ flags.

| Bit | Field | Meaning |
|-----|-------|---------|
| 7 | `I` | DMC interrupt flag |
| 6 | `F` | Frame interrupt flag |
| 5 | — | (open bus) |
| 4 | `D` | DMC active: 1 if DMC bytes-remaining > 0 |
| 3 | `N` | Noise length counter > 0 |
| 2 | `T` | Triangle length counter > 0 |
| 1 | `2` | Pulse 2 length counter > 0 |
| 0 | `1` | Pulse 1 length counter > 0 |

**Reading $4015 clears the frame interrupt flag** (bit 6), but NOT the DMC interrupt flag (bit 7).
The clear happens on read; if the frame IRQ is being set on the very same cycle, the read-clear does
not suppress that same-cycle set (an edge case some tests probe — verify against `frame_counter`).

### 1.15 Frame counter $4017 (`MI-- ----`)

| Bit | Field | Meaning |
|-----|-------|---------|
| 7 | `M` | Mode: 0 = 4-step sequence; 1 = 5-step sequence |
| 6 | `I` | IRQ inhibit. 1 = clear frame interrupt flag and prevent it from being set |

Write side effects: see §2.3.

---

## 2. Frame counter (frame sequencer)

The frame sequencer is driven by an internal divider counting CPU cycles. The canonical NESdev
tables are given in **APU cycles**; multiply by 2 (and account for the standard half-cycle offset)
to get **CPU cycles**. Both are given below. Implement with whichever domain your APU steps in, but
be exact.

### 2.1 Mode 0 — 4-step sequence (`$4017` bit7 = 0)

APU-cycle events (from the wiki), and their CPU-cycle equivalents:

| Event | APU cyc | CPU cyc | Quarter clock | Half clock | Frame IRQ |
|-------|---------|---------|---------------|-----------|-----------|
| Step 1 | 3728.5  | 7457  | yes | — | — |
| Step 2 | 7456.5  | 14913 | yes | yes | — |
| Step 3 | 11185.5 | 22371 | yes | — | — |
| Step 4a | 14914   | 29828 | — | — | IRQ set (if not inhibited) |
| Step 4b | 14914.5 | 29829 | yes | yes | IRQ set |
| Step 4c (wrap) | 0 | 29830→0 | — | — | IRQ set |

Total period = **29830 CPU cycles** (14915 APU cycles), then the counter wraps to 0.

The frame IRQ flag, in 4-step mode with inhibit clear, is asserted on the **last CPU cycle of the
frame and the two adjacent cycles** — practically: it is set on CPU cycles 29828, 29829, and 0
(the wrap). The simplest correct implementation: at the wrap point (step 4), if not inhibited, set
the frame interrupt flag and hold it asserted until it is cleared (by reading $4015 or setting
$4017 bit6). The three-cycle span matters only for cycle-exact IRQ-timing tests; a "set at step 4,
level-held" model passes the common `frame_counter` checks but re-verify the exact-cycle sub-tests.

The IRQ line is **level/latched**: once set, the CPU sees IRQ asserted every cycle until the flag is
cleared. Do not model it as a one-shot pulse.

### 2.2 Mode 1 — 5-step sequence (`$4017` bit7 = 1)

| Event | APU cyc | CPU cyc | Quarter clock | Half clock |
|-------|---------|---------|---------------|-----------|
| Step 1 | 3728.5  | 7457  | yes | — |
| Step 2 | 7456.5  | 14913 | yes | yes |
| Step 3 | 11185.5 | 22371 | yes | — |
| Step 4 | 14914.5 | 29829 | — | — |
| Step 5 | 18640.5 | 37281 | yes | yes |

Total period = **37282 CPU cycles** (18641 APU cycles), then wrap. **No frame IRQ is ever generated
in 5-step mode.**

Note step 4 in 5-step mode generates neither clock (it is a "do nothing" step); the length/sweep and
envelope clocks land on steps 1,2,3,5 (quarter) and 2,5 (half).

### 2.3 Writing $4017

1. Bit 7 selects mode; bit 6 is the IRQ inhibit.
2. If bit 6 (inhibit) is set, **clear the frame interrupt flag immediately**.
3. The internal divider/sequencer is **reset**, but with a delay: the reset takes effect after
   **3 CPU cycles if the write occurred on an APU-aligned cycle (even CPU cycle), or 4 CPU cycles if
   between APU cycles (odd CPU cycle)**. Model this as a small pending-reset countdown (3 or 4).
4. **If the new mode is 5-step (bit7=1), an immediate quarter- AND half-frame clock is generated**
   as part of the write (clocking envelopes, linear counter, length counters, and sweeps once right
   away). This immediate clock happens at the write; the sequence then restarts from step 0 after
   the 3/4-cycle delay. In 4-step mode, no immediate clock.

This "$4017 write immediately clocks half+quarter in 5-step mode" behavior is directly tested by the
`frame_counter` ROM — get it right.

### 2.4 Ordering of quarter vs half within a step
On a step that fires both (half-frame steps), clock the quarter-frame units and half-frame units.
The wiki treats a half-frame step as also doing the quarter-frame work. Concretely, on steps 2 and 4
(mode 0) / steps 2 and 5 (mode 1): clock envelopes + linear counter (quarter) AND length + sweep
(half). On quarter-only steps: only envelopes + linear counter.

---

## 3. Pulse channels (2 identical, differ only in sweep negate)

### 3.1 Duty sequencer

An 8-step sequence per duty. The sequencer steps through positions; output is the selected duty's
bit at the current step. The sequencer clock comes from the 11-bit timer (§3.2).

Recommended representation — store the 8-bit waveform and index by phase 0..7:

| Duty | 8-bit sequence (step 0 → 7) | Description |
|------|-----------------------------|-------------|
| 0 | `0 1 0 0 0 0 0 0` | 12.5% |
| 1 | `0 1 1 0 0 0 0 0` | 25% |
| 2 | `0 1 1 1 1 0 0 0` | 50% |
| 3 | `1 0 0 1 1 1 1 1` | 25% negated |

(NESdev also lists these as raw tables `00000001`, `00000011`, `00001111`, `11111100` read in
reverse because hardware counts the phase downward. The step-0→7 output rows above are equivalent
and simpler to implement: just index the row by a phase that increments 0..7 and wraps.)

The current duty output bit is combined with the length counter and envelope: if length counter is
0, or the sweep is muting, or the duty bit is 0 → channel output value is 0; otherwise output value
= current volume (envelope decay level or constant volume, 0..15).

Writing $4003/$4007 resets the phase to 0.

### 3.2 Timer (11-bit, divides CPU/2)

`t` = 11-bit period from timer-high (bits 2-0 of $4003/$4007) and timer-low ($4002/$4006).
The timer is clocked once per APU cycle (every 2 CPU cycles). It counts down; when it reaches 0 it
reloads with `t` and advances the duty sequencer by one step. The full waveform period is
`8 * (t + 1)` APU cycles = `16 * (t + 1)` CPU cycles. Frequency = fCPU / (16*(t+1)).

The pulse is muted (outputs 0) when the sweep-derived muting condition holds (see §4): current
period `t < 8`, or the sweep target period > $7FF.

### 3.3 Length counter — see §7.
### 3.4 Envelope — see §8.

---

## 4. Sweep unit (per pulse channel)

State: enable flag, divider period P (bits 6-4 of $4001/$4005), negate flag, shift count S (bits
2-0), a divider counter, and a reload flag. Clocked at **half-frame**.

### 4.1 Target period computation (continuous, evaluated every cycle it matters)
1. `change = current_period >> S`  (S = shift count)
2. If negate flag = 0: `target = current_period + change`
3. If negate flag = 1: subtract, with a **per-channel one-off difference**:
   - **Pulse 1**: adds the *ones' complement* → `target = current_period - change - 1`.
   - **Pulse 2**: adds the *two's complement* → `target = current_period - change`.
   - (Clamp the subtraction result at 0 if it would go negative — i.e., don't wrap.)

### 4.2 Muting conditions (checked continuously, independent of enable/divider)
The pulse channel is **muted (outputs 0)** if EITHER:
- the **current** timer period < 8, OR
- the computed **target** period > $7FF (0x7FF).

Muting applies even if the sweep is disabled or shift is 0.

### 4.3 Half-frame clock behavior
On each half-frame clock:
1. If (divider counter == 0) AND (sweep enabled) AND (shift count != 0) AND (not muted):
   `current_period = target` (update the pulse timer period). Recompute target afterward.
2. Then: if (divider counter == 0) OR (reload flag set): reload divider counter = P; clear reload
   flag. Else: decrement divider counter.

Note the update in step 1 uses the *current* divider==0 state, and step 2 handles reload/decrement.
The order (update-then-reload) is what hardware does; some references phrase it as one combined
check — the net effect: the period is only written when the divider has expired AND enabled AND
shift!=0 AND not muted; the divider reloads on expiry or reload flag.

Writing $4001/$4005 sets the reload flag.

---

## 5. Triangle channel

Outputs a 4-bit (0..15) quantized triangle. No envelope, no volume control, no sweep.

### 5.1 32-step sequence
Output values, indexed by a phase 0..31 that advances on each timer clock:

```
15,14,13,12,11,10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0,
 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15
```

### 5.2 Timer — clocked EVERY CPU cycle
The triangle timer's period is the 11-bit value `t` (from $400A/$400B). It counts down once **per
CPU cycle** (NOT per APU cycle — this is the key difference from pulse/noise). On reaching 0 it
reloads with `t` and advances the 32-step sequence by one — **but only if both the linear counter
and the length counter are non-zero** (otherwise the sequencer is halted, holding its current
output; it does not reset). Frequency = fCPU / (32*(t+1)).

### 5.3 Ultrasonic silencing
When `t < 2` (period 0 or 1) the output frequency is ultrasonic. Real hardware still oscillates but
inaudibly; it produces popping artifacts. Recommended emulator behavior: when `t < 2`, do not
advance the sequencer / hold the output constant (freeze), so no ultrasonic buzz enters the mix.
(The sequencer still technically runs on hardware; freezing is a standard, test-safe approximation.
Keep the current output level unchanged — do not force it to 0, to avoid clicks.)

### 5.4 Linear counter
State: control flag (bit7 of $4008, shared with length halt), reload flag, reload value (bits 6-0),
and the counter itself. Clocked at **quarter-frame**:
1. If reload flag is set → counter = reload value.
2. Else if counter > 0 → counter -= 1.
3. After the above: if the **control flag is clear**, clear the reload flag. (If control flag is
   set, the reload flag stays set, so the counter keeps reloading to the reload value each quarter
   frame — i.e., it's held.)

The reload flag is **set by writing $400B**.

### 5.5 Length counter — see §7 (halt bit = $4008 bit7).

The sequencer clocks only while BOTH linear counter > 0 AND length counter > 0.

---

## 6. Noise channel

### 6.1 LFSR (15-bit)
A 15-bit shift register, bits numbered 14..0. On power-up it is loaded with `1` (must never be all
zero, or it locks). Clocked by the noise timer (§6.2).

On each clock:
1. Compute feedback = `bit0 XOR bitN`, where:
   - N = 6 if the mode flag ($400E bit7) is set (mode 1),
   - N = 1 if the mode flag is clear (mode 0).
2. Shift the register right by 1.
3. Set bit 14 to the feedback value.

The channel output bit is the (new) **bit 0** of the register; if bit0 == 1 the channel is silenced
(outputs 0), otherwise it outputs the current volume (envelope/constant, 0..15). (Equivalently:
output is muted when bit0 is set. Some implementations use "output when bit0 == 0".) Also muted when
length counter == 0.

### 6.2 NTSC period table (index = $400E bits 3-0)
Timer reload value in **CPU cycles** (the noise timer counts down these; on reaching 0, clock the
LFSR). These are the pre-divided periods:

| Index | $0 | $1 | $2 | $3 | $4 | $5 | $6 | $7 | $8 | $9 | $A | $B | $C | $D | $E | $F |
|-------|----|----|----|----|----|----|----|----|----|----|----|----|----|-----|-----|-----|
| Period | 4 | 8 | 16 | 32 | 64 | 96 | 128 | 160 | 202 | 254 | 380 | 508 | 762 | 1016 | 2034 | 4068 |

Decimal array (index 0..15):
```
4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068
```

Implementation note: these values are the period in CPU cycles. If your noise timer is clocked at
the APU rate (every 2 CPU cycles), use `period/2` per APU tick — but simplest is to run the noise
timer in CPU cycles directly with these values, or clock the timer at APU rate and treat these as
the count. Match whatever domain the rest of your timers use; the effective LFSR clock rate must be
`fCPU / period`. (Concretely: NESdev lists these as the number of CPU cycles between LFSR clocks.)

### 6.3 Envelope — see §8. Length counter — see §7 (halt bit = $400C bit5).

---

## 7. Length counter

Shared design across pulse, triangle, noise. 8-bit down-counter loaded from a table.

### 7.1 Load table (32 entries, index = 5-bit load field)
Decimal values, index 0..31:
```
 0:  10   1: 254   2:  20   3:   2   4:  40   5:   4   6:  80   7:   6
 8: 160   9:   8  10:  60  11:  10  12:  14  13:  12  14:  26  15:  14
16:  12  17:  16  18:  24  19:  18  20:  48  21:  20  22:  96  23:  22
24: 192  25:  24  26:  72  27:  26  28:  16  29:  28  30:  32  31:  30
```
Flat array:
```
10,254,20,2,40,4,80,6,160,8,60,10,14,12,26,14,
12,16,24,18,48,20,96,22,192,24,72,26,16,28,32,30
```

### 7.2 Loading
On writing the length-load register ($4003/$4007/$400B/$400F), if the channel is **enabled** in
$4015, load `length = table[load_index]`. If the channel is disabled, the write does not load (and
the counter stays forced to 0). The load-index is bits 7-3 of that register.

### 7.3 Halt flag
- Pulse: $4000/$4004 bit 5.
- Triangle: $4008 bit 7.
- Noise: $400C bit 5.
(These bits are shared with the envelope loop / linear-counter control as noted.)

### 7.4 Clocking (half-frame)
On each half-frame clock: if length > 0 AND halt flag is clear → length -= 1. Otherwise unchanged.
A length counter of 0 silences its channel.

### 7.5 $4015 disable
Clearing the channel's enable bit in $4015 forces length = 0 immediately, and it cannot be reloaded
until re-enabled.

---

## 8. Envelope generator (pulse ×2, noise)

State: start flag, divider counter, decay level counter (0..15). Parameters from the channel's
$4000/$4004/$400C: `V` = bits 3-0 (envelope period / constant volume), `C` = bit 4 (constant volume),
`L` = bit 5 (loop, shared with length halt).

Clocked at **quarter-frame**:

- If **start flag is set**: clear start flag; decay level = 15; divider counter = V (reload). (Do NOT
  clock the divider this cycle.)
- Else (start flag clear): clock the divider:
  - If divider counter == 0: reload divider counter = V, and clock the decay level:
    - if decay level > 0 → decay level -= 1;
    - else if loop flag (L) set → decay level = 15.
  - Else: divider counter -= 1.

The divider's effective period is `V + 1` quarter-frames.

Output volume:
- If constant-volume flag `C` = 1 → volume = `V`.
- Else → volume = current decay level.

The **start flag is set by writing** $4003/$4007 (pulse) or $400F (noise). (Note: it is set by the
length-load write, not by $4000/$4004/$400C.)

---

## 9. DMC (Delta Modulation Channel)

Plays 1-bit DPCM deltas fetched from CPU memory, producing a 7-bit output level.

### 9.1 Rate table (NTSC, index = $4010 bits 3-0) — CPU cycles per timer period
```
Index:  $0   $1   $2   $3   $4   $5   $6   $7   $8   $9   $A   $B   $C   $D   $E   $F
Cycles: 428  380  340  320  286  254  226  214  190  160  142  128  106   84   72   54
```
Flat array (index 0..15):
```
428,380,340,320,286,254,226,214,190,160,142,128,106,84,72,54
```
All even (2 CPU cycles per APU cycle). The DMC timer counts these down; each expiry advances the
output unit by one bit.

### 9.2 Output unit
- 7-bit output level (0..127). Directly settable via $4011 (bits 6-0).
- An 8-bit shift register (the "sample buffer bits" being played) + a bits-remaining counter (0..8)
  + a silence flag.
- On each timer expiry (one bit period):
  - If silence flag is clear: look at bit 0 of the shift register. If 1 and output level ≤ 125:
    level += 2. If 0 and output level ≥ 2: level -= 2. (i.e. delta ±2, clamped to [0,127].)
  - Shift the shift register right by 1; decrement bits-remaining.
  - When bits-remaining reaches 0: end of output cycle → start a new one (§9.4): reload
    bits-remaining = 8; if the sample buffer is empty, set silence flag; else clear silence flag and
    load the shift register from the sample buffer, then empty the buffer.

### 9.3 Memory reader
- Sample start address (set on start/restart) = `$C000 + ($4012 << 6)`.
- Sample length (bytes remaining, set on start) = `($4013 << 4) + 1`.
- When the sample buffer is empty and bytes-remaining > 0, the reader fetches one byte:
  - Read the byte at the current address into the sample buffer.
  - Increment address; if it goes past $FFFF, wrap to $8000.
  - Decrement bytes-remaining.
  - When bytes-remaining reaches 0:
    - If loop flag ($4010 bit6) is set → reset address to sample start and bytes-remaining to sample
      length (restart).
    - Else if IRQ-enable flag ($4010 bit7) is set → set the DMC interrupt flag.

### 9.4 Starting / $4015 bit 4
- Writing $4015 with bit 4 = 1: if bytes-remaining is currently 0, (re)start the sample — set
  address = sample start, bytes-remaining = sample length. If bytes-remaining > 0, leave it running.
- Writing $4015 with bit 4 = 0: set bytes-remaining = 0 (sample stops after the current output byte
  finishes; the output unit keeps playing its last buffered bits then goes silent).
- **Any $4015 write clears the DMC interrupt flag.**
- Writing $4010 with bit 7 (IRQ enable) = 0 also clears the DMC interrupt flag immediately.

### 9.5 CPU DMA stall
When the memory reader fetches a sample byte, it does so via a DMA that **stalls the CPU**. The stall
is **1–4 CPU cycles** depending on alignment and what the CPU is doing:
- Typical stall is **4 CPU cycles** (the common case: the CPU is halted, then a dummy read, then the
  fetch). It can be as few as 1–2 cycles in specific alignments (e.g., if it coincides with certain
  read cycles) up to 4.
- A simple, widely-correct model: stall the CPU **4 cycles** on each DMC sample fetch. For higher
  accuracy, model 1–4 based on the CPU's current cycle alignment (the DMA waits for a CPU read
  cycle). This DMA can also collide with OAM DMA and controller reads (the famous DMC/controller
  conflict), but that is a CPU-side concern — for APU correctness, the key is: the fetch consumes CPU
  cycles and happens when the sample buffer empties.

For the Blargg APU tests, exact DMC DMA timing is largely not the focus, but the DMC IRQ timing and
rate are. Get the rate table, IRQ set-on-last-byte, loop, and $4015/$4010 flag clearing right.

---

## 10. Non-linear mixer

Combine the five channel outputs (pulse1, pulse2 ∈ 0..15; triangle ∈ 0..15; noise ∈ 0..15;
dmc ∈ 0..127) into one normalized float ≈ 0.0..1.0.

### 10.1 Exact form (division)
```
if (pulse1 + pulse2) == 0:
    pulse_out = 0.0
else:
    pulse_out = 95.88 / (8128.0 / (pulse1 + pulse2) + 100.0)

denom = triangle/8227.0 + noise/12241.0 + dmc/22638.0
if denom == 0:
    tnd_out = 0.0
else:
    tnd_out = 159.79 / (1.0 / denom + 100.0)

output = pulse_out + tnd_out   // ~0.0 .. ~1.0
```

### 10.2 Linear approximation (cheaper, slightly less accurate)
```
pulse_out = 0.00752 * (pulse1 + pulse2)
tnd_out   = 0.00851 * triangle + 0.00494 * noise + 0.00335 * dmc
output    = pulse_out + tnd_out
```

### 10.3 Recommended implementation
Precompute two lookup tables at init:
- `pulse_table[31]`: index `p = pulse1+pulse2` (0..30); `pulse_table[0]=0`, else the §10.1 formula.
- `tnd_table[203]`: index `i = 3*triangle + 2*noise + dmc` (0..202); `tnd_table[0]=0`, else evaluate
  the §10.1 tnd formula with triangle/noise/dmc reconstructed — but the simplest exact route is to
  build `tnd_table[i]` from the formula `159.79 / (1/(i-based denom) + 100)` where the denom uses the
  standard weighting. Practically most emulators build:
  `tnd_table[i] = 163.67 / (24329.0 / i + 100.0)` for i>0 (this is the algebraically-combined exact
  form used by NESdev's precomputed-table variant; the constants 95.52 / 163.67 in that variant keep
  the output within ~4%). Then `output = pulse_table[p] + tnd_table[i]`.

Either the two-table combined-constant form (95.52/163.67) or the direct §10.1 form is acceptable;
they agree to within a few percent. For `apu_mixer` correctness, use the non-linear form (NOT plain
linear summation) — the test specifically checks the non-linear DAC curve.

After mixing, apply host DSP: the real NES has a first-order high-pass (~90 Hz and ~440 Hz) and a
low-pass (~14 kHz). For test-ROM correctness these filters are not required, but for pleasant game
audio add at least a DC-blocking high-pass before resampling to the host sample rate (44.1/48 kHz).

---

## 11. Power-up / reset state

At power-on:
- All channel registers effectively 0 (no sound); length counters 0; envelopes 0.
- Noise LFSR = 1 (bit 0 set). Must be non-zero.
- Frame counter: mode 4-step, IRQ **inhibit clear** (so frame IRQ will start firing) — but note some
  references say the frame-counter register powers up as if $00 was written; the frame interrupt flag
  is not set at power-up. Treat $4017 as effectively 0 at power-up (4-step, IRQ enabled). On **reset**
  (soft reset), $4017 retains its prior mode/inhibit (implementation-dependent); simplest is to
  re-init to 4-step on power-up only.
- DMC output level = 0 at power-up (games often set it via $4011). Sample address/length 0;
  bytes-remaining 0; DMC IRQ flag clear.
- $4015 = 0 (all channels disabled) at power-up.
- After a $4015=0, all length counters are 0.

Writing $4015 = $00 is the standard "silence everything" done by init code.

---

## 12. What the Blargg test ROMs exercise (edge cases to get right)

- **`length_counter`**: the 32-entry load table (§7.1) must be exact; length loads only when the
  channel is enabled in $4015; disabling via $4015 forces length to 0 and blocks reload; halt bit
  freezes the counter; half-frame clocking decrements. Tests the interaction of writing the
  length-load register when the channel is enabled vs disabled, and the timing of the halt flag.

- **`frame_counter`**: the exact 4-step vs 5-step step timing (§2.1/§2.2); the frame IRQ flag set at
  the end of the 4-step sequence and its clearing (read $4015 clears it; $4017 bit6 clears it);
  the **$4017 write immediately clocking half+quarter in 5-step mode** (§2.3.4); the 3–4 CPU-cycle
  delay before the sequencer reset takes effect; that 5-step mode never sets the IRQ.

- **`apu_test`** (and the `blargg_apu_2005.07.30` suite): comprehensive — length counter, length
  halt timing, IRQ flag timing, the reset/write-$4017 timing, envelope behavior (start flag, decay,
  loop, constant volume), sweep muting and per-pulse negate difference, triangle linear counter,
  and register read-back of $4015 status bits. Subtests include "len_ctr", "len_table", "irq_flag",
  "clock_jitter", "len_timing_mode0/1", "irq_flag_timing", "reset_timing", "len_halt_timing", and
  "len_reload_timing" — most of the pain is precise CPU-cycle timing of the frame sequencer and the
  length-counter clock relative to writes.

- **`apu_mixer`**: verifies the **non-linear mixer** output for each channel individually and
  combined — you must use the non-linear DAC formulas (§10), not a linear sum, and the pulse and
  tnd groups must use the correct separate curves and channel weightings. It plays known input
  levels and checks the resulting analog level.

---

## 13. Implementation order (recommended)

1. **Datapath primitives + frame counter first.** Build the length counter (§7), the envelope
   generator (§8), and the generic down-counter timers. Build the frame sequencer (§2) with exact
   4-step/5-step timing, the quarter/half clock dispatch, $4017 write semantics (mode, inhibit,
   3–4 cycle reset delay, 5-step immediate clock), and the frame IRQ flag. Wire $4015 read/write.
   Validate `length_counter` and `frame_counter` before touching sound generation.

2. **The four tone channels producing sample values.** Pulse (duty sequencer §3 + sweep §4 +
   length + envelope), triangle (32-step §5 + linear counter, timer every CPU cycle, `t<2` freeze),
   noise (LFSR §6 + NTSC period table + envelope + length). Each channel yields a 0..15 value each
   sample; leave DMC at 0 for now.

3. **Non-linear mixer + host resampling.** Implement §10 (precomputed tables), combine to a float,
   add a DC-blocking high-pass, and resample to the host rate. Validate `apu_mixer`.

4. **DMC with DMA stalls.** Rate table (§9.1), output unit ±2 clamp, memory reader with $C000+A*64
   address / L*16+1 length / $8000 wrap, loop, IRQ-on-last-byte, $4015 bit4 start/stop, $4010/$4015
   IRQ-flag clearing, and the 1–4 (use 4) CPU-cycle CPU stall on each fetch.

5. **Frame IRQ + DMC IRQ wiring into the CPU.** Assert the CPU IRQ line as a level from
   `(frame_irq_flag && !inhibit) || dmc_irq_flag`; ensure reads of $4015 clear the frame flag (not
   the DMC flag), $4015 writes clear the DMC flag, and $4010 bit7=0 clears the DMC flag. Re-run the
   full `apu_test` / `blargg_apu_2005.07.30` suite.
