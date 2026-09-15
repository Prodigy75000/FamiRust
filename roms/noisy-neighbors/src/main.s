; SPDX-License-Identifier: CC0-1.0
;
; NOISY NEIGHBORS v0.1
; Written by Prodigy75000. Dedicated to the public domain under CC0 1.0.
;
; A two-player escape for the NES. You are both locked in the same building on
; the same bad night, and a room is not finished until BOTH of you are standing
; in a way out. One of you reaching a door is worth nothing at all, which is
; the whole cartridge stated as a rule.
;
; The pair are not the same. The big one shifts what is heavy and is loud doing
; it; the small one fits where he cannot and is quiet. The one who can do the
; work is therefore the one who keeps getting you both caught, and that is the
; game and the joke at the same time.
;
; TWO THINGS IT WILL NOT DO, both of them for the same reason.
;
; Nothing here asks the two of you to act on the same frame. Every gate that
; needs both of you open wide and stay open, because this cartridge is meant to
; be played over a lockstep netplay link with four frames of input delay on
; each seat, and a window measured in single frames is the one thing that
; cannot survive it. Twenty frames survives it with room to spare, and
; `play --delay 4` is how that stops being an opinion.
;
; Nothing here is decided by who moves first. A room that one of you can lose
; for the other by being early is a room two strangers cannot play.
;
; The controls are the previous cartridge's, deliberately: coyote time on every
; ledge, a jump buffer on every landing, a collision box narrower than the
; sprite. A game that puts your fate in somebody else's hands has no business
; also arguing about whether you pressed the button.
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
SQ2_LO    = $4006
SQ2_HI    = $4007
TRI_LIN   = $4008
TRI_LO    = $400A
TRI_HI    = $400B
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

; Two of you, and neither of you is optional.
;
; Player 1 is the big one: he shifts what is heavy and he is loud doing it.
; Player 2 is the small one: she fits where he cannot and she is quiet. The
; asymmetry is the game twice over, because the one who can do the work is
; also the one who keeps getting you both caught.
PLAYERS  = 2
PL_BIG   = 0
PL_SMALL = 1

; ---------------------------------------------------------------------------
; Noise, and the man it fetches.
;
; One bar for the pair. That is the whole design: his mistakes fill your meter,
; and there is no way to be individually careful.
;
; The bar is twelve cells and the meter runs to 192, so a cell is sixteen and
; working out the fill is four shifts rather than a divide.
;
; Every cost below is paid for by an ACTION or by a POSITION and never by
; timing, and hiding is somewhere you stand rather than something you press.
; That is not only for the four frames of netplay delay. It is what makes a
; one-player mode possible at all, because a character you have parked goes on
; standing where you left him, and therefore goes on being hidden, while you
; are busy being the other one.
; ---------------------------------------------------------------------------
NOISE_MAX   = 192
NOISE_CELLS = 12
NOISE_SHIFT = 4
.assert (NOISE_MAX >> NOISE_SHIFT) == NOISE_CELLS, "the bar and the meter disagree"

; The big one is louder than the small one at everything. That is the entire
; asymmetry between them: the one who can do the heavy work is the one who
; keeps getting you both caught.
NOISE_JUMP_BIG   = 5
NOISE_JUMP_SMALL = 1
NOISE_LAND_BIG   = 10
NOISE_LAND_SMALL = 2

; Both of these are masked rather than divided, so both must be powers of two.
WALK_NOISE_EVERY = 8      ; the big one adds one per this many frames of walking
DECAY_EVERY      = 4      ; once it is draining, one per this many frames
.assert (WALK_NOISE_EVERY & (WALK_NOISE_EVERY - 1)) == 0, "walk period is masked"
.assert (DECAY_EVERY & (DECAY_EVERY - 1)) == 0, "decay period is masked"

; And nothing drains at all until this many frames after the last sound.
;
; The grace is the whole mechanism and the first version did not have it. A
; flat drain has to be slower than the noise going in or nothing ever
; accumulates, and faster than it or waiting is not a real move, and those are
; the same number: one jump and its landing cost fifteen over about fifty
; frames, and a flat drain of one per four frames removed almost exactly
; fifteen over the same fifty. The bar sat at zero however carelessly it was
; played. With a second of grace, being careless never drains at all and
; standing still drains fast, which is the choice the room is supposed to be
; offering.
QUIET_GRACE      = 60

