; SPDX-License-Identifier: CC0-1.0
; FamiRust Demo Cart -- screens, palettes, attributes and music data.

; ---------------------------------------------------------------------------
; Palettes. 32 bytes each: four background palettes then four sprite palettes.
; Entry 0 of every palette mirrors $3F00, so the backdrop is written four times
; on purpose rather than by accident.
; ---------------------------------------------------------------------------

pal_menu:
  .byte $0F,$30,$10,$27     ; text: white, grey, amber
  .byte $0F,$27,$16,$12     ; title: amber
  .byte $0F,$2A,$1A,$0A     ; green
  .byte $0F,$21,$11,$01     ; licence line: cyan
  .byte $0F,$30,$01,$21     ; sprites: white highlight, dark rim, blue body
  .byte $0F,$30,$01,$27     ; amber body
  .byte $0F,$30,$01,$2A     ; green body
  .byte $0F,$30,$01,$24     ; pink body

pal_scroll:
  .byte $0F,$30,$10,$27     ; status text and the white sprite-0 bar
  .byte $0F,$21,$11,$30     ; sky, deep sky, white cloud
  .byte $0F,$16,$06,$00     ; brick, dark brick, grey mortar
  .byte $0F,$17,$2A,$07     ; soil, grass, dark soil
  .byte $0F,$30,$01,$21     ; sprite 0 hides against the white bar
  .byte $0F,$30,$01,$27
  .byte $0F,$30,$01,$2A
  .byte $0F,$30,$01,$24

pal_colors:
  .byte $0F,$30,$10,$27     ; text
  .byte $0F,$0F,$0F,$0F     ; three swatch palettes, filled in at run time
  .byte $0F,$0F,$0F,$0F
  .byte $0F,$0F,$0F,$0F
  .byte $0F,$30,$01,$21
  .byte $0F,$30,$01,$27
  .byte $0F,$30,$01,$2A
  .byte $0F,$30,$01,$24

; ---------------------------------------------------------------------------
; Attribute tables. One byte covers a 32x32 pixel cell; its four 2-bit fields
; are, from bit 0 up: top-left, top-right, bottom-left, bottom-right 16x16
; quadrant. Getting this wrong is the classic "my text went green" bug, so each
; row below names the tile rows it covers.
; ---------------------------------------------------------------------------

attr_menu:
  .res 8, $50               ; rows 0-3   : title (rows 2-3) on palette 1
  .res 8, $F0               ; rows 4-7   : rule (rows 6-7) on palette 3
  .res 8, $00               ; rows 8-11  : menu
  .res 8, $00               ; rows 12-15 : menu
  .res 8, $00               ; rows 16-19 : menu
  .res 8, $00               ; rows 20-23 : help
  .res 8, $FF               ; rows 24-27 : licence, palette 3
  .res 8, $FF               ; rows 28-31 : url, palette 3

attr_scroll:
  .res 8, $00               ; rows 0-3   : status bar and the white bar
  .res 8, $55               ; rows 4-7   : sky
  .res 8, $55               ; rows 8-11  : sky
  .res 8, $50               ; rows 12-15 : banner (0) over sky (1)
  .res 8, $A5               ; rows 16-19 : cloud sky (1) over brick (2)
  .res 8, $AA               ; rows 20-23 : brick
  .res 8, $FA               ; rows 24-27 : brick (2) over ground (3)
  .res 8, $FF               ; rows 28-31 : ground

attr_colors:
  .res 8, $00               ; rows 0-3   : title
  .res 8, $00               ; rows 4-7   : subtitle
  .res 8, $50               ; rows 8-11  : label (0) over swatch row 0 (1)
  .res 8, $A0               ; rows 12-15 : label (0) over swatch row 1 (2)
  .res 8, $F0               ; rows 16-19 : label (0) over swatch row 2 (3)
  .res 8, $00               ; rows 20-23 : help
  .res 8, $00               ; rows 24-27 : help
  .res 8, $00               ; rows 28-31

