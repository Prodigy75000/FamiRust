; SPDX-License-Identifier: CC0-1.0
;
; FamiRust Demo Cart v1.1
; Written by Prodigy75000. Dedicated to the public domain under CC0 1.0.
;
; A test cartridge for the FamiRust NES core. It is not a game: it is five
; screens, each of which puts one part of the machine under load so that a
; person looking at the screen can tell whether the emulator got it right.
;
;   SPRITES  64 objects, and a layout that deliberately breaks the hardware's
;            eight-sprites-per-scanline limit so the dropout is visible.
;   SCROLL   a status bar held still over a scrolling playfield by a mid-frame
;            sprite-0 hit; the scroll crosses both nametables.
;   COLORS   swatches straight out of the 2C02 master palette.
;   AUDIO    all five 2A03 channels, individually mutable, plus a DPCM sample.
;   INPUT    both controller ports, read with the standard double-read that
;            makes the read safe while DMC DMA is running.
;
; Mapper 0 (NROM), 32 KB PRG, 8 KB CHR, vertical mirroring. Nothing here needs
; a mapper, and NROM is the format every emulator loads correctly or fails
; loudly, which is what you want from a reference cart.

.ines prg=32 chr=8 mapper=0 mirror=v

; ---------------------------------------------------------------------------
; Hardware
; ---------------------------------------------------------------------------
PPUCTRL   = $2000
PPUMASK   = $2001
PPUSTATUS = $2002
OAMADDR   = $2003
PPUSCROLL = $2005
PPUADDR   = $2006
PPUDATA   = $2007
OAMDMA    = $4014
JOY1      = $4016
JOY2      = $4017

SQ1_VOL   = $4000
SQ1_SWEEP = $4001
SQ1_LO    = $4002
SQ1_HI    = $4003
SQ2_VOL   = $4004
SQ2_SWEEP = $4005
SQ2_LO    = $4006
SQ2_HI    = $4007
TRI_LIN   = $4008
TRI_LO    = $400A
TRI_HI    = $400B
NOISE_VOL = $400C
NOISE_LO  = $400E
NOISE_HI  = $400F
DMC_FREQ  = $4010
DMC_ADDR  = $4012
DMC_LEN   = $4013
APUSTATUS = $4015
APUFRAME  = $4017

NT0 = $2000
NT1 = $2400

; PPUCTRL: NMI on, 8x8 sprites, backgrounds from pattern table 0, sprites from
; pattern table 1, base nametable 0.
CTRL_BASE = %10001000
; PPUMASK: background and sprites on, including in the leftmost eight pixels.
MASK_ON   = %00011110

BTN_A      = $80
BTN_B      = $40
BTN_SELECT = $20
BTN_START  = $10
BTN_UP     = $08
BTN_DOWN   = $04
BTN_LEFT   = $02
BTN_RIGHT  = $01

; Background tiles. The font occupies $00-$3F (ASCII minus $20); graphics start
; at $60. The asserts at the bottom of this file check these against the labels
; the character ROM actually produced.
TILE_SPACE   = $00
TILE_SOLID1  = $60
TILE_SOLID2  = $61
TILE_SOLID3  = $62
TILE_STAR    = $63
TILE_BRICK   = $64
TILE_GROUND  = $65
TILE_CLOUDL  = $66
TILE_CLOUDR  = $67
TILE_FRAMEH  = $68
TILE_SKYSTAR = $70

; Sprite tiles, relative to pattern table 1.
SPR_BLANK  = 0
SPR_BLOCK  = 1
SPR_ORB    = 2
SPR_GEM    = 3
SPR_CURSOR = 4
SPR_S0LINE = 8

MODE_MENU    = 0
MODE_SPRITES = 1
MODE_SCROLL  = 2
MODE_COLORS  = 3
MODE_AUDIO   = 4
MODE_INPUT   = 5
MODE_COUNT   = 6
MODE_NONE    = $FF

PATCH_MAX = 32

; ---------------------------------------------------------------------------
; Zero page and RAM
; ---------------------------------------------------------------------------
nmi_ready    = $00
frame_lo     = $01
frame_hi     = $02
pad1         = $03
pad1_new     = $04
pad1_prev    = $05
pad2         = $06
pad2_new     = $07
pad2_prev    = $08
mode         = $09
pending      = $0A
menu_sel     = $0B
scroll_lo    = $0C
scroll_hi    = $0D
ctrl_shadow  = $0E
mask_shadow  = $0F
patch_count  = $10
tmp0         = $11
tmp1         = $12
tmp2         = $13
tmp3         = $14
tmp4         = $15
tmp5         = $16
ptr          = $17
ptr2         = $19
anim         = $1B
color_base   = $1C
spr_layout   = $1D
scroll_speed = $1E
audio_sel    = $1F
audio_mute   = $20
music_step   = $21
music_timer  = $22
noise_vol    = $23
blip_timer   = $24
tempo        = $25
patch_tmp    = $26

oam       = $0200
patch_hi  = $0300
patch_lo  = $0320
patch_val = $0340

.org $8000

; ---------------------------------------------------------------------------
; Reset
; ---------------------------------------------------------------------------
reset:
  sei
  cld
  ldx #$40
  stx APUFRAME              ; four-step frame counter, frame IRQ off
  ldx #$FF
  txs
  inx                       ; X = 0
  stx PPUCTRL               ; NMI off while we set up
  stx PPUMASK               ; rendering off
  stx DMC_FREQ              ; DMC IRQ off
  bit PPUSTATUS

@vblank1:
  bit PPUSTATUS
  bpl @vblank1

  ; Clear RAM. Page 2 is the OAM shadow, so it is filled with $FF instead of
  ; zero: a Y of $FF parks every sprite below the visible screen.
  lda #0
  tax