; The neighbor is not a hazard you dodge. He is a deadline you meet.
NB_IDLE   = 0
NB_COMING = 1             ; you can hear him at his door, and that is the warning
NB_SWEEP  = 2             ; he is in the room, and looking
NB_GOING  = 3
NB_WARN         = 120     ; two seconds to reach a wardrobe
NB_SWEEP_FRAMES = 150
NB_GOING_FRAMES = 40

; The bar is drawn out of the font rather than out of art of its own: '#' for
; a cell that is full and '-' for one that is not. .str emits ASCII minus $20,
; and these are the same two bytes typing them would produce.
CH_BAR_FULL  = $23 - $20
CH_BAR_EMPTY = $2D - $20

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

PATCH_MAX = 32
OAM_LIMIT = 240

M_TITLE = 0
M_PLAY  = 1
M_END   = 2
M_OVER  = 3

; Ten tries at the whole keep, not ten at each room. Running out sends you back
; to the first door, which is what turns six rooms you can grind into a run you
; can lose.
LIVES_START = 10

; How loud each voice starts a note, and how quietly it is allowed to end up.
; The hook keeps a floor so it sustains; the arpeggio and the drums decay away.
MUS_LEAD_VOL  = 9
MUS_LEAD_MIN  = 5
MUS_HARM_VOL  = 8
MUS_HARM_MIN  = 4
MUS_KICK_VOL  = 11
MUS_SNARE_VOL = 10
MUS_HAT_VOL   = 4

HS_ALIVE = 0
HS_DEAD  = 1
HS_WIN   = 2

; ---------------------------------------------------------------------------
; Zero page
; ---------------------------------------------------------------------------
nmi_ready    = $00
frame_lo     = $01
frame_hi     = $02
; The two pads sit in pairs so that pick_pad can index them by player number
; rather than branch on it. Splitting a pair costs nothing today and breaks
; `lda pad1,y` silently, which is why they are written out together.
pad1         = $03
pad2         = $04
pad1_new     = $05
pad2_new     = $06
pad1_prev    = $07
pad2_prev    = $08
pad          = $09        ; the pad of whoever is being updated right now
pad_new      = $0A
tmp0         = $0B
tmp1         = $0C
tmp2         = $0D
tmp3         = $0E
tmp4         = $0F
tmp5         = $10
tmp6         = $11
tmp7         = $12
tmpa         = $13
tmpb         = $14
ptr          = $15
ptr2         = $17
room_ptr     = $19
mode         = $1B
ctrl_shadow  = $1C
mask_shadow  = $1D
patch_count  = $1E
patch_tmp    = $1F
oam_ptr      = $20
room_idx     = $21
cur_pl       = $22        ; which player the hero routines are working on

; The working hero.
;
; There is one set of these and not two, and the player being updated is
; swapped into them and back out again around his update. The alternative was
; to index every hero variable by player, which means holding X as a player
; number all the way through the physics and the collision code, and those are
; precisely the routines that were playtested into shape and that already use X
; and Y as scratch. Sixteen bytes in and sixteen out, twice a frame, is 64 byte
; moves against a rewrite of the one part of this cartridge known to be right.
;
; They are contiguous on purpose: hero_load and hero_store move the run, so a
; new per-player variable means putting it inside the run and bumping
; HERO_BYTES, and nothing else anywhere.
hero_xl      = $23
hero_xh      = $24
hero_yl      = $25
hero_yh      = $26
hero_vxl     = $27
hero_vxh     = $28
hero_vyl     = $29
hero_vyh     = $2A
hero_face    = $2B
on_ground    = $2C
coyote       = $2D
jump_buf     = $2E
hero_state   = $2F
state_timer  = $30
crumb_idx    = $31
crumb_timer  = $32
HERO_BYTES   = 16

; Scratch inside move_x and move_y. Deliberately outside the saved run: it
; never has to survive the routine that fills it.
newx_l       = $33
newx_h       = $34
newy_l       = $35
newy_h       = $36

; Counting down the end of a room, whichever way it ended. It is one timer for
; the pair and not one each, because a room ends for both of you at once: see
; room_verdict.
room_timer   = $37
hud_dirty    = $38

