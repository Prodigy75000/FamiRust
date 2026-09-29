; SPDX-License-Identifier: GPL-3.0-or-later
;
; FamiRust HLE FDS BIOS
; Written by Prodigy75000.
;
; A clean-room replacement for `disksys.rom`, so the Famicom Disk System runs
; with no firmware file. It is written against the published RAM Adapter
; interface and against behaviour measured from the real BIOS as a black box;
; no part of it is derived from a disassembly of Nintendo's ROM, and none of
; Nintendo's artwork or text appears in it. The boot screen is ours.
;
; The doctrine, the corpus census and the running order are in
; docs/notes/FDS-HLE.md. In short: the real BIOS is the ORACLE, never a
; requirement. `fdsdiff` boots a disk under both and compares the machine each
; one hands the game.
;
; ---------------------------------------------------------------------------
; Why this is 6502 rather than host code
; ---------------------------------------------------------------------------
;
; The obvious design was a synthetic image full of trap opcodes with the work
; done in Rust. The census killed it with one number. Loading is PHYSICAL: the
; drive delivers a byte every 149 CPU cycles, and the real BIOS spends an
; average of 8.4 million cycles per LoadFiles call, 9,911,532 of them in one
; measured case, which is exactly one side of a disk. A host-side routine has
; to invent that time, and it has to keep inventing it while the game's own
; interrupt handlers run inside the wait. Doing the work in 6502 gets the
; timing for free, because it genuinely does the work.
;
; It also makes save states free (this is just code in the BIOS window, which
; already serializes), and makes the whole thing testable with the tools the
; repo already has.
;
; ---------------------------------------------------------------------------
; The handover contract, as measured from the real BIOS
; ---------------------------------------------------------------------------
;
; Measured with `fdsdiff <disk> --bios dumps/fds/disksys.rom`. Two parts of it
; are a contract and one part only looked like one:
;
;   * control goes to the address in $DFFC, the loaded RESET pseudo-vector.
;     Always, on all 114 disks in the corpus.
;   * the files the boot loader selected are in place, and nothing else is.
;   * the REGISTERS are not a contract. The first three disks tried all handed
;     over a=$10 x=$00 y=$FF sp=$FF p=$20, which looked like one; Xevious hands
;     over x=$FF y=$00 p=$21. They are whatever the real BIOS's last instruction
;     left behind and they vary by disk, so no game can be depending on them.
;     We set a fixed, sane set: interrupts enabled, decimal off, stack reset.
;
; ---------------------------------------------------------------------------
; The file-selection rule, as measured from the real BIOS
; ---------------------------------------------------------------------------
;
; A file is loaded at boot if its file ID is <= the "boot read file code",
; which is byte $19 of the disk-info block. Both halves of that were measured
; rather than read off a spec:
;
;   * the byte is at $19, not $1A. At $1A every disk reads $FF, which selects
;     everything and is wrong: Bio Miracle loads only two of its seven files.
;     Byte $19 reads 15, 15 and 241 on the three disks here, and those values
;     predict exactly which files land in RAM.
;   * the comparison is <=, not <. Falsion's boot code is 15 and its FC_6 file
;     has ID 15, and FC_6 is loaded. (Super Mario Bros. 2 has the same shape
;     but its ID-15 file is one zero byte, which proves nothing either way.)
;
; ---------------------------------------------------------------------------
; Layout
; ---------------------------------------------------------------------------
;
; Routines sit at the addresses the corpus census found games calling, because
; a game that calls $E18B has to arrive at an NMI handler. This step implements
; the three that a cold boot and a running game cannot do without; the other 37
; entry points are step four.

.raw base=$E000 size=8192

; ---------------------------------------------------------------------------
; Hardware
; ---------------------------------------------------------------------------
PPUCTRL   = $2000
PPUMASK   = $2001
PPUSTATUS = $2002
OAMADDR   = $2003
PPUADDR   = $2006
PPUDATA   = $2007
APUSTATUS = $4015
APUFRAME  = $4017
DMC_FREQ  = $4010

; RAM Adapter registers.
FDS_IRQ_LO   = $4020
FDS_IRQ_HI   = $4021
FDS_IRQ_CTRL = $4022
FDS_MASTER   = $4023      ; bit0 disk I/O enable, bit1 sound I/O enable
FDS_WRITE    = $4024
FDS_CTRL     = $4025
FDS_STATUS   = $4030      ; bit0 timer IRQ, bit1 byte ready, bit6 end of head
FDS_DATA     = $4031
FDS_DRIVE    = $4032      ; bit0 no disk, bit1 not ready, bit2 write protected