@clear:
  sta $0000,x
  sta $0100,x
  sta $0300,x
  sta $0400,x
  sta $0500,x
  sta $0600,x
  sta $0700,x
  lda #$FF
  sta $0200,x
  lda #0
  inx
  bne @clear

@vblank2:
  bit PPUSTATUS
  bpl @vblank2

  jsr silence_apu

  lda #MODE_NONE
  sta pending
  lda #2
  sta scroll_speed
  lda #6
  sta tempo
  lda #MODE_MENU
  sta mode
  jsr enter_menu

; ---------------------------------------------------------------------------
; Main loop. One pass per displayed frame: sync, read the pads, run the current
; scene, then honour a scene change if the scene asked for one.
; ---------------------------------------------------------------------------
main:
  jsr wait_frame
  jsr read_pads
  inc anim

  ; B always goes back to the menu, from anywhere but the menu.
  lda mode
  beq @tick
  lda pad1_new
  and #BTN_B
  beq @tick
  lda #MODE_MENU
  sta pending

@tick:
  ; Everything a scene queues for the PPU is rebuilt from scratch each frame,
  ; so the queue is emptied here and not by the NMI. That way the NMI can only
  ; ever flush entries that were completely written.
  lda #0
  sta patch_count
  jsr tick_blip

  ldx mode
  lda tick_lo,x
  sta ptr
  lda tick_hi,x
  sta ptr+1
  jsr call_ptr

  lda pending
  cmp #MODE_NONE
  beq main
  jsr enter_scene
  jmp main

call_ptr:
  jmp (ptr)

; ---------------------------------------------------------------------------
; Scene entry: rendering off, redraw everything, rendering back on.
; ---------------------------------------------------------------------------
enter_scene:
  lda pending
  sta mode
  lda #MODE_NONE
  sta pending
  ldx mode
  lda enter_lo,x
  sta ptr
  lda enter_hi,x
  sta ptr+1
  jmp (ptr)                 ; the enter routine returns to our caller

enter_lo:
  .byte <enter_menu, <enter_sprites, <enter_scroll
  .byte <enter_colors, <enter_audio, <enter_input
enter_hi:
  .byte >enter_menu, >enter_sprites, >enter_scroll
  .byte >enter_colors, >enter_audio, >enter_input
tick_lo:
  .byte <tick_menu, <tick_sprites, <tick_scroll
  .byte <tick_colors, <tick_audio, <tick_input
tick_hi:
  .byte >tick_menu, >tick_sprites, >tick_scroll
  .byte >tick_colors, >tick_audio, >tick_input

; ---------------------------------------------------------------------------
; NMI. Owns everything that has to happen inside vertical blank: the sprite DMA,
; the queued single-tile text updates, and the scroll registers. Scenes never
; touch the PPU directly while rendering is on.
; ---------------------------------------------------------------------------
nmi:
  pha
  txa
  pha
  tya
  pha

  lda #0
  sta OAMADDR
  lda #>oam
  sta OAMDMA

  jsr flush_patches         ; clobbers the PPU address, so it goes before scroll

  lda mode
  cmp #MODE_SCROLL
  beq @split

  lda ctrl_shadow
  sta PPUCTRL
  lda mask_shadow
  sta PPUMASK
  lda #0
  sta PPUSCROLL
  sta PPUSCROLL
  jmp @done

@split:
  jsr split_raster

@done:
  inc frame_lo
  bne @nowrap
  inc frame_hi
@nowrap:
  lda #1
  sta nmi_ready

  pla
  tay
  pla
  tax
  pla
irq:
  rti

; The mid-frame split. Vertical blank sets the registers the status bar wants;
; then we spin on the sprite-0 flag and set them again for the playfield. The
; flag is still set from the previous frame when the NMI fires, so the first
; wait is for it to CLEAR at the pre-render line, and only then for the hit.
; Both waits are bounded: a scene that somehow never produces a hit costs one
; ugly frame, not a hang.
split_raster:
  lda ctrl_shadow
  and #%11111100            ; status bar always comes from nametable 0
  sta PPUCTRL
  lda mask_shadow
  sta PPUMASK
  lda #0
  sta PPUSCROLL
  sta PPUSCROLL

  bit PPUSTATUS             ; known latch state before the spin
  ldy #6
@wait_clear:
  ldx #0
@wc:
  bit PPUSTATUS
  bvc @clear_seen
  dex
  bne @wc
  dey
  bne @wait_clear
  rts                       ; timed out
@clear_seen:

  ldy #6
@wait_hit:
  ldx #0
@wh:
  bit PPUSTATUS
  bvs @hit
  dex
  bne @wh
  dey
  bne @wait_hit
  rts                       ; timed out
@hit:

  ; Past the split. Writing PPUCTRL puts the scroll's ninth bit into the
  ; nametable select, and the first PPUSCROLL write sets coarse and fine X.
  ; Both land in the address latch and are copied to the running address at the
  ; end of this scanline, so the change takes effect on the next line.
  lda ctrl_shadow
  and #%11111100
  ora scroll_hi
  sta PPUCTRL
  lda scroll_lo
  sta PPUSCROLL
  lda #0
  sta PPUSCROLL
  rts

flush_patches:
  lda patch_count
  beq @done
  ldx #0
@l:
  cpx patch_count
  beq @done
  lda patch_hi,x
  sta PPUADDR
  lda patch_lo,x
  sta PPUADDR
  lda patch_val,x
  sta PPUDATA
  inx
  jmp @l
@done:
  rts

; ---------------------------------------------------------------------------
; Frame sync and controllers
; ---------------------------------------------------------------------------
wait_frame:
  lda #0
  sta nmi_ready
