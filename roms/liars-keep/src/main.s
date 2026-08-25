; SPDX-License-Identifier: CC0-1.0
;
; LIAR'S KEEP v1.0
; Written by Prodigy75000. Dedicated to the public domain under CC0 1.0.
;
; A one-screen platformer for the NES, in which some of the floor is not the
; floor. The blocks that hold you up and the blocks that do not are drawn from
; the same tiles out of the same palette, so there is nothing to spot: you find
; out by standing on one, and after that you know, and knowing is the game.
;
; The controls are deliberately generous. There is coyote time on every ledge
; and a jump buffer on every landing, because a game that lies to you about the
; level has no business also lying to you about whether you pressed the button.
; The room is the enemy. The pad is not.
;
; Mapper 0 (NROM), 32 KB PRG, 8 KB CHR, vertical mirroring.

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
TRI_LIN   = $4008
NOISE_VOL = $400C
NOISE_LO  = $400E
NOISE_HI  = $400F
DMC_FREQ  = $4010
APUSTATUS = $4015
APUFRAME  = $4017

NT0 = $2000

CTRL_BASE = %10001000     ; NMI on, 8x8 sprites, backgrounds from table 0
MASK_ON   = %00011110     ; background and sprites on, including the left column

BTN_A      = $80
BTN_B      = $40
BTN_SELECT = $20
BTN_START  = $10
BTN_UP     = $08
BTN_DOWN   = $04
BTN_LEFT   = $02
BTN_RIGHT  = $01

; ---------------------------------------------------------------------------
; Geometry
;
; The screen is 32x30 tiles. The top four rows are the HUD, and the 26 below
; them are thirteen rows of 16x16 blocks. Sixteen wide by thirteen tall is not
; an aesthetic choice: sixteen columns makes a grid index simply row*16+col,
; which is a shift and an OR rather than a multiply, and starting the field at
; pixel 32 puts every block exactly on top of one attribute-table quadrant, so
; every block gets to pick its own palette.
; ---------------------------------------------------------------------------
PLAY_TOP   = 32
GRID_W     = 16
GRID_H     = 13
ROOM_BYTES = GRID_W * GRID_H
NAME_LEN   = 14

; The hero's collision box, as offsets inside his 16x16 sprite. Narrower than
; he looks, which is the traditional lie told in the player's favour.
HB_L = 2
HB_R = 13
HB_T = 2
HB_B = 15

; ---------------------------------------------------------------------------
; Physics. Positions and velocities are 16-bit: the high byte is whole pixels
; and the low byte is 1/256ths, so the numbers below read as 256ths of a pixel
; per frame.
;
; Jumping 1024 with gravity 56 puts the top of the arc 36 pixels up, a little
; over two blocks, and keeps him in the air about 37 frames. At a walk of 1.5
; pixels a frame that is 55 pixels of reach, which is what makes a two-block
; gap crossable and a four-block gap not.
; ---------------------------------------------------------------------------
GRAV      = 56
VY_MAX    = 1024
JUMP_V    = 1024
JUMP_CUT  = 256
WALK_ACC  = 56
AIR_ACC   = 40
FRICTION  = 72
WALK_MAX  = 384
DEATH_POP = 900

; Negative constants are spelled as their two's complement so the expression
; evaluator never has to be trusted with the sign of a shifted value.
NJUMP_LO  = <(65536 - JUMP_V)
NJUMP_HI  = >(65536 - JUMP_V)
NPOP_LO   = <(65536 - DEATH_POP)
NPOP_HI   = >(65536 - DEATH_POP)
NWMAX_LO  = <(65536 - WALK_MAX)
NWMAX_HI  = >(65536 - WALK_MAX)

COYOTE_FRAMES  = 6      ; how long a ledge keeps letting you jump after it ends
JUMPBUF_FRAMES = 6      ; how early a jump press still counts on landing
CRUMBLE_FRAMES = 22     ; how long a cracked block puts up with you
DEATH_FRAMES   = 46
WIN_FRAMES     = 70
DART_PERIOD    = 96
DART_SPEED     = 2
CRUSH_ARM      = 14     ; the pause between a crusher noticing you and falling
CRUSH_SIT      = 26

; What to write to a sweep register to mean "leave this channel's pitch alone".
;
; The value matters far more than it looks. The usual choice is $08: negate set,
; shift zero. On pulse 1 that asks for a target period of `period - period - 1`,
; because that channel's adder is wired to add the ones' complement, so the
; answer is -1. Nothing about -1 is over $7FF and it must not mute anything, but
; an emulator that works the target out in unsigned arithmetic sees $FFFF,
; applies the "a target over $7FF mutes the channel" rule, and takes pulse 1 off
; the air permanently. That is not a hypothetical: it is why the first two
; releases of this cartridge had no jump sound.
;
; $7F asks for `period - (period >> 7) - 1` instead, which is positive and
; comfortably in range for every period a tune will ever use. It leaves nothing
; for anybody to get wrong, which is what a value written into a ROM that is
; meant to run anywhere ought to do.
SWEEP_OFF = $7F

ENT_MAX   = 8
DART_MAX  = 6
PATCH_MAX = 32
OAM_LIMIT = 240

M_TITLE = 0
M_PLAY  = 1
M_END   = 2

HS_ALIVE = 0
HS_DEAD  = 1
HS_WIN   = 2

; ---------------------------------------------------------------------------
; Zero page
; ---------------------------------------------------------------------------
nmi_ready    = $00
frame_lo     = $01
frame_hi     = $02
pad1         = $03
pad1_new     = $04
pad1_prev    = $05
tmp0         = $06
tmp1         = $07
tmp2         = $08
tmp3         = $09
tmp4         = $0A
tmp5         = $0B
tmp6         = $0C
tmp7         = $0D
tmpa         = $0E
tmpb         = $0F
ptr          = $10
ptr2         = $12
room_ptr     = $14
mode         = $16
ctrl_shadow  = $17
mask_shadow  = $18
patch_count  = $19
patch_tmp    = $1A
oam_ptr      = $1B
room_idx     = $1C

hero_xl      = $1D
hero_xh      = $1E
hero_yl      = $1F
hero_yh      = $20
hero_vxl     = $21
hero_vxh     = $22
hero_vyl     = $23
hero_vyh     = $24
hero_face    = $25
on_ground    = $26
coyote       = $27
jump_buf     = $28
hero_state   = $29
state_timer  = $2A
newx_l       = $2B
newx_h       = $2C
newy_l       = $2D
newy_h       = $2E

spawn_bx     = $2F
spawn_by     = $30
ent_count    = $31
crumb_idx    = $32
crumb_timer  = $33
hud_dirty    = $34

sfx_ch       = $35
sfx_dur      = $36
sfx_plo      = $37
sfx_phi      = $38
sfx_dp       = $39
sfx_vol      = $3A
sfx_duty     = $3B
sfx_last_hi  = $3C
sfx_id       = $3D
sv_x         = $3E        ; a loop index that has to survive a subroutine
sv_y         = $3F

; ---------------------------------------------------------------------------
; RAM
; ---------------------------------------------------------------------------
oam        = $0200

; The room, unpacked, one byte of block id per cell. It has to be in RAM and
; not read straight out of the ROM because a cracked block that has given way
; is a change to the room, and the room has to forget that change the moment
; you die, which it does by being loaded again from the cartridge.
grid       = $0300