attr_plain:
  .res 64, $00

; ---------------------------------------------------------------------------
; Screen scripts. Each record is: nametable address high, low, then tile bytes
; until $FF. A high byte of $00 ends the script. `.str` writes text as tile
; indices, which for this cart's font is simply ASCII minus $20.
; ---------------------------------------------------------------------------

scr_menu:
  .byte >(NT0+2*32+8),  <(NT0+2*32+8)
  .str "F A M I R U S T"
  .byte $FF
  .byte >(NT0+4*32+6),  <(NT0+4*32+6)
  .str "DEMO CARTRIDGE  V1.0"
  .byte $FF
  .byte >(NT0+6*32+4),  <(NT0+6*32+4)
  .res 24, TILE_FRAMEH
  .byte $FF
  .byte >(NT0+9*32+8),  <(NT0+9*32+8)
  .str "1  SPRITES"
  .byte $FF
  .byte >(NT0+11*32+8), <(NT0+11*32+8)
  .str "2  SCROLL SPLIT"
  .byte $FF
  .byte >(NT0+13*32+8), <(NT0+13*32+8)
  .str "3  COLORS"
  .byte $FF
  .byte >(NT0+15*32+8), <(NT0+15*32+8)
  .str "4  AUDIO"
  .byte $FF
  .byte >(NT0+17*32+8), <(NT0+17*32+8)
  .str "5  INPUT"
  .byte $FF
  .byte >(NT0+20*32+5), <(NT0+20*32+5)
  .str "UP/DOWN     CHOOSE"
  .byte $FF
  .byte >(NT0+21*32+5), <(NT0+21*32+5)
  .str "A OR START  ENTER"
  .byte $FF
  .byte >(NT0+22*32+5), <(NT0+22*32+5)
  .str "B           BACK HERE"
  .byte $FF
  .byte >(NT0+24*32+4), <(NT0+24*32+4)
  .str "PUBLIC DOMAIN - CC0 1.0"
  .byte $FF
  .byte >(NT0+26*32+4), <(NT0+26*32+4)
  .str "NO RIGHTS RESERVED"
  .byte $FF
  .byte >(NT0+28*32+4), <(NT0+28*32+4)
  .str "GITHUB.COM/PRODIGY75000"
  .byte $FF
  .byte $00

scr_sprites:
  .byte >(NT0+1*32+4),  <(NT0+1*32+4)
  .str "SPRITE ENGINE"
  .byte $FF
  .byte >(NT0+3*32+4),  <(NT0+3*32+4)
  .str "64 OBJECTS  8 PER LINE"
  .byte $FF
  .byte >(NT0+24*32+4), <(NT0+24*32+4)
  .str "A   NEXT ARRANGEMENT"
  .byte $FF
  .byte >(NT0+26*32+4), <(NT0+26*32+4)
  .str "B   BACK TO MENU"
  .byte $FF
  .byte >(NT0+28*32+4), <(NT0+28*32+4)
  .str "MODE"
  .byte $FF
  .byte $00

scr_scroll_bar:
  .byte >(NT0+1*32+5),  <(NT0+1*32+5)
  .str "SPRITE 0 RASTER SPLIT"
  .byte $FF
  .byte >(NT0+2*32+11), <(NT0+2*32+11)
  .str "SPEED"
  .byte $FF
  .byte >(NT0+3*32+0),  <(NT0+3*32+0)
  .res 32, TILE_SOLID1
  .byte $FF
  ; The same white bar in the second nametable. Nothing above the split ever
  ; renders from there deliberately -- but the first PPUSCROLL write of the
  ; split changes fine X immediately, while the coarse scroll only takes effect
  ; at the end of the scanline. So the last few pixels of the split line come
  ; from the next nametable, and how many depends on the current scroll. Giving
  ; that nametable an identical bar makes the artifact invisible instead of
  ; leaving a black notch that breathes with the scroll.
  .byte >(NT1+3*32+0),  <(NT1+3*32+0)
  .res 32, TILE_SOLID1
  .byte $FF
  .byte $00