; $4025, assembled once so the bits are named where they are set.
;   bit0 = 1 do not stop the motor        bit1 = 0 start the motor
;   bit2 = 1 read (0 would be write)      bit3 = 1 HORIZONTAL mirroring
;   bit6 = 1 transfer armed               bit7 = 1 IRQ on each byte
;
; Bit 3 is not a detail and it is not a guess. A file of kind 2 loads into the
; nametables at the PPU address its header names, and which physical bank that
; is depends on the mirroring in force at the time. Loading Zelda with bit 3
; clear put its licence screen in bank 0; the real BIOS puts it in bank 1, and
; with bit 3 set all 224 bytes land in the same place as the oracle's. So the
; real BIOS boots horizontal, and a game that inherits the mirroring rather
; than setting its own would have drawn from the wrong table.
CTRL_IDLE = %00001101     ; motor running, read mode, transfer held in reset
CTRL_READ = %01001101     ; the same with the transfer armed
CTRL_STOP = %00001110     ; motor stopped (bit0 clear), transfer in reset

; The pseudo-vectors, in program RAM. The game's loaded files put its own
; handlers here; the BIOS only ever reads them.
VEC_NMI1  = $DFF6
VEC_NMI2  = $DFF8
VEC_NMI3  = $DFFA
VEC_RESET = $DFFC
VEC_IRQ   = $DFFE

; ---------------------------------------------------------------------------
; Our own RAM
; ---------------------------------------------------------------------------
;
; Boot scratch only: nothing is loaded yet, so there is nothing to protect. It
; is all cleared before handover, which is also what the real BIOS leaves in
; these addresses, so the two agree there by construction rather than by luck.
;
; The interrupt handlers below touch NO zero page at all, because after
; handover every byte of it belongs to the game.
ptr       = $E0           ; 2: where the file being read is going
cnt       = $E2           ; 2: bytes left in the current block
files     = $E4           ; files still to consider
bootcode  = $E5           ; the disk's boot read file code
kind      = $E6           ; 0 program RAM, 1 pattern RAM, 2 nametable
wanted    = $E7           ; nonzero if this file is one we load
tmp       = $E8

SCRATCH_LO = $E0
SCRATCH_HI = $E9          ; one past the last scratch byte

; The 16-byte file header, read whole so its fields can be picked out by index
; rather than by a chain of compares. Top of work RAM, cleared before handover.
hdr       = $07F0
HDR_ID    = hdr + 2
HDR_ADDR  = hdr + 11
HDR_SIZE  = hdr + 13
HDR_KIND  = hdr + 15

; ===========================================================================
; $E000: helpers
; ===========================================================================
.org $F000

; --- read one byte off the disk into A -------------------------------------
;
; The drive presents a byte every 149 cycles and will not advance past one the
; CPU has not taken, so polling cannot lose data. Reading $4030 clears the
; ready flag, which is what releases the head; $4031 then hands back the byte
; that was latched.
;
; Carry clear on success. Carry set means the head ran off the end of the side,
; which is how a truncated or foreign disk ends the load instead of hanging.
read_byte:
@wait:
  lda FDS_STATUS
  and #$42                  ; bit1 ready, bit6 end of head
  beq @wait
  cmp #$02
  bne @ended                ; end-of-head set: no more disk
  lda FDS_DATA
  clc
  rts
@ended:
  sec
  rts

; --- position the drive at the start of the next block ---------------------
;
; Dropping the transfer-armed bit and raising it again restarts the drive's
; gap scan, so it skips the zero gap and the start mark and delivers the next
; block's type byte first. That is how one block is left and the next entered.
next_block:
  lda #CTRL_IDLE
  sta FDS_CTRL
  lda #CTRL_READ
  sta FDS_CTRL
  rts

; --- read `cnt` bytes and throw them away ----------------------------------
skip_bytes:
@loop:
  lda cnt
  ora cnt+1
  beq @done
  jsr read_byte
  bcs @stop
  lda cnt
  bne @nolo
  dec cnt+1
@nolo:
  dec cnt
  jmp @loop
@done:
  clc
@stop:
  rts

; --- read `cnt` bytes into (ptr) -------------------------------------------
read_to_ptr:
  ldy #0
@loop:
  lda cnt
  ora cnt+1
  beq @done
  jsr read_byte
  bcs @stop
  sta (ptr),y
  iny
  bne @nowrap
  inc ptr+1
@nowrap:
  lda cnt
  bne @nolo
  dec cnt+1
@nolo:
  dec cnt
  jmp @loop
@done:
  clc
@stop:
  rts