attr_buf   = $0400
patch_hi   = $0440
patch_lo   = $0460
patch_val  = $0480

ent_type   = $04A0
ent_x      = $04A8
ent_y      = $04B0
ent_p0     = $04B8
ent_p1     = $04C0
ent_p2     = $04C8
ent_p3     = $04D0

dart_x     = $04D8
dart_y     = $04E0
dart_dx    = $04E8

deaths     = $04F0        ; four digits, most significant first
souls      = $04F4

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
  stx PPUCTRL
  stx PPUMASK
  stx DMC_FREQ
  bit PPUSTATUS

@vblank1:
  bit PPUSTATUS
  bpl @vblank1

  ; Clear RAM. Page 2 is the OAM shadow, so it gets $FF: a Y of $FF parks a
  ; sprite below the bottom of the screen.
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
  jsr enter_title

; ---------------------------------------------------------------------------
; Main loop. One pass per displayed frame.
; ---------------------------------------------------------------------------
main:
  jsr wait_frame
  jsr read_pads

  ; The queue is emptied here rather than by the NMI, so the NMI can only ever
  ; flush entries that were written in full.
  lda #0
  sta patch_count

  jsr sfx_tick
  jsr flicker

  lda mode
  cmp #M_PLAY
  beq @play
  cmp #M_END
  beq @end
  jsr tick_title
  jmp main
@play:
  jsr tick_play
  jmp main
@end:
  jsr tick_end
  jmp main

; ---------------------------------------------------------------------------
; NMI. Owns the sprite DMA, the queued single-tile writes, and the scroll.
; Nothing outside it touches the PPU while rendering is on.
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

  lda ctrl_shadow
  sta PPUCTRL
  lda mask_shadow
  sta PPUMASK
  lda #0
  sta PPUSCROLL
  sta PPUSCROLL

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

flush_patches:
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
; Frame sync and the controller
; ---------------------------------------------------------------------------
wait_frame:
  lda #0
  sta nmi_ready
@w:
  lda nmi_ready
  beq @w
  rts

read_pads:
  lda pad1
  sta pad1_prev
@again:
  jsr strobe_read
  lda tmp0
  sta tmp2
  jsr strobe_read
  lda tmp0
  cmp tmp2
  bne @again
  sta pad1
  eor pad1_prev
  and pad1
  sta pad1_new
  rts

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
  ; Drop anything queued for the screen that is about to be painted over.
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

load_palette:
  bit PPUSTATUS
  lda #$3F
  sta PPUADDR
  lda #$00
  sta PPUADDR
  ldy #0
@l:
  lda palette,y
  sta PPUDATA
  iny
  cpy #32
  bne @l
  ; Park the address outside palette space; left inside it with rendering off,
  ; the backdrop shows whichever entry it points at.
  lda #$20
  sta PPUADDR
  lda #$00
  sta PPUADDR
  rts

; ptr -> a screen script: high byte, low byte, tiles, $FF; $00 ends it.
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
  lda #0
  sta oam_ptr
  rts

; A = tile value, tmp4/tmp5 = target address. X and Y survive: every caller is
; a loop that indexes with one of them.
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

; tmpb = how far to step the queued write address along.
adv_addr:
  lda tmp5
  clc
  adc tmpb
  sta tmp5
  bcc @d
  inc tmp4
@d:
  rts

; ---------------------------------------------------------------------------
; Audio
; ---------------------------------------------------------------------------
silence_apu:
  lda #0
  sta APUSTATUS
  sta SQ1_VOL
  sta SQ2_VOL
  sta TRI_LIN
  sta NOISE_VOL
  lda #SWEEP_OFF
  sta SQ1_SWEEP
  sta SQ2_SWEEP
  lda #%00001111
  sta APUSTATUS
  lda #0
  sta sfx_dur
  rts

; Called from inside entity loops, so it gives X back. So do inc_deaths and
; inc_souls below, and hero_die, which calls both: an entity routine that lost
; its own index to a sound effect is a very quiet kind of bug.
sfx_start:
  sta sfx_id
  txa
  pha
  ldx sfx_id
  lda sfx_tbl_dur,x
  sta sfx_dur
  lda sfx_tbl_ch,x
  sta sfx_ch
  lda sfx_tbl_plo,x
  sta sfx_plo
  lda sfx_tbl_phi,x
  sta sfx_phi
  lda sfx_tbl_dp,x
  sta sfx_dp
  lda sfx_tbl_vol,x
  sta sfx_vol
  lda sfx_tbl_duty,x
  sta sfx_duty
  lda #$FF
  sta sfx_last_hi           ; forces the period high byte out on the first tick
  pla
  tax
  rts

sfx_tick:
  lda sfx_dur
  bne @on
  ; Silence is written as volume zero and not as a stopped channel: a channel
  ; left holding a level is inaudible until the stream stops and then is not.
  lda #%00110000
  sta SQ1_VOL
  sta NOISE_VOL
  rts
@on:
  dec sfx_dur

  ; Slide the period. Adding to it slides the pitch down.
  lda sfx_dp
  bmi @neg
  clc
  adc sfx_plo
  sta sfx_plo
  bcc @vol
  inc sfx_phi
  jmp @vol
@neg:
  clc
  adc sfx_plo
  sta sfx_plo
  bcs @vol
  dec sfx_phi

@vol:
  ; Fade the tail out, so an effect ends instead of being cut off.
  lda sfx_dur
  cmp #15
  bcs @full
  cmp sfx_vol
  bcc @have
@full:
  lda sfx_vol
@have:
  sta tmpa

  lda sfx_ch
  bne @noise
  lda sfx_duty
  ora #%00110000
  ora tmpa
  sta SQ1_VOL
  lda #SWEEP_OFF
  sta SQ1_SWEEP
  lda sfx_plo
  sta SQ1_LO
  lda sfx_phi
  cmp sfx_last_hi
  beq @done
  sta sfx_last_hi
  and #$07
  sta SQ1_HI
@done:
  rts
@noise:
  lda #%00110000
  ora tmpa
  sta NOISE_VOL
  lda sfx_plo
  and #$0F
  sta NOISE_LO
  lda #$08
  sta NOISE_HI
  rts

; One palette write a frame animates every flame and every pool of lava in the
; room at once, which is a great deal cheaper than redrawing their tiles.
flicker:
  lda mode
  cmp #M_PLAY
  bne @done
  lda #>HOT_ENTRY
  sta tmp4
  lda #<HOT_ENTRY
  sta tmp5
  lda frame_lo
  and #$08
  beq @a
  lda #FLICKER_B
  jsr push_patch
  rts
@a:
  lda #FLICKER_A
  jsr push_patch
@done:
  rts

; ---------------------------------------------------------------------------
; Title screen
; ---------------------------------------------------------------------------
enter_title:
  jsr rendering_off
  jsr hide_sprites
  jsr clear_vram
  jsr load_palette
  lda #<title_script
  sta ptr
  lda #>title_script
  sta ptr+1
  jsr draw_script
  lda #M_TITLE
  sta mode
  jsr rendering_on
  rts

tick_title:
  lda pad1_new
  and #BTN_START
  beq @done
  lda #SFX_UI
  jsr sfx_start
  ; A new run, so the tally starts again.
  ldx #3
  lda #0
@z:
  sta deaths,x
  sta souls,x
  dex
  bpl @z
  lda #0
  sta room_idx
  jsr enter_play