scr_colors:
  .byte >(NT0+1*32+4),  <(NT0+1*32+4)
  .str "COLOR TEST"
  .byte $FF
  .byte >(NT0+3*32+4),  <(NT0+3*32+4)
  .str "2C02 MASTER PALETTE"
  .byte $FF
  .byte >(NT0+24*32+4), <(NT0+24*32+4)
  .str "LEFT/RIGHT  STEP ONE"
  .byte $FF
  .byte >(NT0+26*32+4), <(NT0+26*32+4)
  .str "UP/DOWN     STEP NINE"
  .byte $FF
  .byte >(NT0+28*32+4), <(NT0+28*32+4)
  .str "B           BACK TO MENU"
  .byte $FF
  .byte $00

scr_audio:
  .byte >(NT0+1*32+4),  <(NT0+1*32+4)
  .str "AUDIO"
  .byte $FF
  .byte >(NT0+3*32+4),  <(NT0+3*32+4)
  .str "2A03  FIVE CHANNELS"
  .byte $FF
  .byte >(NT0+8*32+6),  <(NT0+8*32+6)
  .str "PULSE 1"
  .byte $FF
  .byte >(NT0+10*32+6), <(NT0+10*32+6)
  .str "PULSE 2"
  .byte $FF
  .byte >(NT0+12*32+6), <(NT0+12*32+6)
  .str "TRIANGLE"
  .byte $FF
  .byte >(NT0+14*32+6), <(NT0+14*32+6)
  .str "NOISE"
  .byte $FF
  .byte >(NT0+16*32+6), <(NT0+16*32+6)
  .str "DMC        START"
  .byte $FF
  .byte >(NT0+22*32+4), <(NT0+22*32+4)
  .str "UP/DOWN  PICK CHANNEL"
  .byte $FF
  .byte >(NT0+24*32+4), <(NT0+24*32+4)
  .str "A        MUTE"
  .byte $FF
  .byte >(NT0+26*32+4), <(NT0+26*32+4)
  .str "START    FIRE DMC"
  .byte $FF
  .byte >(NT0+28*32+4), <(NT0+28*32+4)
  .str "B        BACK TO MENU"
  .byte $FF
  .byte $00

scr_input:
  .byte >(NT0+1*32+4),  <(NT0+1*32+4)
  .str "INPUT TEST"
  .byte $FF
  .byte >(NT0+3*32+4),  <(NT0+3*32+4)
  .str "TWO PORTS"
  .byte $FF
  .byte >(NT0+6*32+5),  <(NT0+6*32+5)
  .str "PAD 1"
  .byte $FF
  .byte >(NT0+8*32+4),  <(NT0+8*32+4)
  .str "[ ] UP      [ ] A"
  .byte $FF
  .byte >(NT0+9*32+4),  <(NT0+9*32+4)
  .str "[ ] DOWN    [ ] B"
  .byte $FF
  .byte >(NT0+10*32+4), <(NT0+10*32+4)
  .str "[ ] LEFT    [ ] SELECT"
  .byte $FF
  .byte >(NT0+11*32+4), <(NT0+11*32+4)
  .str "[ ] RIGHT   [ ] START"
  .byte $FF
  .byte >(NT0+14*32+5), <(NT0+14*32+5)
  .str "PAD 2"
  .byte $FF
  .byte >(NT0+16*32+4), <(NT0+16*32+4)
  .str "[ ] UP      [ ] A"
  .byte $FF
  .byte >(NT0+17*32+4), <(NT0+17*32+4)
  .str "[ ] DOWN    [ ] B"
  .byte $FF
  .byte >(NT0+18*32+4), <(NT0+18*32+4)
  .str "[ ] LEFT    [ ] SELECT"
  .byte $FF
  .byte >(NT0+19*32+4), <(NT0+19*32+4)
  .str "[ ] RIGHT   [ ] START"
  .byte $FF
  .byte >(NT0+23*32+4), <(NT0+23*32+4)
  .str "PAD1 $00   PAD2 $00"
  .byte $FF
  .byte >(NT0+25*32+4), <(NT0+25*32+4)
  .str "FRAME $0000"
  .byte $FF
  .byte >(NT0+27*32+4), <(NT0+27*32+4)
  .str "B  BACK TO MENU"
  .byte $FF
  .byte $00