sfx_ch       = $39
sfx_dur      = $3A
sfx_plo      = $3B
sfx_phi      = $3C
sfx_dp       = $3D
sfx_vol      = $3E
sfx_duty     = $3F
sfx_last_hi  = $40
sfx_id       = $41
sv_x         = $42        ; a loop index that has to survive a subroutine
sv_y         = $43

music_on     = $44
music_row    = $45        ; wraps at 256, which is exactly the length of the song
music_timer  = $46
mus_vol1     = $47
mus_vol2     = $48
mus_vol4     = $49
lives        = $4A

noise        = $4B        ; 0 to NOISE_MAX, shared by the pair
noise_shown  = $4C        ; cells currently lit, so the bar patches one at a time
noise_made   = $4D        ; did either of them make any this frame
noise_tick   = $4E        ; free-running, masked for the walk and decay periods
nb_state     = $4F
nb_timer     = $50
quiet_timer  = $51        ; frames left before the building starts forgetting

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

; Both players, parked. Player p occupies psave + p*HERO_BYTES; hero_load and
; hero_store are the only two routines that know that.
psave      = $0500

; The same two blocks, named, for the scripted harness and for anyone watching
; RAM. Derived from the working-set addresses rather than written out as
; numbers, so reordering the hero block moves these with it instead of leaving
; them pointing at whatever slid into place. Nothing in the ROM reads them.
p1_xh      = psave + hero_xh - hero_xl
p1_yh      = psave + hero_yh - hero_xl
p1_state   = psave + hero_state - hero_xl
p2_xh      = psave + HERO_BYTES + hero_xh - hero_xl
p2_yh      = psave + HERO_BYTES + hero_yh - hero_xl
p2_state   = psave + HERO_BYTES + hero_state - hero_xl

; Where each of them starts the room, in blocks. Per player, because the room
; picture marks two spawns and they are never the same cell.
spawn_bx   = $0520
spawn_by   = $0522



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
  lda #LIVES_START
  sta lives
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
  jsr music_tick
  jsr flicker

  lda mode
  cmp #M_PLAY
  beq @play
  cmp #M_END
  beq @end
  cmp #M_OVER
  beq @over
  jsr tick_title
  jmp main
@play:
  jsr tick_play
  jmp main
@end:
  jsr tick_end
  jmp main
@over:
  jsr tick_over
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

; Both pads, read twice and compared, because a read that disagrees with itself
; was corrupted by the DMC and is worth nothing. Both have to agree before
; either is believed: accepting pad 1 from one pass and pad 2 from the next
; would hand the two players input from two different instants, which is the
; one thing a lockstep netplay session must never be given a reason to do.
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

; Called from inside entity loops, so it gives X back. So does hero_die, which
; calls it: an entity routine that lost its own index to a sound effect is a
; very quiet kind of bug.
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

; ---------------------------------------------------------------------------
; Music
;
; Four voices off one row counter. The counter is a byte and the song is exactly
; 256 rows, so looping it costs nothing: it wraps.
;
; Sound effects live on pulse 1 and the noise channel, which are the two the
; tune can spare for a moment. Rather than mixing, each voice asks whether its
; channel is free and simply does not play if it is not, so a jump ducks the
; hook for a fifth of a second and the bass and the arpeggio carry the tune
; underneath it. A note lost that way is not recovered; the next row brings
; another one.
; ---------------------------------------------------------------------------
music_start:
  lda #0
  sta music_row
  sta mus_vol1
  sta mus_vol2
  sta mus_vol4
  lda #1
  sta music_timer
  sta music_on
  rts

music_stop:
  lda #0
  sta music_on
  sta TRI_LIN
  lda #%00110000
  sta SQ2_VOL
  rts

; Carry set if the tune may write to pulse 1 this frame.
p1_free:
  lda sfx_dur
  beq @yes
  lda sfx_ch
  beq @no
@yes:
  sec
  rts
@no:
  clc
  rts

noise_free:
  lda sfx_dur
  beq @yes
  lda sfx_ch
  bne @no
@yes:
  sec
  rts
@no:
  clc
  rts

music_tick:
  lda music_on
  bne @on
  rts
@on:
  dec music_timer
  bne @env
  lda #MUSIC_TEMPO
  sta music_timer
  jsr music_row_step
  inc music_row