@done:
  rts

; ---------------------------------------------------------------------------
; End screen
; ---------------------------------------------------------------------------
enter_end:
  jsr rendering_off
  jsr hide_sprites
  jsr clear_vram
  jsr load_palette
  lda #<end_script
  sta ptr
  lda #>end_script
  sta ptr+1
  jsr draw_script

  lda #>END_DEATHS
  sta PPUADDR
  lda #<END_DEATHS
  sta PPUADDR
  ldx #0
@d:
  lda deaths,x
  clc
  adc #$10                  ; the digit glyphs start at tile $10
  sta PPUDATA
  inx
  cpx #4
  bne @d

  lda #>END_SOULS
  sta PPUADDR
  lda #<END_SOULS
  sta PPUADDR
  ldx #0
@s:
  lda souls,x
  clc
  adc #$10
  sta PPUDATA
  inx
  cpx #4
  bne @s

  lda #M_END
  sta mode
  jsr rendering_on
  rts

tick_end:
  lda pad1_new
  and #BTN_START
  beq @done
  jsr enter_title
@done:
  rts

; ---------------------------------------------------------------------------
; Loading a room
;
; Two passes over the same typed-out characters. The first turns every one of
; them into a block id and notices where the S is; the second spawns the
; entities, and has to come second because a saw works out its own patrol by
; looking along the row it is standing in, which only answers correctly once
; the whole row exists.
; ---------------------------------------------------------------------------
load_room:
  ldx room_idx
  lda room_lo,x
  sta room_ptr
  lda room_hi,x
  sta room_ptr+1

  lda #1
  sta spawn_bx              ; a room with no S starts you in the corner
  lda #9
  sta spawn_by

  ldy #NAME_LEN
  ldx #0
@l:
  lda (room_ptr),y
  sty tmp0
  cmp #CH_SPAWN
  bne @nospawn
  txa
  and #$0F
  sta spawn_bx
  txa
  lsr a
  lsr a
  lsr a
  lsr a
  sta spawn_by
  lda (room_ptr),y
@nospawn:
  tay
  lda charmap,y
  ldy tmp0
  sta grid,x
  iny
  inx
  cpx #ROOM_BYTES
  bne @l

  lda #0
  sta ent_count
  ldy #NAME_LEN
  ldx #0
@e:
  lda (room_ptr),y
  sty tmp0
  tay
  lda charent,y
  sta tmp1
  ldy tmp0
  ; The test is on tmp1 and not on what is in A, because putting Y back set the
  ; flags from Y. Every cell in the room then looked like it wanted an entity,
  ; the table filled up with eight of nothing, and the one flask the room
  ; actually asked for never got a slot.
  lda tmp1
  beq @enext
  jsr spawn_entity
@enext:
  iny
  inx
  cpx #ROOM_BYTES
  bne @e

  ldx #0
  lda #0
@dz:
  sta dart_dx,x
  inx
  cpx #DART_MAX
  bne @dz
  rts

; A = entity type, X = grid index. Both X and Y survive.
spawn_entity:
  sta tmpa
  stx tmpb
  tya
  pha

  ldy ent_count
  cpy #ENT_MAX
  bcc @room
  jmp @full
@room:

  lda tmpa
  sta ent_type,y
  lda tmpb
  and #$0F
  asl a
  asl a
  asl a
  asl a
  sta ent_x,y
  lda tmpb
  and #$F0                  ; (index >> 4) * 16 is the same byte, unshifted
  clc
  adc #PLAY_TOP
  sta ent_y,y
  lda #0
  sta ent_p0,y
  sta ent_p1,y
  sta ent_p2,y
  sta ent_p3,y

  lda tmpa
  cmp #E_SAW
  beq @saw
  cmp #E_CRUSHER
  beq @crusher
  cmp #E_DARTR
  beq @shooter
  cmp #E_DARTL
  beq @shooter
  jmp @keep

  ; A saw takes its patrol from the room instead of from a parameter: it looks
  ; left and right along its own row until it finds something solid and paddles
  ; between the two. Draw it a longer corridor and it patrols a longer
  ; corridor. This is safe to walk off the end of because every room is walled.
@saw:
  ldx tmpb
@sl:
  dex
  lda grid,x
  stx tmp2
  tax
  lda blk_flags,x
  ldx tmp2
  and #BF_SOLID
  beq @sl
  inx
  txa
  and #$0F
  asl a
  asl a
  asl a
  asl a
  sta ent_p1,y
  ldx tmpb
@sr:
  inx
  lda grid,x
  stx tmp2
  tax
  lda blk_flags,x
  ldx tmp2
  and #BF_SOLID
  beq @sr
  dex
  txa
  and #$0F
  asl a
  asl a
  asl a
  asl a
  sta ent_p2,y
  ; Which way it sets off comes from which column it was drawn in, so a room
  ; with several of them gets saws that cross rather than saws in convoy.
  lda tmpb
  and #1
  beq @sawright
  lda #$FF
  sta ent_p3,y
  jmp @keep
@sawright:
  lda #1
  sta ent_p3,y
  jmp @keep

@crusher:
  lda ent_y,y
  sta ent_p1,y              ; where it hangs, and where it goes back to
  jmp @keep

  ; The firing phase comes from where the shooter sits, so a wall of them never
  ; falls into one rhythm and no two rooms of them feel the same.
@shooter:
  lda tmpb
  and #$3F
  clc
  adc #16
  sta ent_p0,y

@keep:
  inc ent_count
@full:
  pla
  tay
  ldx tmpb
  rts

; ---------------------------------------------------------------------------
; Painting a room. Rendering is off, so this writes PPUDATA straight through
; and lets the address auto-increment along each row.
; ---------------------------------------------------------------------------
paint_room:
  jsr clear_vram
  lda #<hud_script
  sta ptr
  lda #>hud_script
  sta ptr+1
  jsr draw_script

  lda #>HUD_NAME
  sta PPUADDR
  lda #<HUD_NAME
  sta PPUADDR
  ldy #0
@nm:
  lda (room_ptr),y
  sta PPUDATA
  iny
  cpy #NAME_LEN
  bne @nm

  ldy #0
@row:
  sty tmp6                  ; block row
  tya
  asl a
  asl a
  asl a
  asl a
  sta tmp7                  ; grid index of the start of that row

  lda row_hi,y
  sta PPUADDR
  lda row_lo,y
  sta PPUADDR
  ldx #0
@top:
  txa
  clc
  adc tmp7
  tay
  lda grid,y
  tay
  lda blk_tl,y
  sta PPUDATA
  lda blk_tr,y
  sta PPUDATA
  inx
  cpx #GRID_W
  bne @top

  ldy tmp6
  lda row_hi,y
  sta tmp4
  lda row_lo,y
  clc
  adc #32
  sta tmp5
  bcc @nc
  inc tmp4
@nc:
  lda tmp4
  sta PPUADDR
  lda tmp5
  sta PPUADDR
  ldx #0
@bot:
  txa
  clc
  adc tmp7
  tay
  lda grid,y
  tay
  lda blk_bl,y
  sta PPUDATA
  lda blk_br,y
  sta PPUDATA
  inx
  cpx #GRID_W
  bne @bot

  ldy tmp6
  iny
  cpy #GRID_H
  bne @row
  jmp paint_attr