@w:
  lda nmi_ready
  beq @w
  rts

; Read both pads, twice, until two consecutive reads agree. DMC DMA can steal a
; cycle from the controller shift register and corrupt a single read; comparing
; two of them is the standard defence, and this cart runs DPCM, so it needs it.
read_pads:
  lda pad1
  sta pad1_prev
  lda pad2
  sta pad2_prev
@again:
  jsr strobe_read
  lda tmp0
  sta tmp2
  lda tmp1
  sta tmp3
  jsr strobe_read
  lda tmp0
  cmp tmp2
  bne @again
  lda tmp1
  cmp tmp3
  bne @again

  lda tmp0
  sta pad1
  eor pad1_prev
  and pad1
  sta pad1_new
  lda tmp1
  sta pad2
  eor pad2_prev
  and pad2
  sta pad2_new
  rts

; One strobe-and-shift pass. Leaves pad 1 in tmp0 and pad 2 in tmp1, with A in
; bit 7 down to right in bit 0.
strobe_read:
  lda #1
  sta JOY1
  lda #0
  sta JOY1
  ldx #8
@l:
  lda JOY1
  lsr a
  rol tmp0
  lda JOY2
  lsr a
  rol tmp1
  dex
  bne @l
  rts

; ---------------------------------------------------------------------------
; PPU helpers. All of these assume rendering is off.
; ---------------------------------------------------------------------------
rendering_off:
  lda #0
  sta PPUMASK
  sta mask_shadow
  sta PPUCTRL
  sta ctrl_shadow
  ; Drop anything the outgoing scene queued. Its tick still ran on the frame the
  ; scene change was requested, so the queue is full of writes aimed at a screen
  ; that is about to be painted over. Flushing them afterwards would stamp the
  ; old scene's text onto the new one.
  sta patch_count
  rts

rendering_on:
  bit PPUSTATUS
@w:
  bit PPUSTATUS
  bpl @w
  lda #0
  sta PPUSCROLL
  sta PPUSCROLL
  lda #MASK_ON
  sta mask_shadow
  sta PPUMASK
  lda #CTRL_BASE
  sta ctrl_shadow
  sta PPUCTRL
  rts

; Fill both nametables and both attribute tables with zero.
clear_vram:
  lda #$20
  sta PPUADDR
  lda #$00
  sta PPUADDR
  lda #0
  ldx #16
  ldy #0
@l:
  sta PPUDATA
  iny
  bne @l
  dex
  bne @l
  rts

; ptr -> palette, 32 bytes.
load_palette:
  bit PPUSTATUS
  lda #$3F
  sta PPUADDR
  lda #$00
  sta PPUADDR
  ldy #0
@l:
  lda (ptr),y
  sta PPUDATA
  iny
  cpy #32
  bne @l
  ; Park the address away from palette space; leaving it inside $3F00-$3FFF
  ; while rendering is off makes the backdrop show the pointed-at entry.
  lda #$20
  sta PPUADDR
  lda #$00
  sta PPUADDR
  rts

; ptr -> 64 attribute bytes, tmp0 = nametable high byte ($20 or $24).
load_attr:
  lda tmp0
  clc
  adc #3
  sta PPUADDR
  lda #$C0
  sta PPUADDR
  ldy #0
@l:
  lda (ptr),y
  sta PPUDATA
  iny
  cpy #64
  bne @l
  rts

; ptr -> screen script.
draw_script:
@rec:
  ldy #0
  lda (ptr),y
  bne @go
  rts
@go:
  sta PPUADDR
  iny
  lda (ptr),y
  sta PPUADDR
  iny
@chars:
  lda (ptr),y
  cmp #$FF
  beq @next
  sta PPUDATA
  iny
  jmp @chars
@next:
  iny
  tya
  clc
  adc ptr
  sta ptr
  lda #0
  adc ptr+1
  sta ptr+1
  jmp @rec

hide_sprites:
  ldx #0
  lda #$FF
@l:
  sta oam,x
  inx
  inx
  inx
  inx
  bne @l
  rts

; A = tile value, tmp4 = target high byte, tmp5 = target low byte.
; X and Y survive: every caller is a loop that indexes with one of them.
push_patch:
  sta patch_tmp
  txa
  pha
  ldx patch_count
  cpx #PATCH_MAX
  bcs @full
  lda patch_tmp
  sta patch_val,x
  lda tmp4
  sta patch_hi,x
  lda tmp5
  sta patch_lo,x
  inc patch_count
@full:
  pla
  tax
  rts

; A = byte. Leaves the high nibble's tile in tmp2 and the low nibble's in tmp3.
byte_to_hex:
  pha
  lsr a
  lsr a
  lsr a
  lsr a
  jsr nybble_tile
  sta tmp2
  pla
  and #$0F
  jsr nybble_tile
  sta tmp3
  rts

nybble_tile:
  cmp #10
  bcs @letter
  clc
  adc #$10                  ; '0' is tile $10
  rts
@letter:
  clc
  adc #$17                  ; 'A' is tile $21, and this nibble is already >= 10
  rts

; Push a two-digit hex number at tmp4/tmp5, then the digit after it.
push_hex:
  jsr byte_to_hex
  lda tmp2
  jsr push_patch
  inc tmp5                  ; the two digits are always adjacent on one row
  lda tmp3
  jsr push_patch
  rts

; ---------------------------------------------------------------------------
; Audio helpers
; ---------------------------------------------------------------------------
silence_apu:
  lda #0
  sta APUSTATUS
  sta SQ1_VOL
  sta SQ2_VOL
  sta TRI_LIN
  sta NOISE_VOL
  lda #$08
  sta SQ1_SWEEP             ; a zero here would trip the sweep mute
  sta SQ2_SWEEP
  lda #%00001111
  sta APUSTATUS
  lda #0
  sta noise_vol
  sta blip_timer
  rts

