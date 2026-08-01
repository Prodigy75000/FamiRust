# NES 2C02 PPU — Clean-Room Implementation Notes

Dot-accurate reference for a from-scratch Rust 2C02 PPU. Synthesized from the
NESdev wiki hardware-reference pages (PPU_registers, PPU_scrolling,
PPU_rendering, PPU_sprite_evaluation, PPU_palettes, PPU_OAM,
PPU_attribute_tables, PPU_memory_map, PPU_nametables, Mirroring,
PPU_frame_timing, Emulator_tests). Target: pass Blargg ppu_vbl_nmi /
sprite_hit / sprite_overflow and render commercial games pixel-correctly.

Conventions used here:
- "dot" = "cycle" = "tick" = one PPU clock = one pixel; 1 CPU cycle = 3 PPU dots.
- Scanlines numbered 0..261. Scanline 261 is the "pre-render" line (also
  referred to elsewhere as line -1).
- Dot 0 is the idle dot at the start of each scanline; dots run 0..340.
- Registers named by their canonical mnemonics. `$2000..$2007` mirror every 8
  bytes through the CPU range `$2000..$3FFF`.

---

## 1. Register-Level Overview

The CPU sees eight registers at `$2000..$2007` (mirrored every 8 bytes up to
`$3FFF`).

| Addr  | Name     | CPU access | Purpose                          |
|-------|----------|------------|----------------------------------|
| $2000 | PPUCTRL  | write      | control flags, NMI enable        |
| $2001 | PPUMASK  | write      | rendering enables, tint, mask    |
| $2002 | PPUSTATUS| read       | vblank / sprite-0 / overflow     |
| $2003 | OAMADDR  | write      | OAM address pointer              |
| $2004 | OAMDATA  | read/write | OAM data port                    |
| $2005 | PPUSCROLL| write x2   | fine/coarse scroll into t,x      |
| $2006 | PPUADDR  | write x2   | VRAM address into t, then v      |
| $2007 | PPUDATA  | read/write | VRAM data port (auto-increment)  |

Open-bus note: reads of write-only registers, and the unused low bits of
PPUSTATUS, return the last value on the PPU I/O bus (open bus / decay latch).
An implementation should maintain a `io_bus: u8` latch: every CPU read/write of
`$2000..$2007` (and $4014 OAM DMA) refreshes it with the byte transferred; reads
of write-only regs return it; PPUSTATUS reads return `(status & 0xE0) | (io_bus
& 0x1F)`.

---

## 2. `$2000` PPUCTRL (write only)

```
7  bit  0
V P H B  S I N N
| | | |  | | | |
| | | |  | | +-+- Base nametable address select (t bits 10-11)
| | | |  | |       00=$2000 01=$2400 10=$2800 11=$2C00
| | | |  | +----- VRAM address increment per $2007 access (0: +1 across, 1: +32 down)
| | | |  +------- Sprite pattern table for 8x8 sprites (0:$0000, 1:$1000; ignored in 8x16)
| | | +--------- Background pattern table (0:$0000, 1:$1000)
| | +----------- Sprite size (0: 8x8, 1: 8x16)
| +------------- PPU master/slave (EXT pins; leave 0, effectively unused on retail NES)
+--------------- Vblank NMI enable (0: off, 1: generate NMI at vblank start)
```

Write side effects:
- Bits 0-1 are copied into `t` bits 10-11 immediately:
  `t: ...GH.. ........ <- d: ......GH`.
- Bit 2 selects the $2007 auto-increment amount: 0 => +1, 1 => +32.
- Bit 7 (NMI enable): if this transitions 0->1 *while* the vblank flag is
  already set (during vblank), an NMI is generated immediately. Toggling it high
  repeatedly during vblank can fire multiple NMIs in one frame. (See §12.)
- On power-up/reset PPUCTRL is 0. Writes to $2000 are ignored for about the
  first ~29658 CPU cycles after reset on real hardware; ppu_vbl_nmi does not
  strictly require modeling that warm-up, but be aware of it.

Ordering gotcha: PPUSCROLL and the PPUCTRL nametable bits must be written
*after* any PPUADDR writes, because $2006's second write overwrites the same `t`
bits.

---

## 3. `$2001` PPUMASK (write only)

```
7  bit  0
B G R s  b M m G
| | | |  | | | |
| | | |  | | | +- Greyscale (0: normal, 1: AND every palette index with $30)
| | | |  | | +--- Show background in leftmost 8 pixels of screen (0: hide)
| | | |  | +----- Show sprites   in leftmost 8 pixels of screen (0: hide)
| | | |  +------- Enable background rendering
| | | +--------- Enable sprite rendering
| | +----------- Emphasize red   (NTSC); PAL/Dendy: green
| | +----------- Emphasize green (NTSC); PAL/Dendy: red
| +------------- Emphasize green (see note)
+--------------- Emphasize blue
```

Bit map (unambiguous):
- bit 0: greyscale
- bit 1: show background left 8px
- bit 2: show sprites left 8px
- bit 3: enable background
- bit 4: enable sprites
- bit 5: emphasize red   (NTSC) / green (PAL, Dendy)
- bit 6: emphasize green (NTSC) / red   (PAL, Dendy)
- bit 7: emphasize blue