; Every block sits on exactly one attribute quadrant, so this is an assignment
; rather than a merge: no two blocks ever compete for the same two bits.
paint_attr:
  lda #0
  ldx #63
@z:
  sta attr_buf,x
  dex
  bpl @z

  ldx #0
@l:
  lda grid,x
  tay
  lda blk_pal,y
  beq @next                 ; palette 0 is what the buffer already says
  sta tmpa

  txa
  lsr a
  lsr a
  lsr a
  lsr a
  sta tmpb                  ; block row
  clc
  adc #2                    ; the HUD occupies the first attribute row
  lsr a
  asl a
  asl a
  asl a
  sta tmp3
  txa
  and #$0F
  lsr a
  clc
  adc tmp3
  sta tmp3                  ; which attribute byte

  lda tmpb
  and #1
  asl a
  sta tmp4
  txa
  and #1
  clc
  adc tmp4
  asl a
  tay                       ; how far up the byte to shift it
  lda tmpa
@sh:
  cpy #0
  beq @put
  asl a
  dey
  jmp @sh
@put:
  ldy tmp3
  ora attr_buf,y
  sta attr_buf,y
@next:
  inx
  cpx #ROOM_BYTES
  bne @l

  lda #$23
  sta PPUADDR
  lda #$C0
  sta PPUADDR
  ldx #0
@w:
  lda attr_buf,x
  sta PPUDATA
  inx
  cpx #64
  bne @w
  rts

; Redraw one cell of the room through the vertical-blank queue, which is how a
; cracked block that has just given way stops being drawn.
; tmp2 = grid index.
repaint_cell:
  lda tmp2
  lsr a
  lsr a
  lsr a
  lsr a
  tay
  lda row_hi,y
  sta tmp4
  lda tmp2
  and #$0F
  asl a
  sta tmpb
  lda row_lo,y
  sta tmp5
  jsr adv_addr

  ldx tmp2
  lda grid,x
  tax
  lda blk_tl,x
  jsr push_patch
  lda #1
  sta tmpb
  jsr adv_addr
  lda blk_tr,x
  jsr push_patch
  lda #31
  sta tmpb
  jsr adv_addr
  lda blk_bl,x
  jsr push_patch
  lda #1
  sta tmpb
  jsr adv_addr
  lda blk_br,x
  jsr push_patch
  rts

; ---------------------------------------------------------------------------
; Reading the room
;
; tmp0 = pixel x, tmp1 = pixel y. Returns the block id in A and its grid index
; in tmp2, or $FF in tmp2 for a sample that fell outside the room. Above the
; room reads as wall, so a jump into the ceiling stops; below it reads as lava,
; so falling out of the world is a death and not a wrapped-around address.
; ---------------------------------------------------------------------------
block_at:
  lda tmp1
  sec
  sbc #PLAY_TOP
  bcc @above
  lsr a
  lsr a
  lsr a
  lsr a
  cmp #GRID_H
  bcs @below
  asl a
  asl a
  asl a
  asl a
  sta tmp2
  lda tmp0
  lsr a
  lsr a
  lsr a
  lsr a
  ora tmp2
  sta tmp2
  tax
  lda grid,x
  rts
@above:
  ldx #$FF
  stx tmp2
  lda #BLK_STONE
  rts
@below:
  ldx #$FF
  stx tmp2
  lda #BLK_LAVA
  rts

; A = block id in, carry set out if you can stand on it.
is_solid:
  tax
  lda blk_flags,x
  and #BF_SOLID
  beq @no
  sec
  rts
@no:
  clc
  rts

; The flags of every block the hero's box touches, OR-ed together. Four corners
; is enough: the box is 12 by 14 and the blocks are 16 by 16, so it can never
; span a block without touching it at a corner.
hero_flags:
  lda #0
  sta tmpa
  lda hero_xh
  clc
  adc #HB_L
  sta tmp0
  lda hero_yh
  clc
  adc #HB_T
  sta tmp1
  jsr flag_at
  lda hero_xh
  clc
  adc #HB_R
  sta tmp0
  jsr flag_at
  lda hero_yh
  clc
  adc #HB_B
  sta tmp1
  jsr flag_at
  lda hero_xh
  clc
  adc #HB_L
  sta tmp0
  jsr flag_at
  lda tmpa
  rts

flag_at:
  jsr block_at
  tax
  lda blk_flags,x
  ora tmpa
  sta tmpa
  rts

; ---------------------------------------------------------------------------
; Movement
; ---------------------------------------------------------------------------
hero_input:
  lda pad1
  and #BTN_LEFT
  bne @left
  lda pad1
  and #BTN_RIGHT
  bne @right
  jmp hero_friction

@right:
  lda #0
  sta hero_face
  lda on_ground
  bne @rg
  lda #AIR_ACC
  jmp @radd
@rg:
  lda #WALK_ACC
@radd:
  clc
  adc hero_vxl
  sta hero_vxl
  lda hero_vxh
  adc #0
  sta hero_vxh
  jmp clamp_vx

@left:
  lda #1
  sta hero_face
  lda on_ground
  bne @lg
  lda #AIR_ACC
  jmp @lsub
@lg:
  lda #WALK_ACC
@lsub:
  sta tmpa
  lda hero_vxl
  sec
  sbc tmpa
  sta hero_vxl
  lda hero_vxh
  sbc #0
  sta hero_vxh
  jmp clamp_vx

hero_friction:
  lda hero_vxh
  bmi @neg
  ; positive: drag it down towards zero, and stop at zero rather than through it
  lda hero_vxl
  sec
  sbc #FRICTION
  sta hero_vxl
  lda hero_vxh
  sbc #0
  sta hero_vxh
  bpl @done
  lda #0
  sta hero_vxl
  sta hero_vxh
  rts
@neg:
  lda hero_vxl
  clc
  adc #FRICTION
  sta hero_vxl
  lda hero_vxh
  adc #0
  sta hero_vxh
  bmi @done
  lda #0
  sta hero_vxl
  sta hero_vxh
@done:
  rts

clamp_vx:
  lda hero_vxh
  bmi @neg
  cmp #>WALK_MAX
  bcc @done
  bne @setmax
  lda hero_vxl
  cmp #<WALK_MAX
  bcc @done
@setmax:
  lda #<WALK_MAX
  sta hero_vxl
  lda #>WALK_MAX
  sta hero_vxh
  rts
@neg:
  ; vx + WALK_MAX still negative means vx is past -WALK_MAX
  lda hero_vxl
  clc
  adc #<WALK_MAX
  lda hero_vxh
  adc #>WALK_MAX
  bpl @done
  lda #NWMAX_LO
  sta hero_vxl
  lda #NWMAX_HI
  sta hero_vxh
@done:
  rts

hero_jump:
  lda pad1_new
  and #BTN_A
  beq @nopress
  lda #JUMPBUF_FRAMES
  sta jump_buf
@nopress:
  lda jump_buf
  beq @nobuf
  lda coyote
  beq @nobuf
  lda #NJUMP_LO
  sta hero_vyl
  lda #NJUMP_HI
  sta hero_vyh
  lda #0
  sta coyote
  sta jump_buf
  sta on_ground
  lda #SFX_JUMP
  jsr sfx_start
  rts
@nobuf:
  lda jump_buf
  beq @cut
  dec jump_buf