; A short pulse-1 click for menu movement, used by every scene except AUDIO
; (which owns the channel).
ui_blip:
  lda mode
  cmp #MODE_AUDIO
  beq @skip
  lda #%10111000            ; duty 50%, halt length, constant volume 8
  sta SQ1_VOL
  lda #$08
  sta SQ1_SWEEP
  lda #$40
  sta SQ1_LO
  lda #$00
  sta SQ1_HI
  lda #4
  sta blip_timer
@skip:
  rts

tick_blip:
  lda mode
  cmp #MODE_AUDIO
  beq @skip
  lda blip_timer
  beq @skip
  dec blip_timer
  bne @skip
  lda #%10110000            ; volume 0
  sta SQ1_VOL
@skip:
  rts

; ---------------------------------------------------------------------------
; Scene: MENU
; ---------------------------------------------------------------------------
MENU_ITEMS = 5
MENU_ROW0  = 9
MENU_STEP  = 2
ORB_ROW    = 7

enter_menu:
  jsr rendering_off
  jsr hide_sprites
  jsr silence_apu
  jsr clear_vram
  lda #<pal_menu
  sta ptr
  lda #>pal_menu
  sta ptr+1
  jsr load_palette
  lda #<attr_menu
  sta ptr
  lda #>attr_menu
  sta ptr+1
  lda #$20
  sta tmp0
  jsr load_attr
  lda #<scr_menu
  sta ptr
  lda #>scr_menu
  sta ptr+1
  jsr draw_script
  jsr rendering_on
  rts

tick_menu:
  lda pad1_new
  and #BTN_UP
  beq @nodown
  dec menu_sel
  bpl @moved
  lda #MENU_ITEMS-1
  sta menu_sel
  jmp @moved
@nodown:
  lda pad1_new
  and #BTN_DOWN
  beq @nomove
  inc menu_sel
  lda menu_sel
  cmp #MENU_ITEMS
  bcc @moved
  lda #0
  sta menu_sel
@moved:
  jsr ui_blip
@nomove:

  lda pad1_new
  and #BTN_A|BTN_START
  beq @nofire
  lda menu_sel
  clc
  adc #1
  sta pending
@nofire:

  ; Sprite 0 is the cursor; four orbs bob along the bottom for a sign of life.
  lda menu_sel
  asl a
  asl a
  asl a
  asl a                     ; menu_sel * 16
  clc
  adc #MENU_ROW0*8-1
  sta oam+0
  lda #SPR_CURSOR
  sta oam+1
  lda #0
  sta oam+2
  lda #48
  sta oam+3

  ; Four orbs bobbing in the empty band at rows 18-19, so the title screen
  ; shows a moving sprite without covering anything. `bob` is 0..7, which keeps
  ; all eight scanlines of the sprite inside that band.
  lda #0
  sta tmp0                  ; orb index 0..3
  ldx #0                    ; OAM byte offset, four bytes per orb
@orb:
  lda tmp0
  asl a
  asl a
  asl a
  asl a                     ; orb * 16, so the four bob out of phase
  clc
  adc anim
  and #$3F
  tay
  lda bob,y
  clc
  adc #ORB_ROW*8
  sta oam+4,x
  lda #SPR_ORB
  sta oam+5,x
  lda tmp0
  and #3
  sta oam+6,x
  lda tmp0
  asl a
  asl a
  asl a
  asl a
  asl a
  asl a                     ; orb * 64
  clc
  adc #28
  sta oam+7,x
  inx
  inx
  inx
  inx
  inc tmp0
  lda tmp0
  cmp #4
  bne @orb
  rts

; ---------------------------------------------------------------------------
; Scene: SPRITES
; ---------------------------------------------------------------------------
enter_sprites:
  jsr rendering_off
  jsr hide_sprites
  jsr silence_apu
  jsr clear_vram
  lda #<pal_menu
  sta ptr
  lda #>pal_menu
  sta ptr+1
  jsr load_palette
  lda #<attr_plain
  sta ptr
  lda #>attr_plain
  sta ptr+1
  lda #$20
  sta tmp0
  jsr load_attr
  lda #<scr_sprites
  sta ptr
  lda #>scr_sprites
  sta ptr+1
  jsr draw_script
  jsr rendering_on
  rts

tick_sprites:
  lda pad1_new
  and #BTN_A
  beq @nocycle
  inc spr_layout
  lda spr_layout
  cmp #SPR_MODE_COUNT
  bcc @keep
  lda #0
  sta spr_layout
@keep:
  jsr ui_blip
@nocycle:

  ; Name of the current arrangement, eight tiles at row 27 column 9.
  lda spr_layout
  asl a
  asl a
  asl a                     ; * 8
  tay
  ldx #0
@name:
  lda #>(NT0+28*32+9)
  sta tmp4
  txa
  clc
  adc #<(NT0+28*32+9)
  sta tmp5
  lda spr_mode_names,y
  jsr push_patch
  iny
  inx
  cpx #8
  bne @name

  ; The arrangements are further away than a branch reaches, so the dispatch
  ; branches locally and jumps from there.
  lda spr_layout
  cmp #1
  beq @limit
  cmp #2
  beq @flicker
  cmp #3
  beq @wave
  jmp layout_ring
@limit:
  jmp layout_limit
@flicker:
  jmp layout_flicker
@wave:
  jmp layout_wave

; Two counter-rotating rings, alternating by sprite index, which spreads all 64
; objects over 64 distinct angles.
layout_ring:
  lda #0
  sta tmp0                  ; sprite index
  ldx #0