@env:
  jmp music_envelopes

music_row_step:
  ldx music_row

  ; ---- pulse 1: the hook ----
  lda song_lead,x
  cmp #NOTE_HOLD
  beq @harm
  cmp #NOTE_REST
  bne @lead
  lda #0
  sta mus_vol1
  jmp @harm
@lead:
  tay
  lda #MUS_LEAD_VOL
  sta mus_vol1
  jsr p1_free
  bcc @harm
  lda note_lo,y
  sta SQ1_LO
  lda note_hi,y
  ora #%00001000            ; a non-zero length, which the halt bit then freezes
  sta SQ1_HI

  ; ---- pulse 2: the arpeggio ----
@harm:
  ldx music_row
  lda song_harm,x
  cmp #NOTE_HOLD
  beq @bass
  cmp #NOTE_REST
  bne @harm_on
  lda #0
  sta mus_vol2
  jmp @bass
@harm_on:
  tay
  lda #MUS_HARM_VOL
  sta mus_vol2
  lda note_lo,y
  sta SQ2_LO
  lda note_hi,y
  ora #%00001000
  sta SQ2_HI

  ; ---- triangle: the bass ----
@bass:
  ldx music_row
  lda song_bass,x
  cmp #NOTE_HOLD
  beq @drum
  cmp #NOTE_REST
  bne @bass_on
  lda #0
  sta TRI_LIN
  jmp @drum
@bass_on:
  tay
  lda #$FF                  ; control set: load the linear counter and hold it
  sta TRI_LIN
  lda tri_lo,y
  sta TRI_LO
  lda tri_hi,y
  ora #%00001000
  sta TRI_HI

  ; ---- noise: the drums ----
@drum:
  ldx music_row
  lda song_drum,x
  beq @done
  cmp #DRUM_KICK
  beq @kick
  cmp #DRUM_SNARE
  beq @snare
  lda #1                    ; hat: the shortest period, so the brightest hiss
  sta tmpa
  lda #MUS_HAT_VOL
  jmp @hit
@kick:
  lda #13                   ; a long period, which on the noise channel is low
  sta tmpa
  lda #MUS_KICK_VOL
  jmp @hit
@snare:
  lda #6
  sta tmpa
  lda #MUS_SNARE_VOL
@hit:
  sta mus_vol4
  jsr noise_free
  bcc @done
  lda tmpa
  sta NOISE_LO
  lda #$08
  sta NOISE_HI
@done:
  rts

music_envelopes:
  jsr p1_free
  bcc @two
  lda mus_vol1
  ora #%10110000            ; duty 50%, halt length, constant volume
  sta SQ1_VOL
  lda #SWEEP_OFF
  sta SQ1_SWEEP
@two:
  lda mus_vol2
  ora #%01110000            ; duty 25%, which sits under the hook rather than on it
  sta SQ2_VOL
  lda #SWEEP_OFF
  sta SQ2_SWEEP

  jsr noise_free
  bcc @decay
  lda mus_vol4
  ora #%00110000
  sta NOISE_VOL

  ; One step of decay every fourth frame. The hook and the arpeggio stop at a
  ; floor so they hold; the drums are allowed all the way down to nothing.
@decay:
  lda frame_lo
  and #3
  bne @done
  lda mus_vol1
  cmp #(MUS_LEAD_MIN+1)
  bcc @d2
  dec mus_vol1
@d2:
  lda mus_vol2
  cmp #(MUS_HARM_MIN+1)
  bcc @d4
  dec mus_vol2
@d4:
  lda mus_vol4
  beq @done
  dec mus_vol4
@done:
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
  jsr music_start
  jsr rendering_on
  rts

tick_title:
  lda pad1_new
  ora pad2_new
  and #BTN_START
  beq @done
  lda #SFX_UI
  jsr sfx_start
  ; A new run, so the tally starts again.
  lda #LIVES_START
  sta lives
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

  lda lives
  jsr split_digits
  lda #>END_LIVES
  sta PPUADDR
  lda #<END_LIVES
  sta PPUADDR
  lda tmp0
  clc
  adc #$10
  sta PPUDATA
  lda tmp1
  clc
  adc #$10
  sta PPUDATA

  lda #M_END
  sta mode
  jsr rendering_on
  rts