@cut:
  ; Letting go of A part way up cuts the climb short, which is what makes the
  ; jump height something the player chooses rather than something they get.
  lda pad1
  and #BTN_A
  bne @done
  lda hero_vyh
  bpl @done
  cmp #$FF
  beq @done
  lda #$00
  sta hero_vyl
  lda #$FF
  sta hero_vyh
@done:
  rts

hero_gravity:
  clc
  lda hero_vyl
  adc #GRAV
  sta hero_vyl
  lda hero_vyh
  adc #0
  sta hero_vyh
  bmi @done                 ; still rising, no terminal velocity to apply
  cmp #>VY_MAX
  bcc @done
  bne @clamp
  lda hero_vyl
  cmp #<VY_MAX
  bcc @done
@clamp:
  lda #<VY_MAX
  sta hero_vyl
  lda #>VY_MAX
  sta hero_vyh
@done:
  rts

move_x:
  clc
  lda hero_xl
  adc hero_vxl
  sta newx_l
  lda hero_xh
  adc hero_vxh
  sta newx_h

  lda hero_vxh
  bmi @left
  ora hero_vxl
  bne @right
  rts

@right:
  lda newx_h
  clc
  adc #HB_R
  sta tmp0
  lda hero_yh
  clc
  adc #HB_T
  sta tmp1
  jsr block_at
  jsr is_solid
  bcs @hitr
  lda newx_h
  clc
  adc #HB_R
  sta tmp0
  lda hero_yh
  clc
  adc #HB_B
  sta tmp1
  jsr block_at
  jsr is_solid
  bcc @store
@hitr:
  lda tmp0
  and #$F0
  sec
  sbc #(HB_R+1)
  sta newx_h
  lda #0
  sta newx_l
  sta hero_vxl
  sta hero_vxh
  jmp @store

@left:
  lda newx_h
  clc
  adc #HB_L
  sta tmp0
  lda hero_yh
  clc
  adc #HB_T
  sta tmp1
  jsr block_at
  jsr is_solid
  bcs @hitl
  lda newx_h
  clc
  adc #HB_L
  sta tmp0
  lda hero_yh
  clc
  adc #HB_B
  sta tmp1
  jsr block_at
  jsr is_solid
  bcc @store
@hitl:
  lda tmp0
  and #$F0
  clc
  adc #16
  sec
  sbc #HB_L
  sta newx_h
  lda #0
  sta newx_l
  sta hero_vxl
  sta hero_vxh

@store:
  lda newx_l
  sta hero_xl
  lda newx_h
  sta hero_xh
  rts

move_y:
  clc
  lda hero_yl
  adc hero_vyl
  sta newy_l
  lda hero_yh
  adc hero_vyh
  sta newy_h

  lda #0
  sta on_ground

  lda hero_vyh
  bmi @up
  ora hero_vyl
  bne @down
  jmp @store

@down:
  lda hero_xh
  clc
  adc #HB_L
  sta tmp0
  lda newy_h
  clc
  adc #HB_B
  sta tmp1
  jsr block_at
  jsr is_solid
  bcs @land
  lda hero_xh
  clc
  adc #HB_R
  sta tmp0
  lda newy_h
  clc
  adc #HB_B
  sta tmp1
  jsr block_at
  jsr is_solid
  bcc @store
@land:
  jsr block_top
  sec
  sbc #(HB_B+1)
  sta newy_h
  lda #0
  sta newy_l
  sta hero_vyl
  sta hero_vyh
  lda #1
  sta on_ground
  jmp @store

@up:
  lda hero_xh
  clc
  adc #HB_L
  sta tmp0
  lda newy_h
  clc
  adc #HB_T
  sta tmp1
  jsr block_at
  jsr is_solid
  bcs @bonk
  lda hero_xh
  clc
  adc #HB_R
  sta tmp0
  lda newy_h
  clc
  adc #HB_T
  sta tmp1
  jsr block_at
  jsr is_solid
  bcc @store
@bonk:
  jsr block_top
  clc
  adc #16
  sec
  sbc #HB_T
  sta newy_h
  lda #0
  sta newy_l
  sta hero_vyl
  sta hero_vyh

@store:
  lda newy_l
  sta hero_yl
  lda newy_h
  sta hero_yh
  rts

; Is he standing on something, asked plainly, rather than inferred from having
; collided with it this frame.
;
; Gravity only moves him a fifth of a pixel a frame while he is resting, so on
; three frames in four he does not actually re-enter the block underneath and a
; flag set by the collision alone blinks off. That flicker is visible (the walk
; animation stutters into the jump pose) and it is worse than visible: coyote
; time restarts on every fourth frame, and how forgiving a ledge is stops being
; a number anybody chose.
ground_probe:
  lda hero_vyh
  bmi @no                   ; on the way up, so certainly not standing on it
  lda hero_xh
  clc
  adc #HB_L
  sta tmp0
  lda hero_yh
  clc
  adc #(HB_B+1)
  sta tmp1
  jsr block_at
  jsr is_solid
  bcs @yes
  lda hero_xh
  clc
  adc #HB_R
  sta tmp0
  lda hero_yh
  clc
  adc #(HB_B+1)
  sta tmp1
  jsr block_at
  jsr is_solid
  bcc @no
@yes:
  lda #1
  sta on_ground
@no:
  rts

; The pixel y of the top of whichever block tmp1 last landed in.
block_top:
  lda tmp1
  sec
  sbc #PLAY_TOP
  bcs @ok
  lda #0
@ok:
  and #$F0
  clc
  adc #PLAY_TOP
  rts

; ---------------------------------------------------------------------------
; Play
; ---------------------------------------------------------------------------
enter_play:
  jsr rendering_off
  jsr hide_sprites
  jsr load_palette
  jsr load_room
  jsr paint_room
  jsr place_hero
  lda #M_PLAY
  sta mode
  lda #1
  sta hud_dirty
  jsr rendering_on
  rts

place_hero:
  lda spawn_bx
  asl a
  asl a
  asl a
  asl a
  sta hero_xh
  lda spawn_by
  asl a
  asl a
  asl a
  asl a
  clc
  adc #PLAY_TOP
  sta hero_yh
  lda #0
  sta hero_xl
  sta hero_yl
  sta hero_vxl
  sta hero_vxh
  sta hero_vyl
  sta hero_vyh
  sta hero_face
  sta on_ground
  sta coyote
  sta jump_buf
  sta hero_state
  sta crumb_timer
  lda #$FF
  sta crumb_idx
  rts

tick_play:
  lda hero_state
  cmp #HS_DEAD
  bne @notdead
  jmp tick_dying
@notdead:
  cmp #HS_WIN
  bne @alive
  jmp tick_winning
@alive:

  ; Giving up is a legitimate move and should cost exactly one button.
  lda pad1_new
  and #BTN_SELECT
  beq @go
  jsr hero_die
  jmp @draw

@go:
  jsr hero_input
  jsr hero_jump
  jsr hero_gravity
  jsr move_x
  jsr move_y
  jsr ground_probe

  lda on_ground
  beq @air
  lda #COYOTE_FRAMES
  sta coyote
  jmp @coyote_done
@air:
  lda coyote
  beq @coyote_done
  dec coyote
@coyote_done:

  jsr check_crumble
  jsr tick_entities
  jsr tick_darts

  ; Whatever the hero is standing in decides whether he is still alive. This
  ; runs after the entities so that a saw and a spike pit reaching him on the
  ; same frame still only cost one death.
  jsr hero_flags
  sta tmpb
  and #BF_HAZARD
  beq @nohazard
  jsr hero_die
  jmp @draw