; --- read `cnt` bytes into PPU memory at HDR_ADDR --------------------------
;
; Pattern RAM and nametable files go here. Rendering is off for the whole of
; boot, so a straight run of $2007 writes is safe and there is no need to chase
; vblank.
read_to_ppu:
  bit PPUSTATUS             ; clear the address latch
  lda HDR_ADDR+1
  sta PPUADDR
  lda HDR_ADDR
  sta PPUADDR
@loop:
  lda cnt
  ora cnt+1
  beq @done
  jsr read_byte
  bcs @stop
  sta PPUDATA
  lda cnt
  bne @nolo
  dec cnt+1
@nolo:
  dec cnt
  jmp @loop
@done:
  clc
@stop:
  rts

; ===========================================================================
; The entry points we still owe
; ===========================================================================
;
; The corpus census found games calling 40 BIOS entry points. Three of them are
; implemented above, because a cold boot and a running game cannot do without
; them. The other 37 are step four, and until then each one is a stub that says
; so on the screen.
;
; This is worth the 111 bytes. Without it a game that calls a routine we have
; not written executes whatever fill byte happens to be at that address and
; wanders off, which looks like a hundred different bugs. Zelda does exactly
; that: it hands over correctly, runs its own code for a few thousand
; instructions, jumps to $EA84 and disappears into the fill. With the stub it
; says NO ROUTINE, and `fdstrace` names the address.
;
; The addresses come from docs/notes/FDS-CENSUS-2026-09-29.md, ordered as they
; sit in memory rather than by how often they are called.

.org $E149
  jmp unimplemented
.org $E153
  jmp unimplemented
.org $E161
  jmp unimplemented
.org $E16B
  jmp unimplemented
.org $E171
  jmp unimplemented
.org $E17E
  jmp unimplemented
.org $E185
  jmp unimplemented
.org $E1B2
  jmp unimplemented
.org $E1F8
  jmp unimplemented
.org $E237
  lda #0                    ; $E237 and $E239 are two apart, so this
                            ; one falls into the stub below rather
                            ; than having room for its own jmp
.org $E239
  jmp unimplemented
.org $E305
  jmp unimplemented
.org $E32A
  jmp unimplemented
.org $E3E7
  jmp unimplemented
.org $E445
  jmp unimplemented
.org $E484
  jmp unimplemented
.org $E4A0
  jmp unimplemented
.org $E4F9
  jmp unimplemented
.org $E68F
  jmp unimplemented
.org $E778
  jmp unimplemented
.org $E7BB
  jmp unimplemented
.org $E844
  jmp unimplemented
.org $E86A
  jmp unimplemented
.org $E8D2
  jmp unimplemented
.org $E8E1
  jmp unimplemented
.org $E997
  jmp unimplemented
.org $E9B1
  jmp unimplemented
.org $E9C8
  jmp unimplemented
.org $E9D3
  jmp unimplemented
.org $EA1F
  jmp unimplemented
.org $EA4C
  jmp unimplemented
.org $EA84
  jmp unimplemented
.org $EAD2
  jmp unimplemented
.org $EAEA
  jmp unimplemented
.org $EAFD
  jmp unimplemented
.org $EBAF
  jmp unimplemented
.org $EC22
  jmp unimplemented

; ===========================================================================
; $E18B: the NMI handler
; ===========================================================================
;
; Which of the three NMI pseudo-vectors gets used is chosen by $0100, and both
; that address and its encoding were measured rather than assumed.
;
; The first attempt here dispatched through $DFFA unconditionally, because all
; three disks in this repo use it. A scan of the corpus killed that: Doki Doki
; Panic uses vector 1 and Tama & Friends uses vector 2, with all three of their
; vectors distinct at the moment of dispatch. So the selection is real, and an
; unconditional dispatch would have sent both of those games to an address they
; never asked for.
;
; Finding WHICH byte selects was done by correlation, not by reading the ROM.
; Ten disks were run to their first NMI, work RAM captured at that instant, and
; every one of the 2048 bytes tested for whether its value predicts the vector.
; Exactly one address survives: $0100, holding $40, $80 and $C0 for vectors 1,
; 2 and 3. Those are bit 6 and bit 7, which is a `bit` test:
;
;   $0100   N(7) V(6)   vector
;   $C0      1    1     3   ($DFFA)   the value the BIOS leaves at boot
;   $80      1    0     2   ($DFF8)
;   $40      0    1     1   ($DFF6)
;   $00      0    0     no handler selected (never observed; we return)
;
; The cycle counts confirm the shape independently. Measured: 13 cycles for a
; vector-3 dispatch and 14 for vectors 1 and 2. The code below costs 4 + 2 + 2
; + 5 = 13 for $C0, and 14 for either of the others, because exactly one branch
; is taken instead of falling through. The branch targets also land on the same
; addresses the real handler's control flow visits. Three independent things
; agreeing is as close to certainty as a black box gets.
;
; The timing matters, which is why it is worth matching rather than
; approximating: games start timing raster splits from the moment their handler
; runs, so arriving early moves every split on the screen.
;
; Nothing here touches a register, a flag the game can see, or the stack. The
; real handler pushes nothing either (measured: the stack pointer is the same
; going in and coming out).
.org $E18B
nmi:
  bit $0100
  bpl @low                  ; bit7 clear: vector 1, or nothing at all
  bvc @vector2              ; bit7 set, bit6 clear: vector 2
  jmp (VEC_NMI3)            ; bit7 set, bit6 set: vector 3, the boot default