; The nine-character banner that scrolls through the middle of the split-scroll
; playfield. Repeated across both nametables by fill_playfield.
banner_text:
  .str "FAMIRUST"
BANNER_LEN = 8
; The banner is masked, not divided, so its length has to be a power of two;
; and it has to divide 32 exactly or the word restarts at the nametable seam.
.assert (BANNER_LEN & (BANNER_LEN-1)) == 0, "the banner length must be a power of two"
.assert 32 % BANNER_LEN == 0, "the banner must tile a 32-column nametable exactly"

; Arrangement names for the sprite scene, eight tiles each, patched in live.
spr_mode_names:
  .str "RING    "
  .str "LIMIT   "
  .str "FLICKER "
  .str "WAVE    "
SPR_MODE_COUNT = 4

; ON / OFF, three tiles each, patched into the audio channel list.
text_on:
  .str "ON "
text_off:
  .str "OFF"

; Screen addresses that get live single-tile updates. Kept as tables so the
; scene code stays arithmetic-free.

; First hex digit of each of the nine colour swatch labels.
colors_label_lo:
  .byte <(NT0+9*32+5),  <(NT0+9*32+13),  <(NT0+9*32+21)
  .byte <(NT0+13*32+5), <(NT0+13*32+13), <(NT0+13*32+21)
  .byte <(NT0+17*32+5), <(NT0+17*32+13), <(NT0+17*32+21)
colors_label_hi:
  .byte >(NT0+9*32+5),  >(NT0+9*32+13),  >(NT0+9*32+21)
  .byte >(NT0+13*32+5), >(NT0+13*32+13), >(NT0+13*32+21)
  .byte >(NT0+17*32+5), >(NT0+17*32+13), >(NT0+17*32+21)

; Palette RAM addresses for the nine swatch entries: palettes 1, 2 and 3,
; entries 1 through 3. Entry 0 of each is a mirror of the backdrop and is
; deliberately left alone.
colors_pal_lo:
  .byte $05,$06,$07, $09,$0A,$0B, $0D,$0E,$0F

; Top-left corner of each swatch block.
colors_swatch_lo:
  .byte <(NT0+10*32+4), <(NT0+10*32+12), <(NT0+10*32+20)
  .byte <(NT0+14*32+4), <(NT0+14*32+12), <(NT0+14*32+20)
  .byte <(NT0+18*32+4), <(NT0+18*32+12), <(NT0+18*32+20)
colors_swatch_hi:
  .byte >(NT0+10*32+4), >(NT0+10*32+12), >(NT0+10*32+20)
  .byte >(NT0+14*32+4), >(NT0+14*32+12), >(NT0+14*32+20)
  .byte >(NT0+18*32+4), >(NT0+18*32+12), >(NT0+18*32+20)

; The tile used for each swatch column: palette entry 1, 2 and 3.
colors_swatch_tile:
  .byte TILE_SOLID1, TILE_SOLID2, TILE_SOLID3
  .byte TILE_SOLID1, TILE_SOLID2, TILE_SOLID3
  .byte TILE_SOLID1, TILE_SOLID2, TILE_SOLID3

; Audio channel state field, three tiles at column 18 of each channel row.
audio_state_lo:
  .byte <(NT0+8*32+18), <(NT0+10*32+18), <(NT0+12*32+18), <(NT0+14*32+18)