; ---------------------------------------------------------------------------
; Out of lives. The building wins.
; ---------------------------------------------------------------------------
enter_over:
  jsr rendering_off
  jsr hide_sprites
  jsr music_stop
  jsr clear_vram
  jsr load_palette
  lda #<over_script
  sta ptr
  lda #>over_script
  sta ptr+1
  jsr draw_script
  lda #M_OVER
  sta mode
  lda #SFX_DIE
  jsr sfx_start
  jsr rendering_on
  rts

tick_over:
  lda pad1_new
  ora pad2_new
  and #BTN_START
  beq @done
  jsr enter_title
@done:
  rts

; A = a number under 100. Leaves its tens in tmp0 and its units in tmp1.
; The 2A03 has its decimal mode fused off, so this is the whole of the
; arithmetic available: subtract ten until it stops going.
split_digits:
  ldx #0
@l:
  cmp #10
  bcc @done
  sec
  sbc #10
  inx
  jmp @l
@done:
  stx tmp0
  sta tmp1
  rts

tick_end:
  lda pad1_new
  ora pad2_new
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

  ; A room missing a 1 or a 2 starts that player in a corner rather than
  ; refusing to load. It is not a real fallback: tools/reach2.py fails the
  ; build long before this can happen. It exists so that a half-typed room
  ; still boots while you are looking at it.
  lda #1
  sta spawn_bx + PL_BIG
  lda #2
  sta spawn_bx + PL_SMALL
  lda #9
  sta spawn_by + PL_BIG
  sta spawn_by + PL_SMALL

  ldy #NAME_LEN
  ldx #0
@l:
  lda (room_ptr),y
  sty tmp0
  cmp #CH_SPAWN1
  beq @s1
  cmp #CH_SPAWN2
  bne @nospawn
  ldy #PL_SMALL
  jmp @mark
@s1:
  ldy #PL_BIG
@mark:
  txa
  and #$0F
  sta spawn_bx,y
  txa
  lsr a
  lsr a
  lsr a
  lsr a
  sta spawn_by,y
  ldy tmp0                  ; Y was the room offset before it was the player
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

  rts

; ---------------------------------------------------------------------------
; The two of them, in and out of the working set.
;
; Everything from hero_input down to ground_probe reads and writes one hero
; and has no idea there is another. That is the whole point: those routines
; came over from a cartridge where they were already right, and they were not
; touched.
; ---------------------------------------------------------------------------
hero_load:
  jsr hero_slot
  ldx #0
@l:
  lda psave,y
  sta hero_xl,x
  iny
  inx
  cpx #HERO_BYTES
  bne @l
  rts

hero_store:
  jsr hero_slot
  ldx #0
@l:
  lda hero_xl,x
  sta psave,y
  iny
  inx
  cpx #HERO_BYTES
  bne @l
  rts

; Y = where cur_pl's block starts inside psave. The block is sixteen bytes so
; the multiply is four shifts, and a fifth would silently start overlapping the
; next player, which is why HERO_BYTES and this shift count are asserted
; against each other rather than merely agreeing today.
.assert HERO_BYTES == 16, "hero_slot shifts by four, so the block must be 16 bytes"
hero_slot:
  lda cur_pl
  asl a
  asl a
  asl a
  asl a
  tay
  rts

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
  lda pad
  and #BTN_LEFT
  bne @left
  lda pad
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
  lda pad_new
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
  lda pad
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
  jsr place_heroes
  lda #M_PLAY
  sta mode
  lda #1
  sta hud_dirty
  jsr rendering_on
  rts

place_heroes:
  lda #0
  sta room_timer
  ; The room starts quiet and the bar starts empty. noise_shown has to be
  ; cleared with it, because paint_room has just drawn twelve empty cells and
  ; the bar patches the difference between what is true and what is on screen.
  sta noise
  sta noise_shown
  sta noise_tick
  sta noise_made
  sta nb_state
  sta nb_timer
  sta quiet_timer
  sta cur_pl
@l:
  jsr place_one
  jsr hero_store
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @l
  rts