@vector2:
  jmp (VEC_NMI2)
@low:
  bvc @none                 ; both clear: no handler was ever selected
  jmp (VEC_NMI1)
@none:
  rti

; The layout above is not free-floating: these are the addresses the real
; handler's own control flow visits, so a game that has somehow learned them
; still lands on the right thing.
.assert @vector2 == $E195
.assert @low == $E198

; ===========================================================================
; $E1C7: the IRQ handler
; ===========================================================================
;
; $0101 selects here the way $0100 does for NMI, and the BIOS leaves it $80,
; bit 7 set. Only one vector exists to dispatch to, so the test is a single
; bit rather than a three-way split.
;
; The real handler does more than this: on a disk-transfer interrupt it services
; the drive itself and never leaves the window, which is how a game that hands
; its loading to the BIOS gets its data. That belongs with the rest of the load
; routines in step four. It is not needed yet because this BIOS generates no
; interrupts of its own: the timer is switched off at boot and the loader polls
; the drive rather than taking transfer interrupts, so every interrupt that
; arrives after handover is one the game asked for.
;
; This dispatch costs 11 cycles against the real one's measured 14. The
; difference is left rather than padded out with an instruction chosen only for
; its duration: an IRQ handler is entered on a timer the game itself programmed,
; so a fixed latency shifts every interrupt equally instead of moving one thing
; relative to another.
.org $E1C7
irq:
  bit $0101
  bpl @none
  jmp (VEC_IRQ)
@none:
  rti

; ===========================================================================
; The boot screen
; ===========================================================================
;
; Ours, and it has to be: the real BIOS draws a Nintendo wordmark and the words
; PLEASE SET DISK CARD, which is exactly the kind of thing a firmware
; replacement exists to stop shipping.
.org $F200

; --- upload the font to pattern RAM ----------------------------------------
;
; One bit plane in the image, both planes in memory: the second plane is
; written as zeros, so every glyph is colour 1 and the font costs eight bytes a
; tile instead of sixteen.
load_font:
  bit PPUSTATUS
  lda #$00
  sta PPUADDR
  sta PPUADDR             ; pattern table 0, tile 0
  lda #<font
  sta ptr
  lda #>font
  sta ptr+1
  ldx #FONT_TILES
@tile:
  ldy #0
@plane0:
  lda (ptr),y
  sta PPUDATA
  iny
  cpy #8
  bne @plane0
  lda #0
  ldy #0
@plane1:
  sta PPUDATA
  iny
  cpy #8
  bne @plane1
  lda ptr                 ; on to the next glyph
  clc
  adc #8
  sta ptr
  bcc @nocarry
  inc ptr+1
@nocarry:
  dex
  bne @tile
  rts

; --- grey-on-black palette -------------------------------------------------
load_palette:
  bit PPUSTATUS
  lda #$3F
  sta PPUADDR
  lda #$00
  sta PPUADDR
  ldx #0
@loop:
  lda palette,x
  sta PPUDATA
  inx
  cpx #8
  bne @loop
  rts

; --- clear both nametables -------------------------------------------------
clear_screen:
  bit PPUSTATUS
  lda #$20
  sta PPUADDR
  lda #$00
  sta PPUADDR
  lda #0                  ; tile 0 is blank
  ldx #8                  ; 8 x 256 bytes covers $2000-$27FF
  ldy #0
@loop:
  sta PPUDATA
  iny
  bne @loop
  dex
  bne @loop
  rts

; --- write the string at (ptr) to the nametable address in tmp/tmp+1 -------
;
; Strings are stored as font indices with $FF for the terminator, so there is
; no character translation to do at runtime.
put_string:
  bit PPUSTATUS
  lda tmp+1
  sta PPUADDR
  lda tmp
  sta PPUADDR
  ldy #0
@loop:
  lda (ptr),y
  cmp #$FF
  beq @done
  sta PPUDATA
  iny
  bne @loop
@done:
  rts