audio_state_hi:
  .byte >(NT0+8*32+18), >(NT0+10*32+18), >(NT0+12*32+18), >(NT0+14*32+18)

; The eight button indicator cells for a pad, relative to the pad's first row.
; Left column at x=5, right column at x=17; four rows.
input_cell_lo:
  .byte <(NT0+8*32+5),  <(NT0+9*32+5),  <(NT0+10*32+5),  <(NT0+11*32+5)
  .byte <(NT0+8*32+17), <(NT0+9*32+17), <(NT0+10*32+17), <(NT0+11*32+17)
input_cell_hi:
  .byte >(NT0+8*32+5),  >(NT0+9*32+5),  >(NT0+10*32+5),  >(NT0+11*32+5)
  .byte >(NT0+8*32+17), >(NT0+9*32+17), >(NT0+10*32+17), >(NT0+11*32+17)

; Same cells for pad 2, which sits eight rows lower.
input_cell2_lo:
  .byte <(NT0+16*32+5),  <(NT0+17*32+5),  <(NT0+18*32+5),  <(NT0+19*32+5)
  .byte <(NT0+16*32+17), <(NT0+17*32+17), <(NT0+18*32+17), <(NT0+19*32+17)
input_cell2_hi:
  .byte >(NT0+16*32+5),  >(NT0+17*32+5),  >(NT0+18*32+5),  >(NT0+19*32+5)
  .byte >(NT0+16*32+17), >(NT0+17*32+17), >(NT0+18*32+17), >(NT0+19*32+17)

; Button bit for each indicator, in the order the cells are listed:
; up, down, left, right, then A, B, select, start.
input_cell_bit:
  .byte BTN_UP, BTN_DOWN, BTN_LEFT, BTN_RIGHT
  .byte BTN_A,  BTN_B,    BTN_SELECT, BTN_START

; ---------------------------------------------------------------------------
; Music: one 32-step pattern, original to this cart. Steps advance every
; `tempo` frames. HOLD leaves the channel sounding, REST silences it.
;
; The triangle's timer runs at half the pulse rate, so its notes are written an
; octave above where they sound: N_A3 here is heard as A2.
; ---------------------------------------------------------------------------

mus_pulse1:
  .byte N_A4, HOLD, N_C5, HOLD, N_E5, HOLD, N_C5, HOLD
  .byte N_A4, HOLD, N_B4, HOLD, N_C5, HOLD, N_B4, HOLD
  .byte N_A4, HOLD, N_C5, HOLD, N_F5, HOLD, N_E5, HOLD
  .byte N_D5, HOLD, N_C5, HOLD, N_B4, HOLD, N_G4, REST

mus_pulse2:
  .byte N_E4, HOLD, HOLD, HOLD, N_A4, HOLD, HOLD, HOLD
  .byte N_E4, HOLD, HOLD, HOLD, N_G4, HOLD, HOLD, HOLD
  .byte N_C4, HOLD, HOLD, HOLD, N_A4, HOLD, HOLD, HOLD
  .byte N_B3, HOLD, HOLD, HOLD, N_G4, HOLD, HOLD, REST

mus_tri:
  .byte N_A3, HOLD, HOLD, HOLD, N_A3, HOLD, HOLD, HOLD
  .byte N_E3, HOLD, HOLD, HOLD, N_E3, HOLD, HOLD, HOLD
  .byte N_F3, HOLD, HOLD, HOLD, N_F3, HOLD, HOLD, HOLD
  .byte N_G3, HOLD, HOLD, HOLD, N_G3, HOLD, HOLD, HOLD

; Noise: the value is the period index written to $400E, or REST.
mus_noise:
  .byte  13, REST,    2, REST,   13, REST,    2, REST
  .byte  13, REST,    2, REST,   13,    2,    2, REST
  .byte  13, REST,    2, REST,   13, REST,    2, REST
  .byte  13, REST,    2, REST,   13,    2,    2,    2

MUS_STEPS = 32