@l:
  lda tmp0
  and #1
  bne @inner

  lda tmp0
  asl a
  clc
  adc anim
  and #$3F
  tay
  lda ring_y,y
  sta oam,x
  lda ring_x,y
  sta tmp1
  jmp @place

@inner:
  lda tmp0
  asl a
  sta tmp2
  lda anim
  sec
  sbc tmp2                  ; the inner ring turns the other way
  and #$3F
  tay
  lda inner_y,y
  sta oam,x
  lda inner_x,y
  sta tmp1

@place:
  lda #SPR_ORB
  sta oam+1,x
  lda tmp0
  lsr a
  and #3
  sta oam+2,x
  lda tmp1
  sta oam+3,x
  inx
  inx
  inx
  inx
  inc tmp0
  lda tmp0
  cmp #64
  bne @l
  rts

; Sixteen sprites to a row. The hardware renders eight; the rest vanish. That
; is the point of this arrangement, so nothing here hides it.
layout_limit:
  lda #0
  sta tmp0
  ldx #0
@l:
  lda tmp0
  lsr a
  lsr a
  lsr a
  lsr a                     ; row = index / 16
  asl a
  asl a
  asl a
  asl a                     ; row * 16
  clc
  adc #104
  sta oam,x
  lda #SPR_GEM
  sta oam+1,x
  lda tmp0
  lsr a
  lsr a
  lsr a
  lsr a
  and #3
  sta oam+2,x
  lda tmp0
  and #15
  jsr times14
  clc
  adc #14
  sta oam+3,x
  inx
  inx
  inx
  inx
  inc tmp0
  lda tmp0
  cmp #64
  bne @l
  rts

; The same sixteen-to-a-row layout, but which sprite occupies which OAM slot
; rotates every frame. The dropout then lands on a different eight each frame
; and the eye reassembles the row: the flicker trick every NES game used.
layout_flicker:
  lda #0
  sta tmp0                  ; OAM slot
  ldx #0
@l:
  lda tmp0
  clc
  adc anim
  and #$3F
  sta tmp1                  ; logical sprite for this slot

  lda tmp1
  lsr a
  lsr a
  lsr a
  lsr a
  asl a
  asl a
  asl a
  asl a
  clc
  adc #104
  sta oam,x
  lda #SPR_GEM
  sta oam+1,x
  lda tmp1
  lsr a
  lsr a
  lsr a
  lsr a
  and #3
  sta oam+2,x
  lda tmp1
  and #15
  jsr times14
  clc
  adc #14
  sta oam+3,x
  inx
  inx
  inx
  inx
  inc tmp0
  lda tmp0
  cmp #64
  bne @l
  rts

layout_wave:
  lda #0
  sta tmp0
  ldx #0
@l:
  lda tmp0
  asl a
  clc
  adc anim
  and #$3F
  tay
  lda wave_y,y
  sta oam,x
  lda #SPR_ORB
  sta oam+1,x
  lda tmp0
  lsr a
  lsr a
  lsr a
  and #3
  sta oam+2,x
  lda tmp0
  asl a
  asl a                     ; x = index * 4, spanning the whole screen
  sta oam+3,x
  inx
  inx
  inx
  inx
  inc tmp0
  lda tmp0
  cmp #64
  bne @l
  rts

; A * 14, without a multiply: (A << 3) + (A << 2) + (A << 1).
times14:
  sta tmp2
  asl a
  sta tmp3                  ; 2A
  asl a                     ; 4A
  clc
  adc tmp3                  ; 6A
  sta tmp3
  lda tmp2
  asl a
  asl a
  asl a                     ; 8A
  clc
  adc tmp3
  rts

; ---------------------------------------------------------------------------
; Scene: SCROLL SPLIT
; ---------------------------------------------------------------------------
enter_scroll:
  jsr rendering_off
  jsr hide_sprites
  jsr silence_apu
  jsr clear_vram
  lda #<pal_scroll
  sta ptr
  lda #>pal_scroll
  sta ptr+1
  jsr load_palette

  lda #<attr_scroll
  sta ptr
  lda #>attr_scroll
  sta ptr+1
  lda #$20
  sta tmp0
  jsr load_attr
  lda #<attr_scroll
  sta ptr
  lda #>attr_scroll
  sta ptr+1
  lda #$24
  sta tmp0
  jsr load_attr

  lda #$20
  sta tmp0
  jsr fill_playfield
  lda #$24
  sta tmp0
  jsr fill_playfield

  lda #<scr_scroll_bar
  sta ptr
  lda #>scr_scroll_bar
  sta ptr+1
  jsr draw_script

  ; Sprite 0 sits on the white bar at row 3 and is coloured to match it, so the
  ; thing that drives the split is invisible. Its top scanline is 31, which is
  ; the last line of the bar, so the scroll changes at line 32 exactly.
  lda #30
  sta oam+0
  lda #SPR_S0LINE
  sta oam+1
  lda #0
  sta oam+2
  lda #8
  sta oam+3

  lda #0
  sta scroll_lo
  sta scroll_hi
  jsr rendering_on
  rts

tick_scroll:
  lda pad1_new
  and #BTN_RIGHT
  beq @noright
  lda scroll_speed
  cmp #6
  bcs @noright
  inc scroll_speed
  jsr ui_blip
@noright:
  lda pad1_new
  and #BTN_LEFT
  beq @noleft
  lda scroll_speed
  cmp #256-6
  beq @noleft
  dec scroll_speed
  jsr ui_blip
@noleft:

  ; Nine-bit scroll: the low byte lives in PPUSCROLL, the ninth bit is the
  ; nametable select in PPUCTRL. A signed step means the carry means different
  ; things in each direction, so the two cases are handled apart.
  lda scroll_speed
  bmi @back
  lda scroll_lo
  clc
  adc scroll_speed
  sta scroll_lo
  bcc @shown
  jmp @flip