; --- show one line of status, centred on row 14 ----------------------------
;
; `A` selects the message. The screen is otherwise static, so a status change
; is one short row of writes rather than a redraw.
; Row 14, column 10. Every status string is padded to exactly STATUS_LEN
; glyphs, so writing one over another leaves nothing of the old one behind and
; the row never has to be cleared first.
STATUS_AT  = $21CA
STATUS_LEN = 12

show_status:
  asl a
  tax
  lda status_table,x
  sta ptr
  lda status_table+1,x
  sta ptr+1
  lda #<STATUS_AT
  sta tmp
  lda #>STATUS_AT
  sta tmp+1
  jmp put_string

; --- draw the whole boot screen --------------------------------------------
boot_screen:
  jsr load_font
  jsr load_palette
  jsr clear_screen

  lda #<str_title
  sta ptr
  lda #>str_title
  sta ptr+1
  lda #<$214C               ; row 10, column 12
  sta tmp
  lda #>$214C
  sta tmp+1
  jsr put_string

  lda #<str_sub
  sta ptr
  lda #>str_sub
  sta ptr+1
  lda #<$218A               ; row 12, column 10
  sta tmp
  lda #>$218A
  sta tmp+1
  jsr put_string

  rts

; --- rendering on and off --------------------------------------------------
;
; Every PPU write in this BIOS happens between a `screen_off` and a `screen_on`,
; and that rule is the whole reason these exist. A PPU that is rendering owns
; its address register, so a $2006/$2007 write made while the picture is up
; both lands in the wrong place and drags the scroll to wherever it was
; pointed. The first version of this file broke twice that way: pattern data
; scattered across 5648 bytes of Zelda's CHR RAM, and the status line written
; one character per row while the screen scrolled down to meet it.
screen_off:
  lda #0
  sta PPUCTRL
  sta PPUMASK
  rts

; --- a routine we have not written yet -------------------------------------
;
; Reached from one of the stubs. Says so and stops, rather than letting the
; machine wander. Which routine was wanted is a question for `fdstrace`, which
; names the entry address exactly; putting it on the screen would cost a hex
; printer for a number only a developer can use.
unimplemented:
  sei
  ldx #$FF
  txs
  jsr screen_off
  ; Redraw from scratch. By the time a game calls a BIOS routine it has loaded
  ; its own tiles over ours, so writing the message without putting the font
  ; back spells it out of whatever art the game happened to leave behind.
  jsr boot_screen
  lda #STATUS_NO_ROUTINE
  jsr show_status
  jsr screen_on
@forever:
  jmp @forever

screen_on:
  lda #0
  sta PPUCTRL               ; NMI off: the loader polls, it does not wait
  ; $2006 sets the scroll as well as the address, and $2000 means fine Y,
  ; coarse Y and coarse X all zero, so the top left corner without needing
  ; $2005 at all.
  bit PPUSTATUS
  lda #$20
  sta PPUADDR
  lda #$00
  sta PPUADDR
  lda #%00001010            ; background on, including the left column
  sta PPUMASK
  rts

; ===========================================================================
; $EE24: reset
; ===========================================================================
;
; The reset vector points here, and six of the 72 original titles in the census
; arrive here deliberately rather than through the CPU's reset, so the address
; is part of the interface and not just where the code happens to start.
.org $EE24
reset:
  sei
  cld
  ldx #$FF
  txs
  inx                       ; X = 0
  stx PPUCTRL
  stx PPUMASK
  stx DMC_FREQ              ; no DMC IRQ
  lda #$40
  sta APUFRAME              ; four-step frame counter, frame IRQ off
  bit PPUSTATUS

  ; The PPU is not usable until it has seen two vblanks from power-on.
@vblank1:
  bit PPUSTATUS
  bpl @vblank1

  ; Clear work RAM. Page 2 is the OAM shadow by convention, so it gets $FF,
  ; which parks every sprite below the bottom of the screen.
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

  ; Silence the APU before anything can be heard.
  lda #0
  sta APUSTATUS
  ldx #0
@quiet:
  sta $4000,x
  inx
  cpx #$14
  bne @quiet

  ; Disk I/O and sound I/O on, timer IRQ off. Nothing works before this.
  lda #$03
  sta FDS_MASTER
  lda #0
  sta FDS_IRQ_CTRL

  jsr boot_screen

  ; The drive reports all three of its status bits active low, so a set bit0
  ; means no disk is in. With one already in, which is the normal case for an
  ; emulator, this says nothing and goes straight on.
  lda FDS_DRIVE
  and #$01
  beq @have_disk

  lda #STATUS_NO_DISK
  jsr show_status
  jsr screen_on
@wait_disk:
  lda FDS_DRIVE
  and #$01
  bne @wait_disk
  jsr screen_off