; Puts cur_pl on his own spawn. Y holds the player all the way through, which
; is why nothing in here is allowed to touch it.
place_one:
  ldy cur_pl
  lda spawn_bx,y
  asl a
  asl a
  asl a
  asl a
  sta hero_xh
  lda spawn_by,y
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
  sta coyote
  sta jump_buf
  sta hero_state
  sta crumb_timer
  lda #$FF
  sta crumb_idx
  ; He arrives standing, not falling. Zeroing this instead made the first frame
  ; of every room read as a landing, so the room opened by charging both of
  ; them for a noise neither of them made and the bar was never empty. If a
  ; room does spawn somebody over a hole, ground_probe corrects this on the
  ; first frame and the landing that follows is genuinely theirs.
  lda #1
  sta on_ground
  rts

; The pad of whoever is being updated. Everything downstream reads `pad` and
; never asks who it belongs to.
pick_pad:
  ldy cur_pl
  lda pad1,y
  sta pad
  lda pad1_new,y
  sta pad_new
  rts

tick_play:
  lda room_timer
  beq @live
  jmp tick_settle

@live:
  ; Giving up is a legitimate move and costs exactly one button, from either of
  ; them. A room only one of you can see the way out of is still stuck, and
  ; making the other one walk over and ask for it is not a puzzle.
  lda pad1_new
  ora pad2_new
  and #BTN_SELECT
  beq @go
  jsr kill_room
  jmp @draw

@go:
  lda #0
  sta noise_made
  sta cur_pl
@ploop:
  jsr hero_load
  jsr pick_pad
  jsr update_hero
  jsr hero_store
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @ploop

  jsr noise_frame
  jsr neighbor_tick
  jsr room_verdict

@draw:
  jsr draw_frame
  rts

; One player's frame. It knows nothing about the other one, which is what lets
; it be the code that came over unchanged.
update_hero:
  lda hero_state
  beq @alive
  rts                       ; dead, or already at his door and waiting
@alive:
  lda on_ground
  sta tmp6                  ; where his feet were before any of this
  jsr hero_input
  jsr hero_jump
  jsr hero_gravity
  jsr move_x
  jsr move_y
  jsr ground_probe
  jsr noise_from_motion

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

  ; Whatever he is standing in decides whether he is still alive.
  jsr hero_flags
  sta tmpb
  and #BF_HAZARD
  beq @nohazard
  lda #HS_DEAD
  sta hero_state
  rts
@nohazard:
  lda tmpb
  and #BF_GOAL
  beq @done
  ; A door is not a tripwire. You have to stand in it and ask, which costs one
  ; button and buys the room the right to put a door somewhere you would
  ; otherwise run straight through by accident.
  lda pad
  and #BTN_UP
  beq @done
  lda #HS_WIN
  sta hero_state
  lda #SFX_DOOR
  jsr sfx_start
@done:
  rts

; ---------------------------------------------------------------------------
; Noise
; ---------------------------------------------------------------------------

; A = how much to add. Clamps at the top rather than wrapping, because a meter
; that reads quiet at the exact instant it is loudest is the worst bug in this
; cartridge to have to reproduce.
noise_add:
  cmp #0
  beq @none
  sta tmpa
  lda #1
  sta noise_made
  lda noise
  clc
  adc tmpa
  bcs @full
  cmp #NOISE_MAX
  bcc @set
@full:
  lda #NOISE_MAX
@set:
  sta noise
@none:
  rts

; What the two of them just did, worked out by comparing where his feet were
; before the move with where they are now.
;
; This is here and not inside hero_jump on purpose. The movement routines came
; over from the other cartridge already playtested, and they are worth keeping
; recognisable; nothing is gained by threading a side effect through them.
;
; tmp6 holds on_ground from before the move.
noise_from_motion:
  lda on_ground
  bne @grounded

  ; In the air now. If his feet were down a moment ago and he is travelling
  ; upward, he jumped. If he is not travelling upward he walked off an edge,
  ; and walking off an edge is free: it is the one way to get down quietly and
  ; the rooms are allowed to be built around it.
  lda tmp6
  beq @none
  lda hero_vyh
  bpl @none
  ldx cur_pl
  lda noise_jump_cost,x
  jmp noise_add

@grounded:
  lda tmp6
  bne @walking
  ; He was in the air and he is not any more.
  ldx cur_pl
  lda noise_land_cost,x
  jmp noise_add