@back:
  lda scroll_lo
  clc
  adc scroll_speed
  sta scroll_lo
  bcs @shown
@flip:
  lda scroll_hi
  eor #1
  sta scroll_hi
@shown:

  ; "SPEED" readout: a direction arrow then one digit.
  lda #>(NT0+2*32+17)
  sta tmp4
  lda #<(NT0+2*32+17)
  sta tmp5
  lda scroll_speed
  bmi @neg
  beq @zero
  lda #$1E                  ; '>'
  jsr push_patch
  lda scroll_speed
  jmp @digit
@neg:
  lda #$1C                  ; '<'
  jsr push_patch
  lda #0
  sec
  sbc scroll_speed
  jmp @digit
@zero:
  lda #$0D                  ; '-'
  jsr push_patch
  lda #0
@digit:
  clc
  adc #$10                  ; digit tile
  pha
  inc tmp5
  pla
  jsr push_patch
  rts

; Paint rows 4 to 29 of the nametable whose high byte is in tmp0. Rows 0 to 3
; are the status bar and are left to the script.
fill_playfield:
  lda #4
  sta tmp1
@row:
  lda tmp1
  lsr a
  lsr a
  lsr a
  clc
  adc tmp0
  sta PPUADDR
  lda tmp1
  asl a
  asl a
  asl a
  asl a
  asl a                     ; (row * 32) & $FF
  sta PPUADDR
  ldx #0
@col:
  jsr playfield_tile
  sta PPUDATA
  inx
  cpx #32
  bne @col
  inc tmp1
  lda tmp1
  cmp #30
  bne @row
  rts

; Row in tmp1, column in X. Returns the tile in A and must leave X alone.
playfield_tile:
  lda tmp1
  cmp #12
  bcc @sky
  cmp #14
  bcc @band
  cmp #18
  bcc @sky
  cmp #26
  bcc @brick
  lda #TILE_GROUND
  rts
@brick:
  lda #TILE_BRICK
  rts
@band:
  lda tmp1
  cmp #13
  beq @blank
  txa
  and #BANNER_LEN-1
  tay
  lda banner_text,y
  rts
@blank:
  lda #TILE_SPACE
  rts
@sky:
  txa
  asl a
  eor tmp1
  and #$0F
  cmp #5
  beq @star
  lda tmp1
  cmp #16
  bcc @plain
  cmp #18
  bcs @plain
  txa
  and #7
  cmp #2
  beq @cloudl
  cmp #3
  beq @cloudr
@plain:
  lda #TILE_SOLID1
  rts
@cloudl:
  lda #TILE_CLOUDL
  rts
@cloudr:
  lda #TILE_CLOUDR
  rts
@star:
  lda #TILE_SKYSTAR
  rts

; ---------------------------------------------------------------------------
; Scene: COLORS
; ---------------------------------------------------------------------------
enter_colors:
  jsr rendering_off
  jsr hide_sprites
  jsr silence_apu
  jsr clear_vram
  lda #<pal_colors
  sta ptr
  lda #>pal_colors
  sta ptr+1
  jsr load_palette
  lda #<attr_colors
  sta ptr
  lda #>attr_colors
  sta ptr+1
  lda #$20
  sta tmp0
  jsr load_attr
  lda #<scr_colors
  sta ptr
  lda #>scr_colors
  sta ptr+1
  jsr draw_script

  ; Nine swatch blocks, seven tiles wide and two tall, plus the dollar sign in
  ; front of each label. All static; only the palette and the digits move.
  ldx #0
@block:
  lda colors_swatch_hi,x
  sta tmp0
  lda colors_swatch_lo,x
  sta tmp1
  lda colors_swatch_tile,x
  sta tmp2
  ldy #0                    ; which of the block's two rows
@rowpair:
  lda tmp0
  sta PPUADDR
  lda tmp1
  sta PPUADDR
  sty tmp3
  ldy #0
@run:
  lda tmp2
  sta PPUDATA
  iny
  cpy #7
  bne @run
  ldy tmp3
  lda tmp1
  clc
  adc #32
  sta tmp1
  lda tmp0
  adc #0
  sta tmp0
  iny
  cpy #2
  bne @rowpair
  inx
  cpx #9
  bne @block

  ldx #0
@dollar:
  lda colors_label_hi,x
  sta PPUADDR
  lda colors_label_lo,x
  sec
  sbc #1                    ; the dollar sits one tile left of the digits
  sta PPUADDR
  lda #$04                  ; '$'
  sta PPUDATA
  inx
  cpx #9
  bne @dollar

  jsr rendering_on
  rts

tick_colors:
  lda pad1_new
  and #BTN_RIGHT
  beq @noright
  inc color_base
  jsr ui_blip
@noright:
  lda pad1_new
  and #BTN_LEFT
  beq @noleft
  dec color_base
  jsr ui_blip
@noleft:
  lda pad1_new
  and #BTN_DOWN
  beq @nodown
  lda color_base
  clc
  adc #9
  sta color_base
  jsr ui_blip
@nodown:
  lda pad1_new
  and #BTN_UP
  beq @noup
  lda color_base
  sec
  sbc #9
  sta color_base
  jsr ui_blip
@noup:
  lda color_base
  and #$3F
  sta color_base

  ; Nine palette entries and nine two-digit labels, all through the vblank
  ; queue: 27 writes, comfortably inside one blanking interval.
  ldx #0
@l:
  txa
  clc
  adc color_base
  and #$3F
  sta tmp0                  ; the colour this swatch shows

  lda #$3F
  sta tmp4
  lda colors_pal_lo,x
  sta tmp5
  lda tmp0
  jsr push_patch

  lda colors_label_hi,x
  sta tmp4
  lda colors_label_lo,x
  sta tmp5
  lda tmp0
  jsr push_hex

  inx
  cpx #9
  bne @l
  rts