@have_disk:
  lda #STATUS_LOADING
  jsr show_status
  jsr screen_on

  ; Hold the screen long enough to be read. The real BIOS sits on its own for
  ; about three seconds before it starts; a second and a half is enough to see
  ; and still leaves this boot several times quicker than the original.
  ldx #90
@hold:
  bit PPUSTATUS
  bpl @hold
@held:
  bit PPUSTATUS
  bmi @held
  dex
  bne @hold

  ; Rendering off for the load, which is what the real BIOS does too: its own
  ; screen is long gone by the time it hands over. Wipe our text with it, so the
  ; only thing in the nametables when the game starts is what the disk put
  ; there. A file of kind 2 loads straight into this memory, and a game is
  ; entitled to find its own licence screen and nothing else.
  jsr screen_off
  jsr clear_screen
  jsr load_disk
  bcs fail

  ; Rendering off before the game gets the machine, so it inherits a PPU it can
  ; set up from scratch rather than one mid-frame with our screen on it.
  jsr screen_off

  ; Leave the work RAM we borrowed the way we found it.
  ldx #SCRATCH_LO
  lda #0
@wipe_zp:
  sta $00,x
  inx
  cpx #SCRATCH_HI
  bne @wipe_zp
  ldx #0
@wipe_hdr:
  sta hdr,x
  inx
  cpx #16
  bne @wipe_hdr

  ; ---- the BIOS-owned RAM that has a meaning ----
  ;
  ; $0100 chooses which of the three NMI pseudo-vectors the handler dispatches
  ; through, and $C0 is the value the real BIOS leaves: bits 7 and 6 both set,
  ; which selects vector 3. Most games never touch it. See the NMI handler for
  ; how the encoding was found.
  lda #$C0
  sta $0100
  ; $0101 does the same job for the IRQ handler; $80 is the value the real BIOS
  ; leaves, and it means "dispatch to the game".
  lda #$80
  sta $0101
  ; The signature a warm boot looks for to tell itself the machine is already
  ; initialised.
  lda #$35
  sta $0102
  lda #$AC
  sta $0103

  ; ---- handover ----
  ;
  ; Measured from the real BIOS and identical on every disk tried: a=$10,
  ; x=$00, y=$FF, sp=$FF, p=$20. p=$20 means every flag clear with interrupts
  ; ENABLED, so the order below matters: the loads come last because they set
  ; N and Z, and $10 leaves both clear.
  ldx #$FF
  txs
  cld
  clc
  clv
  cli
  ldy #$FF
  ldx #$00
  lda #$10
  jmp (VEC_RESET)

; A disk that cannot be read is a dead end, not a crash. Say so and stop; the
; player can eject and try another, which re-runs this from the top.
fail:
  jsr screen_off
  lda #STATUS_BAD_DISK
  jsr show_status
  jsr screen_on
@forever:
  jmp @forever

; ===========================================================================
; The loader
; ===========================================================================
.org $F600
;
; Every routine here returns carry set if the disk could not be read, and the
; whole thing is cut into short pieces for one dull reason: the failure branch
; has to reach its handler in a signed byte. Written as one routine the error
; exits sat 164 bytes from the branches that wanted them.
;
; Carry set on return means the disk could not be read.
load_disk:
  ; Stop the motor before starting it. Only the off-to-on edge rewinds the head
  ; to the start of the side, so a loader entered with the drive already
  ; spinning would begin reading from wherever the head was left and see the
  ; middle of a file as a block header. That happens for real: a game that
  ; jumps back to the reset entry gets a second load.
  lda #CTRL_STOP
  sta FDS_CTRL
  lda #CTRL_IDLE
  sta FDS_CTRL
@spinup:
  lda FDS_DRIVE
  and #$03                  ; bit0 no disk, bit1 not ready
  bne @spinup

  jsr read_info
  bcs @bad
  jsr read_count
  bcs @bad

@each_file:
  lda files
  beq @done
  dec files
  jsr load_one_file
  bcc @each_file
@bad:
  sec
  rts

@done:
  ; Stop the transfer but leave the motor turning, which is the state the drive
  ; is in when a game takes over and starts driving it itself.
  lda #CTRL_IDLE
  sta FDS_CTRL
  clc
  rts

; --- block 1: the 56-byte disk info. Only byte $19 is wanted ---------------
;
; That byte is the boot read file code, and finding it there rather than at $1A
; is the measurement the file-selection rule rests on. See the header of this
; file.
read_info:
  jsr next_block
  jsr read_byte
  bcs @bad
  cmp #$01                  ; block type
  bne @bad
  ldy #1
