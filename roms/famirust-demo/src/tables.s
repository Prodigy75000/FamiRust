; SPDX-License-Identifier: CC0-1.0
; FamiRust Demo Cart -- generated numeric tables.
;
; Nothing creative lives in this file: it is trigonometry and the NTSC 2A03
; timer arithmetic, worked out once at build time so the 6502 never has to
; multiply. Regenerate with tools/gentables.py.

; ---------------------------------------------------------------------------
; Ring positions, 64 evenly spaced angles. The values are already screen
; coordinates for an 8x8 sprite, including the fact that OAM Y is the sprite's
; top scanline minus one.
;   outer ring: centre (128,132), radii 60 x 44
;   inner ring: centre (128,132), radii 30 x 22
; ---------------------------------------------------------------------------
ring_x:
  .byte 184,184,183,181,179,177,174,170
  .byte 166,162,157,152,147,141,136,130
  .byte 124,118,112,107,101, 96, 91, 86
  .byte  82, 78, 74, 71, 69, 67, 65, 64
  .byte  64, 64, 65, 67, 69, 71, 74, 78
  .byte  82, 86, 91, 96,101,107,112,118
  .byte 124,130,136,141,147,152,157,162
  .byte 166,170,174,177,179,181,183,184

ring_y:
  .byte 127,131,136,140,144,148,151,155
  .byte 158,161,164,166,168,169,170,171
  .byte 171,171,170,169,168,166,164,161
  .byte 158,155,151,148,144,140,136,131
  .byte 127,123,118,114,110,106,103, 99
  .byte  96, 93, 90, 88, 86, 85, 84, 83
  .byte  83, 83, 84, 85, 86, 88, 90, 93
  .byte  96, 99,103,106,110,114,118,123

inner_x:
  .byte 154,154,153,153,152,150,149,147
  .byte 145,143,141,138,135,133,130,127
  .byte 124,121,118,115,113,110,107,105
  .byte 103,101, 99, 98, 96, 95, 95, 94
  .byte  94, 94, 95, 95, 96, 98, 99,101
  .byte 103,105,107,110,113,115,118,121
  .byte 124,127,130,133,135,138,141,143
  .byte 145,147,149,150,152,153,153,154

inner_y:
  .byte 127,129,131,133,135,137,139,141
  .byte 143,144,145,146,147,148,149,149
  .byte 149,149,149,148,147,146,145,144
  .byte 143,141,139,137,135,133,131,129
  .byte 127,125,123,121,119,117,115,113
  .byte 111,110,109,108,107,106,105,105
  .byte 105,105,105,106,107,108,109,110
  .byte 111,113,115,117,119,121,123,125

; Vertical sine for the WAVE arrangement.
wave_y:
  .byte 127,132,136,140,145,149,153,156
  .byte 160,163,165,168,169,171,172,173
  .byte 173,173,172,171,169,168,165,163
  .byte 160,156,153,149,145,140,136,132
  .byte 127,122,118,114,109,105,101, 98
  .byte  94, 91, 89, 86, 85, 83, 82, 81
  .byte  81, 81, 82, 83, 85, 86, 89, 91
  .byte  94, 98,101,105,109,114,118,122

; Shallow 0..7 bob for idle decoration.
bob:
  .byte   4,  4,  4,  5,  5,  5,  5,  6
  .byte   6,  6,  6,  7,  7,  7,  7,  7
  .byte   7,  7,  7,  7,  7,  7,  6,  6
  .byte   6,  6,  5,  5,  5,  5,  4,  4
  .byte   4,  3,  3,  2,  2,  2,  2,  1
  .byte   1,  1,  1,  0,  0,  0,  0,  0
  .byte   0,  0,  0,  0,  0,  0,  1,  1
  .byte   1,  1,  2,  2,  2,  2,  3,  3

; ---------------------------------------------------------------------------
; Pulse and triangle timer periods.
;
;   pulse frequency = CPU / (16 * (timer + 1))
;
; so timer = round(CPU / (16 * f)) - 1. The triangle's timer clocks at half the
; pulse rate, so writing a note from this table to the triangle sounds an
; octave lower; the music data compensates by naming the octave above.
;
; 48 notes, C2 up to B5. Index 0 is C2. N_<NOTE><OCTAVE> constants follow, so
; the pattern data reads like music rather than like arithmetic.
; ---------------------------------------------------------------------------
note_lo:
  .byte $AD,$4D,$F3,$9D,$4C,$00,$B8,$74
  .byte $34,$F8,$BF,$89,$56,$26,$F9,$CE
  .byte $A6,$80,$5C,$3A,$1A,$FB,$DF,$C4
  .byte $AB,$93,$7C,$67,$52,$3F,$2D,$1C
  .byte $0C,$FD,$EF,$E1,$D5,$C9,$BD,$B3
  .byte $A9,$9F,$96,$8E,$86,$7E,$77,$70
note_hi:
  .byte $06,$06,$05,$05,$05,$05,$04,$04
  .byte $04,$03,$03,$03,$03,$03,$02,$02
  .byte $02,$02,$02,$02,$02,$01,$01,$01
  .byte $01,$01,$01,$01,$01,$01,$01,$01
  .byte $01,$00,$00,$00,$00,$00,$00,$00
  .byte $00,$00,$00,$00,$00,$00,$00,$00

; Note-name constants for the pattern data.
N_C2 = 0
N_CS2 = 1
N_D2 = 2
N_DS2 = 3
N_E2 = 4
N_F2 = 5
N_FS2 = 6
N_G2 = 7
N_GS2 = 8
N_A2 = 9
N_AS2 = 10
N_B2 = 11
N_C3 = 12
N_CS3 = 13
N_D3 = 14
N_DS3 = 15
N_E3 = 16
N_F3 = 17
N_FS3 = 18
N_G3 = 19
N_GS3 = 20
N_A3 = 21
N_AS3 = 22
N_B3 = 23
N_C4 = 24
N_CS4 = 25
N_D4 = 26
N_DS4 = 27
N_E4 = 28
N_F4 = 29
N_FS4 = 30
N_G4 = 31
N_GS4 = 32
N_A4 = 33
N_AS4 = 34
N_B4 = 35
N_C5 = 36
N_CS5 = 37
N_D5 = 38
N_DS5 = 39
N_E5 = 40
N_F5 = 41
N_FS5 = 42
N_G5 = 43
N_GS5 = 44
N_A5 = 45
N_AS5 = 46
N_B5 = 47
REST = $FF
HOLD = $FE