@walking:
  ; Under a pixel a frame is a shuffle rather than a walk, and costs nothing.
  lda hero_vxh
  beq @none
  lda cur_pl
  bne @none                 ; the small one walks silently: it is her whole job
  lda noise_tick
  and #(WALK_NOISE_EVERY - 1)
  bne @none
  lda #1
  jmp noise_add
@none:
  rts

noise_jump_cost:
  .byte NOISE_JUMP_BIG, NOISE_JUMP_SMALL
noise_land_cost:
  .byte NOISE_LAND_BIG, NOISE_LAND_SMALL

; Once a frame, after both of them have moved. A frame in which neither of them
; made a sound is a frame the building forgets a little of what it heard, which
; is what makes standing still a real move rather than a wasted one.
noise_frame:
  inc noise_tick
  lda noise_made
  beq @quiet
  lda #QUIET_GRACE
  sta quiet_timer
  rts
@quiet:
  lda quiet_timer
  beq @draining
  dec quiet_timer
  rts
@draining:
  lda noise
  beq @done
  lda noise_tick
  and #(DECAY_EVERY - 1)
  bne @done
  dec noise
@done:
  rts

; The bar, one cell per frame.
;
; It can only ever be one cell out, because the largest single cost is ten and
; a cell is sixteen, so one action cannot cross two boundaries. Both of them
; landing on the same frame can, and then the bar is briefly one cell behind
; the truth and catches up on the next frame. That is a deliberate trade: a
; bar that repaints itself all at once is a burst of patches that vertical
; blank has no time to flush.
push_noise:
  lda noise
  lsr a
  lsr a
  lsr a
  lsr a
  cmp noise_shown
  beq @done
  bcc @quieter
  ldx noise_shown
  inc noise_shown
  lda #CH_BAR_FULL
  jmp patch_bar_cell
@quieter:
  dec noise_shown
  ldx noise_shown
  lda #CH_BAR_EMPTY
  jmp patch_bar_cell
@done:
  rts

; X = which cell, A = the glyph that goes in it.
patch_bar_cell:
  sta patch_tmp
  txa
  clc
  adc #<HUD_NOISE
  sta tmp5
  lda #>HUD_NOISE
  adc #0
  sta tmp4
  lda patch_tmp
  jmp push_patch

; ---------------------------------------------------------------------------
; The neighbor
; ---------------------------------------------------------------------------

; He arrives when the bar fills, and the two seconds between hearing him and
; seeing him are the entire warning. There is no way to make him leave early
; and no way to fight him. The only move is to already be somewhere else.
neighbor_tick:
  lda nb_state
  bne @running

  lda noise
  cmp #NOISE_MAX
  bcc @done
  lda #NB_COMING
  sta nb_state
  lda #NB_WARN
  sta nb_timer
  ; The tune stops rather than ducking. Two seconds of hearing only your own
  ; footsteps is worth more than any sting would be.
  jsr music_stop
  lda #SFX_KNOCK
  jsr sfx_start
@done:
  rts

@running:
  lda nb_state
  cmp #NB_SWEEP
  bne @tick
  jsr check_hidden
@tick:
  dec nb_timer
  bne @done
  lda nb_state
  cmp #NB_COMING
  bne @after_sweep
  lda #NB_SWEEP
  sta nb_state
  lda #NB_SWEEP_FRAMES
  sta nb_timer
  rts
@after_sweep:
  cmp #NB_SWEEP
  bne @after_going
  lda #NB_GOING
  sta nb_state
  lda #NB_GOING_FRAMES
  sta nb_timer
  rts
@after_going:
  ; Back inside his own flat, and the building is quiet again.
  lda #NB_IDLE
  sta nb_state
  lda #0
  sta noise
  jsr music_start
  rts

; While he is in the room, either of them not standing in a wardrobe is caught.
;
; There is no line of sight and no geometry to learn. He is here, and you are
; either in a wardrobe or you are not. A hiding place whose angles have to be
; worked out is not a hiding place when the working out has to happen in two
; seconds, and it would be an unplayable one over a link with input delay on it.
check_hidden:
  lda #0
  sta cur_pl
@l:
  jsr hero_load
  jsr hero_flags
  and #BF_HIDE
  beq @caught
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @l
  rts
@caught:
  jmp kill_room