Semantics:
- "Rendering enabled" for all internal timing purposes means `(bit3 || bit4)`
  is set. This gates: scroll increments, horizontal/vertical `v<-t` copies, the
  odd-frame dot skip, OAMADDR-reset-to-0 during fetch, and the $2002 read race
  window behavior for the address bus.
- Greyscale: bitwise-AND the final 6-bit palette *index used to look up the
  master palette* with `$30` (forces the color to the grey column). Apply after
  emphasis-independent index formation.
- Emphasis: color-tint that darkens (attenuates) the two channels other than
  the selected one, making the selected channel comparatively brighter. Setting
  all three dims all channels. Implemented at the RGB-output stage as a
  per-channel multiplier keyed on the 3 emphasis bits (see §11).
- Left-8 masking: when bit1=0, background pixels at screen x=0..7 are forced
  transparent; when bit2=0, sprite pixels at x=0..7 are suppressed. This masking
  also affects sprite-0-hit (see §10).

---

## 4. `$2002` PPUSTATUS (read only)

```
7  bit  0
V S O .  . . . .
| | |
| | +----------- Sprite overflow flag (bit 5)
| +------------- Sprite 0 hit flag    (bit 6)
+--------------- Vblank flag          (bit 7)
```
- Bits 0-4: open bus (last value on PPU I/O bus).
- Sprite overflow (bit 5): set when the buggy sprite-eval logic thinks >8
  sprites are on a line (see §9). Cleared at pre-render dot 1.
- Sprite 0 hit (bit 6): set when an opaque sprite-0 pixel overlaps an opaque
  background pixel (see §10). Cleared at pre-render dot 1.
- Vblank (bit 7): set at scanline 241 dot 1; cleared at pre-render (261) dot 1.

Read side effects (critical):
1. Return current flags, THEN clear the vblank flag (bit 7 <- 0).
2. Reset the write toggle: `w <- 0`.
3. Vblank race: reading in the exact window when the flag is being set
   suppresses the NMI for that frame (see §12).

Sprite-0-hit and sprite-overflow flags are NOT cleared by reading $2002 (only
vblank is). They clear only at pre-render dot 1.

---

## 5. `$2003` OAMADDR / `$2004` OAMDATA

OAMADDR ($2003, write): sets the 8-bit OAM byte pointer.
- Set to 0 by the sprite-fetch phase: during rendering, OAMADDR is forced to 0
  at each of dots 257..320 (so at the end of a rendered scanline OAMADDR reads
  as 0). Games that need a specific OAMADDR must write it during vblank.
- 2C02G quirk: writing OAMADDR during rendering can corrupt OAM (an internal
  copy of sprites 8-9 lands near the target). Not required for the Blargg tests;
  don't emulate unless a game depends on it.

OAMDATA ($2004, read/write): 8-bit port into 256-byte primary OAM at OAMADDR.
- Write: stores data at OAM[OAMADDR], then OAMADDR += 1 (wraps at 256).
- Read: returns OAM[OAMADDR] and does NOT increment OAMADDR.
- During dots 1..64 (secondary-OAM clear) a read of $2004 returns `$FF`.
- Writes to $2004 during rendering do NOT modify OAM but do a glitchy OAMADDR
  bump (increments only the high 6 bits). Games avoid this; low priority.
- OAM byte 2 (attributes) bits 2-4 are unimplemented and read back as 0. When
  emulating OAM reads, mask attribute reads with `& 0xE3`.

OAM DMA ($4014, in the CPU space) copies 256 bytes CPUpage*$100 -> OAM starting
at the current OAMADDR; costs 513/514 CPU cycles. Implement in the CPU/bus, but
it feeds this OAM.

---

## 6. `$2005` PPUSCROLL / `$2006` PPUADDR / `$2007` PPUDATA

These manipulate the internal scroll/address registers (§7). PPUSCROLL and
PPUADDR share the same write toggle `w`.

### $2005 PPUSCROLL (write x2)
- First write (w==0): X scroll.
  ```
  t: ....... ...ABCDE <- d: ABCDE...   (coarse X -> t bits 0-4)
  x:              FGH <- d: .....FGH   (fine X -> x, 3 bits)
  w: 1
  ```
- Second write (w==1): Y scroll.
  ```
  t: FGH..AB CDE..... <- d: ABCDEFGH
     (fine Y = d bits 0-2 -> t 12-14; coarse Y = d bits 3-7 -> t 5-9)
  w: 0
  ```

### $2006 PPUADDR (write x2)
- First write (w==0): high byte.
  ```
  t: .CDEFGH ........ <- d: ..CDEFGH   (d bits 0-5 -> t bits 8-13)
  t: Z...... ........ <- 0             (t bit 14 cleared)
  w: 1
  ```
- Second write (w==1): low byte.
  ```
  t: ....... ABCDEFGH <- d: ABCDEFGH   (d bits 0-7 -> t bits 0-7)
  v: <all 15 bits>    <- t             (v copied from t)
  w: 0
  ```
  The `v <- t` copy takes effect a few PPU cycles later on hardware, but a
  same-cycle copy is fine for the target tests.