; ---------------------------------------------------------------------------
; Scene: AUDIO
; ---------------------------------------------------------------------------
AUDIO_ROWS = 4

enter_audio:
  jsr rendering_off
  jsr hide_sprites
  jsr silence_apu
  jsr clear_vram
  lda #<pal_menu
  sta ptr
  lda #>pal_menu
  sta ptr+1
  jsr load_palette
  lda #<attr_plain
  sta ptr
  lda #>attr_plain
  sta ptr+1
  lda #$20
  sta tmp0
  jsr load_attr
  lda #<scr_audio
  sta ptr
  lda #>scr_audio
  sta ptr+1
  jsr draw_script

  lda #0
  sta audio_mute
  sta audio_sel
  sta music_step
  lda #1
  sta music_timer
  lda #%00011111            ; all four tone channels plus DMC
  sta APUSTATUS
  lda #$08
  sta SQ1_SWEEP
  sta SQ2_SWEEP
  lda #$FF
  sta TRI_LIN
  jsr rendering_on
  rts

tick_audio:
  lda pad1_new
  and #BTN_DOWN
  beq @noup
  inc audio_sel
  lda audio_sel
  cmp #AUDIO_ROWS
  bcc @noup
  lda #0
  sta audio_sel
@noup:
  lda pad1_new
  and #BTN_UP
  beq @nodown
  dec audio_sel
  bpl @nodown
  lda #AUDIO_ROWS-1
  sta audio_sel
@nodown:

  lda pad1_new
  and #BTN_A
  beq @nomute
  ldx audio_sel
  lda bit_for,x
  eor audio_mute
  sta audio_mute
@nomute:

  lda pad1_new
  and #BTN_START
  beq @nodmc
  jsr fire_dmc
@nodmc:

  jsr music_tick

  ; Cursor sprite beside the selected channel row.
  lda audio_sel
  asl a
  asl a
  asl a
  asl a                     ; row index * 16
  clc
  adc #8*8-1
  sta oam+0
  lda #SPR_CURSOR
  sta oam+1
  lda #0
  sta oam+2
  lda #32
  sta oam+3

  ; ON / OFF beside each of the four tone channels.
  ldx #0
@state:
  lda audio_state_hi,x
  sta tmp0
  lda audio_state_lo,x
  sta tmp1
  lda bit_for,x
  and audio_mute
  beq @on
  lda #<text_off
  sta ptr
  lda #>text_off
  sta ptr+1
  jmp @emit
@on:
  lda #<text_on
  sta ptr
  lda #>text_on
  sta ptr+1
@emit:
  ldy #0
@emit_l:
  lda tmp0
  sta tmp4
  tya
  clc
  adc tmp1
  sta tmp5
  lda (ptr),y
  jsr push_patch
  iny
  cpy #3
  bne @emit_l
  inx
  cpx #AUDIO_ROWS
  bne @state
  rts

bit_for:
  .byte 1, 2, 4, 8

; Advance the pattern and push the notes that changed.
music_tick:
  dec music_timer
  beq @advance
  jmp @sustain              ; the tail of this routine is out of branch range
@advance:
  lda tempo
  sta music_timer

  ldy music_step

  lda #1
  and audio_mute
  bne @p1_off
  lda mus_pulse1,y
  cmp #HOLD
  beq @p2
  cmp #REST
  beq @p1_off
  tax
  lda note_lo,x
  sta SQ1_LO
  lda note_hi,x
  ora #%00011000            ; length counter load, high three bits of the timer
  sta SQ1_HI
  lda #%10111000            ; duty 50%, halt length, constant volume 8
  sta SQ1_VOL
  jmp @p2
@p1_off:
  lda #%10110000
  sta SQ1_VOL

@p2:
  lda #2
  and audio_mute
  bne @p2_off
  lda mus_pulse2,y
  cmp #HOLD
  beq @tri
  cmp #REST
  beq @p2_off
  tax
  lda note_lo,x
  sta SQ2_LO
  lda note_hi,x
  ora #%00011000
  sta SQ2_HI
  lda #%01110110            ; duty 25%, halt length, constant volume 6
  sta SQ2_VOL
  jmp @tri
@p2_off:
  lda #%01110000
  sta SQ2_VOL

@tri:
  lda #4
  and audio_mute
  bne @tri_off
  lda mus_tri,y
  cmp #HOLD
  beq @noise
  cmp #REST
  beq @tri_off
  tax
  lda #$FF
  sta TRI_LIN
  lda note_lo,x
  sta TRI_LO
  lda note_hi,x
  ora #%00011000
  sta TRI_HI
  jmp @noise
@tri_off:
  lda #0
  sta TRI_LIN

@noise:
  lda #8
  and audio_mute
  bne @step
  lda mus_noise,y
  cmp #REST
  beq @step
  sta NOISE_LO
  lda #%00011000
  sta NOISE_HI
  lda #12
  sta noise_vol

@step:
  inc music_step
  lda music_step
  cmp #MUS_STEPS
  bcc @sustain
  lda #0
  sta music_step

@sustain:
  ; Software decay on the noise channel; the tone channels hold their level.
  lda #8
  and audio_mute
  bne @hush
  lda noise_vol
  beq @hush
  dec noise_vol
  lda noise_vol
  ora #%00110000            ; constant volume, halt length
  sta NOISE_VOL
  rts
@hush:
  lda #%00110000
  sta NOISE_VOL
  rts