@byte:
  jsr read_byte
  bcs @bad
  cpy #$19
  bne @not_code
  sta bootcode
@not_code:
  iny
  cpy #56
  bne @byte
  clc
  rts
@bad:
  sec
  rts

; --- block 2: how many files the side holds --------------------------------
read_count:
  jsr next_block
  jsr read_byte
  bcs @bad
  cmp #$02
  bne @bad
  jsr read_byte
  bcs @bad
  sta files
  clc
  rts
@bad:
  sec
  rts

; --- one file: its header, then its body -----------------------------------
load_one_file:
  jsr read_header
  bcs @bad

  ; A file is ours if its ID is at most the boot read file code. Measured, not
  ; assumed: see the header of this file. `cmp` leaves carry set when the boot
  ; code is the larger, and that includes the two being equal, which is the
  ; half of the rule Falsion settled.
  lda #0
  sta wanted
  lda bootcode
  cmp HDR_ID
  bcc @body                 ; boot code below the file ID: not one of ours
  lda #1
  sta wanted

@body:
  jsr next_block
  jsr read_byte
  bcs @bad
  cmp #$04
  bne @bad

  lda HDR_SIZE
  sta cnt
  lda HDR_SIZE+1
  sta cnt+1

  lda wanted
  beq @discard

  lda HDR_ADDR
  sta ptr
  lda HDR_ADDR+1
  sta ptr+1
  lda HDR_KIND
  beq @to_prg               ; kind 0: program RAM
  jmp read_to_ppu           ; kind 1 pattern RAM, 2 nametable: both PPU space
@to_prg:
  jmp read_to_ptr
@discard:
  jmp skip_bytes
@bad:
  sec
  rts

; --- block 3: the 16-byte file header --------------------------------------
;
; Read whole into RAM so its fields can be picked out by index afterwards,
; rather than by a chain of compares inside the read loop.
read_header:
  jsr next_block
  jsr read_byte
  bcs @bad
  cmp #$03
  bne @bad
  sta hdr
  ldy #1
@byte:
  jsr read_byte
  bcs @bad
  sta hdr,y
  iny
  cpy #16
  bne @byte
  clc
  rts
@bad:
  sec
  rts


; ===========================================================================
; Data
; ===========================================================================
.org $F900

palette:
  .byte $0F, $30, $10, $00      ; black, white, grey, black
  .byte $0F, $30, $10, $00

; Status messages, indexed by the constants below.
STATUS_NO_DISK    = 0
STATUS_LOADING    = 1
STATUS_BAD_DISK   = 2
STATUS_NO_ROUTINE = 3

status_table:
  .word str_no_disk
  .word str_loading
  .word str_bad_disk
  .word str_no_routine

; Strings are font indices, terminated by $FF. Tile 0 is blank, so a space is
; a zero and the terminator cannot be confused with one.
GL_SPACE = 0

str_title:
  .byte 6, 1, 13, 9, 18, 21, 19, 20, $FF        ; FAMIRUST
str_sub:
  .byte 4, 9, 19, 11, 0, 19, 25, 19, 20, 5, 13, $FF   ; DISK SYSTEM
str_no_disk:
  .byte 9, 14, 19, 5, 18, 20, 0, 4, 9, 19, 11, 0, $FF     ; INSERT DISK
str_loading:
  .byte 0, 0, 12, 15, 1, 4, 9, 14, 7, 0, 0, 0, $FF        ; LOADING
str_bad_disk:
  .byte 0, 4, 9, 19, 11, 0, 5, 18, 18, 15, 18, 0, $FF     ; DISK ERROR

str_no_routine:
  .byte 0, 14, 15, 0, 18, 15, 21, 20, 9, 14, 5, 0, $FF     ; NO ROUTINE

; They are all the same width, so one overwrites another completely.
.assert (str_loading - str_no_disk) == STATUS_LEN + 1
.assert (str_bad_disk - str_loading) == STATUS_LEN + 1
.assert (str_no_routine - str_bad_disk) == STATUS_LEN + 1