@nohazard:
  lda tmpb
  and #BF_GOAL
  beq @draw
  lda hero_state
  bne @draw
  lda #HS_WIN
  sta hero_state
  lda #WIN_FRAMES
  sta state_timer
  lda #SFX_WIN
  jsr sfx_start

@draw:
  jsr draw_frame
  rts

; Death is a short animation and not a stop: he pops up, falls through
; everything, and the room reloads underneath him. Falling through the floor he
; was just killed by is the point, and is why this ignores collision entirely.
tick_dying:
  jsr hero_gravity
  clc
  lda hero_yl
  adc hero_vyl
  sta hero_yl
  lda hero_yh
  adc hero_vyh
  bcc @noclip
  lda #$F8                  ; he is well past the bottom; stop there
@noclip:
  sta hero_yh
  dec state_timer
  bne @draw
  jsr enter_play
  rts
@draw:
  jsr draw_frame
  rts

tick_winning:
  dec state_timer
  bne @draw
  inc room_idx
  lda room_idx
  cmp #ROOM_COUNT
  bcc @next
  jsr enter_end
  rts
@next:
  jsr enter_play
  rts
@draw:
  jsr draw_frame
  rts

; ---------------------------------------------------------------------------
; The cracked floor. It is the honest trap: it looks cracked, it is cracked,
; and it still catches people, because the fair amount of warning turns out to
; be shorter than it feels.
; ---------------------------------------------------------------------------
check_crumble:
  lda on_ground
  beq @none

  lda hero_xh
  clc
  adc #HB_L
  sta tmp0
  lda hero_yh
  clc
  adc #(HB_B+1)
  sta tmp1
  jsr block_at
  tax
  lda blk_flags,x
  and #BF_CRUMBLE
  bne @found

  lda hero_xh
  clc
  adc #HB_R
  sta tmp0
  lda hero_yh
  clc
  adc #(HB_B+1)
  sta tmp1
  jsr block_at
  tax
  lda blk_flags,x
  and #BF_CRUMBLE
  beq @none

@found:
  lda tmp2
  cmp crumb_idx
  beq @tick
  sta crumb_idx
  lda #CRUMBLE_FRAMES
  sta crumb_timer
  rts
@tick:
  dec crumb_timer
  bne @done
  ldx crumb_idx
  lda #BLK_AIR
  sta grid,x
  jsr repaint_cell
  lda #SFX_CRUMBLE
  jsr sfx_start
  lda #$FF
  sta crumb_idx
@done:
  rts
@none:
  lda #$FF
  sta crumb_idx
  rts

hero_die:
  lda hero_state
  bne @already
  lda #HS_DEAD
  sta hero_state
  lda #DEATH_FRAMES
  sta state_timer
  lda #NPOP_LO
  sta hero_vyl
  lda #NPOP_HI
  sta hero_vyh
  lda #0
  sta hero_vxl
  sta hero_vxh
  jsr inc_deaths
  lda #SFX_DIE
  jsr sfx_start
@already:
  rts

; Four digits, rippled by hand. The 2A03 has its decimal mode fused off, so
; there is no adc shortcut to reach for here even if it were tempting.
inc_deaths:
  txa
  pha
  ldx #3
@l:
  inc deaths,x
  lda deaths,x
  cmp #10
  bcc @done
  lda #0
  sta deaths,x
  dex
  bpl @l
@done:
  lda #1
  sta hud_dirty
  pla
  tax
  rts

inc_souls:
  txa
  pha
  ldx #3
@l:
  inc souls,x
  lda souls,x
  cmp #10
  bcc @done
  lda #0
  sta souls,x
  dex
  bpl @l
@done:
  lda #1
  sta hud_dirty
  pla
  tax
  rts

; ---------------------------------------------------------------------------
; Entities
; ---------------------------------------------------------------------------
tick_entities:
  ldx #0
@l:
  cpx ent_count
  beq @done
  lda ent_type,x
  beq @next
  cmp #E_BAIT
  beq @bait
  cmp #E_SAW
  beq @saw
  cmp #E_CRUSHER
  beq @crush
  jsr ent_shooter
  jmp @next
@bait:
  jsr ent_bait
  jmp @next
@saw:
  jsr ent_saw
  jmp @next
@crush:
  jsr ent_crusher
@next:
  inx
  jmp @l
@done:
  rts

; Does this thing overlap the hero?
;
; tmp0/tmp1 = its top-left, tmp4 = its width minus one, tmp6 = its height minus
; one. Width and height are separate, and every caller passes a box smaller than
; the sprite it belongs to, because a saw is a disc drawn inside a 16x16 tile and
; being killed by the empty corner of a circle is the kind of unfairness that
; reads as a broken game rather than a hard one.
hero_hits:
  lda hero_xh
  clc
  adc #HB_R
  cmp tmp0
  bcc @no
  lda tmp0
  clc
  adc tmp4
  sta tmp3
  lda hero_xh
  clc
  adc #HB_L
  cmp tmp3
  beq @xok
  bcs @no
@xok:
  lda hero_yh
  clc
  adc #HB_B
  cmp tmp1
  bcc @no
  lda tmp1
  clc
  adc tmp6
  sta tmp3
  lda hero_yh
  clc
  adc #HB_T
  cmp tmp3
  beq @yes
  bcs @no
@yes:
  sec
  rts
@no:
  clc
  rts

; The bait. It bobs, it glitters, and it is worth exactly one point. Where it
; hangs is the room's business, and every room hangs it somewhere expensive.
ent_bait:
  ; The one box left at full size. Everything else here is trying to kill you
  ; and gets shrunk; a prize should be easy to take.
  lda ent_x,x
  sta tmp0
  lda ent_y,x
  sta tmp1
  lda #15
  sta tmp4
  sta tmp6
  jsr hero_hits
  bcc @done
  lda #0
  sta ent_type,x
  jsr inc_souls
  lda #SFX_PICK
  jsr sfx_start
@done:
  rts

ent_saw:
  lda ent_p3,x
  bmi @left
  lda ent_x,x
  clc
  adc #1
  sta ent_x,x
  cmp ent_p2,x
  bcc @move_done
  lda #$FF
  sta ent_p3,x
  jmp @move_done
@left:
  lda ent_x,x
  sec
  sbc #1
  sta ent_x,x
  cmp ent_p1,x
  bcs @move_done
  lda #1
  sta ent_p3,x
@move_done:
  lda ent_x,x
  clc
  adc #2
  sta tmp0
  lda ent_y,x
  clc
  adc #2
  sta tmp1
  lda #11
  sta tmp4
  sta tmp6
  jsr hero_hits
  bcc @done
  jsr hero_die
@done:
  rts

; The crusher. It notices you, waits just long enough for you to notice it
; noticing, then falls. The wait is the whole design: without it the thing is
; unreadable, and with it the room is a rhythm rather than a coin toss.
;
; p0 = state, p1 = where it hangs, p2 = how long it has been falling,
; p3 = timer.
ent_crusher:
  stx sv_x
  lda ent_p0,x
  beq @resting
  cmp #1
  beq @arming
  cmp #2
  bne @late
  jmp @falling
@late:
  cmp #3
  bne @up
  jmp @sitting
@up:
  jmp @rising