; Restart the DPCM sample. Clearing the DMC enable bit first is what makes a
; second press retrigger rather than be ignored.
fire_dmc:
  lda #%00001111
  sta APUSTATUS
  lda #$0F                  ; fastest rate, no loop, no IRQ
  sta DMC_FREQ
  lda #(dmc_sample-$C000)/64
  sta DMC_ADDR
  lda #DMC_SAMPLE_LEN/16
  sta DMC_LEN
  lda #%00011111
  sta APUSTATUS
  rts

; ---------------------------------------------------------------------------
; Scene: INPUT
; ---------------------------------------------------------------------------
enter_input:
  jsr rendering_off
  jsr hide_sprites
  jsr silence_apu
  jsr clear_vram
  lda #<pal_menu
  sta ptr
  lda #>pal_menu
  sta ptr+1
  jsr load_palette
  lda #<attr_plain
  sta ptr
  lda #>attr_plain
  sta ptr+1
  lda #$20
  sta tmp0
  jsr load_attr
  lda #<scr_input
  sta ptr
  lda #>scr_input
  sta ptr+1
  jsr draw_script
  jsr rendering_on
  rts

tick_input:
  ldx #0
@p1:
  lda input_cell_hi,x
  sta tmp4
  lda input_cell_lo,x
  sta tmp5
  lda input_cell_bit,x
  and pad1
  beq @p1_off
  lda #TILE_SOLID1
  jmp @p1_put
@p1_off:
  lda #TILE_SPACE
@p1_put:
  jsr push_patch
  inx
  cpx #8
  bne @p1

  ldx #0
@p2:
  lda input_cell2_hi,x
  sta tmp4
  lda input_cell2_lo,x
  sta tmp5
  lda input_cell_bit,x
  and pad2
  beq @p2_off
  lda #TILE_SOLID1
  jmp @p2_put
@p2_off:
  lda #TILE_SPACE
@p2_put:
  jsr push_patch
  inx
  cpx #8
  bne @p2

  ; Raw port bytes, then the frame counter, all in hex.
  lda #>(NT0+23*32+10)
  sta tmp4
  lda #<(NT0+23*32+10)
  sta tmp5
  lda pad1
  jsr push_hex

  lda #>(NT0+23*32+21)
  sta tmp4
  lda #<(NT0+23*32+21)
  sta tmp5
  lda pad2
  jsr push_hex

  lda #>(NT0+25*32+11)
  sta tmp4
  lda #<(NT0+25*32+11)
  sta tmp5
  lda frame_hi
  jsr push_hex
  inc tmp5
  lda frame_lo
  jsr push_hex
  rts

; ---------------------------------------------------------------------------
; Data, tables and art
; ---------------------------------------------------------------------------
.include "data.s"
.include "tables.s"

; The DPCM sample. DMC fetches start at $C000 plus a multiple of 64, so this is
; placed by hand rather than left where the assembler happened to be. The
; content is a four-stage rising tone: each byte is a run of delta bits, and
; the shorter the run, the higher the pitch.
.org $F000
dmc_sample:
  .res 64, $00
  .res 64, $FF
  .res 128, $0F
  .res 128, $33
  .res 128, $55
DMC_SAMPLE_LEN = 512
.assert (dmc_sample & 63) == 0, "the DPCM sample must start on a 64-byte boundary"
.assert dmc_sample >= $C000, "the DPCM sample must live in the $C000-$FFFF window"

.include "chr.s"

; The character ROM decides where each tile really landed. These assertions are
; what stops a tile being inserted into the middle of the font and silently
; shifting every graphic constant above.
;
; Labels in the character ROM count tiles from the start of the whole 8 KB, so
; a sprite label is 256 higher than the index OAM wants once PPUCTRL points
; sprites at the second pattern table. PT1 is spelled out rather than folded
; into the numbers, so the offset stays visible.
PT1 = 256
.assert TILE_SOLID1  == chr_solid1,  "TILE_SOLID1 no longer matches the art"
.assert TILE_SOLID2  == chr_solid2,  "TILE_SOLID2 no longer matches the art"
.assert TILE_SOLID3  == chr_solid3,  "TILE_SOLID3 no longer matches the art"
.assert TILE_STAR    == chr_star,    "TILE_STAR no longer matches the art"
.assert TILE_BRICK   == chr_brick,   "TILE_BRICK no longer matches the art"
.assert TILE_GROUND  == chr_ground,  "TILE_GROUND no longer matches the art"
.assert TILE_CLOUDL  == chr_cloudl,  "TILE_CLOUDL no longer matches the art"
.assert TILE_CLOUDR  == chr_cloudr,  "TILE_CLOUDR no longer matches the art"
.assert TILE_FRAMEH  == chr_frameh,  "TILE_FRAMEH no longer matches the art"
.assert TILE_SKYSTAR == chr_skystar, "TILE_SKYSTAR no longer matches the art"
.assert PT1 + SPR_BLOCK    == spr_block,   "SPR_BLOCK no longer matches the art"
.assert PT1 + SPR_ORB      == spr_orb,     "SPR_ORB no longer matches the art"
.assert PT1 + SPR_GEM      == spr_gem,     "SPR_GEM no longer matches the art"
.assert PT1 + SPR_CURSOR   == spr_cursor,  "SPR_CURSOR no longer matches the art"
.assert PT1 + SPR_S0LINE   == spr_s0line,  "SPR_S0LINE no longer matches the art"
; The font is addressed as ASCII minus $20, which only works if it starts at
; tile 0 and nothing was inserted before the letters.
.assert chr_glyph_A == $21, "the font is no longer ASCII minus $20"
.assert chr_digit_0 == $10, "the font is no longer ASCII minus $20"

; ---------------------------------------------------------------------------
; Vectors
; ---------------------------------------------------------------------------
.org $FFFA
  .word nmi, reset, irq
