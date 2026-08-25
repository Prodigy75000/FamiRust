; SPDX-License-Identifier: CC0-1.0
;
; LIAR'S KEEP -- palettes, the title screen, and the sound effect table.

; ---------------------------------------------------------------------------
; Palettes. Every background palette shares entry 0, because the PPU only has
; one backdrop colour and shows it through every transparent pixel on the
; screen; the three that follow are all a 16x16 block of the world gets.
;
; The honest platform and the liar are drawn out of palette 2 together. They
; have to be: giving the liar its own palette would be a tell, and a tell is
; the one thing this cartridge cannot afford.
; ---------------------------------------------------------------------------
palette:
  .byte $0F, $30, $10, $27   ; 0  HUD:   white, grey, torch orange
  .byte $0F, $2D, $00, $10   ; 1  ROCK:  three greys, dark to light
  .byte $0F, $06, $16, $27   ; 2  BRICK: the masonry you stand on. Or do not.
  .byte $0F, $16, $28, $30   ; 3  HOT:   fire, lava, the bonfire

  .byte $0F, $36, $16, $0F   ; 4  hero:  skin, red tunic, black outline
  .byte $0F, $30, $10, $0F   ; 5  steel: saw, crusher
  .byte $0F, $28, $27, $06   ; 6  fire:  darts
  .byte $0F, $2A, $1A, $0F   ; 7  green: the bait

; The torch colour that gets swapped every few frames. One palette write per
; frame animates every flame and every pool of lava in the room at once, which
; is a great deal cheaper than redrawing their tiles would be.
HOT_ENTRY = $3F0E
FLICKER_A = $28
FLICKER_B = $27

; ---------------------------------------------------------------------------
; The title screen, as a script: high byte, low byte, characters, $FF. A $00
; where the next high byte would be ends it.
; ---------------------------------------------------------------------------
title_script:
  .byte >(NT0+ 4*32+10), <(NT0+ 4*32+10)
  .str "LIAR", $07, "S KEEP"
  .byte $FF

  .byte >(NT0+ 8*32+ 4), <(NT0+ 8*32+ 4)
  .str "SOME OF THE FLOOR IS NOT"
  .byte $FF
  .byte >(NT0+ 9*32+ 4), <(NT0+ 9*32+ 4)
  .str "THE FLOOR. THERE IS NO"
  .byte $FF
  .byte >(NT0+10*32+ 4), <(NT0+10*32+ 4)
  .str "WAY TO TELL BY LOOKING."
  .byte $FF
  .byte >(NT0+11*32+ 4), <(NT0+11*32+ 4)
  .str "THAT IS THE GAME."
  .byte $FF

  .byte >(NT0+15*32+10), <(NT0+15*32+10)
  .str "PRESS START"
  .byte $FF

  .byte >(NT0+19*32+ 7), <(NT0+19*32+ 7)
  .str "D-PAD    MOVE"
  .byte $FF
  .byte >(NT0+20*32+ 7), <(NT0+20*32+ 7)
  .str "A        JUMP"
  .byte $FF
  .byte >(NT0+21*32+ 7), <(NT0+21*32+ 7)
  .str "SELECT   GIVE UP"
  .byte $FF

  .byte >(NT0+25*32+ 5), <(NT0+25*32+ 5)
  .str "PUBLIC DOMAIN  CC0 1.0"
  .byte $FF
  .byte >(NT0+26*32+ 5), <(NT0+26*32+ 5)
  .str "PRODIGY75000"
  .byte $FF
  .byte $00

; The screen shown once every room is behind you.
end_script:
  .byte >(NT0+ 9*32+ 8), <(NT0+ 9*32+ 8)
  .str "THE KEEP IS EMPTY"
  .byte $FF
  .byte >(NT0+12*32+ 9), <(NT0+12*32+ 9)
  .str "DEATHS"
  .byte $FF
  .byte >(NT0+14*32+ 9), <(NT0+14*32+ 9)
  .str "SOULS"
  .byte $FF
  .byte >(NT0+19*32+ 8), <(NT0+19*32+ 8)
  .str "PRESS START"
  .byte $FF
  .byte $00

; The HUD frame, drawn once per room and then only ever patched a digit at a
; time. Rows 0 to 3 of the screen; the playfield starts on row 4.
hud_script:
  .byte >(NT0+ 1*32+17), <(NT0+ 1*32+17)
  .str "DEATHS"
  .byte $FF
  .byte >(NT0+ 2*32+17), <(NT0+ 2*32+17)
  .str "SOULS"
  .byte $FF
  .byte >(NT0+ 3*32+ 1), <(NT0+ 3*32+ 1)
  .str "------------------------------"
  .byte $FF
  .byte $00

HUD_NAME   = NT0 + 1*32 + 1    ; where the room's name goes
HUD_DEATHS = NT0 + 1*32 + 24   ; four digits
HUD_SOULS  = NT0 + 2*32 + 24   ; four digits
END_DEATHS = NT0 + 12*32 + 17
END_SOULS  = NT0 + 14*32 + 17

; ---------------------------------------------------------------------------
; Sound effects. One effect at a time, which is all a game this small needs:
; each is a starting period, a signed amount added to that period every frame,
; a duration and a volume, on either pulse 1 or the noise channel.
;
; Adding to the period slides the pitch DOWN, so a rising chirp is a negative
; delta. That is backwards from how it sounds and is the sort of thing worth
; writing down once rather than rediscovering every time an effect comes out
; pointing the wrong way.
; ---------------------------------------------------------------------------
SFX_NONE    = 0
SFX_JUMP    = 1
SFX_LAND    = 2
SFX_DIE     = 3
SFX_PICK    = 4
SFX_CRUMBLE = 5
SFX_DART    = 6
SFX_WIN     = 7
SFX_UI      = 8
SFX_COUNT   = 9

SFX_PULSE = 0
SFX_NOISE = 1

sfx_tbl_ch:
  .byte SFX_PULSE, SFX_PULSE, SFX_PULSE, SFX_NOISE
  .byte SFX_PULSE, SFX_NOISE, SFX_NOISE, SFX_PULSE, SFX_PULSE
sfx_tbl_dur:
  .byte 0, 12, 6, 34, 12, 14, 4, 44, 6
sfx_tbl_plo:
  .byte 0, $A0, $20, $08, $60, $06, $02, $00, $80
sfx_tbl_phi:
  .byte 0, $01, $03, $00, $01, $00, $00, $02, $01
sfx_tbl_dp:
  .byte 0, $F4, $18, $01, $EC, $01, $00, $F8, $00
sfx_tbl_vol:
  .byte 0, 7, 4, 12, 9, 8, 5, 8, 6
sfx_tbl_duty:
  .byte 0, $80, $00, $00, $80, $00, $00, $40, $80