@resting:
  ; Armed by the hero's column overlapping its own, not by his box touching it:
  ; a crusher you have to already be under is a crusher nobody ever survives.
  lda hero_xh
  clc
  adc #HB_R
  cmp ent_x,x
  bcc @idle
  lda ent_x,x
  clc
  adc #15
  sta tmp3
  lda hero_xh
  clc
  adc #HB_L
  cmp tmp3
  beq @arm
  bcs @idle
@arm:
  lda #1
  sta ent_p0,x
  lda #CRUSH_ARM
  sta ent_p3,x
@idle:
  rts

@arming:
  dec ent_p3,x
  bne @idle
  lda #2
  sta ent_p0,x
  lda #0
  sta ent_p2,x
  rts

@falling:
  ; Speed ramps one pixel a frame per three frames, capped at six, which is
  ; fast enough to be frightening and slow enough to be dodged.
  lda ent_p2,x
  cmp #20
  bcs @atspeed
  inc ent_p2,x
@atspeed:
  lda ent_p2,x
  lsr a
  lsr a
  clc
  adc #1
  clc
  adc ent_y,x
  sta ent_y,x

  ; Stop on whatever is under it, or on the bottom of the room. The floor
  ; check is not belt and braces: a crusher whose landing block is a cracked
  ; one has a real chance of finding nothing under it on the second attempt,
  ; and without a floor it would ride its position byte round past 255 and
  ; reappear falling out of the ceiling.
  lda ent_y,x
  cmp #(PLAY_TOP + (GRID_H-1)*16)
  bcs @stop
  lda ent_x,x
  clc
  adc #8
  sta tmp0
  lda ent_y,x
  clc
  adc #16
  sta tmp1
  jsr block_at
  jsr is_solid
  ldx sv_x
  bcc @hurt
@stop:
  lda ent_y,x
  and #$F0
  sta ent_y,x
  lda #3
  sta ent_p0,x
  lda #CRUSH_SIT
  sta ent_p3,x
  lda #SFX_LAND
  jsr sfx_start
  jmp @hurt

@sitting:
  dec ent_p3,x
  bne @hurt
  lda #4
  sta ent_p0,x
  jmp @hurt

@rising:
  lda ent_y,x
  sec
  sbc #1
  sta ent_y,x
  cmp ent_p1,x
  bne @done
  lda #0
  sta ent_p0,x
  rts

@hurt:
  lda ent_x,x
  clc
  adc #1
  sta tmp0
  lda ent_y,x
  sta tmp1
  lda #13
  sta tmp4
  lda #12                   ; its teeth stop three quarters of the way down
  sta tmp6
  jsr hero_hits
  bcc @done
  jsr hero_die
@done:
  rts

ent_shooter:
  dec ent_p0,x
  bne @done
  lda #DART_PERIOD
  sta ent_p0,x
  jsr fire_dart
@done:
  rts

fire_dart:
  ldy #0
@f:
  lda dart_dx,y
  beq @free
  iny
  cpy #DART_MAX
  bne @f
  rts                       ; every slot busy: the shot is simply lost
@free:
  lda ent_type,x
  cmp #E_DARTR
  bne @lft
  lda ent_x,x
  clc
  adc #16
  sta dart_x,y
  lda #DART_SPEED
  sta dart_dx,y
  jmp @fin
@lft:
  lda ent_x,x
  sec
  sbc #8
  sta dart_x,y
  lda #(256-DART_SPEED)
  sta dart_dx,y
@fin:
  lda ent_y,x
  clc
  adc #4
  sta dart_y,y
  lda #SFX_DART
  jsr sfx_start
  rts

tick_darts:
  ldx #0
@l:
  lda dart_dx,x
  beq @next
  clc
  adc dart_x,x
  sta dart_x,x

  sta tmp0
  lda dart_y,x
  clc
  adc #4
  sta tmp1
  stx tmpb
  jsr block_at
  jsr is_solid
  ldx tmpb
  bcc @alive
  lda #0
  sta dart_dx,x
  jmp @next
@alive:
  lda dart_x,x
  clc
  adc #1
  sta tmp0
  lda dart_y,x
  clc
  adc #2
  sta tmp1
  lda #5
  sta tmp4
  lda #3
  sta tmp6
  stx tmpb
  jsr hero_hits
  ldx tmpb
  bcc @next
  jsr hero_die
@next:
  inx
  cpx #DART_MAX
  bne @l
  rts

; ---------------------------------------------------------------------------
; Drawing
; ---------------------------------------------------------------------------
draw_frame:
  lda #0
  sta oam_ptr
  jsr draw_hero
  jsr draw_entities
  jsr draw_darts
  jsr end_sprites
  jsr push_hud
  rts

draw_hero:
  lda hero_state
  cmp #HS_DEAD
  beq @dead
  lda on_ground
  beq @jump
  lda hero_vxh
  bne @walk
  lda hero_vxl
  cmp #64
  bcs @walk
  lda #MS_HERO_IDLE
  jmp @go
@walk:
  lda frame_lo
  and #$08
  beq @w1
  lda #MS_HERO_WALK2
  jmp @go
@w1:
  lda #MS_HERO_WALK1
  jmp @go
@jump:
  lda #MS_HERO_JUMP
  jmp @go
@dead:
  lda #MS_HERO_DEAD
@go:
  sta tmp7
  lda hero_xh
  sta tmp0
  lda hero_yh
  sta tmp1
  lda #0
  ldy hero_face
  beq @nf
  lda #$40
@nf:
  sta tmp2
  lda tmp7
  jsr draw_ms
  rts

; Entities are drawn from a rotating start, so that when a room puts more than
; eight sprites on one scanline the hardware's dropout moves around instead of
; always eating the same saw. The hero is drawn first and never flickers: this
; cartridge lies about the floor, not about where you are.
draw_entities:
  lda frame_lo
  and #$07
  sta sv_x
  lda #0
  sta sv_y
@l:
  lda sv_y
  cmp ent_count
  bcs @done
  lda sv_x
  cmp ent_count
  bcc @ok
  lda #0
  sta sv_x
@ok:
  ldx sv_x
  lda ent_type,x
  beq @next
  cmp #E_BAIT
  beq @bait
  cmp #E_SAW
  beq @saw
  cmp #E_CRUSHER
  beq @crusher
  jmp @next                 ; a shooter is part of the wall and draws nothing
@bait:
  lda #MS_FLASK
  sta tmp7
  lda #3
  sta tmp2
  ; A slow bob, so it reads as something on offer rather than something stuck.
  lda frame_lo
  and #$10
  beq @b0
  lda #1
  jmp @b1
@b0:
  lda #0
@b1:
  clc
  adc ent_y,x
  sta tmp1
  jmp @emit
@saw:
  lda #MS_SAW
  sta tmp7
  lda #1
  sta tmp2
  lda ent_y,x
  sta tmp1
  jmp @emit
@crusher:
  lda #MS_CRUSHER
  sta tmp7
  lda #1
  sta tmp2
  lda ent_y,x
  sta tmp1
  ; While it is arming it shudders, which is the only warning it gives.
  lda ent_p0,x
  cmp #1
  bne @emit
  lda frame_lo
  and #1
  clc
  adc tmp1
  sta tmp1
@emit:
  lda ent_x,x
  sta tmp0
  lda tmp7
  jsr draw_ms
@next:
  inc sv_x
  inc sv_y
  jmp @l
@done:
  rts