; Whether the room is still going, asked of the pair rather than of either of
; them. One of you dying is both of you starting it again; both of you at your
; own door is the way out, and one of you at a door is nothing at all. That
; last one is the whole game: the door does not care that you reached it, it
; cares that you both did.
room_verdict:
  lda #0
  sta cur_pl
  sta tmp4                  ; how many are standing in their door
@l:
  jsr hero_load
  lda hero_state
  cmp #HS_DEAD
  beq @died
  cmp #HS_WIN
  bne @next
  inc tmp4
@next:
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @l

  lda tmp4
  cmp #PLAYERS
  bne @nothing
  lda #WIN_FRAMES
  sta room_timer
@nothing:
  rts
@died:
  jsr kill_room
  rts

; Death is a short animation and not a stop: he pops up, falls through
; everything, and the room reloads underneath him. Falling through the floor he
; was just killed by is the point, and is why this ignores collision entirely.
; The end of a room, either way it went. One countdown for the pair, because
; both of them are always in the same ending: kill_room puts everybody in
; HS_DEAD, and the timer only starts on a win when both are in HS_WIN. So
; asking the big one which ending this was answers for both.
tick_settle:
  lda #0
  sta cur_pl
@l:
  jsr hero_load
  lda hero_state
  cmp #HS_DEAD
  bne @nofall
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
@nofall:
  jsr hero_store
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @l

  dec room_timer
  bne @draw

  lda #PL_BIG
  sta cur_pl
  jsr hero_load
  lda hero_state
  cmp #HS_WIN
  bne @again
  inc room_idx
  lda room_idx
  cmp #ROOM_COUNT
  bcc @next
  jsr enter_end
  rts
@next:
  jsr enter_play
  rts
@again:
  lda lives
  bne @replay
  jsr enter_over
  rts
@replay:
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

; One of you died, so the room did. Whoever was still standing goes down with
; it: there is no state in which one of them is picking their way onward
; without the other, and allowing one would mean a room that can be entered
; alone, which none of them can be solved alone.
;
; Guarded on room_timer so that two players landing in the same spike pit on
; the same frame costs one life and not two.
kill_room:
  lda room_timer
  bne @already
  lda #DEATH_FRAMES
  sta room_timer
  lda lives
  beq @nolife
  dec lives
@nolife:
  lda #1
  sta hud_dirty
  lda #SFX_DIE
  jsr sfx_start

  lda #0
  sta cur_pl
@l:
  jsr hero_load
  lda #HS_DEAD
  sta hero_state
  lda #NPOP_LO
  sta hero_vyl
  lda #NPOP_HI
  sta hero_vyh
  lda #0
  sta hero_vxl
  sta hero_vxh
  jsr hero_store
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @l
@already:
  rts

draw_frame:
  lda #0
  sta oam_ptr
  sta cur_pl
@l:
  jsr hero_load
  jsr draw_hero
  inc cur_pl
  lda cur_pl
  cmp #PLAYERS
  bne @l
  jsr end_sprites
  jsr push_noise
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
  ; Bits 0 and 1 of the attribute byte choose the sprite palette, so the pair
  ; are told apart by colour out of one set of tiles. cur_pl is the palette
  ; number, which holds only while there are exactly two of them.
  ora cur_pl
  sta tmp2
  lda tmp7
  jsr draw_ms
  rts
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
  bne @go
  rts
@go:
  lda #0
  sta hud_dirty

  lda #>HUD_LIVES
  sta tmp4
  lda #<HUD_LIVES
  sta tmp5
  lda lives
  jmp push_two

; A = a number under 100, tmp4/tmp5 = where its tens digit goes.
push_two:
  jsr split_digits
  lda tmp0
  clc
  adc #$10
  jsr push_patch
  inc tmp5
  lda tmp1
  clc
  adc #$10
  jmp push_patch

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
.include "music.s"
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
.assert (BLKF_PLAT & BF_SOLID) != 0, "the honest platform stopped being solid"
.assert (BLKF_CLOSET & BF_HIDE) != 0, "the wardrobe stopped being somewhere to hide"
.assert (BLKF_CLOSET & BF_SOLID) == 0, "the wardrobe became solid, so nobody can get in"
.assert (BLKF_DOOR & BF_GOAL) != 0, "the way out stopped being a way out"

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