### $2007 PPUDATA (read/write)
Address used = `v & 0x3FFF` (top bit of the 15-bit v is not on the address bus).

- Write: store data at PPU address `v & 0x3FFF` (dispatch to CHR / nametable /
  palette per §13). Then increment v by (PPUCTRL bit2 ? 32 : 1).
- Read: BUFFERED.
  - If `(v & 0x3FFF) < $3F00`: return the current contents of the internal read
    buffer; THEN load the buffer with the byte at `v & 0x3FFF`. (One-read delay:
    the first read after setting an address returns stale data — a "priming
    read" is required.)
  - If `(v & 0x3FFF) >= $3F00` (palette region): return the palette byte
    IMMEDIATELY (6-bit palette value, with open-bus in the top 2 bits: return
    `(palette & 0x3F) | (io_bus & 0xC0)`); AND simultaneously load the read
    buffer with the *nametable* byte "underneath" the palette, i.e. the byte at
    `(v & 0x3FFF) - $1000` (the mirrored nametable at `$2Fxx`/`$2Xxx`). No
    priming read is required for palette reads.
  - After either, increment v by 1 or 32 as above.
- $2007 access DURING rendering (rendering enabled, on visible or pre-render
  lines) does NOT use the normal increment. Instead it triggers a simultaneous
  coarse-X increment AND Y increment on v (the same routines as §8). Games avoid
  reading/writing $2007 mid-frame; model it only if needed.

---

## 7. Internal registers v, t, x, w (the "loopy" registers)

- `v` : current VRAM address, 15 bits. Used as the live rendering address and
  the $2007 address.
- `t` : temporary VRAM address, 15 bits. The "top-left of screen" latch that
  register writes accumulate into; copied into v at specific dots.
- `x` : fine X scroll, 3 bits. Selects which of the 8 shifted pixels is output.
- `w` : write toggle, 1 bit. Shared by $2005 and $2006. Reset by reading $2002.

Bit layout of v and t (15 bits):
```
  14 13 12 11 10 09 08 07 06 05 04 03 02 01 00
   y  y  y  N  N  Y  Y  Y  Y  Y  X  X  X  X  X
  \--fineY-/ \NT/ \--coarseY--/ \--coarseX---/
```
- bits 0-4  : coarse X       (0..31, tile column within a nametable)
- bits 5-9  : coarse Y       (0..31, tile row; only 0..29 are real rows)
- bits 10-11: nametable select (bit10 = horizontal NT, bit11 = vertical NT)
- bits 12-14: fine Y         (0..7, pixel row within a tile)
- bit 14 is the top fine-Y bit; bit 15 does not exist. When v is used as a
  memory address only the low 14 bits matter.

Useful masks: coarse X `0x001F`, coarse Y `0x03E0`, NT select `0x0C00`,
horizontal NT `0x0400`, vertical NT `0x0800`, fine Y `0x7000`.

---

## 8. Scroll increment & copy routines

### Coarse-X increment ("increment hori(v)")
```
if (v & 0x001F) == 31 {        // coarse X == 31 (last column)
    v &= !0x001F;              // coarse X = 0
    v ^=  0x0400;              // flip horizontal nametable (bit 10)
} else {
    v += 1;                    // coarse X += 1
}
```

### Y increment ("increment vert(v)")
```
if (v & 0x7000) != 0x7000 {    // fine Y < 7
    v += 0x1000;               // fine Y += 1
} else {
    v &= !0x7000;              // fine Y = 0
    let mut y = (v & 0x03E0) >> 5;   // coarse Y
    if y == 29 {
        y = 0;                 // wrap 29 -> 0
        v ^= 0x0800;           // flip vertical nametable (bit 11)
    } else if y == 31 {
        y = 0;                 // wrap 31 -> 0, NO nametable flip
    } else {
        y += 1;
    }
    v = (v & !0x03E0) | (y << 5);
}
```
Note the two wrap points: 29 (normal, because there are only 30 tile rows) flips
the vertical nametable; 31 (garbage rows in the attribute area) wraps without
flipping.

### Copy horizontal bits ("hori(v) = hori(t)") — happens at dot 257
```
v = (v & !0x041F) | (t & 0x041F);   // coarse X (0-4) + horizontal NT (bit 10)
```

### Copy vertical bits ("vert(v) = vert(t)") — repeated at dots 280..304 of
the pre-render line (261)
```
v = (v & !0x7BE0) | (t & 0x7BE0);   // fine Y (12-14) + coarse Y (5-9) + vert NT (bit 11)
```

When each fires (only while rendering enabled — bit3 or bit4 of PPUMASK set):
- Coarse-X increment: at dot 8,16,24,...,256 (end of each 8-dot tile fetch) on
  visible + pre-render lines, and again at dot 328 and 336 (the two prefetch
  tiles).
- Y increment: once, at dot 256, on visible + pre-render lines.
- hori(v)=hori(t): at dot 257, on visible + pre-render lines.
- vert(v)=vert(t): repeatedly at dots 280..304, pre-render line only.

---

## 9. Frame / scanline / dot geometry

Frame = 262 scanlines x 341 dots (NTSC). 89342 dots/frame on even frames; 89341
on odd frames when rendering is enabled (one dot skipped, see below).

| Scanline | Name         | Role                                             |
|----------|--------------|--------------------------------------------------|
| 0..239   | Visible      | Fetch BG + sprite data; output 256 pixels each   |
| 240      | Post-render  | Idle. Vblank NOT yet set.                         |
| 241      | Vblank start | At dot 1: set vblank flag, fire NMI if enabled    |
| 241..260 | Vblank       | CPU may freely access PPU memory                  |
| 261      | Pre-render   | Dummy line; clears flags dot 1; reloads v; fills  |
|          |              | shift regs for line 0; odd-frame dot skip         |

Dot 0 of every scanline is an idle cycle (no memory access) — except that on
odd frames with rendering enabled, the pre-render line ends one dot early
(jumps from (261,339) directly to (0,0)), effectively skipping the idle dot at
the start of line 0. See §12 for the exact jump.

---

## 10. Background rendering pipeline (per dot)

Active on visible scanlines 0..239 and the pre-render line 261 (which fetches
but produces no visible pixels).

### 8-dot fetch cadence
Each background tile needs 4 memory accesses, 2 dots each = 8 dots. Within an
8-dot group (aligned so the group boundary is at dots 1,9,17,...):
```
dot%8 == 1..2 : fetch Nametable byte     -> tile index at addr $2000|(v & 0x0FFF)
dot%8 == 3..4 : fetch Attribute byte     -> addr 0x23C0|(v&0x0C00)|((v>>4)&0x38)|((v>>2)&0x07)
dot%8 == 5..6 : fetch Pattern low byte   -> chr[ base + tile*16 + fineY ]
dot%8 == 7..0 : fetch Pattern high byte  -> chr[ base + tile*16 + fineY + 8 ]
```
where `base = (PPUCTRL bit4 ? 0x1000 : 0x0000)` and `fineY = (v >> 12) & 7`.
A per-dot state machine reading only on the second dot of each 2-dot access is
sufficient; a simpler implementation may perform the whole fetch group at once
on the group-boundary dot as long as scroll increments still happen on the exact
dots below.

### Attribute quadrant selection
The attribute byte packs four 2-bit palette selectors for a 32x32 px (4x4 tile)
region, split into four 16x16 px (2x2 tile) quadrants:
```
value = (bottomRight << 6) | (bottomLeft << 4) | (topRight << 2) | (topLeft << 0)
bits 1-0 top-left, 3-2 top-right, 5-4 bottom-left, 7-6 bottom-right
```
Select the 2 bits for the current tile using coarse-X bit1 and coarse-Y bit1:
```
shift = ((v >> 4) & 4) | (v & 2);   // = (coarseYbit1 ? 4 : 0) | (coarseXbit1 ? 2 : 0)
attr2 = (attr_byte >> shift) & 0x3; // 2-bit palette high bits
```

### Shift registers
- Two 16-bit pattern shift registers (`pat_lo`, `pat_hi`).
- Two 8-bit attribute latches expanded into two 1-bit-per-pixel shift registers
  (commonly modeled as two 16-bit `attr_lo`, `attr_hi` fed from a latch each
  reload; the attribute bits are constant across the 8 pixels of a tile).
- Reload: at dots 9,17,25,...,257 and 329,337 (the first dot after each
  completed 8-dot fetch), load the freshly fetched pattern bytes into the LOW 8
  bits of the pattern shift registers' incoming half and set the attribute
  latch from `attr2`. (Equivalently: the newly fetched tile is placed in the
  high byte and shifted down toward output; either convention works if fine X
  indexing matches.)
- Shift: on every dot in the fetch region (dots 2..257 and 322..337), shift both
  pattern registers left by 1 and clock the attribute shifters.

### Producing a background pixel
For visible dots 1..256 (screen x = dot-1, so x in 0..255):
```
bit = 15 - x_fine;                       // x_fine = the x register (0..7)
p0  = (pat_lo >> bit) & 1;
p1  = (pat_hi >> bit) & 1;
a0  = (attr_lo >> bit) & 1;
a1  = (attr_hi >> bit) & 1;
bg_pattern = (p1 << 1) | p0;             // 2-bit, 0 = transparent
bg_palette = (a1 << 1) | a0;             // 2-bit palette select
bg_index   = (bg_palette << 2) | bg_pattern;  // 4-bit index into $3F00..$3F0F
```
If `bg_pattern == 0`, the pixel is transparent (use backdrop). If background
rendering is disabled, or bit1 masks it in x=0..7, force `bg_pattern = 0`.

### Dot-region summary (visible + pre-render lines)
| Dots     | Activity                                                            |
|----------|--------------------------------------------------------------------|
| 0        | idle                                                               |
| 1..256   | BG tile fetches; output pixel for x=dot-1 (visible lines only)      |
| 257      | hori(v)=hori(t); OAMADDR forced 0 begins                            |
| 257..320 | Sprite pattern fetches for NEXT scanline (garbage NT reads); OAMADDR=0 |
| 321..336 | Fetch first TWO BG tiles of next scanline into shift registers      |
| 337..340 | Two dummy nametable fetches (unused; some mappers watch these reads) |
| 280..304 | (pre-render only) vert(v)=vert(t) repeated                          |

---

## 11. Final pixel: BG/sprite multiplexing & palette

Compose per output pixel:
1. Compute `bg_index` (§10) and the sprite pixel (§9) for this x.
2. Multiplex:
   ```
   bg_opaque  = (bg_pattern  != 0)
   sp_opaque  = (sprite_pattern != 0)
   if !bg_opaque && !sp_opaque { out = $3F00 (backdrop) }
   else if !bg_opaque &&  sp_opaque { out = sprite palette entry }
   else if  bg_opaque && !sp_opaque { out = bg palette entry }
   else { // both opaque
       // sprite priority bit: 0 = in front of BG, 1 = behind BG
       out = (sprite_priority == 0) ? sprite entry : bg entry;
       // AND: evaluate sprite-0 hit here (see below)
   }
   ```
3. Look up the resulting 5-bit palette address (`$3F00 | index`) in palette RAM
   -> 6-bit master-palette color index -> master palette RGB.
4. Apply greyscale (`index & 0x30`) BEFORE the master-palette lookup if
   PPUMASK bit0 set. Apply emphasis dimming to the resulting RGB.

Sprite-0 hit is asserted at step 2's "both opaque" branch when the sprite pixel
belongs to sprite 0 — regardless of the priority winner. See §10 conditions.

---

## 12. Sprite evaluation (per dot on visible scanlines)

Uses 256-byte primary OAM (64 sprites x 4 bytes) and 32-byte secondary OAM (8
sprites). OAM byte layout per sprite:
```
byte0 = Y (top of sprite MINUS 1; sprite appears on scanline Y+1..)
byte1 = tile index (8x16: bit0 selects pattern table $0000/$1000, bits1-7 tile;
                    the two 8x8 halves are tile&0xFE and tile|0x01)
byte2 = attributes: bits0-1 palette (4..7), bits2-4 unimplemented(read 0),
                    bit5 priority (0 front / 1 behind BG),
                    bit6 flip horizontal, bit7 flip vertical
byte3 = X (left edge)
```
Sprite height = 8 (8x8) or 16 (8x16) from PPUCTRL bit5. In-range test for the
line being *evaluated* (evaluation on line L fetches sprites shown on line L+1;
equivalently compare against the current scanline using the delayed-by-one
model): a sprite is in range if `Y <= line < Y + height`.

### Dots 1..64 — clear secondary OAM
Fill all 32 bytes of secondary OAM with `$FF`. (Hardware does this as reads of
primary OAM that are forced to return $FF; a read of $2004 during this window
returns $FF.)

### Dots 65..256 — evaluation state machine
Alternates: odd dots read primary OAM, even dots write secondary OAM (or, once
full, read it). Algorithm with sprite index `n` and byte index `m`:

```
1. Read OAM[n][0] (Y). Copy it into the next open secondary-OAM slot
   (write ignored if 8 sprites already found).
   If Y is in range (Y <= line < Y+height):
       copy OAM[n][1], [2], [3] into secondary OAM (m advances 0->3).
2. Increment n.
   2a. if n overflowed 64 back to 0  -> go to step 4.
   2b. if fewer than 8 sprites found -> go to step 1.
   2c. if exactly 8 sprites found    -> disable writes to secondary OAM.
3. (Overflow scan — buggy) Evaluate OAM[n][m] as a Y-coordinate:
   3a. if in range: SET sprite-overflow ($2002 bit5) and read the next 3
       entries of OAM (advancing m and n), then continue.
   3b. if NOT in range: increment n AND m (WITHOUT carry) -- THIS IS THE BUG.
       (m should stay 0; incrementing it makes evaluation walk OAM
       "diagonally", reading attribute/tile/X bytes as if they were Y.)
       If n overflowed to 0 -> go to step 4.
4. Remaining dots: repeatedly (and fruitlessly) try to copy OAM[n][0] into
   secondary OAM, incrementing n, until dot 256 / HBLANK.
```
Consequences to reproduce exactly:
- With <=8 sprites on a line, overflow is never set.
- With >8 sprites, overflow is usually set, but the diagonal-walk bug can cause
  false negatives (misses) and false positives depending on OAM contents; the
  sprite_overflow test ROM probes these. Do NOT "fix" the bug.

### Dots 257..320 — sprite tile fetch (8 sprites, 8 dots each)
For each of the up-to-8 secondary-OAM sprites:
- dots 1-2 of the group: garbage nametable byte fetch (still drives the address
  bus; some mappers count these).
- dots 3-4: garbage attribute fetch.
- dots 5-6: sprite pattern low byte.
- dots 7-8: sprite pattern high byte.
Compute the pattern row using vertical flip and, in 8x16, the correct half.
OAMADDR is held at 0 across this whole range. Empty slots ($FF Y) fetch a
transparent/dummy pattern (commonly tile $FF from the sprite table).
Latch each sprite's X, attributes, and its two shifted pattern bytes into 8
per-sprite output units; each unit has an X counter that counts down and begins
shifting when it reaches 0.

### Dots 321..340
Dot 321..: read the first secondary-OAM byte while BG prefetch proceeds; the
sprite units are ready for the next line. Nothing sprite-critical here.

Sprite evaluation itself does NOT produce sprite-0 hit — that is decided during
pixel output (§10/§13).

---

## 13. Sprite pixel output & sprite-0 hit

During visible dots 1..256, each of the 8 sprite units whose X counter has
reached 0 produces a candidate pixel:
```
sp_pattern = (spat_hi_bit << 1) | spat_lo_bit;  // 2-bit, 0 = transparent
```
The FIRST (lowest secondary-OAM index) unit with `sp_pattern != 0` wins sprite
priority among sprites; its palette (bits0-1 + 4) and priority bit go to the
multiplexer (§11). Sprite palette address = `$3F10 | (sp_palette << 2) |
sp_pattern`.

Sprite-0 hit sets PPUSTATUS bit6 at the pixel where ALL hold:
- The winning-evaluation sprite this scanline includes sprite 0 (i.e. OAM sprite
  index 0 was copied into secondary OAM slot 0) AND its pixel here is opaque
  (`sp_pattern != 0`).
- The background pixel here is opaque (`bg_pattern != 0`).
- Both background AND sprite rendering are enabled (PPUMASK bit3 AND bit4).
- Not masked away by left-8 clipping: if x in 0..7 and either bit1=0 (BG hidden)
  or bit2=0 (sprites hidden), no hit.
- x != 255 (the rightmost pixel never triggers a hit — pixel-pipeline quirk).
It is set AT the dot the overlapping pixel is produced. It does not depend on
sprite priority, palette values, or which pixel actually displays. It can only
be set once per frame; it and overflow are cleared at pre-render (261) dot 1.

---

## 14. VBlank / NMI timing and races

Baseline events (rendering state independent unless noted):
- Scanline 241, dot 1: set vblank flag (PPUSTATUS bit7). If PPUCTRL bit7 (NMI
  enable) is set, assert NMI.
- Scanline 261 (pre-render), dot 1: clear vblank, sprite-0-hit, and
  sprite-overflow flags.

NMI generation model: the NMI line to the CPU is (roughly) the logical AND of
"vblank flag set" and "NMI enable (PPUCTRL bit7)". Model an edge-triggered NMI
that fires when this AND transitions low->high. Therefore:
- Setting PPUCTRL bit7 while vblank is already set fires an NMI immediately.
- Clearing then re-setting bit7 during vblank fires another NMI (multiple NMIs
  per frame are possible — a real behavior the tests check).

The $2002-read race (must be exact for ppu_vbl_nmi):
- Reading PPUSTATUS on the SAME PPU cycle the vblank flag is set (scanline 241,
  dot 1) returns the flag as 0 (clear), the flag ends up clear, AND the NMI for
  that frame is suppressed.
- Reading one PPU cycle BEFORE it would be set (scanline 241, dot 0) reads 0 and
  prevents the flag from being set that frame (and thus no NMI).
- Reading one cycle after (dot 2+) reads 1, clears the flag as usual; an NMI, if
  enabled, may have already been asserted.
Recommended implementation: at (241, dot 1), if PPUSTATUS was read on this exact
cycle (or the immediately preceding one), do not set the flag / do not assert
the NMI for this frame; otherwise set flag and (if enabled) request NMI.

Odd-frame dot skip (rendering enabled only):
- On odd frames, when rendering is enabled, the pre-render line is one dot
  shorter: after (261, 339) the PPU jumps directly to (0, 0), skipping (261,340)
  / the (0,0) idle. This makes odd rendered frames 89341 dots. On even frames,
  or whenever rendering is disabled, no skip occurs (89342 dots).
- Practically: track a `frame_is_odd` toggle; at the pre-render line, if
  `frame_is_odd && rendering_enabled`, advance from dot 339 straight to the next
  frame's (0,0).

---

## 15. PPU address space & memory map

14-bit address space `$0000..$3FFF`, separate from the CPU bus.

| Range        | Size    | Contents                    | Backed by            |
|--------------|---------|-----------------------------|----------------------|
| $0000..$0FFF | 4 KiB   | Pattern table 0             | Cartridge CHR ROM/RAM|
| $1000..$1FFF | 4 KiB   | Pattern table 1             | Cartridge CHR ROM/RAM|
| $2000..$23BF | 960 B   | Nametable 0 tilemap         | CIRAM (via mirroring)|
| $23C0..$23FF | 64 B    | Attribute table 0           | CIRAM                |
| $2400..$27FF | 1 KiB   | Nametable 1 (+ attr)        | CIRAM                |
| $2800..$2BFF | 1 KiB   | Nametable 2 (+ attr)        | CIRAM                |
| $2C00..$2FFF | 1 KiB   | Nametable 3 (+ attr)        | CIRAM                |
| $3000..$3EFF | 3.75KiB | Mirror of $2000..$2EFF      | (as above)           |
| $3F00..$3F1F | 32 B    | Palette RAM                 | Internal PPU         |
| $3F20..$3FFF | 224 B   | Mirrors of $3F00..$3F1F     | Internal PPU         |

Pattern tables come from the cartridge (mapper). Nametable reads/writes go
through the cartridge's mirroring wiring into the console's 2 KiB CIRAM (or
cartridge-supplied RAM for four-screen). Palette RAM is internal to the PPU and
always responds regardless of the cartridge.

A single 8x8 tile in a pattern table = 16 bytes: bytes 0-7 = bit-plane 0 (low),
bytes 8-15 = bit-plane 1 (high). Pixel(row r, col c) = `((hi>>(7-c))&1)<<1 |
((lo>>(7-c))&1)`. Pattern address = `table_base + tile*16 + row` (+8 for high
plane).

---

## 16. Nametable mirroring

Four logical 1 KiB nametable slots ($2000/$2400/$2800/$2C00) map onto physical
banks. Console CIRAM has two 1 KiB banks (call them A and B). The mapper's
wiring picks the mode:

| Mode              | $2000 | $2400 | $2800 | $2C00 | Notes                       |
|-------------------|-------|-------|-------|-------|-----------------------------|
| Horizontal        | A     | A     | B     | B     | "vertical arrangement"; 32x60 |
| Vertical          | A     | B     | A     | B     | "horizontal arrangement"; 64x30 |
| Single-screen A   | A     | A     | A     | A     | mapper-selected bank         |
| Single-screen B   | B     | B     | B     | B     | mapper-selected bank         |
| Four-screen       | A     | B     | C     | D     | needs extra cart VRAM        |

Naming tip (a common source of confusion): "horizontal mirroring" duplicates
horizontally-adjacent slots ($2000==$2400, $2800==$2C00) — good for games that
scroll vertically. "Vertical mirroring" duplicates vertically-adjacent slots
($2000==$2800, $2400==$2C00) — good for horizontal scrolling.

Address-to-bank helper (2 KiB CIRAM, index bit chooses bank):
```
// addr in $2000..$3EFF, reduce to $2000..$2FFF first (mask 0x0FFF gives 0..0xFFF within the 4 KiB NT space; then pick the 1KiB region)
nt = (addr >> 10) & 0x3;   // which logical slot 0..3
// Horizontal: bank = (nt >> 1) & 1;        (slots 0,1 -> A; 2,3 -> B)
// Vertical:   bank = nt & 1;               (slots 0,2 -> A; 1,3 -> B)
// Single A:   bank = 0;   Single B: bank = 1;
offset = addr & 0x03FF;
ciram[bank*0x400 + offset]
```
`$3000..$3EFF` mirror `$2000..$2EFF`, so mask nametable addresses with `& 0x2FFF`
before decoding (palette region $3F00+ handled separately).

---

## 17. Palette RAM & master palette

### Layout ($3F00..$3F1F, 32 bytes)
- `$3F00` : universal backdrop color (shown for any transparent pixel).
- `$3F01..$3F03` : background palette 0 colors 1-3.
- `$3F05..$3F07` : background palette 1.
- `$3F09..$3F0B` : background palette 2.
- `$3F0D..$3F0F` : background palette 3.
- `$3F11..$3F13` : sprite palette 0 colors 1-3.
- `$3F15..$3F17` : sprite palette 1.
- `$3F19..$3F1B` : sprite palette 2.
- `$3F1D..$3F1F` : sprite palette 3.
- `$3F04/$3F08/$3F0C` : usually unused "color 0" of BG palettes 1-3 (writable,
  readable, only shown via specific tricks).

Each stored byte is a 6-bit master-palette index (bits 6-7 unused / open bus).

### Mirroring
- The whole `$3F00..$3F1F` block repeats every 32 bytes across `$3F00..$3FFF`
  (mask address with `& 0x1F`).
- `$3F10`, `$3F14`, `$3F18`, `$3F1C` are mirrors of `$3F00`, `$3F04`, `$3F08`,
  `$3F0C` respectively (the sprite palettes' "color 0" alias the background
  ones). Implement as: after `idx = addr & 0x1F`, if `idx & 0x13 == 0x10`
  (equivalently `idx in {0x10,0x14,0x18,0x1C}`), subtract 0x10. Apply on BOTH
  reads and writes so writing $3F10 updates $3F00 and vice versa.
```
fn pal_index(addr: u16) -> usize {
    let mut i = (addr & 0x1F) as usize;
    if i >= 0x10 && (i & 0x03) == 0 { i -= 0x10; }  // $3F10/14/18/1C -> $3F00/04/08/0C
    i
}
```

### Backdrop rendering nuance
When rendering is disabled but `v` points into the palette region, the PPU
actually outputs the palette entry at `v & 0x1F` (the "background palette hack").
For a first pass, outputting `$3F00` for all transparent pixels is sufficient to
render games and pass the target tests; model the v-in-palette case later if
needed.

### 64-color NTSC master palette (RGB)
The NESdev PPU_palettes page does NOT print an in-page RGB table; the 2C02
generates NTSC composite directly and there is no single "true" digital RGB
palette (the wiki ships downloadable `.pal` files, e.g. 2C02G_U_wiki.pal). Use a
widely-adopted decoded palette. Below is the common "2C02 / Nintendulator NTSC"
style table (index $00..$3F, RGB hex). Treat these as a good default; the
implementer can swap in a wiki `.pal` for closer accuracy. Entry $0D is a
"blacker-than-black" that some tables render as $000000.

```
$00 7C7C7C  $01 0000FC  $02 0000BC  $03 4428BC  $04 940084  $05 A80020  $06 A81000  $07 881400
$08 503000  $09 007800  $0A 006800  $0B 005800  $0C 004058  $0D 000000  $0E 000000  $0F 000000
$10 BCBCBC  $11 0078F8  $12 0058F8  $13 6844FC  $14 D800CC  $15 E40058  $16 F83800  $17 E45C10
$18 AC7C00  $19 00B800  $1A 00A800  $1B 00A844  $1C 008888  $1D 000000  $1E 000000  $1F 000000
$20 F8F8F8  $21 3CBCFC  $22 6888FC  $23 9878F8  $24 F878F8  $25 F85898  $26 F87858  $27 FCA044
$28 F8B800  $29 B8F818  $2A 58D854  $2B 58F898  $2C 00E8D8  $2D 787878  $2E 000000  $2F 000000
$30 FCFCFC  $31 A4E4FC  $32 B8B8F8  $33 D8B8F8  $34 F8B8F8  $35 F8A4C0  $36 F0D0B0  $37 FCE0A8
$38 F8D878  $39 D8F878  $3A B8F8B8  $3B B8F8D8  $3C 00FCFC  $3D F8D8F8  $3E 000000  $3F 000000
```

### Greyscale and emphasis at output
- Greyscale (PPUMASK bit0): AND the 6-bit palette index with `$30` before the
  master-palette lookup (keeps only the grey column $00,$10,$20,$30).
- Emphasis (PPUMASK bits 5,6,7 = R,G,B on NTSC; swap R/G on PAL/Dendy): darken
  the two non-selected channels. A simple, widely-used approximation multiplies
  each non-emphasized channel by ~0.816 per active emphasis bit; with all three
  set, all channels are dimmed. For pixel-exact emphasis, use a precomputed
  512-entry table (64 colors x 8 emphasis combos) from a `.pal` that includes
  emphasis. Exact emphasis is not required to pass the Blargg PPU timing tests.

---

## 18. What the Blargg PPU test ROMs exercise

- ppu_vbl_nmi: VBL flag + NMI enable + NMI interrupt, timed to ONE PPU clock.
  Must get right: vblank set at (241,1) and clear at (261,1); the $2002-read
  race at (241,0) and (241,1) that suppresses the flag/NMI; NMI as level-AND so
  toggling PPUCTRL bit7 during vblank fires (possibly multiple) NMIs; the exact
  ~6820-PPU-clock lifetime of the flag; suppression when reading $2002 near the
  set edge.
- sprite_hit (ppu_sprite_hit / sprite_hit_tests): sprite-0-hit condition and
  timing. Must get right: opaque-BG AND opaque-sprite-0 requirement; both
  rendering enables on; left-8 masking (bits 1,2) suppressing hits at x0..7;
  NO hit at x=255; hit set exactly at the producing dot; single hit per frame;
  clear at (261,1).
- sprite_overflow (ppu_sprite_overflow / sprite_overflow_tests): the overflow
  flag and its diagonal-scan hardware bug. Must faithfully reproduce the buggy
  step-3 `increment n AND m without carry`, the >8-sprite trigger, and the
  flag's set/clear timing (clear at pre-render dot 1). Do not fix the bug.
- (vbl_nmi_timing is an older, superseded version of ppu_vbl_nmi — same scope.)

---

## 19. Implementation order

1. Registers + memory + mirroring skeleton.
   - CPU-facing $2000..$2007 with mirroring; PPUCTRL/MASK/STATUS bit fields;
     OAMADDR/OAMDATA; PPUADDR/PPUSCROLL with shared `w`; PPUDATA buffered read
     + palette-read exception + auto-increment.
   - v/t/x/w registers and all the write/read update formulas (§6, §7).
   - PPU address decode (§15), nametable mirroring (§16), palette RAM mirroring
     (§17). Get a simple "poke VRAM via $2006/$2007, read it back buffered"
     round-trip working.
2. Background rendering.
   - Dot/scanline clock (341x262). 8-dot fetch cadence, shift registers, fine-X
     mux (§10). Coarse-X/Y increments and hori/vert copies at the exact dots
     (§8). Attribute quadrant selection. Output BG pixels through palette +
     master palette. Verify with a static-scroll test image, then scrolling.
3. Sprites.
   - OAM + secondary OAM; the dots 1-64 clear, 65-256 evaluation, 257-320 fetch
     (§12). Per-sprite shift units, X counters, flips, 8x16 mode. BG/sprite mux
     and priority (§11, §13).
4. Exact vblank/NMI timing + sprite-0 hit + overflow edge cases.
   - Vblank set/clear dots, NMI level-AND semantics, the $2002-read race, the
     odd-frame dot skip (§14). Sprite-0-hit exact conditions incl. x=255 and
     left-8 masking (§13). The sprite-overflow diagonal bug (§12). Validate
     against ppu_vbl_nmi, then sprite_hit, then sprite_overflow.
```