draw_darts:
  ldx #0
@l:
  lda dart_dx,x
  beq @next
  lda dart_x,x
  sta tmp0
  lda dart_y,x
  sta tmp1
  lda #2
  sta tmp2
  lda dart_dx,x
  bpl @right
  lda #$42                  ; palette 2, flipped
  sta tmp2
@right:
  stx sv_x
  lda #SPR_DART
  jsr draw_ms8
  ldx sv_x
@next:
  inx
  cpx #DART_MAX
  bne @l
  rts

; A = metasprite index, tmp0 = x, tmp1 = y, tmp2 = OAM attribute. Setting the
; flip bit swaps the left and right halves as well as the tiles themselves,
; which is the whole reason the hero is drawn facing one way only.
draw_ms:
  tay
  lda tmp2
  and #$40
  bne @flip
  lda ms_tl,y
  sta tmp3
  lda ms_tr,y
  sta tmp4
  lda ms_bl,y
  sta tmp5
  lda ms_br,y
  sta tmp6
  jmp @emit
@flip:
  lda ms_tr,y
  sta tmp3
  lda ms_tl,y
  sta tmp4
  lda ms_br,y
  sta tmp5
  lda ms_bl,y
  sta tmp6
@emit:
  ldx oam_ptr
  cpx #OAM_LIMIT
  bcs @full
  lda tmp1
  sec
  sbc #1
  sta oam,x
  lda tmp3
  sta oam+1,x
  lda tmp2
  sta oam+2,x
  lda tmp0
  sta oam+3,x

  lda tmp1
  sec
  sbc #1
  sta oam+4,x
  lda tmp4
  sta oam+5,x
  lda tmp2
  sta oam+6,x
  lda tmp0
  clc
  adc #8
  sta oam+7,x

  lda tmp1
  clc
  adc #7
  sta oam+8,x
  lda tmp5
  sta oam+9,x
  lda tmp2
  sta oam+10,x
  lda tmp0
  sta oam+11,x

  lda tmp1
  clc
  adc #7
  sta oam+12,x
  lda tmp6
  sta oam+13,x
  lda tmp2
  sta oam+14,x
  lda tmp0
  clc
  adc #8
  sta oam+15,x

  txa
  clc
  adc #16
  sta oam_ptr
@full:
  rts

draw_ms8:
  ldx oam_ptr
  cpx #OAM_LIMIT
  bcs @full
  sta tmp3
  lda tmp1
  sec
  sbc #1
  sta oam,x
  lda tmp3
  sta oam+1,x
  lda tmp2
  sta oam+2,x
  lda tmp0
  sta oam+3,x
  txa
  clc
  adc #4
  sta oam_ptr
@full:
  rts

end_sprites:
  ldx oam_ptr
  lda #$FF
@l:
  sta oam,x
  inx
  inx
  inx
  inx
  bne @l
  rts

push_hud:
  lda hud_dirty
  beq @done
  lda #0
  sta hud_dirty

  lda #>HUD_DEATHS
  sta tmp4
  lda #<HUD_DEATHS
  sta tmp5
  ldx #0
@d:
  lda deaths,x
  clc
  adc #$10
  jsr push_patch
  inc tmp5
  inx
  cpx #4
  bne @d

  lda #>HUD_SOULS
  sta tmp4
  lda #<HUD_SOULS
  sta tmp5
  ldx #0
@s:
  lda souls,x
  clc
  adc #$10
  jsr push_patch
  inc tmp5
  inx
  cpx #4
  bne @s
@done:
  rts

; ---------------------------------------------------------------------------
; The address of the first tile of each block row. The playfield starts on
; screen row 4, so row zero is $2000 + 4*32 and each one after it is 64 bytes
; further on.
; ---------------------------------------------------------------------------
row_lo:
  .byte <(NT0+$80+ 0*64), <(NT0+$80+ 1*64), <(NT0+$80+ 2*64), <(NT0+$80+ 3*64)
  .byte <(NT0+$80+ 4*64), <(NT0+$80+ 5*64), <(NT0+$80+ 6*64), <(NT0+$80+ 7*64)
  .byte <(NT0+$80+ 8*64), <(NT0+$80+ 9*64), <(NT0+$80+10*64), <(NT0+$80+11*64)
  .byte <(NT0+$80+12*64)
row_hi:
  .byte >(NT0+$80+ 0*64), >(NT0+$80+ 1*64), >(NT0+$80+ 2*64), >(NT0+$80+ 3*64)
  .byte >(NT0+$80+ 4*64), >(NT0+$80+ 5*64), >(NT0+$80+ 6*64), >(NT0+$80+ 7*64)
  .byte >(NT0+$80+ 8*64), >(NT0+$80+ 9*64), >(NT0+$80+10*64), >(NT0+$80+11*64)
  .byte >(NT0+$80+12*64)

; ---------------------------------------------------------------------------
; Data, tables and art
; ---------------------------------------------------------------------------
.include "data.s"
.include "artmap.s"
.include "rooms.s"
.include "chr.s"

; ---------------------------------------------------------------------------
; What the cartridge asserts about itself, checked once every label is final.
; ---------------------------------------------------------------------------

; The font is addressed as ASCII minus $20, which is what lets a room be typed
; out and a screen script hold text. It only works if the font starts at tile 0
; with nothing inserted ahead of the letters.
.assert chr_glyph_A == $21, "the font is no longer ASCII minus $20"
.assert chr_digit_0 == $10, "the font is no longer ASCII minus $20"
.assert chr_dash    == $0D, "the font is no longer ASCII minus $20"

; The whole joke rests on these four. The liar has to look exactly like the
; honest platform and behave differently, so: same palette, same tiles, and a
; solid bit on one and not the other. Change the liar's picture in genart.py
; and LIAR_TILES_MATCH comes back zero; take the SOLID off the real platform,
; or put one on the liar, and the flags stop agreeing. Either way the build
; stops, which is the only place a claim like this can be defended, since the
; player is never going to be able to check it.
.assert LIAR_TILES_MATCH == 1, "the liar no longer shares the platform's tiles"
.assert BLKP_PLAT == BLKP_LIAR, "the liar and the platform drifted apart in palette"
.assert (BLKF_PLAT & BF_SOLID) != 0, "the honest platform stopped being solid"
.assert (BLKF_LIAR & BF_SOLID) == 0, "the liar became solid, so it is not lying"

; The playfield has to divide the screen the way the collision code assumes.
.assert PLAY_TOP + GRID_H * 16 == 240, "the block grid does not fill the screen"
.assert GRID_W == 16, "a grid index is only row*16+col while the grid is 16 wide"
.assert (PLAY_TOP & 15) == 0, "blocks would not line up with attribute quadrants"

; A hero wider than a block could never fit through a one-block gap; a hero
; taller than one could never stand in a one-block corridor.
.assert HB_R - HB_L <= 15, "the hero is too wide to fit through his own doors"
.assert HB_B - HB_T <= 15, "the hero is too tall to stand in his own corridors"

; Nothing may outgrow the queue that has to drain inside vertical blank: eight
; HUD digits, four tiles of a block that just gave way, and one palette entry.
.assert 8 + 4 + 1 <= PATCH_MAX, "a frame can queue more than vertical blank can flush"

; ---------------------------------------------------------------------------
; Vectors
; ---------------------------------------------------------------------------
.org $FFFA
  .word nmi, reset, irq