; ---------------------------------------------------------------------------
; The font
; ---------------------------------------------------------------------------
;
; Tile 0 blank, tiles 1-26 A to Z, tiles 27-36 the digits. One bit plane, eight
; bytes a glyph; `load_font` writes the second plane as zeros.
.org $FA00
font:
  ; 0: blank
  .byte %00000000,%00000000,%00000000,%00000000,%00000000,%00000000,%00000000,%00000000
  ; A
  .byte %00111000,%01101100,%11000110,%11000110,%11111110,%11000110,%11000110,%00000000
  ; B
  .byte %11111100,%11000110,%11000110,%11111100,%11000110,%11000110,%11111100,%00000000
  ; C
  .byte %00111100,%01100110,%11000000,%11000000,%11000000,%01100110,%00111100,%00000000
  ; D
  .byte %11111000,%11001100,%11000110,%11000110,%11000110,%11001100,%11111000,%00000000
  ; E
  .byte %11111110,%11000000,%11000000,%11111000,%11000000,%11000000,%11111110,%00000000
  ; F
  .byte %11111110,%11000000,%11000000,%11111000,%11000000,%11000000,%11000000,%00000000
  ; G
  .byte %00111100,%01100110,%11000000,%11001110,%11000110,%01100110,%00111110,%00000000
  ; H
  .byte %11000110,%11000110,%11000110,%11111110,%11000110,%11000110,%11000110,%00000000
  ; I
  .byte %01111100,%00011000,%00011000,%00011000,%00011000,%00011000,%01111100,%00000000
  ; J
  .byte %00011110,%00001100,%00001100,%00001100,%11001100,%11001100,%01111000,%00000000
  ; K
  .byte %11000110,%11001100,%11011000,%11110000,%11011000,%11001100,%11000110,%00000000
  ; L
  .byte %11000000,%11000000,%11000000,%11000000,%11000000,%11000000,%11111110,%00000000
  ; M
  .byte %11000110,%11101110,%11111110,%11010110,%11000110,%11000110,%11000110,%00000000
  ; N
  .byte %11000110,%11100110,%11110110,%11011110,%11001110,%11000110,%11000110,%00000000
  ; O
  .byte %00111000,%01101100,%11000110,%11000110,%11000110,%01101100,%00111000,%00000000
  ; P
  .byte %11111100,%11000110,%11000110,%11111100,%11000000,%11000000,%11000000,%00000000
  ; Q
  .byte %00111000,%01101100,%11000110,%11000110,%11010110,%01101100,%00111010,%00000000
  ; R
  .byte %11111100,%11000110,%11000110,%11111100,%11011000,%11001100,%11000110,%00000000
  ; S
  .byte %01111100,%11000110,%11000000,%01111100,%00000110,%11000110,%01111100,%00000000
  ; T
  .byte %11111110,%00011000,%00011000,%00011000,%00011000,%00011000,%00011000,%00000000
  ; U
  .byte %11000110,%11000110,%11000110,%11000110,%11000110,%11000110,%01111100,%00000000
  ; V
  .byte %11000110,%11000110,%11000110,%11000110,%11000110,%01101100,%00111000,%00000000
  ; W
  .byte %11000110,%11000110,%11000110,%11010110,%11111110,%11101110,%11000110,%00000000
  ; X
  .byte %11000110,%01101100,%00111000,%00111000,%00111000,%01101100,%11000110,%00000000
  ; Y
  .byte %11000110,%11000110,%01101100,%00111000,%00011000,%00011000,%00011000,%00000000
  ; Z
  .byte %11111110,%00001100,%00011000,%00110000,%01100000,%11000000,%11111110,%00000000
  ; 0
  .byte %01111100,%11000110,%11001110,%11010110,%11100110,%11000110,%01111100,%00000000
  ; 1
  .byte %00011000,%00111000,%01111000,%00011000,%00011000,%00011000,%01111110,%00000000
  ; 2
  .byte %01111100,%11000110,%00000110,%00011100,%01110000,%11000000,%11111110,%00000000
  ; 3
  .byte %01111100,%11000110,%00000110,%00111100,%00000110,%11000110,%01111100,%00000000
  ; 4
  .byte %00001100,%00011100,%00111100,%01101100,%11111110,%00001100,%00001100,%00000000
  ; 5
  .byte %11111110,%11000000,%11111100,%00000110,%00000110,%11000110,%01111100,%00000000
  ; 6
  .byte %00111100,%01100000,%11000000,%11111100,%11000110,%11000110,%01111100,%00000000
  ; 7
  .byte %11111110,%00000110,%00001100,%00011000,%00110000,%00110000,%00110000,%00000000
  ; 8
  .byte %01111100,%11000110,%11000110,%01111100,%11000110,%11000110,%01111100,%00000000
  ; 9
  .byte %01111100,%11000110,%11000110,%01111110,%00000110,%00001100,%01111000,%00000000
font_end:
FONT_BYTES = font_end - font
FONT_TILES = FONT_BYTES / 8

; The font has to be a whole number of glyphs, or the last one is uploaded from
; whatever bytes follow it.
.assert (FONT_TILES * 8) == FONT_BYTES
; And it has to fit in a byte, because X counts the glyphs.
.assert FONT_TILES <= 255

; ===========================================================================
; Vectors
; ===========================================================================
.org $FFFA
  .word nmi
  .word reset
  .word irq
