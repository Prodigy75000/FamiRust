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

; fill=$02 is a JAM opcode, deliberately. The gaps between routines are what a
; game lands on when it calls one of the entry points we have not written and
; the census never saw, so there is no stub there to say NO ROUTINE. The
; default $FF is a three-byte illegal opcode: Kaettekita Mario Bros. jumped
; into the gap at $E3xx, walked twelve instructions through it, and jammed at
; $E3EA on a byte belonging to an unrelated stub's operand, which names nothing
; useful. A JAM stops on the spot, so `fdstrace` reports the address the game
; actually asked for.
.raw base=$E000 size=8192 fill=$02

; ---------------------------------------------------------------------------
; Hardware
; ---------------------------------------------------------------------------
PPUCTRL   = $2000
PPUMASK   = $2001
PPUSTATUS = $2002
OAMADDR   = $2003
OAMDMA    = $4014
PPUSCROLL = $2005
PPUADDR   = $2006
PPUDATA   = $2007

; The queued VRAM transfer buffer: $E8D2 appends to it and $E86A empties it.
; $0301 is the write index, which $E8D2 commits and $E86A resets to zero.
; $0300 is how many bytes the buffer holds, which the game sets and the BIOS
; only reads: the corpus has it at $4E and $5F on different titles.
BUF       = $0302
JOY1      = $4016
JOY2      = $4017
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
booted    = $E9           ; files loaded so far; none means a disk with no boot file

SCRATCH_LO = $E0
SCRATCH_HI = $EA          ; one past the last scratch byte

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

; --- $E149: delay exactly 131 cycles ---------------------------------------
;
; The most-called routine in the corpus by a wide margin: 1,054,357 calls
; across 17 titles, and the single biggest blocker, with 15 titles stopped on
; it. Falsion calls it about 64 times a frame, Green Beret 57, and Suishou no
; Dragon from 42 distinct sites. At 131 cycles it is about 1.15 scanlines, which
; is the shape of a timing quantum that games multiply rather than a routine
; that does anything.
;
; It does nothing but take time. The entire bus activity of one real call is
; the `pha` of a, the `pla` reading it back, and the `rts`: no zero page, no
; hardware, nothing. Ours has to be the same, because a routine that runs a
; million times and dirties one cell is a corpus-wide bug.
;
; **This is the one routine where "faster is safe" is wrong.** Everywhere else
; in this BIOS we are happy to beat the original, because a game that budgeted
; for the slow version still fits. Here the duration IS the product: a game
; counting these to pace a PPU access gets a shorter wait than it asked for.
; So 131 exactly, measured min=max over 48 calls on six titles.
;
; It also has to be decimal-proof. Forced into the call with D=1 the real one
; still took 131, so whatever counts inside it is not an `adc`/`sbc` chain.
; Ours shifts instead, which cannot care.
;
; Exit: a, x and y preserved, N and Z from a, D and I preserved, and C and V
; forced CLEAR whatever came in (p=$61 in came back $20).
.org $E149
  jmp delay_131             ; 3 of the 131; the body will not fit in the ten
                            ; bytes before the next entry point

.org $FB28
delay_131:
  pha                       ; 3   the one bus write the real one makes
  lda #$80                  ; 2
@shift:
  lsr a                     ; 2   eight shifts, seven of them branching back:
  bne @shift                ; 3   7*(2+3) + (2+2) = 39, and no arithmetic, so
                            ;     decimal mode cannot change the count
  clc                       ; 2   measured clear on exit regardless of entry
  clv                       ; 2
  .byte $EA, $EA, $EA, $EA, $EA, $EA, $EA  ; 7 nops, 14 cycles
  .byte $EA, $EA, $EA, $EA, $EA, $EA, $EA  ; 7 nops, 14 cycles
  .byte $EA, $EA, $EA, $EA, $EA, $EA, $EA  ; 7 nops, 14 cycles
  .byte $EA, $EA, $EA, $EA, $EA, $EA, $EA  ; 7 nops, 14 cycles
  .byte $EA, $EA, $EA, $EA, $EA, $EA, $EA  ; 7 nops, 14 cycles
  pla                       ; 4   restores a and sets N and Z from it
  rts                       ; 6
; 3 + 3 + 2 + 39 + 2 + 2 + 70 + 4 + 6 = 131
.assert (delay_131_end - delay_131) == 45
delay_131_end:
; --- $E153: delay y milliseconds -------------------------------------------
;
; Four titles were stopped on it: 19 - Neunzehn, Lutter, and both Super Lode
; Runners. It takes y and comes back when that many milliseconds have gone.
;
;   cycles = 5 + 1790*y, with y=0 meaning 256
;
; 1790 cycles is 1.0001 ms on NTSC, which is what makes "milliseconds" the
; right word rather than a coincidence. Measured exactly: forced y=$01 gave
; 1795, y=$02 3585, y=$0A 17905. Interrupts taken during the wait are on top,
; since it counts cycles rather than watching a clock.
;
; **This is a delay, so the duration IS the product and faster is NOT safe**,
; the same rule as $E149. The count has to be exact in both directions.
;
; That is what forces the shape below. `a` is preserved and the routine makes
; NO stack writes at all, so there is no `pha` available and `a` cannot be
; borrowed as a counter: `x` and `y` are the only registers in play and `y` is
; already the argument. One 8-bit inner loop therefore covers 1790 cycles on
; its own, and the compare that gives the carry sits inside it:
;
;   2 (ldx) + [8*223 - 1] + 2 (dey) + 3 (bne) = 1790
;
; Entry overhead has to be ZERO for the total to come out at 5 + 1790*y, so
; the loop starts on the entry point itself with nothing in front of it.
;
; Exit: `a` preserved, `x` and `y` both $00, N=0 and Z=1 from the final `dey`,
; and **C = (a >= [$00])**, the carry of the `cmp $00` in the loop. V, D and I
; are preserved. Decimal-proof by construction: nothing here adds.
;
; **The carry took four goes.** "C=0 always" came from a live Lutter taking
; interrupts mid-call, so its handler altered the pushed P. "C=1" came from a
; probe that happened to hold `a`=$5A. "a >= $20" survived two rounds, because
; the real BIOS leaves $20 at $0000 at handover and no test planted it. The
; original's repeated reads of $0000 while it waits ARE the compare: forcing
; $00 changed no cycle count, which is why they were first taken for dummy
; reads, but it does change the carry. Super Lode Runner calls this with
; a=$10 over a $00 below it and branches on the answer.
.org $E153
delay_ms:
  ldx #223                  ; 2
@inner:
  cmp $00                   ; 3   carry out is (a >= [$00]), measured
  dex                       ; 2
  bne @inner                ; 3, and 2 on the last pass
  dey                       ; 2
  bne delay_ms              ; 3, and 2 on the last pass. y=0 wraps to 255 and
                            ;    so runs 256 times, which is the contract
  rts                       ; 6
.assert (delay_ms_end - delay_ms) <= 14
delay_ms_end:
; --- $E161 $E16B $E171 $E17E $E185: the PPUMASK family ----------------------
;
; Five entry points that are one routine with five masks. $E161 alone stops 10
; titles and was top of the queue; the other four came nearly free with it.
;
; Each reads the PPUMASK shadow at $FE, transforms it, and writes the result to
; the shadow FIRST and then to $2001. They never read $2002, so they are
; callable mid-frame and what a mid-frame $2001 write does to the picture is
; the caller's problem, not ours.
;
; They are masks, not constants: emphasis bits 5-7 and the left-column bits
; 0-2 pass straight through. Bubble Bobble's natural $26 comes back $26, which
; is what proved it. Forcing the caller's shadow to $00 and to $FF separated
; the five transforms:
;
;   entry   $00 gives   $FF gives   transform           cycles
;   $E161   $00         $E7         and #%11100111      18   screen off
;   $E16B   $18         $FF         ora #%00011000      21   screen on
;   $E171   $00         $EF         and #%11101111      21   sprites off
;   $E17E   $00         $F7         and #%11110111      21   background off
;   $E185   $08         $FF         ora #%00001000      21   background on
;
; No arguments in any register: forced a=$FF x=$AA y=$55 changed nothing, and
; the 13 callers of $E161 enter with freely varying registers. No inline
; arguments either, so the `rts` goes to jsr+3. Exit: a is the masked result
; and always clobbered, x and y are preserved, N and Z come from the result,
; and C, V, D and I are ALL preserved (p=$61 in came back $61; p=$E9 with a
; zero result came back $6B).
;
; They make no stack writes at all, not even a `pha`, so the bytes below the
; returned stack pointer come back exactly as the caller left them. The whole
; bus activity of one call is: read $FE, write $FE, write $2001, then the `rts`
; reads. Ours matches that byte for byte.
;
; Cycles were min=max across every caller and every forced state including
; D=1, so there is nothing conditional inside. $E161's body is ten bytes and
; $E16B is ten bytes away, so it goes inline with no trampoline. The other
; four have six, thirteen, seven and six bytes of room, which buys `lda #mask`
; plus a `jmp` to one of two shared 16-cycle tails: 2 + 3 + 16 = 21, exactly.
;
; The set is conspicuously missing a "sprites on" (ora #%00010000). If it
; exists it is in the 13-byte $E171-$E17D gap, but no corpus title enters
; anything there, so black-box observation cannot locate it and we owe nothing.
.org $E161
mask_off:
  lda $FE                   ; 3
  and #%11100111            ; 2   clears background and sprite enable, keeps
  sta $FE                   ; 3   emphasis and the left-column bits
  sta PPUMASK               ; 4
  rts                       ; 6
; 3 + 2 + 3 + 4 + 6 = 18
.assert (mask_off_end - mask_off) == 10
mask_off_end:
.assert mask_off_end == $E16B
.org $E16B
  lda #%00011000            ; 2   screen on
  jmp mask_or               ; 3
.org $E171
  lda #%11101111            ; sprites off
  jmp mask_and
; $E178, sprites on: seven titles, among them Cocona World and Santa Claus no
; Takarabako, which call it after every metasprite draw. Six bytes between its
; neighbours, five used. Like the other four it reads $FE on cycle 8 where the
; original reads it on cycle 3; values and every write cycle agree.
.org $E178
  lda #%00010000
  jmp mask_or
.org $E17E
  lda #%11110111            ; background off
  jmp mask_and
.org $E185
  lda #%00001000            ; background on
  jmp mask_or

; The two tails, sixteen cycles each. They cannot share a store: folding them
; into one `sta` run would cost the `or` side or the `and` side an extra jmp
; and break the 21.
.org $FB55
mask_or:
  ora $FE                   ; 3
  sta $FE                   ; 3
  sta PPUMASK               ; 4
  rts                       ; 6
mask_and:
  and $FE                   ; 3
  sta $FE                   ; 3
  sta PPUMASK               ; 4
  rts                       ; 6
.assert (mask_and - mask_or) == 8
.assert (mask_tails_end - mask_or) == 16
mask_tails_end:
; --- $E1B2: wait for the next vblank NMI -----------------------------------
;
; 18 of the 114 corpus titles call it, it is the first thing Adian no Tsue asks
; for, and four titles are stopped on it outright.
;
; Not a disk routine, despite a 27,870-cycle reading from the census that looked
; like one. Across all 18 callers the cost runs 256 to 29,773 cycles and never
; exceeds one NTSC frame, which is the shape of "until the next vblank" rather
; than of any transfer. It touches nothing in $4020-$40FF at all.
;
; It parks the caller on the stack, enables NMI through the $FF shadow, and
; dead-spins. The spin is a spin and not a poll: zero external accesses happen
; during the wait, so it never looks at $2002. The NMI handler above finds the
; selector at zero, switches NMI back off, drops the interrupt frame and
; returns straight past this routine to whoever called it.
;
; Takes nothing, cannot fail, and gives back a, x and y untouched. Carry and
; decimal survive, overflow is cleared by the handler's `bit`, and interrupts
; come back DISABLED because the NMI's own I is never cleared.
;
; The spin has to be at exactly $E1C5. That is the interrupted PC left on the
; stack by every measured call, and it is also one byte short of the IRQ entry
; at $E1C7, so there is room for the branch and nothing else.
.org $E1B2
vint_wait:
  pha                       ; a comes back untouched; this is where it waits
  lda $0100
  pha                       ; saved and restored, not reset: a caller entering
                            ; with $0100=$80 came back out with $80
  lda #$00
  sta $0100                 ; zero means "the NMI handler owns this exit"
  lda $FF
  ora #$80                  ; NMI on; forced $FF=$7F wrote $FF, so bit 7 is
  sta $FF                   ; the only bit this adds
  sta PPUCTRL
@spin:
  bmi @spin                 ; a is shadow|$80, so N is set and this is taken
                            ; forever; `rti` restores flags, so a game IRQ
                            ; landing mid-wait resumes the spin correctly
.assert @spin == $E1C5
.org $E237
  lda #$FF                  ; $E237: append. Two bytes, then $E239
.org $E239
  jmp write_file            ; the body is at $F6E0
.org $E23C
; --- $E2BB: drop the side's file count by a ---------------------------------
;
; Twenty-three disks save with it: Ultraman 2, Dirty Pair, both Deep Dungeons,
; Super Boy Allan, Gun Fighter. Each is the same idiom, after a load that found
; the old save: `lda #1 / jsr $E2BB / .word template / beq`, then $E237 appends
; the new one. Forget the last file, then write it again.
;
; $E305's entry and $E305's write, with a reading pass in front: block 1 and
; the ID checked against the template, block 2's count read off the medium,
; the motor stopped, and then $E305's whole attempt with count - a. A borrow is
; error $31 with y = $23 and nothing written. Measured by a helper and fuzzed
; against the original on the Ultraman 2 call: 900 cases.
;
; Its exits are $E305's with three differences, all measured: $09 is cleared at
; entry and so on every exit; the pre-motor exits leave $07 as found; a flat
; battery on the reading pass keeps the caller's V.
adjust_count:
  ldx #$00
@common:                    ; $E2B7 enters here with x = $FF
  stx $09
  sta $02                   ; the operand, or $FF for "it is inline"
  tsx
  dex
  dex
  dex
  stx $04
  pla
  sta $05
  pla
  sta $06
  ldy #1
  lda ($05),y
  sta $00
  iny
  lda ($05),y
  sta $01
  ldx #2
  lda $02
  cmp #$FF
  bne @one
  iny
  lda ($05),y
  sta $02
  iny
  lda ($05),y
  sta $03
  ldx #4
@one:
  php
  txa
  clc
  adc $05
  tax
  lda $06
  adc #0
  plp
  pha
  txa
  pha
  lda FDS_DRIVE
  lsr a
  bcc @disk
  lda #$01
  bne @early
@disk:
  lsr a
  lsr a
  bcc @go
  lda #$03
@early:
  tax
  ldy $0E
  cli
  clc
  txa
  rts
@go:
  lda $0101
  pha
  lda $09
  pha
  lda $09                   ; $00 for $E2BB, $FF for $E2B7
  and #$20
  ora #$80                  ; the reading pass, not yet retried; $06 keeps
  sta $05                   ; the return address high, as measured
  jmp set_count_c@attempt
@under:
  lda #$02
  sta $05
  ldy #$23
  lda #$31                  ; C clear and V from the subtraction
  jmp set_count_c@tail
@entry_end:
.assert @entry_end <= $E2B7     ; $E2B7 is an entry point of its own

; $E2B7, seven disks' save code: $E2BB's check, but the count becomes the
; operand itself rather than count less it, and $09 ends $FF, not $00.
.org $E2B7
  ldx #$FF
  bne @common

.org $E2BB
  jmp adjust_count

; Block 1 has passed; read block 2's count instead of writing it.
@read:
  lda #$02
  sta $07
  jsr lf_next
  jsr read_byte
  cmp #$02                  ; carry: type >= 2, as measured
  beq @counted
  lda #$23
  ldy #$23
  jmp set_count_c@fail        ; retried once, as every first-pass failure
@counted:
  jsr read_byte
  pha
  jsr wf_crc
  lda LF_SHADOW
  and #$08
  ora #$27
  sta FDS_CTRL              ; the motor stops between the passes
  pla
  sta $06                   ; the old count, which a borrow leaves here
  sec
  sbc $02
  bcc @under
  tax
  lda $05
  and #$20
  beq @less
  ldx $02                   ; $E2B7: the operand itself
@less:
  stx $06                   ; the new count, for $E305's write
  lda #$40
  sta $05                   ; the writing pass, its own retry fresh
  jmp set_count_c@attempt
adjust_count_end:
.assert adjust_count_end <= $E301     ; and so is $E301

.org $E7BE
; $E4F9: header bytes 11-15, then the body. $0A/$0B the address, $0C/$0D the
; size less one, which counts down to $FFFF. A kind-0 file is REFUSED if it
; would wrap past $FFFF or land in the first two pages of a 2K mirror below
; $2000; that check comes first and sets y = $FF even when the file is then
; skipped. Skipped ($09 nonzero): y = $09, and the body has no status checks
; at all. Loaded: y = 0, and the block end's errors return C clear. A PPU
; kind blanks the screen first. Header errors return y = 4.
pf_e4f9:
  lda #$40
  sta $0101
  ldx #$00
@hdr:
  lda LF_SHADOW
  ora #$80                  ; before each byte, as the original
  jsr pf_ctrl
  jsr pf_rd
  sta LF_DST,x
  inx
  cpx #$04
  bne @hdr
  jsr pf_rd                 ; the kind
  pha
  lda LF_CNT
  bne @lo
  dec LF_CNT+1
@lo:
  dec LF_CNT
  ldy #$04
  jsr pf_end1
  lda #$04
  jsr pf_arm
  ldy #$00
  pla
  tax                       ; the kind, to the end
  bne @checked              ; a PPU kind has no range check
  lda LF_DST
  clc
  adc LF_CNT
  lda LF_DST+1
  adc LF_CNT+1
  bcs @refuse               ; it would wrap
  lda LF_DST+1
  cmp #$20
  bcs @checked
  and #$07
  cmp #$02
  bcs @checked
@refuse:
  ldy #$FF
@checked:
  lda LF_SKIP
  bne @skipping
  jmp pf_take               ; the second half is at $E8E4: 199 bytes fit in
@skipping:                  ; no one gap once $E2BB needed this one
  cpy #$FF
  beq @skip
  tay                       ; skipped: y = $09
@skip:
  jsr pf_rd
  jsr pf_down
  bne @skip
  jsr pf_rd                 ; the CRC, as data, and no checks
  jsr pf_rd
  jsr pf_off
  jmp pf_ok
pf_e4f9_a_end:
.assert pf_e4f9_a_end <= $E844

.org $E8E4
pf_take:
  cpy #$FF
  beq @drop                 ; refused: read through, then the block end
  txa
  bne @ppu
@ram:
  jsr pf_rd
  sta (LF_DST),y
  inc LF_DST
  bne @nowrap
  inc LF_DST+1
@nowrap:
  jsr pf_down
  bne @ram
  beq @end
@ppu:
  lda LF_MASK
  and #%11100111
  sta LF_MASK
  sta PPUMASK
  bit PPUSTATUS
  lda LF_DST+1
  sta PPUADDR
  lda LF_DST
  sta PPUADDR
@vram:
  jsr pf_rd
  sta PPUDATA
  jsr pf_down
  bne @vram
  beq @end
@drop:
  jsr pf_rd
  jsr pf_down
  bne @drop
@end:
  ldx #$00                  ; the body's end errors return C clear
  jsr pf_end
  jmp pf_ok

pf_body_end:
.assert pf_body_end <= queue_step

.org $E301
  jmp set_count1
.org $E305
  jmp set_count             ; the body is at $E4FC
.org $E32A
  jmp disk_info             ; the body is at $FA00, its blocks at $EF07
.org $E3E7
; --- $E3E7: fetch the inline arguments of the routine that called us, and ---
; --- check a disk is in --------------------------------------------------------
;
; 1 of the 114 corpus titles calls it: Tobidase Daisakusen, `jsr $E3E7` from
; $62CA with a=$FF, inside a routine of its own that is called as `jsr G`
; followed by its arguments. Like $E844 this reaches one level up the stack, to
; G's return address, but it takes four bytes when a is $FF and two otherwise,
; and it reads $4032 before it returns.
;
; Measured by a helper with a cycle number on every access, so the shape below
; is fixed by where each access lands, dummy reads included:
;
;   * $04 = SP-1 at entry, kept to index the return address after the copy.
;   * a php at $0100+SP is left below the stack and popped again; the pushed
;     C is the sec's, which is why the final bcs is always taken.
;   * $02 = a as given; a then survives the pointer fetch, so the fetch goes
;     through y. The pointer lands in $05/$06 and stays there.
;   * the bytes after G's jsr go to $00/$01, and $02/$03 too when a is $FF.
;     G's return address steps over them with the carry rippled.
;   * $4032 bit 0 only, write protect is ignored. A disk: a=0, x=1, y=2 or 4,
;     back to G. No disk: a=1, x=1, y=[$0E], I cleared, and our return
;     address is dropped, so G's caller gets the error past G's arguments.
;
; With SP at entry $FC or above, abs,x reaches page 2 rather than wrapping on
; the stack, as the original's does; the same addressing modes give that for
; free. 107 cycles, +21 for four bytes, +13 for no disk, plus one for each
; page crossed. Matched against the original on 3000 fuzzed calls.
fetch_args:
  sec
  bcs @body                 ; 3 cycles the original spends here
@body:
  tsx
  dex                       ; N/Z of this end up in the pushed P
  stx $04
  php
  sta $02
  ldy $0104,x               ; G's return address, low
  sty $05
  ldy $0105,x               ; and high
  sty $06
  ldy #$01
  cmp #$FF                  ; C set: four bytes
  lda ($05),y
  sta $00
  iny
  lda ($05),y
  sta $01
  nop                       ; 4 cycles with nothing observable
  nop
  bcc @two
  iny
  lda ($05),y
  sta $02
  iny
  lda ($05),y
  sta $03
  clc
@two:
  ldx $04
  tya                       ; y is the count, and C is clear on both paths
  adc $05
  sta $0104,x
  lda #$00
  adc $06
  sta $0105,x
  plp
  ldx #$01
  lda $4032
  and #$01
  bne @nodisk
  bcs @done                 ; always taken: the pushed C is the sec's
@done:
  rts
@nodisk:
  pla                       ; drop our own return address
  pla
  ldy $0E
  txa                       ; a = $01, no disk
  cli
  rts
; ===========================================================================
; $E445 $E484 $E68F $E4A0 $E4F9 $E778: the loader in pieces
; ===========================================================================
;
; Tobidase Daisakusen does not call $E1F8. It carries its own copy of it, one
; that walks one file past the count to reach a hidden ninth, built from these
; six entry points: $E3E7 for its arguments, then $E445 (spin up and check
; block 1), $E484 (block 2), and per file $E68F (open a block of type a),
; $E4A0 (header bytes 1-10, the ID against the list) and $E4F9 (the rest of
; the header and the body), then $E778 to coast. The five Namco titles that
; load through their own code at $0514 call $E778 as well.
;
; Measured by a helper, one call at a time and chained the way a loader
; chains them, against a model fuzzed on 2200 cases and 14402 calls with 0
; mismatches. What they share:
;
;   * an error does not return to the caller: S goes back to [$04], the
;     frame $E3E7 left, and the rts lands past the game's own inline bytes,
;     with a = x = the code, N and Z from it, I clear, D kept, C per check,
;     and V set if the call read any byte of the disk. $FA = ($FA & 9) | $26.
;   * $FA is ORed and ANDed a step at a time, never rebuilt: arm sets $40
;     then $80, CRC mode $10, disarm keeps $2F. We write $4025 from it with
;     the byte IRQ bit CLEAR, since we poll: the original leaves the drive
;     armed with I clear between $E4A0 and $E4F9, and our IRQ handler would
;     return into a level interrupt forever.
;   * a block ends on one CRC byte and end of head ($28), CRC mode and the
;     other byte and CRC ($27), disarm, then no disk ($01). Success returns
;     $4032 shifted right in a, x = 1, V and I set.
;
; Every one but $E778 drops the original's counted delays, so they are
; faster; $E778 is 29 cycles, as the original's.
PF_TYPE   = $07           ; the block type expected
.org $E445
  jmp pf_e445               ; the body is at $FF28

; ---- the exits, and a $4025 write with the byte IRQ off --------------------

; a = the code, y and C as the error returns. V set: a byte was read.
pf_err_v:
  bit lf_vflag              ; V, and C is kept
pf_err:
  tax
  lda LF_SHADOW             ; inline, not pf_ctrl: an error exit is where the
  and #$09                  ; original is quickest, and bit 7 is clear here
  ora #$26
  sta LF_SHADOW
  sta FDS_CTRL
  txa
  ldx LF_FRAME              ; back to the frame $E3E7 left
  txs
  tax                       ; a = x = the code, N and Z from it
  cli
  rts


; $0C/$0D down one; Z clear until it has gone past zero to $FFFF.
pf_down:
  lda LF_CNT
  bne @lo
  dec LF_CNT+1
@lo:
  dec LF_CNT
  lda LF_CNT
  and LF_CNT+1
  cmp #$FF
  rts
pf_low_end:
.assert pf_low_end <= $E484

.org $E484
; $E484: block 2. $06 = the count as it stands; y = $23 on every exit.
  lda #$02
  jsr pf_arm
  jsr pf_rd
  sta LF_LEFT
  ldy #$23
  jsr pf_end1
  jmp pf_ok
pf_count_end:
.assert pf_count_end <= $E4A0

.org $E4A0
  jmp pf_e4a0               ; the body is at $F550
.org $E4F9
  jmp pf_e4f9               ; the body is at $E7BE and $E8E4
.org $E68F
; $E68F: open the next block and check its type is a, any a. Out: a kept,
; x = y = (a + $21) & $FF, the code a mismatch gives; V I Z C set.
  jsr pf_arm
  lda PF_TYPE
  clc
  adc #$21
  tax
  tay
  php
  pla
  and #$08                  ; D kept
  ora #$47
  pha
  lda PF_TYPE
  plp
  rts

; a = what $4032 gave, shifted; y set. x = 1, V and I set, N Z C clear.
pf_ok:
  sei
  clc
  bit lf_vflag
  ldx #$01
  rts

; a into $FA, and to the drive without the byte IRQ.
pf_ctrl:
  sta LF_SHADOW
  and #$7F
  sta FDS_CTRL
  rts

; ---- open a block of type a: $07, arm, the type byte ------------------------
pf_arm:
  sta PF_TYPE
  lda LF_SHADOW
  ora #$40
  jsr pf_ctrl
  lda #$40
  sta $0101                 ; the original's byte-IRQ state; ours polls
  lda LF_SHADOW
  ora #$80
  jsr pf_ctrl
  jsr pf_rd
  cmp PF_TYPE
  bne @bad
  rts
@bad:
  php                       ; C = disk byte >= the type
  lda PF_TYPE
  clc
  adc #$21
  tay
  plp
  tya
  jmp pf_err_v

; ---- one byte. Off the end of the side, where the original waits forever,
; ours gives $28 ------------------------------------------------------------
pf_rd:
  jsr read_byte
  bcs @eoh
  rts
@eoh:
  lda #$28
  jmp pf_err_v

; ---- the end of a header-type block. y is what its errors return; x is
; their carry, which pf_end1 sets ---------------------------------------------
; Polled inline rather than through pf_rd: the original's error exits here are
; its quickest paths, and a jsr chain per byte lost them. Each check is its
; own status read AFTER the byte is taken, as the original's are; testing the
; read that brought the byte instead put every check one byte early.
pf_end1:
  ldx #$01
pf_end:
@one:
  lda FDS_STATUS            ; CRC byte one
  and #$42
  beq @one
  lda FDS_DATA
  lda FDS_STATUS            ; end of head is checked here
  and #$40
  bne @eoh
  lda LF_SHADOW
  ora #$10                  ; CRC mode
  sta LF_SHADOW
  and #$7F
  sta FDS_CTRL
@two:
  lda FDS_STATUS            ; and two
  and #$42
  beq @two
  lda FDS_DATA
  lda FDS_STATUS            ; CRC is checked here
  and #$10
  beq pf_off
  lda #$27
  bne @fail
@eoh:
  lda #$28
@fail:
  cpx #$01
  jmp pf_err_v
; disarm, then the drive: a = $4032 shifted right, C clear; no disk is $01
pf_off:
  lda LF_SHADOW
  and #$2F                  ; bit 7 goes with it
  sta LF_SHADOW
  sta FDS_CTRL
  lda FDS_DRIVE
  lsr a
  bcs @gone
  rts
@gone:
  lda #$01
  jmp pf_err_v

pf_high_end:
.assert pf_high_end <= $E778

.org $FF28
; ---- $E445: spin up, then block 1 against the template at ($00) -------------
; One attempt, no retry. $08 ends as the boot read file code.
pf_e445:
  lda LF_SHADOW
  and #$08
  ora #$26                  ; stop
  sta FDS_CTRL
  ora #$01
  sta FDS_CTRL
  and #$FD                  ; and start: $25 with the mirroring
  sta LF_SHADOW
  sta FDS_CTRL
  lda $F9
  ora #$80
  sta $F9
  sta FDS_EXT
  lda FDS_BATT
  bmi @battery
  lda #$02
  ldy #$00
  sec
  jmp pf_err                ; V kept: no byte read
@battery:
  lda LF_SHADOW
  and #$08
  ora #$26
  sta FDS_CTRL
  ora #$01
  sta FDS_CTRL
  and #$FD
  sta LF_SHADOW
  sta FDS_CTRL
@ready:
  lda FDS_DRIVE
  lsr a
  bcs @nodisk
  lsr a
  bcs @ready
  lda #$01
  jsr pf_arm
  ldx #$00
@magic:
  jsr pf_rd
  cmp lf_magic,x
  bne @bad_magic
  inx
  cpx #14
  bne @magic
  lda #$04                  ; $08 walks the ID codes, BCD at the end
  sta LF_BOOT
  ldy #$00
@id:
  jsr pf_rd
  pha
  lda (LF_P1),y
  cmp #$FF
  beq @wild
  tsx
  cmp $0101,x
  bne @id_bad
@wild:
  pla
  cpy #$01                  ; it steps after bytes 0 and 4 to 9
  bcc @step
  cpy #$04
  bcc @nostep
@step:
  inc LF_BOOT
@nostep:
  iny
  cpy #10
  bne @id
  jsr pf_rd
  sta LF_BOOT               ; disk-info byte $19
  ldx #30
@rest:
  jsr pf_rd
  dex
  bne @rest
  ldy #$00
  jsr pf_end1
  jmp pf_ok
@nodisk:
  lda #$01
  ldy #$00
  sec
  jmp pf_err
@bad_magic:
  php                       ; C from the compare; y = 13 - x
  txa
  eor #$FF
  clc
  adc #14
  tay
  plp
  lda #$21
  jmp pf_err_v
@id_bad:
  lda LF_BOOT               ; the code is $08 in BCD, which only shows at 10
  cmp #10
  bne @bcd
  lda #$10
@bcd:
  clc
  jmp pf_err_v
pf_e445_end:
.assert pf_e445_end <= $FFFA
.org $E778
; $E778: coast. $FA = ($FA & 9) | $26 to the drive, a = x = 0, y kept, Z set,
; N and I clear, C V D kept. 29 cycles, the original's to the access.
  ldx #$00
  jmp @tail                 ; 3 cycles with nothing outside the BIOS
@tail:
  lda $FA                   ; literal: LF_SHADOW is defined further down, and
  and #$09                  ; a forward name assembles absolute, two cycles over
  ora #$26
  sta $FA
  sta FDS_CTRL
  txa
  cli
  rts
pf_coast_end:
.assert pf_coast_end <= $E7BB
; --- $E7BB: walk a VRAM write structure the caller points at inline ---------
;
; 18 of the 114 corpus titles call it and six are stopped on it, Zelda among
; them. The two bytes after the `jsr` are a little-endian pointer to a
; structure, and the routine steps its own return address over them, so it
; comes back to jsr+5. Same convention as $E1F8 and $EBAF.
;
; The structure is a little language, and every part of it below was pinned by
; poking a real one field at a time rather than inferred:
;
;   first byte with bit 7 SET      end. The byte itself comes back in a, so it
;                                  is not only $FF: $80 and $BE both ended one.
;   $4C lo hi                      push this position, continue at $hhll. The
;                                  operand is little-endian, unlike the VRAM
;                                  addresses below, which are big-endian.
;   $60                            pop, and resume three bytes past the $4C.
;   anything else                  a VRAM entry: this byte is the address high,
;                                  then the low, then a length byte.
;
; The length byte splits three ways:
;   bit 7   step the VRAM address by 32 instead of 1. $2000 is written with
;           (shadow & ~$04) with bit 2 put back if set, and the SHADOW KEEPS
;           IT, so the stepping outlives the call. Forced $FF=$FF wrote $FB
;           plain and $FF with bit 7, which is what pins the mask as exactly
;           bit 2.
;   bit 6   repeat: ONE data byte follows and is written `count` times.
;   0-5     the count, and zero means 64, not nothing. A forced $00 wrote
;           $2007 exactly 64 times.
;
; After an entry whose address high byte is exactly $3F, and only then, it
; writes $2006 four more times with $3F,$00 and then $00,$00. That is the
; palette-safe reset: leaving the address inside the palette makes the PPU
; show the palette entry instead of the backdrop. $2F, $3E and $7F were all
; measured NOT to trigger it.
;
; One $2002 read happens per element, which resets the address latch, and x is
; left holding the last one. Nothing can sensibly use that, but it costs
; nothing to reproduce by reading into x in the first place.
;
; Exit: rts to jsr+5, a is the terminator so N is always set and Z clear, y is
; zero, and the carry is incidental. $00/$01 are left pointing at the
; terminator and $05/$06 at the caller's jsr+2.
.org $E7BB
  jmp vram_struct

.org $F320
vram_struct:
  tsx                       ; the inline pointer, read through the stacked
  lda $0101,x               ; return address, which points at the jsr's last
  sta $05                   ; byte. $05/$06 keep the pre-bump copy, measured.
  lda $0102,x
  sta $06
  ldy #$01
  lda ($05),y
  sta $00
  iny
  lda ($05),y
  sta $01
  lda $05                   ; step the caller over the operand
  clc
  adc #$02
  sta $0101,x
  lda $06
  adc #$00
  sta $0102,x

@element:
  ldx PPUSTATUS             ; one per element; resets the address latch, and
  ldy #$00                  ; leaves x holding the last value read
  lda ($00),y
  bpl @live                 ; any bit-7 byte ends the structure, and the
  rts                       ; terminator itself comes back in a, so N is
                            ; always set and Z clear. y is zero here and x
                            ; holds the $2002 read just above.
@live:
  cmp #$4C
  beq @call
  cmp #$60
  beq @back

  ; ---- a VRAM entry ----
  pha                       ; the address high, kept for the $3F test
  sta PPUADDR
  iny
  lda ($00),y
  sta PPUADDR
  iny
  lda ($00),y               ; the length byte
  pha
  and #$80                  ; bit 7 chooses the VRAM stepping
  beq @step1
  lda $FF
  ora #$04
  jmp @setctrl
@step1:
  lda $FF
  and #$FB
@setctrl:
  sta $FF                   ; the shadow keeps it: measured
  sta PPUCTRL
  pla
  pha                       ; the length byte, wanted twice
  and #$3F
  bne @count
  lda #$40                  ; a count of zero means sixty-four
@count:
  tax
  pla
  and #$40
  bne @repeat

@copy:                      ; `count` bytes straight through
  iny
  lda ($00),y
  sta PPUDATA
  dex
  bne @copy
  jmp @entry_done

@repeat:                    ; one byte, written `count` times
  iny
  lda ($00),y
@rep:
  sta PPUDATA
  dex
  bne @rep

@entry_done:
  pla                       ; the address high from the top of the entry
  cmp #$3F
  bne @advance
  lda #$3F                  ; the palette-safe reset: point at $3F00, then at
  sta PPUADDR               ; $0000, so the PPU is not left showing a palette
  lda #$00                  ; entry in place of the backdrop
  sta PPUADDR
  sta PPUADDR
  sta PPUADDR
@advance:
  tya                       ; y indexes the last byte consumed, so the step is
  sec                       ; y + 1
  adc $00
  sta $00
  bcc @loop
  inc $01
@loop:
  jmp @element

@call:
  lda $00                   ; push this position, low byte first: measured
  pha
  lda $01
  pha
  ldy #$01
  lda ($00),y               ; the operand IS little-endian here
  tax
  iny
  lda ($00),y
  sta $01
  stx $00
  jmp @element

@back:
  pla                       ; high byte first, mirroring the push
  sta $01
  pla
  clc
  adc #$03                  ; resume past the three bytes of the $4C. No
  sta $00                   ; measured case crossed a page; carry anyway
  bcc @loop2
  inc $01
@loop2:
  jmp @element


.org $E844
; --- $E844: fetch an inline pointer for the routine that called us ---------
;
; 2 of the 114 corpus titles call it and both were stopped on it: Knight Move
; and Seiken Psycho Calibur. It serves the GAME's own subroutines, the ones
; written to be called as `jsr G` followed by a two-byte pointer. G calls this,
; and this reaches past its own return address to G's: it copies the two bytes
; after G's jsr into $00/$01 and steps G's return address over them, so G's
; rts lands after the pointer.
;
; Measured on both titles, 22 accesses a call and every one accounted for:
;
;   * G's return address is read from $0103,x and $0104,x after a tsx, which
;     is SP+3 and SP+4 at entry: under our own return address, not ours.
;   * it goes through $05/$06 as the pointer, and leaves it there.
;   * the step is +2 with the carry rippled into the high byte, written back
;     through sta abs,x, whose dummy read of the target is in the log too.
;
; Exit: a is G's new return high byte, x is the stack pointer as it was on
; entry, y is 2, and the flags are those of the high byte's adc #0. 64 cycles,
; matching the original on both callers. 38 bytes, which is exactly the gap to
; $E86A.
inline_ptr:
  tsx
  lda $0103,x               ; G's return address, low
  sta $05
  lda $0104,x               ; and high
  sta $06
  ldy #$01                  ; it points at the last byte of G's jsr
  lda ($05),y
  sta $00
  iny
  lda ($05),y
  sta $01
  lda $05
  clc
  adc #$02                  ; step G's return address over the pointer
  sta $0103,x
  lda $06
  adc #$00
  sta $0104,x
  rts
inline_ptr_end:
.assert (inline_ptr_end - inline_ptr) == 38
; --- $E86A: flush the queued VRAM transfer buffer at $0302 -----------------
;
; 10 of the 114 corpus titles call it, 6305 calls between them, and 6 titles
; were stopped on it. The companion to $E7BB: the same shape of data, but on a
; buffer at a FIXED address that a game fills during the frame and this empties
; during vblank.
;
; Takes nothing and returns nothing. The whole interface is the buffer.
;
;   $0302   the structure, walked until a byte with bit 7 set ends it
;   $0301   the game's own write index. **Never read here**, only reset to $00
;           on the way out, so the game finds an empty queue.
;
; Each entry is address high, address low, a count, then that many bytes. An
; empty queue is a single $FF at $0302.
;
; **The count is the WHOLE byte, and zero means 256.** That is worth stating
; loudly because $E7BB's count looks the same and is not: there the low six
; bits are the count, bit 6 means repeat-one-byte, bit 7 picks the VRAM
; increment, and zero means 64. None of that is true here. Measured by forcing
; lengths $05, $45, $85, $C5 into a buffer and counting the writes to $2007:
; 5, 69, 133, 197, which only the whole-byte reading explains. A forced $00
; wrote 256. Two routines, two languages, and implementing this one by calling
; the other would quietly corrupt any entry of 64 bytes or more.
;
; The preamble masks the PPUCTRL shadow with `and #$FB`, forcing the VRAM
; increment to +1, and keeps the masked value in the shadow where the game can
; see it. Every other bit survives, NMI enable included. This was nearly missed:
; with the shadow at $30 the mask changes nothing, and $30 is what the first
; title measured happened to hold. Forced $34 came back $30 and $FB came back
; $FB, which is what pins it.
;
; `$2002` is read ONCE, in the preamble, not once per entry as in $E7BB.
;
; Bytes beginning $4C or $60 are ordinary VRAM address high bytes here, NOT the
; call and return of $E7BB's little language. Measured rather than assumed: a
; forced entry starting $4C wrote $2006 twice and streamed its data, with no
; stack traffic beyond the one push every entry gets. It could hardly be
; otherwise, since this walks a fixed base with an 8-bit index and has no
; pointer for a call to redirect.
;
; After each entry it pulls the saved address high byte back and, if it is
; EXACTLY $3F, writes $2006 with $3F, $00, $00, $00: the palette-safe reset,
; pointing at $3F00 and then at $0000 so the PPU is not left showing a palette
; entry instead of the backdrop. $3E, $2F and $7F were each measured NOT to
; trigger it, so the compare is the full byte. Per entry, not once per call.
;
; On the way out it stores the terminator byte it READ back to $0302, rather
; than a constant: a buffer ended with $80 comes back with $80 there. Then
; $0301 = $00, which is also where the `a` of $00 on exit comes from.
.org $E86A
vram_flush:
  lda $FF                   ; 3
  and #$FB                  ; 2   force the VRAM increment to +1
  sta PPUCTRL               ; 4   the shadow goes out before it goes back
  sta $FF                   ; 3
  ldx PPUSTATUS             ; 4   reset the address latch, once for the call
  ldy #$00                  ; 2
  jmp vf_check              ; 3   the loop is at $F5B8, out of $E8B3's way
;
; The loop is laid out with its test at the BOTTOM, which costs this one `jmp`
; and is what makes the palette tail come out right. The palette block FALLS
; THROUGH into the test, and the common path branches over it to the same
; place, so the palette entry costs exactly four stores more than a plain one
; rather than four stores plus a branch back. That is the difference between
; +15 and +18 cycles, and +18 would make us slower than the original on any
; entry that touches the palette.
.assert (vram_flush_end - vram_flush) == 17
vram_flush_end:
; --- $E8D2: append one block to the VRAM transfer buffer at $0302 ----------
;
; 5 of the 114 corpus titles call it and four are stopped on it: Druid, Kick
; Challenger, Konamic Tennis and Topple Zip. The producer side of $E86A, and
; the fourth routine in the file to take its argument inline:
;
;   a   the VRAM address high byte      the two bytes after the `jsr` are a
;   x   the VRAM address low byte       little-endian pointer to the data, and
;   y   how many bytes to copy          the return address steps over them
;
; It APPENDS: $0301 is read as the starting index and the block goes in as
; address high, address low, count, then the data, followed by a $FF
; terminator, which is the empty-queue byte $E86A leaves behind.
;
; Everything below was pinned by forcing one field at a time on Topple Zip and
; reading the access log, not inferred from the shape of the thing:
;
;   * $0301 is honoured rather than assumed zero. Forced to $10 the block
;     landed at $0312 and the committed length came back $1B, not $0B.
;   * the count is the whole byte and a count of zero means 256. A forced
;     y=$00 walked 253 bytes past the source and off the end of the buffer.
;   * $0300 is a hard limit on the index, checked AFTER every single byte, and
;     an overflow does not truncate: it ROLLS BACK. The $FF goes to the index
;     the call started at, $0301 is left alone, and the caller gets a=$01 with
;     carry SET, against a=$FF and carry clear on success. Forced limits of
;     $00 $01 $02 $03 $04 $0B and $0C gave 1, 1, 2, 3, 4, 11 and 12 bytes
;     written, which is what puts the check after the store rather than before
;     it and what shows the terminator needs a byte of its own: with a start
;     of $00 and a count of 8 the limit has to reach $0C, not $0B.
;   * y comes back as count-1, because the copy loop leaves it indexing the
;     last byte it read. y=$08 in gave $07 out, y=$20 gave $1F, and an
;     overflow before the first data byte gave $FF, which is what pins
;     `ldy #$FF` ahead of the header rather than inside the loop.
;   * on the way out $02/$03 move on by one row of 32. Carry out comes from
;     that add, so it is clear unless the high byte itself wraps: a forced
;     x=$F0 still returned carry clear, because the $20 carried into $03 and
;     the `adc #$00` there consumed it.
;   * $05 is a block counter the tail decrements, and this entry sets it to 1.
;     It is 0 on success and 1 after an overflow. $E8E1 next door sets it
;     higher, which is where the back edge below was measured; see that
;     routine for what a second block does.
;
; The original costs 224 + 41 per byte, measured across four titles with
; counts of 2, 8, 9, 24 and 32 (552 cycles for 8, 1536 for 32). This runs in
; 182 + 28 per byte, so it is faster everywhere. It builds a buffer rather
; than driving the PPU, so there is no window for it to be too fast for.
.org $E8D2
  jmp vram_queue
; $E8DB: $E8D2 with its arguments already in zero page, $00/$01 the data,
; $02/$03 the address low first, $04 the count. Dogfight Spirit draws its rows
; with it and Jirai Kun its palettes; both read $02 back, stepped by 32.
.org $E8DB
  jmp vram_queue_zp

; --- $E8E1: append SEVERAL blocks, with the shape packed into the data ------
;
; 3 of the 114 corpus titles call it, Druid among them, which is why it is
; written beside $E8D2 rather than after it: Druid is stopped on $E8D2 and
; calls this one too, so writing the first alone would have moved its wall
; fifteen bytes down the page.
;
; Same arguments as $E8D2 minus the count:
;
;   a   the VRAM address high byte      the two bytes after the `jsr` are a
;   x   the VRAM address low byte       pointer, and the FIRST byte there is
;   y   ignored, measured               the shape: blocks in the high nibble,
;                                       bytes per block in the low one
;
; Each block gets its own three-byte header and its own commit to $0301, and
; the VRAM address walks on by 32 between them, so a packed $2E is two rows of
; fourteen bytes one under the other. Measured on Druid by forcing the shape
; byte:
;
;   $21 -> two blocks of one byte, $0301 committed at $16 then $1A
;   $11 -> one block of one byte
;   $FF -> fifteen of fifteen, so the low nibble is masked and not the byte
;   $10 -> a count of 0 is 256, not 16 and not nothing: it ran off the buffer
;   $01 -> a block count of 0 is likewise 256, committing 27 times before the
;          buffer filled
;
; `y` is not an argument and not preserved: forced to $00 and to $55 the call
; came out identical, because the copy index starts at the shape byte and
; walks on through every block without resetting. Two blocks of fourteen leave
; it at 28.
;
; The original costs 119 + 120 per block + 41 per byte (280 cycles for one
; block of one, 1507 for two of fourteen). This runs in 117 + 80 per block +
; 28 per byte.
.org $E8E1
  jmp vram_queue_packed

.org $F400
; $E8D2 and $E8E1 differ only in where the shape comes from. Everything from
; `queue_block` down is shared, including the back edge, which is the part
; only $E8E1 ever takes.
vram_queue:
  sta $03                   ; 3   the VRAM address high
  stx $02                   ; 3   and low
  sty $04                   ; 3   the count is an argument here
  jsr queue_inline          ; 6
vram_queue_zp:              ;     $E8DB enters here, the arguments already set
  lda #$01                  ; 2   one block
  sta $05                   ; 3
  ldx BUF-1                 ; 4   the write index: we append to what is there
  ldy #$FF                  ; 2   the copy loop `iny`s before its first read,
  jmp queue_block           ; 3   and here the data starts at the pointer
; 90 cycles to queue_block, and 182 + 28 per byte for the call.

vram_queue_packed:
  sta $03                   ; 3
  stx $02                   ; 3   and y is not an argument: measured ignored
  jsr queue_inline          ; 6
  ldy #$00                  ; 2
  lda ($00),y               ; 5   the shape byte, read twice because that is
  and #$0F                  ; 2   cheaper than keeping it: bytes per block
  sta $04                   ; 3
  lda ($00),y               ; 5
  lsr a                     ; 2
  lsr a                     ; 2
  lsr a                     ; 2
  lsr a                     ; 2   and blocks
  sta $05                   ; 3
  ldx BUF-1                 ; 4   y stays at $00, so the first `iny` steps off
                            ;     the shape byte and onto the data
; 105 cycles to here, and it falls straight in.

queue_block:
  lda $03                   ; 3
  sta BUF,x                 ; 5
  inx                       ; 2
  cpx BUF-2                 ; 4   the buffer size. Every byte is checked, and
  bcs @full                 ; 2   the check is after the store, not before
  lda $02                   ; 3
  sta BUF,x                 ; 5
  inx                       ; 2
  cpx BUF-2                 ; 4
  bcs @full                 ; 2
  lda $04                   ; 3
  sta $06                   ; 3   the countdown, all eight bits: 0 means 256
  sta BUF,x                 ; 5
@data:
  inx                       ; 2
  cpx BUF-2                 ; 4
  bcs @full                 ; 2
  iny                       ; 2
  lda ($00),y               ; 5
  sta BUF,x                 ; 5
  dec $06                   ; 5
  bne @data                 ; 3 taken
  inx                       ; 2   room for the terminator is checked too
  cpx BUF-2                 ; 4
  bcs @full                 ; 2
  stx BUF-1                 ; 4   commit, per block: nothing before here is
  lda $02                   ; 3   visible to the game if the block did not fit
  clc                       ; 2
  adc #$20                  ; 2   on to the next row of the nametable
  sta $02                   ; 3
  lda $03                   ; 3
  adc #$00                  ; 2   and this is where the exit carry comes from
  sta $03                   ; 3
  dec $05                   ; 5
  bne queue_block           ; 3   $E8D2 passes 1 and never takes this
  lda #$FF                  ; 2
  sta BUF,x                 ; 5   so $E86A finds the queue ended
  rts                       ; 6
; 80 cycles a block plus 28 a byte, and 13 more on the way out.
@full:
  ldx BUF-1                 ; 4   roll back to the last committed length
  lda #$FF                  ; 2
  sta BUF,x                 ; 5
  lda #$01                  ; 2   the error code. `a` last so N and Z are its
  rts                       ; 6   own and carry is still the compare's

; The inline operand, shared because stepping the caller over it is the one
; part of this worth having only one copy of. Called with a `jsr`, so the
; game's return address is two further up the stack than it would be at the
; entry point: $0103/$0104 past x rather than $0101/$0102.
queue_inline:
  tsx                       ; 2
  lda $0103,x               ; 4   the stacked return address points at the
  sta $05                   ; 3   jsr's last byte. $05/$06 hold the pre-bump
  clc                       ; 2   copy, which is what the operand is read
  adc #$02                  ; 2   through, and the bump rides along on the
  sta $0103,x               ; 5   same `a` rather than loading it twice
  lda $0104,x               ; 4
  sta $06                   ; 3
  adc #$00                  ; 2   the carry from the low byte, still live
  sta $0104,x               ; 5
  ldy #$01                  ; 2
  lda ($05),y               ; 5   the two bytes after the jsr: the pointer
  sta $00                   ; 3
  iny                       ; 2
  lda ($05),y               ; 5
  sta $01                   ; 3
  rts                       ; 6
.assert (vram_queue_end - vram_queue) == 169
vram_queue_end:
.org $E997
; --- $E997: a nametable address to pixel coordinates -----------------------
;
; 1 of the 114 corpus titles calls it, Knight Move, which was stopped on it
; once $E844 let it through. $00/$01 is a nametable address, HIGH byte first,
; and it comes back as the pixel position of that tile:
;
;   $03 = X = column * 8, the address's low five bits
;   $02 = Y = row * 8, the next five bits
;
; Measured by setting one input bit at a time: $01 bits 0-4 land in $03 bits
; 3-7, $01 bits 5-7 in $02 bits 3-5, $00 bits 0-1 in $02 bits 6-7, and nothing
; else in a, x, y or $00 has any effect. Knight Move's own call, $21D9, gives
; Y $70 and X $C8.
;
; Exit: a = Y, x and y untouched, N and Z from a, and carry is bit 1 of $01,
; shifted out by the second ror. The original's access log is this sequence
; exactly, including the two read-modify-writes of $02, and it is 49 cycles
; and 26 bytes, the gap to $E9B1.
tile_to_pixel:
  lda $01
  asl a
  asl a
  asl a
  sta $03                   ; X: the column, times 8
  lda $01
  sta $02
  lda $00
  lsr a
  ror $02
  lsr a
  ror $02                   ; the row's top two bits, shifted in from $00
  lda $02
  and #$F8                  ; Y: the row, times 8
  sta $02
  rts
tile_to_pixel_end:
.assert (tile_to_pixel_end - tile_to_pixel) == 26
; --- $E9B1: shift a multi-byte register right one bit, with feedback --------
;
; 10 of the 114 corpus titles call it, 7021 calls between them, and 7 titles
; were stopped on it. A random number generator: the caller keeps a seed of Y
; bytes in zero page and this advances it one bit per call.
;
; The census records the entry kind as `jmp`, and that is true of only two of
; the nine callers that reach it: Akuu Senki Raijin and Exciting Baseball
; tail-call it, so their `rts` returns to a `jsr` further back in the game. The
; other seven, Bubble Bobble and Zelda among them, enter by a plain
; `jsr $E9B1`. Either way it must leave the stack alone and end in `rts`, which
; it does; there are no pushes anywhere in it.
;
; `x` = the zero page address of the register, `y` = how many bytes. Real
; callers pass `y` from 1 to 13: Akuu asks for one byte, Nazo no Murasame-jou
; for eight, Zelda for thirteen. It shifts
; the whole thing RIGHT one bit. `byte[x]` is the most significant end: the
; feedback bit enters bit 7 of `byte[x]`, each byte's bit 0 falls into the next
; byte's bit 7, and the bit off the far end lands in carry.
;
;   feedback = bit1(byte[x]) XOR bit1(byte[(x+1) & $FF])
;
; **The second tap is at x+1 absolutely, not at the end of the register.** That
; took a probe matrix at y=4 to settle, forcing all five of `$AC-$B0` one at a
; time on Gun.Smoke: only `$AC` and `$AD` moved the result, which rules out
; x+y, x+y-1 and x+2. Nothing but bit 1 of either byte matters; a tap byte of
; `$FD` behaves as `$00` and `$FF` as `$02`.
;
; **At y=1 the tap reads a byte OUTSIDE the register**, and its bit 1 really
; does change both the result and the cycle count, measured on two titles. So
; the read has to stay even though it looks like a bug. Akuu Senki Raijin is
; the one natural y=1 caller.
;
; Our shape happens to reproduce the original's access trace byte for byte,
; which was not aimed at. Zero-page,X addressing makes the 6502 read the
; UN-INDEXED base address as a dummy cycle, so `lda $00,x` reads `$0000` then
; `byte[x]`, and `lda $01,x` reads `$0001` then `byte[x+1]`. Those dummy reads
; are exactly the otherwise inexplicable `$0000` and `$0001` reads in the real
; routine's log, and `$0001`'s value provably never matters.
;
; The `x=$ff` collision comes out of that for free. There the tap wraps to
; `$0000`, which `sta $00` filled two instructions earlier, so the feedback is
; bit1(byte[x]) XOR itself and is **always 0**. Measured on the real routine,
; and ours cannot do otherwise.
;
; `y=0` means 256, because the loop is a do-while. The real one scrambles the
; whole of zero page and returns normally, 3356 cycles, writing no stack byte.
; Ours does the same thing for the same reason rather than by a special case.
;
; Exit: `a` = the feedback bit as `$00` or `$02`, `x` = x+y, `y` = `$00`,
; carry = the bit shifted off the end, `N=0` and `Z=1` from the final `dey`,
; and `V`, `D` and `I` preserved. `$0000` is the only scratch and holds
; bit1(byte[x]), an intermediate rather than the feedback; `$0001` is read but
; never written.
;
; Cost 28 + 13*y + feedback against our 25 + 13*y, so we are three or four
; cycles quick. Safe: this is a generator, not a delay, and its product is the
; bit sequence rather than the time. Decimal-proof either way, confirmed on the
; original with D=1 at two register lengths.
.org $E9B1
random_shift:
  lda $00,x                 ; 4   byte[x]; the dummy read of $0000 is the real
                            ;     routine's first access too
  and #$02                  ; 2
  sta $00                   ; 3   the first tap, parked in bit 1
  lda $01,x                 ; 4   byte[x+1], wrapping inside zero page
  and #$02                  ; 2
  eor $00                   ; 3   a = the feedback bit, $00 or $02
  cmp #$02                  ; 2   carry = it, and a survives to be returned
@shift:
  ror $00,x                 ; 6   read, write old, write new: the same
  inx                       ; 2   read-modify-write signature the original
  dey                       ; 2   leaves, 13 cycles a byte
  bne @shift                ; 3, and 2 on the last pass
  rts                       ; 6
.assert (random_shift_end - random_shift) == 21
random_shift_end:
; --- $E9C8: copy the sprite page to OAM ------------------------------------
;
; 27 of the 114 corpus titles call it, and it is the one routine the profiler
; settled on its own: every caller, on every call, writes $2003=$00 then
; $4014=$02 and reads $0200-$02FF, returns by `rts` with a=$02, and costs
; exactly 18 CPU cycles. That is a sprite DMA from page 2 with the page fixed,
; and 18 is precisely lda(2) + sta abs(4) + lda(2) + sta abs(4) + rts(6).
;
; The DMA's own 513 stall cycles are on top and are not part of the 18; the CPU
; makes no bus accesses while the DMA unit has the bus.
.org $E9C8
sprite_dma:
  lda #$00
  sta OAMADDR
  lda #$02
  sta OAMDMA
  rts
.org $E9D3
; --- $E9D3: count a block of zero-page timers down, a tenth of them slowly ---
;
; 3 of the 114 corpus titles call it and all three were stopped on it: Bubble
; Bobble, Nazo no Murasame-jou and Zelda no Densetsu. No inline arguments; the
; stack reads in its access log are the rts.
;
;   x   the PRESCALER, a zero-page byte counting 9 down to 0
;   a   the last of the FAST timers, x+1 up to a
;   y   the last of the SLOW timers, x+1 up to y
;
; Each call decrements the prescaler. If that takes it below zero it is
; reloaded with 9 and every timer from y down to x+1 is decremented; otherwise
; only those from a down to x+1 are. A timer at zero stays at zero. So the
; slow ones tick once every ten calls.
;
; The walk stops when the index comes back round to $00, which holds x. That
; is a compare against memory, not against a register, and it is observable:
; with a below x the index wraps the whole zero page, decrementing $00 itself
; on the way, and the walk then ends one byte later than it would have. Forced
; with x=$3C and a=$34 the prescaler went $49 -> $47 and the call took 4269
; cycles.
;
; Exit: x comes back as it went in, because that is where the walk stops. y is
; untouched. a is the last timer value LOADED, before its decrement. N, Z and C
; are the final cpx (equal, so Z and C set) and V, D and I pass through.
;
; Every read in the original's access log is accounted for by this exact
; shape, including the dummy read of $00 that each zp,x access makes: write
; $00, then the prescaler's read-modify-write, then per timer a read of $00, the
; timer, and $00 again for the compare.
counter_logic:
  stx $00                   ; where the walk stops
  dec $00,x                 ; the prescaler
  bpl @go                   ; still counting: a is the start, fast timers only
  lda #$09
  sta $00,x                 ; underflowed: reload, and walk the slow ones too
  tya
@go:
  tax
@timer:
  lda $00,x
  beq @next                 ; a timer at zero stays there
  dec $00,x
@next:
  dex
  cpx $00
  bne @timer
  rts
; --- $EA1F: read both pads and work out what was newly pressed -------------
;
; 26 of the 114 corpus titles call it. No register arguments; everything comes
; from and goes to zero page:
;
;   $FB  in   the byte to hold on $4016's output lines while strobing. The
;             strobe writes are $FB+1 then $FB, which is an increment and not
;             an or: poking $FB=$07 produced writes of $08 then $07, where an
;             `ora #1` would have produced $07 twice. Bit 0 is the strobe, and
;             the other bits are there for whatever the expansion port wants.
;   $F7  in   what was held last call, for pad 1. $F8 for pad 2.
;   $F5  out  newly pressed since the last call, pad 1. $F6 for pad 2.
;   $F7  out  held now, pad 1. $F8 for pad 2.
;   $00  out  the expansion pad's held state, pad 1. $01 for pad 2.
;
; The "newly pressed" arithmetic was proven with a sentinel rather than read
; off: poking $F7=$EE before a call holding A, Select, Down and Right returned
; $01, which is $A5 AND NOT $EE exactly.
;
; Bit order in the stored byte is A=$80 down to Right=$01, proven by feeding
; $0F and getting $F0 back, and $33 giving $CC.
;
; Exit: a = $F5, y = $F7, x = $FF, and the flags do NOT describe a (a call
; returning a=$00 still came back with Z clear). We reproduce x=$FF and the
; register values; the exit carry varies with the data on the real BIOS from a
; source that could not be identified, so it is not reproduced and should not
; be relied on.
;
; Bit 1 of each read is the Famicom expansion pad and goes to $00/$01, which is
; structural inference rather than measurement: our core never drives that bit,
; so it was never seen lit. Nothing in the corpus depends on it, and routing it
; here costs nothing if it is right and nothing if it is wrong.
.org $EA1A
; $EA1A: $EA1F without the expansion merge. Eight titles, among them both
; Zatsugaku Olympic quizzes and Bishoujo Control. The expansion bits still land
; in $00 and $01; they just do not reach $F5 and $F6. 427 cycles against ours.
  jmp read_down
.org $EA1F
  jmp read_pads             ; body below; 70 bytes will not fit in 45

; $EA36: $EA4C on the raw pass, with no expansion merge: read both pads until
; two passes agree, pad 2 compared first. Tarot's "press A or B" loop. The real
; one costs 813 cycles when the first two passes agree; ours, the same passes
; through pads_raw, a little under.
.org $EA36
  jmp verify_raw

; --- $EA4C: read both pads twice and only believe a repeated answer --------
;
; 17 of the 114 corpus titles call it and twelve were stopped on it. It is
; $EA1F with a verification pass: read both pads, read them again, and if the
; two merged answers disagree take the newer one as the reference and read once
; more. That is the classic defence against a read corrupted mid-way by the
; DPCM channel stealing a cycle from the controller port.
;
; It takes no arguments, touches no disk and no PPU, and cannot fail; it simply
; does not return until two consecutive reads agree. The real one costs 873
; cycles when they agree, which is every call unless the input moves during it,
; and about 417 more per retry. Ours costs 832: the same two passes with a
; cheaper merge, and faster is safe here as everywhere but $E149. Under buttons deliberately alternated every pass it was still
; going after 4,797 passes, so there is no retry cap to reproduce.
;
; The retry lands back on the STASH and not on the entry, so each attempt
; compares against the most recent read rather than against the first one.
; Measured by flipping pad 1 and pad 2 separately part way through a call.
;
; Exit: a is the new $F5 and y the new $F7, both pad 1; x is $FF; N set and Z
; clear from the final `dex`; carry SET from the compare that passed.
.org $EA4C
verify_pads:
  jsr pads_held
@stash:
  ldx $F5                   ; the answer so far, re-taken on every retry
  lda $F6
  pha
  jsr pads_held
  pla
  cmp $F6                   ; pad 2 is compared first: a pad-2 mismatch was
  bne @stash                ; measured skipping the pad-1 compare entirely
  cpx $F5
  bne @stash
  jmp pads_diff2

; The body of $EA36, in the free bytes where the original has its bare read
; pass; no disk in the corpus calls that address.
.org $E9EB
verify_raw:
  jsr pads_raw
@stash:
  ldx $F5
  lda $F6
  pha
  jsr pads_raw
  pla
  cmp $F6
  bne @stash
  cpx $F5
  bne @stash
  jmp pads_diff2
verify_raw_end:
.assert verify_raw_end <= $EA1A

; --- $EA68: four pads kept apart --------------------------------------------
;
; Bakutoushi Patton-kun and Kind Gal's, which take two more players on the
; expansion port. The same read pass, no merge; the expansion pads become pads
; 3 and 4 at $F7 and $F8, and newly-pressed for all four comes out in $F5-$F8
; with the held state kept at $F1-$F4 instead. The old $F7 and $F8 are not
; inputs. Exit as the others, but y is pad 1 held ($F1), not $F7.
;
; It fills $EA68-$EA83 exactly, which is why $EA4C hands its diff to the shared
; one above: its own copy reached $EA70.
.org $EA68
read_four:
  jsr pads_raw
  lda $00
  sta $F7
  lda $01
  sta $F8
  ldx #$03
@diff:
  lda $F5,x                 ; newly pressed = held now AND NOT held before
  tay
  eor $F1,x
  and $F5,x
  sta $F5,x
  sty $F1,x
  dex
  bpl @diff
  rts
read_four_end:
.assert read_four_end == $EA84
; --- $EA84: fill a page of video memory ------------------------------------
;
; 41 of the 114 corpus titles call it, the joint most-called routine after the
; NMI handler. Measured with the profiler across 39 callers plus forced-register
; probes, which is how the argument meanings and the path boundary were pinned
; rather than guessed:
;
;   a = the VRAM page, i.e. the high byte of the start address; the low byte is
;       always zero.
;   x = the byte to fill with.
;   y = the attribute byte when a >= $20, the page count when a < $20, and in
;       the second case y=0 means 256 pages (forced a=$00 y=$00 ran 590,919
;       cycles, which is exactly 256 pages).
;
; The boundary is exactly $20: forced a=$19 and a=$1F take the short path and
; a=$20 takes the nametable path.
;
;   a >= $20   1024 bytes of x at a<<8, which covers the nametable and its
;              attribute area, then 64 bytes of y at ((a+3)<<8)|$C0.
;              9889 cycles, constant, on every one of the 36 callers that take
;              this path.
;   a <  $20   y*256 bytes of x and no attribute pass.
;              2308*pages + 71 cycles, checked at 2, 8, 16, 32 and 256 pages.
;
; Two things here are published state rather than implementation. The three
; arguments are spilled to $00, $01 and $02 and LEFT there, so a game may read
; them back after the call. And $FF, the PPUCTRL shadow, is updated to the
; masked value: forced $FF=$FF left $FB in both $2000 and $FF, $FF=$04 left
; $00, $FF=$14 left $10. Bit 2 is the VRAM increment, so clearing it forces the
; +1 stepping this routine needs, and persisting it means a following $EAEA
; keeps that stepping.
;
; The attribute address really is computed as a+3 by an `adc #$02` under a set
; carry rather than by an `ora`: forcing a=$7E wrote $2006=$81,$C0 and returned
; with V set, and no `ora` sets overflow. It is also computed blindly, so a
; caller passing a=$21..$27 has its "attribute" pass land in the middle of a
; nametable. The real BIOS does that too, and copying it is the point.
;
; Exactly 78 bytes, which puts the `rts` on $EAD1 and the next entry point at
; $EAD2 with nothing between them. Every branch stays inside page $EA, which
; the cycle counts above depend on: a branch crossing a page costs one more.
.org $EA84
vram_fill:
  sta $00                   ; the arguments, spilled and deliberately left
  stx $01
  sty $02
  lda PPUSTATUS             ; reset the write latch
  lda $FF
  and #%11111011            ; force the VRAM increment to +1
  sta PPUCTRL
  sta $FF                   ; and keep it, for whoever reads the shadow next
  lda $00
  sta PPUADDR               ; high byte is the page
  ldx #$00
  stx PPUADDR               ; low byte is always zero
  ldx #$04                  ; a whole nametable is four pages
  cmp #$20
  bcs @go
  ldx $02                   ; below $20 the page count is the y argument
@go:
  ldy #$00
  lda $01
@fill:
  sta PPUDATA
  iny
  bne @fill
  dex
  bne @fill
  ldy $02                   ; the y argument back; also the attribute byte
  lda $00
  cmp #$20
  bcc @done
  adc #$02                  ; carry is set, so this is page + 3
  sta PPUADDR
  lda #$C0
  sta PPUADDR
  ldx #$40
@attr:
  sty PPUDATA
  dex
  bne @attr
@done:
  ldx $01                   ; the x argument back; the exit N and Z are its
  rts

; The reconstruction is only cycle-exact if it is also size-exact, and it comes
; out at precisely the 78 bytes between this entry point and the next.
.assert vram_fill_end == $EAD2
vram_fill_end:
; --- $EAD2: fill whole pages of RAM ----------------------------------------
;
; 27 of the 114 corpus titles call it. Measured: a = the fill byte, x = the
; first page, y = the last, both inclusive, so x=y fills one page. Dead Zone
; calls a=$F8 x=2 y=2 and fills $0200-$02FF; Adian a=0 x=2 y=7 fills
; $0200-$07FF; an injected a=$55 x=3 y=3 filled page 3 and left $02FF alone.
; The entry carry does not take part (tried both ways, identical).
;
; It touches no hardware at all. Pages go DOWNWARD from y to x, and inside a
; page $p00 is written first and then $pFF down to $p01, which is what a
; `sta ($00),y` loop starting at y=0 does. Every one of the 6172 accesses in a
; six-page call is accounted for by exactly that shape.
;
; Exit: a preserved (it goes on the stack and comes back), x=0, y=0, $00=0 and
; $01=x-1.
;
; Two cycles short of the real one at entry, and it leaves carry set where the
; real one leaves it clear. Both are deliberate rather than unnoticed: the
; measured entry is 24 cycles and no encoding I can find that produces the
; measured access order does it in fewer than 26. A fill of 2825 cycles a page
; is one and a half scanlines, so nothing raster-times through it, and no
; caller can sensibly branch on the carry out of a memory fill. Being FASTER
; is the safe direction for a routine a game may run inside vblank; being
; slower is not, and this is not slower.
;
; Not defended, deliberately: x > y. On the real BIOS the count wraps toward
; 256 pages, the fill reaches zero page, overwrites its own pointer and the
; machine destroys itself. No corpus title does it. We do the same thing for
; the same reason, because a clamp the original lacks is its own bug.
.org $EAD2
  jmp mem_fill              ; see the body below: it does not fit in the 24
                            ; bytes between here and the next entry point, and
                            ; the real BIOS puts its bulk elsewhere too

; ===========================================================================
; Routine bodies that do not fit between their entry points
; ===========================================================================
;
; Three cycles of `jmp` each. That makes these three slower to start and, with
; the savings below, still comfortably faster overall than the routines they
; replace, which is the safe direction: a game that runs one of these inside
; vblank has more room than it had, never less.
.org $FB80

mem_fill:
  pha                       ; a survives the call; the stack is where it waits
  sty $01                   ; the page pointer starts at the LAST page
  txa
  eor #$FF
  sec
  adc $01                   ; a = y - x
  tax
  inx                       ; x = the page count, y - x + 1
  pla
  ldy #$00
  sty $00
@page:
  sta ($00),y               ; y walks 0, $FF, $FE ... $01: measured order
  dey
  bne @page
  dec $01
  dex
  bne @page
  rts

; One strobe-and-shift pass, shared by $EA1F and $EA4C.
;
; Split out because $EA4C is measurably this routine run twice with a compare
; between, so the two entry points are one read pass and two different things
; done with it. Leaves the merged held state in $F5 and $F6 and the raw
; expansion bytes in $00 and $01.
pads_raw:
  ldy $FB
  iny
  sty JOY1                  ; strobe on:  $FB + 1
  dey
  sty JOY1                  ; strobe off: $FB
  ldy #$07                  ; the bit counter is Y, deliberately, so that X
                            ; survives a call. $EA4C stashes its first answer
                            ; in X across a second call to this, and when the
                            ; counter was X that stash was silently destroyed
                            ; and the two answers could never agree: the
                            ; routine spun forever instead of returning.
@bit:
  lda JOY1
  lsr a                     ; bit 0 is the pad that is wired to the port
  rol $F5
  lsr a                     ; bit 1 is the expansion port
  rol $00
  lda JOY2
  lsr a
  rol $F6
  lsr a
  rol $01
  dey
  bpl @bit                  ; eight bits; falls out with N set
  rts                       ; carry is bit 0 of $01 as it came in, rotated out

pads_held:
  jsr pads_raw
  lda $00                   ; the expansion pad merges into the wired one
  ora $F5
  sta $F5
  lda $01
  ora $F6
  sta $F6
  rts

read_down:
  jsr pads_raw
  bmi pads_diff2            ; always: pads_raw returns with N set
read_pads:
  jsr pads_held
pads_diff2:
  ldx #$01                  ; pad 2 then pad 1, so a and y end as pad 1's
@diff:
  lda $F5,x                 ; newly pressed = held now AND NOT held before
  tay
  eor $F7,x
  and $F5,x
  sta $F5,x
  sty $F7,x
  dex
  bpl @diff                 ; x falls out $FF, with N set and Z clear
  rts
; --- $EAEA: write the scroll and PPUCTRL from their zero-page shadows -------
;
; 32 of the 114 corpus titles call it, and all 32 look identical under the
; profiler: one $2002 read, reads of $FD, $FC and $FF, two $2005 writes and one
; $2000 write, no RAM written at all, 31 cycles, rts.
;
; $FF is the PPUCTRL shadow the BIOS shares across routines; $FD and $FC are the
; horizontal and vertical scroll shadows. $FD goes out first, so it is X.
; Forcing $FD=$11 and $FC=$22 at the call produced $2005=$11 then $2005=$22,
; which is what settles which is which.
;
; PPUCTRL is copied VERBATIM here, with no mask: forced $FF=$14 wrote $14 and
; forced $FF=$FF wrote $FF. That is the difference from $EA84 below, which
; masks bit 2 and writes the masked value back.
;
; The per-instruction rhythm is 4,3,4,3,4,3,4,6, which adds to the measured 31
; and admits exactly this stream.
.org $EAEA
set_scroll:
  lda PPUSTATUS             ; reset the write latch; the value is not used
set_scroll_latched:
  ; $EAED: the same without the status read, for a caller that has reset the
  ; latch itself. Bodycon Quest I and Fruits Mahjong call it from their NMIs.
.assert set_scroll_latched == $EAED
  lda $FD
  sta PPUSCROLL             ; first write is X
  lda $FC
  sta PPUSCROLL             ; second is Y
  lda $FF
  sta PPUCTRL
  rts
; --- $EAFD: jump through a table of addresses written after the call --------
;
; 11 of the 114 corpus titles call it, 5713 calls between them, and 7 titles
; were stopped on it. A jump-table dispatcher, and the shape games use for a
; state machine: `jsr $EAFD` with `a` holding the state, followed inline by a
; list of 16-bit addresses.
;
;     lda state
;     jsr $EAFD
;     .word handler_0, handler_1, handler_2, ...
;
; `a` is the index. It doubles it, pulls its OWN return address off the stack,
; and reads the selected entry from the table through that pointer, so the
; byte at offset 2a+1 past the return address is the low byte of the target
; and 2a+2 the high byte.
;
; **It consumes the call frame.** Two `pla`s and no push, so the stack comes
; back two bytes shallower and the target runs as though it had been called by
; whoever called the caller. A target's own `rts` returns past the `jsr $EAFD`
; entirely. This is a tail call, not a call.
;
; There is no bounds check of any kind, which is measured rather than assumed:
; forced `a=$80` doubles to `$00` and quietly dispatches entry 0 with carry
; set, and forced `a=$FF` reads offset `$FF` and then wraps `y` to `$00` and
; takes the high byte from offset 0 of the same page. A game that passes a
; state number past the end of its own table jumps wherever the bytes there
; happen to point, on the original exactly as here.
;
; Exit, as the target sees it: `a` = the high byte of the target address, `x`
; = the low byte, `y` = 2a+2 from the entry value, and `$0000`/`$0001` hold the
; target. N and Z come from that high byte. **Carry is bit 7 of the `a` that
; came in**, which is the doubling leaking into the flags rather than anything
; meaningful, and it has to be reproduced because it is what the original
; leaves. V, D and I are preserved.
;
; 45 cycles, min=max across all callers and every forced index. Ours is the
; same 45, which was not aimed at: the count fell out of the instruction
; sequence the access trace forced, and matching it exactly is the evidence
; that the sequence is right rather than merely equivalent.
.org $EAFD
jump_table:
  asl a                     ; 2   index -> byte offset; carry is bit 7 of a,
                            ;     which the original leaks too
  tay                       ; 2
  iny                       ; 2   y = 2a+1, the low byte of the entry
  pla                       ; 4   our own return address, consumed
  sta $00                   ; 3
  pla                       ; 4
  sta $01                   ; 3
  lda ($00),y               ; 5   low byte of the target
  tax                       ; 2
  iny                       ; 2
  lda ($00),y               ; 5   high byte, and N and Z come from here
  sta $01                   ; 3
  stx $00                   ; 3
  jmp ($0000)               ; 5
; 2+2+2+4+3+4+3+5+2+2+5+3+3+5 = 45
.assert (jump_table_end - jump_table) == 22
jump_table_end:
; --- $EBAF: copy 16-byte units from RAM into video memory ------------------
;
; 29 of the 114 corpus titles call it. Measured with the profiler plus forced
; registers across several callers:
;
;   a = the VRAM address low byte, y = its high byte
;   x = how many 16-byte units to move; x=0 means 256, which is how BurgerTime
;       and Deep Dungeon move 4096 bytes in one call
;   the two bytes after the caller's `jsr` are the SOURCE address, little
;   endian, and the routine steps its own return address over them
;
; The argument order was pinned by the first three writes it makes: a caller
; entering a=$C0 x=$1C y=$20 wrote $C0, $1C and $20 to $04, $02 and $03 in that
; order, then set $2006 to $20,$C0 and moved 448 bytes, which is 28 units.
;
; $FF is the shared PPUCTRL shadow. It is masked with #$FB, forcing the VRAM
; increment to +1 and leaving every other bit alone including NMI enable, and
; the masked value is written to the SHADOW FIRST and then to $2000. Poking
; $FF=$FF before a call produced writes of $FB; poking $04 produced $00.
;
; It does not wait for vblank and does not disable NMI, so a game that calls it
; with rendering on gets what it asked for.
;
; Exit: a is the high byte of the first address past the source data, which
; falls out of the pointer advance rather than being computed; x=0, y=0, and
; Z=1 N=0 C=0.
;
; We are faster than the original, which spends about 362 cycles a unit against
; our ~282 by running each unit as two called halves. Faster is the safe
; direction for a routine a game may run inside vblank: it finishes earlier
; than the caller budgeted for, never later.
.org $EBAF
; Two bodies, chosen by the mode bits in a's low nibble. A plain copy (all
; four clear) takes the original routine, verified on 29 titles; anything else
; takes vram_upload2, which does the fill, interleave and read-back modes as
; measured. Bubble Bobble is why the split exists: it runs a task switcher off
; its NMI, makes a 213-unit plain copy over two frames long, and halted when
; that copy ran 3200 cycles slower in the general routine than in this one.
; Both were faster than the original; which NMI lands inside the call is what
; changed. Section-Z is why the modes exist: it expands a 1-bit font with a=$04.
  pha
  and #$0F
  bne @modes
  pla
  jmp vram_upload
@modes:
  pla
  jmp vram_upload2

.org $FEB0
vram_upload:
  sta UP_LO                 ; the three arguments, in the order measured
  stx UP_UNITS
  sty UP_HI

  ; ---- the inline source pointer, and the return address past it ----
  tsx
  lda $0101,x               ; the stacked return address points at the last
  sta UP_RET                ; byte of the jsr, so the pointer is at +1 and +2
  lda $0102,x
  sta UP_RET+1
  ldy #1
  lda (UP_RET),y
  sta UP_SRC
  iny
  lda (UP_RET),y
  sta UP_SRC+1
  lda UP_RET                ; step the caller over the two operand bytes
  clc
  adc #2
  sta $0101,x
  lda UP_RET+1
  adc #0
  sta $0102,x               ; $05/$06 keep the PRE-bump copy, as measured

  bit PPUSTATUS
  lda $FF
  and #$FB                  ; force the VRAM increment to +1, touch nothing else
  sta $FF                   ; shadow first, then the register: measured order
  sta PPUCTRL
  lda UP_HI
  sta PPUADDR
  lda UP_LO
  sta PPUADDR
  lda #0
  sta UP_HI                 ; $03 is left zero

@unit:
  ldy #0
@byte:
  lda (UP_SRC),y
  sta PPUDATA
  iny
  cpy #16
  bne @byte
  lda UP_SRC                ; advance by a unit. The adc is why a ends up as
  clc                       ; the past-end pointer's high byte, which is what
  adc #16                   ; the real routine returns.
  sta UP_SRC
  lda UP_SRC+1
  adc #0
  sta UP_SRC+1
  dec UP_UNITS              ; x=0 on entry therefore means 256 units
  bne @unit
  ldx #0
  ldy #0                    ; sets Z=1 and N=0, the measured exit flags
  rts
.org $EBFE
; ---- the sound chip at power-up ------------------------------------------------
;
; Silence the APU before anything can be heard, then turn sound back on the
; way the original leaves it, measured from its writes: 5-step frame counter,
; the four 2A03 channels enabled, the FDS volume envelope off and its
; envelope clock at $E8. Some games never write $4015 themselves (Metroid
; played only its FDS channel without it), and with $408A at 0 no FDS
; envelope ever moves. Out here because reset has no room for it.
apu_init:
  lda #0
  sta APUSTATUS
  ldx #0
@quiet:
  sta $4000,x
  inx
  cpx #$14
  bne @quiet
  lda #$C0
  sta APUFRAME
  lda #$0F
  sta APUSTATUS
  lda #$80
  sta $4080
  lda #$E8
  sta $408A
  rts

.org $EC22
; --- $EC22: draw a metasprite into the sprite page --------------------------
;
; 3 of the 114 corpus titles call it and two were stopped on it, Druid and
; Youkai Yashiki; the third, Kaettekita Mario Bros., calls it 1386 times once
; it is playing. The measurement is in docs/notes/FDS-ROUTINES-PENDING.md. It
; touches no hardware: it writes OAM entries into page 2 and borrows $02-$0C.
;
; a, x and y are not arguments. $00/$01 point at a twelve-byte block:
;
;   +00 control: $00 draws, bit 7 draws with every Y at $F4, anything else
;       returns at once having drawn nothing
;   +01 Y of the top row            +03 X of the left strip
;   +05 frame: the tile source moves on by frame * rows * strips
;   +06 +07 tile source, high then low. After the frame is added, a ZERO high
;       byte makes the low byte a base tile number counting up, and anything
;       else makes the pair a pointer to one tile index per sprite
;   +08 flips: bit 0 vertical, bit 4 horizontal
;   +09 attribute, masked with $23     +0A rows high nibble, strips low
;   +0B where in page 2 the first entry goes
;   (+02 and +04 are never read)
;
; Sprites go strip by strip, left to right, and top to bottom within a strip,
; Y stepping 8 and X stepping 8, except that a row at $F4, the off-screen Y,
; stays there instead of stepping on: $EC $F4 $F4, but $E5 $ED $F5 $FD $05.
; That is also all the hide bit is: it starts every strip at $F4. A flip sets
; the hardware flip bit and walks the tiles from the other end: vertical
; reverses them within each strip and horizontal reverses the order of the
; strips. No sprite moves.
;
; Two passes, like the original, and the split is observable: Y, attribute
; and X go through ($02),y with y wrapping at 256, so a grid that runs past
; the end of the page carries on into $03xx, while the tiles go through
; $0200,x and wrap WITHIN the page. Measured with 120 sprites from $F8: Y,
; attribute and X reached $03F7 and the tiles landed from $0201.
;
; The exit state was fitted against the real BIOS over about 1100 forced
; blocks, every field and all four flip settings. x is $00 and Z is set on
; every drawn path. a, y, carry and overflow come from the last sum the
; original does, which differs per flip, and each tile loop below is shaped so
; that its own last sum is that one:
;
;   none        a = $02, y = base + n   flags of (off + 4n - 3) + 4
;   both flips  a = $02, y = base - 1   the same
;   vertical    a = y = base + n        flags of (base + n - r) + r
;   horizontal  a = y = base - 1        flags of (base + r - 1) - r
;
; where n is rows * strips, r is rows, and base is the base tile in literal
; mode and 0 in pointer mode. $05 ends as the strip count on the first two and
; 0 on the others, $0B as 0 and as n, and $0C as r except -r for horizontal.
; The early exits leave x and overflow as the caller had them, with carry
; holding bit 3 of +0A out of the shifts. That is why the tile loops test the
; mode with lda rather than bit, which would overwrite overflow.
;
; Never slower than the original, which costs about 206 + 131 a sprite. This
; is about 192 + 87 a sprite + 21 a strip unflipped: 300 against 337 for one
; sprite, 756 against 1008 for Kaettekita's six, 2364 against 3382 for
; Youkai's 24. The margin is thinnest at a lone sprite, and thinnest of all,
; 3 cycles over 1100 swept, when it is hidden, flipped vertically and its
; literal base has been carried into pointer mode by the frame; the original
; is quicker when hiding. That is what the one-sprite path through the frame
; add is for, and why the tile loops test the mode inline rather than paying
; to copy a pointer.
metasprite:
  ldy #$0B
  lda ($00),y
  sta $02                   ; the OAM pointer, $0200 + offset
  lda #$02
  sta $03
  dey
  lda ($00),y               ; the shape
  lsr a
  lsr a
  lsr a
  lsr a                     ; carry is now bit 3, which the exits keep
  beq @out                  ; no rows: a=0, y=$0A, x untouched
  sta $04
  sta $0C                   ; every exit from here on leaves the rows here
  lda ($00),y
  and #$0F
  beq @out                  ; no strips: a=0, y=$0A, x untouched
  sta $05
  ldy #$01
  lda ($00),y
  tax                       ; x = the Y origin, and the control exit's x
  dey
  lda ($00),y               ; the control byte
  beq @show
  bmi @hide
@out:
  rts                       ; a = +00, x = +01, y = 0
@hide:
  ldx #$F4                  ; bit 7: start at $F4, which then never moves
@show:
  stx $06                   ; the Y of the top row

  ; ---- the attribute: (+08 bit 0 -> $80) | (+08 bit 4 -> $40) | +09 & $23
  ldy #$08
  lda ($00),y
  sta $0C                   ; the flips, parked until the tiles
  and #$11
  tax
  iny
  lda ($00),y
  and #$23
  ora flip_bits,x
  sta $09
  ldy #$03
  lda ($00),y
  sta $0A                   ; X of the current strip
  lda $05
  sta $07                   ; strips left

  ; ---- pass 1: Y, attribute and X, through ($02),y ----
  ldy #$00
@strip:
  lda $06
  sta $08                   ; Y of the current row
  ldx $04
@first:                     ; the top row of a strip takes no step
  sta ($02),y
  iny
  iny                       ; the tile byte belongs to pass 2
  lda $09
  sta ($02),y
  iny
  lda $0A
  sta ($02),y
  iny
  dex
  beq @strip_done
  lda $08
  cmp #$F4                  ; $F4 stays $F4
  beq @first
  clc
  adc #$08
  sta $08
  jmp @first
@strip_done:
  dec $07
  beq @pass1_done
  lda $0A
  clc
  adc #$08
  sta $0A
  jmp @strip
@pass1_done:

  ; ---- n = rows * strips, at most 225, so the adds never carry ----
  lda $04
  ldx $05
  dex
  beq @n_done
  clc
@n:
  adc $04
  dex
  bne @n
@n_done:
  sta $0B

  ; ---- the tile source, moved on by frame * n ----
  ldy #$07
  lda ($00),y
  sta $07
  dey
  lda ($00),y
  sta $08
  dey
  lda ($00),y
  beq @moved                ; frame 0, the common case
  ldx $0B
  dex
  bne @multi
  clc                       ; one sprite: the frame is the whole product
  adc $07
  sta $07
  bcc @moved
  inc $08
  bcs @moved                ; always: inc leaves the carry set
@multi:
  sta $06                   ; add the frame n times, sixteen bits, the sum
  clc                       ; kept in a and the first add taken out of the
  adc $07                   ; loop so the count in x is n - 1
  bcc @add
  inc $08
  clc
@add:
  adc $06
  bcc @nc
  inc $08
  clc
@nc:
  dex
  bne @add
  sta $07

  ; ---- the mode, decided AFTER the frame is added: a literal base can
  ; spill into pointer mode. y starts at base, x at the first tile byte.
@moved:
  ldy #$00
  ldx #$00
  lda $08
  bne @ptrmode
  ldy $07                   ; literal: base = the tile number
  dex
@ptrmode:
  stx $0A                   ; $FF literal, $00 pointer
  ldx $02
  inx
  lda $0C
  and #$11
  beq @none
  cmp #$01
  beq @vert_in
  cmp #$10
  bne @both
  jmp @horiz

  ; ---- no flip: every tile, first first ----
@none:
  lda $0A
  beq @n_ptr
  tya                       ; literal: the tile is its own number
  jmp @n_have
@n_ptr:
  lda ($07),y
@n_have:
  sta $0200,x
  iny
  txa
  clc
  adc #$04                  ; the last of these is where the flags come from
  tax
  dec $0B
  bne @none                 ; y = base + n, $0B = 0
@linear:
  lda $04
  sta $0C
  stx $02                   ; offset + 4n + 1
  txa
  ldx #$00                  ; x = 0, and Z set, N clear
  stx $06
  rts

  ; ---- both flips: every tile, last first ----
@both:
  tya
  clc
  adc $0B
  tay
  dey                       ; base + n - 1
@b_tile:
  lda $0A
  beq @b_ptr
  tya
  jmp @b_have
@b_ptr:
  lda ($07),y
@b_have:
  sta $0200,x
  dey
  txa
  clc
  adc #$04
  tax
  dec $0B
  bne @b_tile
  beq @linear               ; y = base - 1

  ; ---- vertical: strips in order, each read bottom up ----
@vert_in:
  sty $06                   ; base
@vert:
  lda $04
  sta $0C                   ; rows left. A counter rather than a cpy, which
  lda $06                   ; would overwrite the carry the add below leaves
  clc
  adc $04                   ; the last of these is where the flags come from
  sta $06
  tay
@v_tile:
  dey
  lda $0A
  beq @v_ptr
  tya
  jmp @v_have
@v_ptr:
  lda ($07),y
@v_have:
  sta $0200,x
  inx
  inx
  inx
  inx
  dec $0C
  bne @v_tile
  dec $05
  bne @vert
  lda $04
  sta $0C
  stx $02                   ; offset + 4n + 1
  lda $06                   ; base + n
  tay
  ldx #$00
  stx $06
  rts

  ; ---- horizontal: strips last first, each read top down ----
@horiz:
  lda #$00
  sec
  sbc $04
  sta $02                   ; -rows. $02 is free until it is rebuilt from x
  tya
  clc
  adc $0B
  sec
  sbc #$01
  sta $06                   ; base + n - 1
@h_strip:
  lda $02
  sta $0C                   ; -rows, counted up to zero
  lda $06
  sec
  sbc $04                   ; the last of these is where the flags come from
  sta $06
  tay
@h_tile:
  iny
  lda $0A
  beq @h_ptr
  tya
  jmp @h_have
@h_ptr:
  lda ($07),y
@h_have:
  sta $0200,x
  inx
  inx
  inx
  inx
  inc $0C
  bne @h_tile
  dec $05
  bne @h_strip
  lda $02
  sta $0C                   ; -rows on the way out
  stx $02                   ; offset + 4n + 1
  lda $06                   ; base - 1
  tay
  ldx #$00
  stx $06
  rts

; The hardware flip bits, indexed by +08 & $11: bit 0 is vertical and becomes
; $80, bit 4 is horizontal and becomes $40.
flip_bits:
  .byte $00, $80, $00, $00, $00, $00, $00, $00
  .byte $00, $00, $00, $00, $00, $00, $00, $00
  .byte $40, $C0

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
  bvc @none                 ; both clear: a vint_wait is parked on the stack
  jmp (VEC_NMI1)

; ---- $0100 = $00: finish the wait that $E1B2 started ----------------------
;
; This arm was an `rti` and that was wrong. The doctrine table called this row
; "never observed; we return", which it no longer is: profiling $E1B2 showed
; what a zero selector actually means. It is not "no handler installed", it is
; "somebody is parked in vint_wait and the NMI owns their exit". So this does
; not return to the interrupted spin, it throws the interrupt frame away and
; returns to whoever called $E1B2, several frames up the stack.
;
; Measured: the shadow rule is exactly `and #$7F`, with a forced $FF=$7F
; writing $FF to $2000 and then $7F back, so bit 7 is the only bit this ever
; removes. $2002 is read once, which eats the vblank flag and resets the write
; latch. The selector is RESTORED from the stack rather than reset to $C0: a
; caller that entered with $0100=$80 came back out with $80.
;
; It also closes a race for nothing: an NMI arriving between vint_wait's
; `sta $0100` and its `sta PPUCTRL`, which a caller with NMI already enabled
; can cause, lands here and still returns correctly.
@none:
  lda $FF
  and #$7F                  ; NMI off; every other bit of the shadow survives
  sta $FF
  sta PPUCTRL
  lda PPUSTATUS             ; eat the vblank flag and reset the write latch
  pla                       ; the interrupt frame: P, then PCL, then PCH
  pla
  pla
  pla                       ; the selector vint_wait pushed
  sta $0100
  pla                       ; the caller's a, which sets the exit N and Z
  rts                       ; to vint_wait's caller, not to the spin

; The layout above is not free-floating: these are the addresses the real
; handler's own control flow visits, so a game that has somehow learned them
; still lands on the right thing. The exit arm's length is load-bearing too:
; 21 bytes from $E19D is exactly $E1B2, the entry it serves.
.assert @vector2 == $E195
.assert @low == $E198
.assert @none == $E19D
.assert nmi_arm_end == $E1B2
nmi_arm_end:

; ===========================================================================
; $E1C7: the IRQ handler
; ===========================================================================
;
; $0101 is NOT a two-way selector, and the symmetry with $0100 that this
; comment used to claim was wrong. Measured by forcing it and watching where
; control goes:
;
;   $C0   hand the interrupt to the game, through $DFFE. 14 cycles, a plain
;         `jmp`. Seen live on Bio Miracle and Arumana no Kiseki.
;   $80   164 cycles, and it READS $4030. That is the drive's status register,
;         so this is the BIOS asking whether the drive caused the interrupt.
;   $40   dives into the BIOS's own transfer machinery around $E6A6. Seen live
;         on Akumajou Dracula, whose interrupt arrives while the BIOS is
;         already running.
;   $00   33 cycles, a third path.
;
; So $0101 is the state machine of the BIOS's own disk-transfer interrupt, and
; only $C0 means "this one is the game's". That matches what profiling $E1F8
; found independently: it uses $0101 throughout a load as its transfer state,
; holding $40 per block and counting $08 down to $00 in the gaps.
;
; **Games drive that machinery themselves.** Our own loaders poll and never
; arm the byte interrupt, so for a while this dispatched on bit 7 alone. Super
; Lode Runner proved that wrong: it carries its own loader, sets $0101 = $40
; and takes every disk byte through this handler. Returning without reading
; $4031 left the interrupt asserted, it fired 4.7 million times, and the game
; never got an NMI again. So every range is now served, as measured by forcing
; $0101 at that game's first interrupt:
;
;   $00       rti, nothing read, nothing acknowledged
;   $01-$3F   read and drop one byte, count $0101 down, rti
;   $40-$7F   the byte from $4031 in a and x, the caller's a to $4024, then
;             the interrupt frame is dropped and the rts returns from the
;             routine the game was waiting in: N and Z from the byte, V set
;   $80-$BF   read $4030 to acknowledge, rti, 164 cycles as the original
;   $C0-$FF   the game's, through $DFFE
.org $E1C7
irq:
  bit $0101                 ; N = bit 7, V = bit 6
  bmi @high
  bvc @skip
  ; $40-$7F: one byte of a transfer. The byte comes back in a and x, the
  ; caller's a goes to the write register, and the interrupt's own frame is
  ; dropped so the rts returns from the routine the game was waiting in.
  ldx FDS_DATA
  sta FDS_WRITE
  pla
  pla
  pla
  txa                       ; N and Z from the byte; V set by the bit above
  rts
@skip:                      ; $00-$3F: skip a byte, $0101 counts them down
  pha
  lda $0101
  beq @skip_out             ; zero: nothing read, nothing acknowledged
  dec $0101
  lda FDS_DATA              ; reading the byte acknowledges it
@skip_out:
  pla
  rti
@high:
  bvc @ack
  jmp (VEC_IRQ)             ; $C0-$FF: the game's
@ack:                       ; $80-$BF: acknowledge through the status read
  pha
  lda FDS_STATUS
  jsr $E149                 ; the original's 164 cycles: 27 + 6 + 131
  pla
  rti
irq_end:
.assert irq_end <= $E1F8

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
  lda #$10
  sta PPUADDR
  lda #$00
  sta PPUADDR             ; pattern table 1, tile 0
  lda #<font
  sta ptr
  lda #>font
  sta ptr+1
  ldx #FONT_TILES
@tile:
  ldy #0
  lda #$FF
@plane0:
  sta PPUDATA
  iny
  cpy #8
  bne @plane0
  ldy #0
@plane1:
  lda (ptr),y
  eor #$FF
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
  jmp blank_nt

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
  lda #$10
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
  jsr warm_check
  nop
  nop
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

  jsr apu_init

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
  jsr arm_c0                ; clears the screen, then NMI selector $C0
  jsr load_disk
  bcs fail

  ; The PPU and its shadows as the original leaves them, rendering off.
  jsr ho_ppu

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
  lda #0
  sta booted

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
  ; A side with no file at or below its boot code (the second disk of a
  ; two-disk game, a data disk) loads nothing, and the real BIOS stops on
  ; ERR.20 rather than jump through vectors nothing wrote. Twelve disks in the
  ; merged corpus are this; handing over sent every one of them into the stack.
  lda booted
  beq @bad
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
  jsr info_byte             ; read_byte, and refuse anything but side A, disk 1
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
  inc booted

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
  .byte $0F, $30, $10, $0F      ; black, white, grey, black
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

; Strings are font indices, terminated by $FF: digits are 0-9, A-Z $0A-$23,
; and a space is $24, the blank, as in every licence file.

str_title:
  .byte 15, 10, 22, 18, 27, 30, 28, 29, $FF        ; FAMIRUST
str_sub:
  .byte 13, 18, 28, 20, 36, 28, 34, 28, 29, 14, 22, $FF   ; DISK SYSTEM
str_no_disk:
  .byte 18, 23, 28, 14, 27, 29, 36, 13, 18, 28, 20, 36, $FF     ; INSERT DISK
str_loading:
  .byte 36, 36, 21, 24, 10, 13, 18, 23, 16, 36, 36, 36, $FF        ; LOADING
str_bad_disk:
  .byte 36, 13, 18, 28, 20, 36, 14, 27, 27, 24, 27, 36, $FF     ; DISK ERROR

str_no_routine:
  .byte 36, 23, 24, 36, 27, 24, 30, 29, 18, 23, 14, 36, $FF     ; NO ROUTINE

; They are all the same width, so one overwrites another completely.
.assert (str_loading - str_no_disk) == STATUS_LEN + 1
.assert (str_bad_disk - str_loading) == STATUS_LEN + 1
.assert (str_no_routine - str_bad_disk) == STATUS_LEN + 1

; ---------------------------------------------------------------------------
; The font
; ---------------------------------------------------------------------------
;
; Digits $00-$09, A-Z $0A-$23, blank $24, then four marks. One bit plane,
; eight bytes a glyph. `load_font` puts them in pattern table 1 as the
; original leaves its own: colour 1 glyphs on a colour 3 cell.
.org $E000
  .byte $00                 ; $E000 reads zero: Super Lode Runner sums $6000
                            ; through $E000 and checks the total
font:
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
  ; $24: blank
  .byte %00000000,%00000000,%00000000,%00000000,%00000000,%00000000,%00000000,%00000000
  ; $25: comma
  .byte %00000000,%00000000,%00000000,%00000000,%00000000,%00011000,%00011000,%00110000
  ; $26: full stop
  .byte %00000000,%00000000,%00000000,%00000000,%00000000,%00011000,%00011000,%00000000
  ; $27: apostrophe
  .byte %00011000,%00011000,%00110000,%00000000,%00000000,%00000000,%00000000,%00000000
  ; $28: registered mark
  .byte %00111100,%01000010,%10111010,%10100110,%10111010,%10100110,%01000010,%00111100
font_end:
; The font is an interface, not just our boot screen's: games read it straight
; out of the ROM. Dig Dug, Dig Dug II, Galaga, Galaxian, Pac-Man and Xevious
; read $E001-$E148 under the original, and Section-Z expands it into pattern
; RAM through $EBAF from $E001. 41 glyphs, one plane, in the index order every
; licence file uses: digits, A to Z, blank at $24, then four marks. The glyph
; ART is ours; only where each one sits is the interface.
.assert font == $E001
.assert font_end == $E149
FONT_BYTES = font_end - font
FONT_TILES = FONT_BYTES / 8

; The font has to be a whole number of glyphs, or the last one is uploaded from
; whatever bytes follow it.
.assert (FONT_TILES * 8) == FONT_BYTES
; And it has to fit in a byte, because X counts the glyphs.
.assert FONT_TILES <= 255


; ===========================================================================
; $E1F8: LoadFiles
; ===========================================================================
;
; The general "load files from disk" entry, and after the NMI handler the joint
; most-called routine in the corpus: 41 of the 114 titles. A game uses it to
; pull in a level or a new bank once it is already running.
;
; The interface was measured, not read, and the whole of it is written up in
; docs/notes/FDS-LOADFILES.md. The short version:
;
;   Two little-endian pointer words sit INLINE after the `jsr`, and the routine
;   rewrites the stacked return address to step over them, so it returns to
;   jsr + 5. It has to be a real pop, read, push-back-plus-four: Topple Zip
;   enters by `jmp` with a word it arranged on the stack itself, so a version
;   that peeked at sp+1/sp+2 without rewriting would break it.
;
;   First word  -> a 10-byte disk-ID template, compared against block-1 bytes
;                  $0F-$18. A template byte of $FF is a per-byte wildcard.
;   Second word -> a file-ID list, one byte each, terminated by $FF. A file
;                  loads if its ID appears anywhere in the list. A list whose
;                  FIRST byte is $FF means the boot set instead: every file
;                  whose ID is at most the boot read file code at disk-info
;                  byte $19, the same rule the boot loader uses.
;
;   Out: a = x = $00 on success, or the error number in BCD. y = how many file
;   bodies were loaded, also left in $0E. A requested file that is not on the
;   side is NOT an error, so a caller that cares has to look at y.
;
; The list is re-read from RAM at every header, so a file that loads over the
; list changes the selection for the files after it. That is real behaviour and
; comes free from not caching it.
;
; ---------------------------------------------------------------------------
; Two places this deliberately differs
; ---------------------------------------------------------------------------
;
; The real routine transfers under the byte IRQ with $0101 as its state
; machine. This polls, as our boot loader does. Observably equivalent for every
; caller profiled: none of them runs game code inside the call, the drive
; cannot overrun because it will not advance past a byte the CPU has not taken,
; and $0101 is saved and restored either way. Written down as an assumption
; because it is one about the other 36 callers.
;
; The real prelude spends about 1.67 million cycles on counted delays around
; the motor. Ours polls the drive and gets on with it, because our drive model
; spins up in a few hundred cycles rather than a physical second. That makes us
; faster, which is the safe direction for everything but interrupt dispatch.
;
; ---------------------------------------------------------------------------
; Zero page
; ---------------------------------------------------------------------------
;
; $00-$0E, which is where the real routine works, and the exit values are part
; of what a game can see. This is NOT the boot loader's scratch at $E0-$E8:
; those cells are boot-only and are the game's RAM by the time anything calls
; this.
LF_P1     = $00           ; 2: disk-ID template pointer, left behind
LF_P2     = $02           ; 2: file-ID list pointer, left behind
LF_FRAME  = $04           ; entry stack pointer minus 3
LF_K05    = $05           ; file kind while looping; ends $02, as measured
LF_LEFT   = $06           ; files still to walk; ends $00
LF_BLK    = $07           ; current block type; ends $04
LF_BOOT   = $08           ; boot read file code, from disk-info byte $19
LF_SKIP   = $09           ; $FF if the last file was skipped, $00 if loaded
LF_DST    = $0A           ; 2: destination pointer
LF_CNT    = $0C           ; 2: bytes left in the block; ends $FFFF
LF_LOADED = $0E           ; files loaded; returned in y

; The real routine builds EVERY $4025 value from this shadow, which is how the
; game's mirroring bit survives a load. We read it and never write it until the
; epilogue, so the low nibble left behind is the caller's own.
LF_SHADOW = $FA
LF_MASK   = $FE           ; PPUMASK shadow, for blanking before a PPU file

; $EBAF's working set. Separate names from LoadFiles' because they are
; different routines that happen to share the cheap end of zero page.
UP_SRC    = $00           ; 2: source pointer; ends past the end of the data
UP_UNITS  = $02           ; units left; ends $00
UP_HI     = $03           ; VRAM address high on the way in; ends $00
UP_LO     = $04           ; VRAM address low, left as passed
UP_RET    = $05           ; 2: the caller's return address before the bump

FDS_EXT   = $4026
FDS_BATT  = $4033

; A disk that will not read. NOT measured: the real error numbers for a CRC
; failure, a truncated side, no disk and a flat battery cannot be reached from
; a disk that also boots, and settling them needs fault injection in fds.rs.
; This is a placeholder that is at least in the right range and at least
; nonzero, so a caller's `beq` still sees failure.
LF_ERR_IO = $21

.org $E1F8
  jmp load_files

.org $FC40

load_files:
  ; The frame anchor the real routine leaves at $04. Measured on two callers
  ; with different entry stack pointers, so it is the offset that is fixed.
  tsx
  dex
  dex
  dex
  stx LF_FRAME

  ; ---- the two inline pointer words, and the return address past them ----
  ; $0C/$0D is the scratch: it is the byte counter later and is not wanted
  ; until the first block 4.
  pla                       ; the stacked return address, low byte first
  sta LF_CNT
  pla
  sta LF_CNT+1
  ldy #1
  lda (LF_CNT),y
  sta LF_P1
  iny
  lda (LF_CNT),y
  sta LF_P1+1
  iny
  lda (LF_CNT),y
  sta LF_P2
  iny
  lda (LF_CNT),y
  sta LF_P2+1
  lda LF_CNT                ; step the return address over the four bytes
  clc
  adc #4
  sta LF_CNT
  lda LF_CNT+1
  adc #0
  pha                       ; high byte back on first
  lda LF_CNT
  pha

  lda $0101                 ; the game's IRQ selector; restored on every exit
  pha

  lda #0
  sta LF_LOADED
  sta LF_K05                ; doubles as the retry count until the file loop

; ---- one attempt. A disk-ID mismatch comes back here exactly once ----------
@attempt:
  jsr lf_spin
  jsr lf_info
  bcs @failed
  jsr lf_count
  bcs @failed
@each_file:
  lda LF_LEFT
  beq @done
  dec LF_LEFT
  jsr lf_one_file
  bcc @each_file

@failed:
  ; Carry set and a holds the code. A disk-ID mismatch is worth one more go at
  ; the whole thing, which is what the real routine does.
  tax
  lda LF_K05
  bne @give_up
  inc LF_K05
  jmp @attempt
@give_up:
  txa
  ldy #0
  jmp lf_exit

@done:
  ldy LF_LOADED
  lda #0
  ; fall through

; ---------------------------------------------------------------------------
; The epilogue, taken by success and failure alike
; ---------------------------------------------------------------------------
; In: a = status, y = the loaded count. Out: a = x = status, y untouched.
lf_exit:
  tax                       ; x mirrors a, which is also x's contract value
  pla                       ; the IRQ selector saved at entry
  sta $0101
  lda LF_SHADOW             ; coast: motor turning, transfer off, IRQ off, and
  and #$0F                  ; the caller's low nibble kept, mirroring included
  ora #$23
  sta LF_SHADOW
  sta FDS_CTRL
  lda #$02                  ; the leftovers a game may look at
  sta LF_K05
  lda #$FF
  sta LF_CNT
  sta LF_CNT+1
  cli                       ; measured: interrupts are enabled on exit even
                            ; when the caller arrived with them masked
  ; Measured exit flags are N=0, V=1, C=0, Z per the status. `bit` on a byte
  ; with bit 6 set and bit 7 clear gives V and N; `txa` then overwrites N and Z
  ; from the status itself and leaves V and C alone.
  bit lf_vflag
  clc
  txa                       ; a = status, and its own N and Z with it
  rts

lf_vflag:
  .byte $40

; ---------------------------------------------------------------------------
; The drive, always built from the shadow so the mirroring bit survives
; ---------------------------------------------------------------------------
; Returns in a: read mode, transfer disarmed, the caller's mirroring.
lf_base:
  lda LF_SHADOW
  and #$08                  ; bit 3 is mirroring and is the game's business
  ora #$04                  ; bit 2: read
  rts

; Stop the motor, start it, wait for the drive. Only the off-to-on edge rewinds
; the head, and this routine always starts from the beginning of the side.
lf_spin:
  jsr lf_base
  ora #$02                  ; bit0 clear stops it, bit1 set means "do not start"
  sta FDS_CTRL
  lda $F9                   ; bit 7 of $F9 is marked and that goes to $4026,
  ora #$80                  ; measured on the loader and the writer: an OR,
  sta $F9                   ; since $11 planted came out $91
  sta FDS_EXT
  lda FDS_BATT              ; bit 7 is the battery; read as the real one does
  jsr lf_base
  ora #$01                  ; bit0 set, bit1 clear: start, and keep it running
  sta FDS_CTRL
@wait:
  lda FDS_DRIVE
  and #$03                  ; bit0 no disk, bit1 not ready, both active low
  bne @wait
  rts

; Move to the next block: drop the armed bit and raise it, which restarts the
; drive's gap scan.
lf_next:
  jsr lf_base
  ora #$01
  sta FDS_CTRL
  jsr lf_base
  ora #$41                  ; bit 6: transfer armed
  sta FDS_CTRL
  rts

; ---------------------------------------------------------------------------
; Block 1: check the disk is readable, then match the caller's template
; ---------------------------------------------------------------------------
lf_info:
  lda #$01
  sta LF_BLK
  jsr lf_next
  jsr read_byte
  bcs @io
  cmp #$01
  bne @io
  ldx #0                    ; x indexes the ten-byte template
  ldy #1                    ; y indexes the disk-info block
@byte:
  jsr read_byte
  bcs @io
  cpy #$0F
  bcc @magic
  cpy #$19
  beq @boot_code
  bcs @next
  ; $0F..$18: the disk ID, against (LF_P1) with $FF as a per-byte wildcard
  sta LF_SKIP               ; park the disk's byte; LF_SKIP is free until the
                            ; file loop gives it its real meaning
  tya                       ; y is the disk index and the only index register
  pha                       ; (ptr),y can use, so borrow it for the template
  txa
  tay
  lda (LF_P1),y
  cmp #$FF
  beq @wild
  cmp LF_SKIP
  bne @mismatch
@wild:
  pla
  tay
  inx
  jmp @next
@magic:
  ; bytes 1-14 are "*NINTENDO-HVC*", checked against our own copy rather than
  ; against anything read off the disk
  cmp lf_magic-1,y
  bne @io
  jmp @next
@boot_code:
  sta LF_BOOT
@next:
  iny
  cpy #56
  bne @byte
  clc
  rts
@mismatch:
  pla                       ; drop the saved disk index; we are leaving
  lda lf_iderr,x
  sec
  rts
@io:
  lda #LF_ERR_IO
  sec
  rts

; ---------------------------------------------------------------------------
; Block 2: how many files the side holds
; ---------------------------------------------------------------------------
lf_count:
  lda #$02
  sta LF_BLK
  jsr lf_next
  jsr read_byte
  bcs @io
  cmp #$02
  bne @io
  jsr read_byte
  bcs @io
  sta LF_LEFT
  clc
  rts
@io:
  lda #LF_ERR_IO
  sec
  rts

; ---------------------------------------------------------------------------
; One file: stream the header, decide, then take or discard the body
; ---------------------------------------------------------------------------
lf_one_file:
  lda #$03
  sta LF_BLK
  jsr lf_next
  jsr read_byte
  bcs @io
  cmp #$03
  bne @io
  lda #$FF
  sta LF_SKIP               ; assume passed over until the list says otherwise
  ldy #1
@hdr:
  jsr read_byte
  bcs @io
  cpy #$02
  beq @file_id
  cpy #$0B
  beq @addr_lo
  cpy #$0C
  beq @addr_hi
  cpy #$0D
  beq @size_lo
  cpy #$0E
  beq @size_hi
  cpy #$0F
  beq @kind
@hdr_next:
  iny
  cpy #16
  bne @hdr
  jmp @body
@file_id:
  jsr lf_wanted             ; decided here, so the ID never needs storing
  jmp @hdr_next
@addr_lo:
  sta LF_DST
  jmp @hdr_next
@addr_hi:
  sta LF_DST+1
  jmp @hdr_next
@size_lo:
  sta LF_CNT
  jmp @hdr_next
@size_hi:
  sta LF_CNT+1
  jmp @hdr_next
@kind:
  sta LF_K05
  jmp @hdr_next

@body:
  lda #$04
  sta LF_BLK
  jsr lf_next
  jsr read_byte
  bcs @io
  cmp #$04
  bne @io
  lda LF_SKIP
  beq @take
  jmp lf_discard
@take:
  inc LF_LOADED
  lda LF_K05
  beq @to_ram               ; kind 0 is program RAM
  jmp lf_to_ppu             ; kind 1 pattern RAM, 2 nametable: both PPU space
@to_ram:
  jmp lf_to_ram
@io:
  lda #LF_ERR_IO
  sec
  rts

; ---- is this file one the caller asked for? a holds its ID ----------------
;
; Leaves LF_SKIP $00 to take it, $FF to pass. The list is walked from the head
; every time, which is what makes a file loading over the list change the
; selection for the files after it. y is the caller's header index and has to
; come back untouched.
lf_wanted:
  sta LF_SKIP               ; park the ID; the verdict overwrites it
  tya
  pha
  ldy #0
@scan:
  lda (LF_P2),y
  cmp #$FF
  beq @end
  cmp LF_SKIP
  beq @take
  iny
  bne @scan
@pass:
  lda #$FF
  sta LF_SKIP
  jmp @out
@take:
  lda #$00
  sta LF_SKIP
  jmp @out
@end:
  cpy #0                    ; $FF as the FIRST entry means the boot set
  bne @pass
  lda LF_BOOT
  cmp LF_SKIP               ; boot code at least the file ID?
  bcs @take
  bcc @pass
@out:
  pla
  tay
  rts

; ---- move LF_CNT bytes into program RAM at LF_DST -------------------------
;
; LF_DST itself walks, and ends one past the last byte: measured on a file of
; $20 loaded at $0580, which left $0A/$0B at $05A0 under the original. Walking
; with y instead left it at the start, which nothing tested until the writer's
; read-back did.
lf_to_ram:
  ldy #0
@loop:
  lda LF_CNT
  ora LF_CNT+1
  beq @done
  jsr read_byte
  bcs @io
  sta (LF_DST),y
  inc LF_DST
  bne @nowrap
  inc LF_DST+1
@nowrap:
  lda LF_CNT
  bne @nolo
  dec LF_CNT+1
@nolo:
  dec LF_CNT
  jmp @loop
@done:
  clc
  rts
@io:
  lda #LF_ERR_IO
  sec
  rts

; ---- move LF_CNT bytes into video memory at LF_DST ------------------------
;
; The screen goes off first, once per file that lands in PPU space, and is NOT
; put back: games turn rendering on themselves. Measured, including that it
; happens for PPU files only and never for a plain RAM load.
lf_to_ppu:
  lda LF_MASK
  and #%11100111
  sta LF_MASK
  sta PPUMASK
  bit PPUSTATUS
  lda LF_DST+1
  sta PPUADDR
  lda LF_DST
  sta PPUADDR
@loop:
  lda LF_CNT
  ora LF_CNT+1
  beq @done
  jsr read_byte
  bcs @io
  sta PPUDATA
  lda LF_CNT
  bne @nolo
  dec LF_CNT+1
@nolo:
  dec LF_CNT
  jmp @loop
@done:
  clc
  rts
@io:
  lda #LF_ERR_IO
  sec
  rts

; ---- read LF_CNT bytes and throw them away --------------------------------
;
; A passed-over body really is read off the disk a byte at a time, which is why
; a call wanting one file of ten still costs a whole side.
lf_discard:
  lda LF_CNT
  ora LF_CNT+1
  beq @done
  jsr read_byte
  bcs @io
  lda LF_CNT
  bne @nolo
  dec LF_CNT+1
@nolo:
  dec LF_CNT
  jmp lf_discard
@done:
  clc
  rts
@io:
  lda #LF_ERR_IO
  sec
  rts

lf_magic:
  .byte $2A, $4E, $49, $4E, $54, $45, $4E, $44, $4F, $2D, $48, $56, $43, $2A

; lf_iderr, the disk-ID error codes, lives below $F600 with the writer's
; read-back: this block is full to the byte.

.org $F6E0
; ===========================================================================
; $E239 / $E237: write a file to the disk
; ===========================================================================
;
; 3 of the 114 corpus titles save through these: Dandy and Hao-kun no Fushigi
; na Tabi call $E239, Suishou no Dragon calls $E237. Measured black-box, with
; the disk image diffed before and after, by a helper session on 2026-10-16.
;
; Two little-endian words sit inline after the jsr, as for $E1F8, and the
; return address is stepped over them:
;
;   first   a 10-byte disk-ID template, compared against block-1 bytes
;           $0F-$18 with $FF as a per-byte wildcard: the loader's check, its
;           error table and its single retry exactly
;   second  a 17-byte file descriptor: ID, name[8], load address, size, type,
;           source address, source type (0 CPU RAM, anything else PPU)
;
;   a       the file index to write at. $FF means append: use the side's own
;           file count. $E237 is `lda #$FF` and falls into $E239, which is what
;           makes it the append entry: measured identical but for 2 cycles.
;
; Pass one reads block 1, then either rewrites block 2 with the count set to
; the index or, appending, reads the count, walks that many files, and writes
; the new header (03, index, then descriptor bytes 0-13) and data (04, then the
; bytes from the source). Pass two spins the disk again, rewrites the count as
; index + 1, walks back to the new file and compares it with the descriptor and
; the source. Files past the new one stay on the medium, hidden by the count.
; A verify that fails is retried once, and a second failure rolls the count
; back to the index and returns $26.
;
; Every block read finishes with the CRC byte, in CRC mode, as the original
; does. That is not ceremony: it leaves the head where the original leaves it,
; so the blocks we write land on the same bytes of the medium as the
; original's, and a test can hold the two images equal.
;
; Out: a = x = $00 and y = $00 on success ($10 for a PPU source), with V and Z
; set, C and N clear, and interrupts enabled. Errors: $01 no disk and $03 write
; protected, both before the motor moves and with y = a; $02 a flat battery,
; carry set; the loader's ID codes $04-$10 with y = the template byte that
; failed; $26 a verify failure. $4025 is left at $27 with the caller's
; mirroring, and $0101 as it was found.
;
; Polls where the original takes the byte IRQ, for the same reason and with
; the same caveat as $E1F8.

WF_RET    = $05           ; 2: the caller's return address, while parsing
WF_IDX    = $06           ; the index being written
WF_BLK    = $07           ; the block in hand; ends $04
WF_LEFT   = $08           ; files left to walk; ends $00
WF_TMP    = $09           ; a $4025 value being built; ends $FF
WF_SRC    = $0A           ; 2: the data source; ends past its end
WF_CNT    = $0C           ; 2: bytes left; ends $FFFF
WF_ARG    = $0E           ; a, as it came in
WF_ERR_VERIFY = $26

write_file:
  sta WF_ARG
  tsx
  dex
  dex
  dex
  stx LF_FRAME
  pla                       ; the two inline words, and the return past them
  sta WF_RET
  pla
  sta WF_RET+1
  ldy #1
  lda (WF_RET),y
  sta LF_P1
  iny
  lda (WF_RET),y
  sta LF_P1+1
  iny
  lda (WF_RET),y
  sta LF_P2
  iny
  lda (WF_RET),y
  sta LF_P2+1
  lda WF_RET
  clc
  adc #4
  tax
  lda WF_RET+1
  adc #0
  pha
  txa
  pha
  lda $0101
  pha
  lda WF_TMP                ; $09, which the disk-ID check borrows and the
  pha                       ; original's never touches: put back on that exit

  ; ---- the drive, before the motor moves ----
  lda FDS_DRIVE
  lsr a                     ; bit 0: no disk
  bcc @disk
  lda #$01
  jmp wf_early
@disk:
  lsr a
  lsr a                     ; bit 2: write protected
  bcc @writable
  lda #$03
  jmp wf_early
@writable:
  jsr lf_spin
  lda FDS_BATT
  bmi @battery_ok
  lda #$02
  jmp wf_battery
@battery_ok:

  ; ---- pass one: find the place and write the file ----
  lda #0
  sta LF_K05                ; the disk-ID retry, as the loader's
@attempt:
  jsr lf_info
  bcc @id_ok
  ldy LF_K05
  bne @id_fail
  inc LF_K05
  jsr lf_spin
  jmp @attempt
@id_fail:
  txa                       ; the template byte that failed
  tay
  lda lf_iderr,x
  jmp wf_id_exit
@id_ok:
  jsr wf_crc
  lda WF_ARG
  cmp #$FF
  beq @append
  sta WF_IDX
  jsr wf_put_count          ; the count becomes the index, for now
  jmp @walk
@append:
  jsr wf_get_count
  bcs @io
  sta WF_IDX
@walk:
  jsr wf_walk
  bcs @io
  jsr wf_put_header
  jsr wf_put_data

  ; ---- pass two: count it in and read it back ----
  lda #$02
  pha                       ; two goes at the verify
@verify:
  jsr lf_spin
  jsr lf_info
  bcs @verify_io
  jsr wf_crc
  lda WF_IDX
  clc
  adc #1
  jsr wf_put_count_a
  jsr wf_walk
  bcs @verify_io
  jsr wf_check_header
  bcs @mismatch
  jsr wf_check_data
  bcs @mismatch
  pla
  ldx #16                   ; the source type
  jsr wf_desc
  ldy #$00
  cmp #$00
  beq @ram
  ldy #$10                  ; measured: a PPU source leaves y at $10
@ram:
  lda #$00
  jmp wf_exit
@mismatch:
  pla
  sec
  sbc #1
  pha
  bne @verify
  pla
  jsr lf_spin               ; give up: the count goes back to the index
  jsr lf_info
  jsr wf_crc
  jsr wf_put_count
  lda #WF_ERR_VERIFY
  ldy #$00
  jmp wf_verify_exit
@verify_io:
  pla
@io:
  lda #LF_ERR_IO
  ldy #$00
  ; fall through

; ---- the epilogue for anything that moved the motor ------------------------
; In: a = status, y = what y returns. Out: a = x = status, V set, C clear.
wf_exit:
  tax
  pla                       ; $09 as found: not wanted on this exit
  pla
  sta $0101
  lda LF_SHADOW
  and #$08
  ora #$27
  sta LF_SHADOW
  sta FDS_CTRL
  lda #$02
  sta LF_K05
  lda #$FF
  sta WF_TMP
  sta WF_CNT
  sta WF_CNT+1
  cli
  bit lf_vflag
  clc
  txa
  rts

; ---- the file would not read back --------------------------------------------
; As the success exit but for two measured differences: $05 holds the code,
; and $0A-$0D are left where the failed compare stopped, pointing at the byte
; that differed with the count already past it.
wf_verify_exit:
  tax
  pla
  pla
  sta $0101
  lda LF_SHADOW
  and #$08
  ora #$27
  sta LF_SHADOW
  sta FDS_CTRL
  stx LF_K05
  lda #$FF
  sta WF_TMP
  cli
  bit lf_vflag
  clc
  txa
  rts

; ---- the disk is not the one the caller named ------------------------------
; Measured against the success exit: $05 goes to $00 rather than $02, $08
; holds the code in BINARY ($0A where a says $10), and $09 and $0C/$0D are left
; as found. a = x = the BCD code, y = the template byte that failed.
wf_id_exit:
  tax
  cmp #$10
  bne @binary
  lda #$0A
@binary:
  sta WF_LEFT
  pla
  sta WF_TMP
  pla
  sta $0101
  lda LF_SHADOW
  and #$08
  ora #$27
  sta LF_SHADOW
  sta FDS_CTRL
  lda #$00
  sta LF_K05
  cli
  bit lf_vflag
  clc
  txa
  rts

; ---- before the motor: no disk, or protected. V passes through -------------
wf_early:
  tax
  ldy WF_ARG
  pla
  pla
  sta $0101
  cli
  clc
  txa
  rts

; ---- a flat battery: carry set, V passes through ---------------------------
wf_battery:
  tax
  ldy #$00
  pla
  pla
  sta $0101
  cli
  sec
  txa
  rts

write_file_end:
.assert write_file_end <= $F900

; The writer's block primitives, in the free run below $F200.
.org $F080
; ---------------------------------------------------------------------------
; Blocks
; ---------------------------------------------------------------------------

; Build a $4025 value: a without its mirroring, plus the caller's.
wf_ctrl:
  sta WF_TMP
  lda LF_SHADOW
  and #$08
  ora WF_TMP
  sta FDS_CTRL
  rts

; Finish a block read with its CRC, in CRC mode, then stop the transfer. BOTH
; bytes: the original reads one under its IRQ, but it is slow enough leaving
; the handler that the drive has delivered the second by the time the transfer
; stops, and a write that follows starts on the gap after the CRC, not on its
; second byte. Reading one left every block we wrote a byte early.
wf_crc:
  lda #$75
  jsr wf_ctrl
  jsr read_byte
  jsr read_byte
  lda #$25
  jmp wf_ctrl

; Read block 2 and return the count in a, carry set on a bad read.
wf_get_count:
  lda #$02
  sta WF_BLK
  jsr lf_next
  jsr read_byte
  bcs @out
  jsr read_byte
  bcs @out
  pha
  jsr wf_crc
  pla
  clc
@out:
  rts

; Walk WF_IDX files: each header for its size, then the body read and thrown
; away. Carry set on a bad read.
wf_walk:
  lda WF_IDX
  sta WF_LEFT
@file:
  lda WF_LEFT
  beq @done
  dec WF_LEFT
  lda #$03
  sta WF_BLK
  jsr lf_next
  ldy #0
@hdr:
  jsr read_byte
  bcs @out
  cpy #13
  bne @not_lo
  sta WF_CNT
@not_lo:
  cpy #14
  bne @not_hi
  sta WF_CNT+1
@not_hi:
  iny
  cpy #16
  bne @hdr
  jsr wf_crc
  lda #$04
  sta WF_BLK
  jsr lf_next
  jsr read_byte
  bcs @out
  jsr lf_discard
  bcs @out
  jsr wf_crc
  jmp @file
@done:
  clc
@out:
  rts

; Open a block for writing. The first byte out is the $00 already in the
; register, which is the gap ahead of the start mark.
wf_open:
  sta WF_BLK
  lda #$00
  sta FDS_WRITE
  lda #$21
  jsr wf_ctrl
  lda #$61
  jsr wf_ctrl
  lda #$80                  ; the start mark
  jsr wf_put
  lda WF_BLK                ; and the block type
  ; fall through

; Hand the drive one byte, once it has taken the last.
wf_put:
  pha
@wait:
  lda FDS_STATUS
  and #$42
  beq @wait
  pla
  sta FDS_WRITE
  rts

; Close a written block: a is the byte the original leaves in the register,
; then CRC mode writes the two CRC bytes, then the transfer stops.
wf_close:
  jsr wf_put
  lda #$71
  jsr wf_ctrl
@wait:
  lda FDS_STATUS
  and #$42
  beq @wait
  lda FDS_DRIVE
  lda #$25
  jmp wf_ctrl

; Block 2 with the count set to the index, or to a.
wf_put_count:
  lda WF_IDX
wf_put_count_a:
  pha
  lda #$02
  jsr wf_open
  pla
  jsr wf_put
  lda #$00
  jmp wf_close

; Descriptor byte x, in a, Z set if it is zero. Uses y.
wf_desc:
  txa
  tay
  lda (LF_P2),y
  rts

; Block 3: 03, the index as the file number, then descriptor bytes 0-13.
wf_put_header:
  lda #$03
  jsr wf_open
  lda WF_IDX
  jsr wf_put
  ldx #0
@byte:
  jsr wf_desc
  jsr wf_put
  inx
  cpx #14
  bne @byte
  lda #$00
  jmp wf_close

; Point WF_SRC and WF_CNT at the data, and the PPU at it if that is where it
; lives. Z clear on return for a PPU source.
wf_source:
  ldx #11
  jsr wf_desc
  sta WF_CNT
  inx
  jsr wf_desc
  sta WF_CNT+1
  ldx #14
  jsr wf_desc
  sta WF_SRC
  inx
  jsr wf_desc
  sta WF_SRC+1
  inx
  jsr wf_desc
  beq @ram
  lda LF_MASK               ; screen off, as for a PPU file loaded
  and #%11100111
  sta LF_MASK
  sta PPUMASK
  bit PPUSTATUS
  lda WF_SRC+1
  sta PPUADDR
  lda WF_SRC
  sta PPUADDR
  lda PPUDATA               ; the read buffer's stale byte
  lda #$01
@ram:
  rts

; Count one byte off WF_CNT. Z set when it was the last.
wf_count:
  lda WF_CNT
  bne @lo
  dec WF_CNT+1
@lo:
  dec WF_CNT
  lda WF_CNT
  ora WF_CNT+1
  rts

; Block 4: 04, then the data.
;
; The loop is kept short on purpose, about 40 cycles a byte against a byte
; period of 149. Reading $4030 to see the drive is ready also clears the flag,
; so if the drive's next period lands between that read and the store to
; $4024 it writes the previous byte again. That only happens to a loop slow
; enough to arrive at the poll late, and an earlier version, calling through a
; descriptor lookup per byte, was: it doubled five bytes in seventy-two.
wf_put_data:
  jsr wf_source
  lda #$04
  jsr wf_open
  ldx #16
  jsr wf_desc               ; the source type, once
  bne @ppu
@ram:
  lda WF_CNT
  ora WF_CNT+1
  beq @end
  ldy #0
  lda (WF_SRC),y
  inc WF_SRC
  bne @ram_put
  inc WF_SRC+1
@ram_put:
  jsr wf_put
  jsr wf_count
  jmp @ram
@ppu:
  lda WF_CNT
  ora WF_CNT+1
  beq @end
  lda PPUDATA
  jsr wf_put
  jsr wf_count
  jmp @ppu
@end:
  lda #$FF
  jmp wf_close

wf_blocks_end:
.assert wf_blocks_end <= $F200

; And the read-back, below $F600.
.org $F4B0
; Read back the header and compare it. Carry set on a difference.
wf_check_header:
  lda #$03
  sta WF_BLK
  jsr lf_next
  jsr read_byte
  bcs @bad
  cmp #$03
  bne @bad
  jsr read_byte
  bcs @bad
  cmp WF_IDX
  bne @bad
  ldx #0
@byte:
  jsr read_byte
  bcs @bad
  sta WF_TMP
  jsr wf_desc
  cmp WF_TMP
  bne @bad
  inx
  cpx #14
  bne @byte
  jsr wf_crc
  clc
  rts
@bad:
  sec
  rts

; Read back the data and compare it with the source. Carry set on a difference.
wf_check_data:
  jsr wf_source
  lda #$04
  sta WF_BLK
  jsr lf_next
  jsr read_byte
  bcs @bad
  cmp #$04
  bne @bad
; The order is measured: a mismatch on the first byte of $48 left the count
; at $47 and the source pointer still ON the byte, so the count goes down
; before the compare and the pointer moves on only after it agrees.
; Short for the same reason as the write: a late poll can lose a byte.
  ldx #16
  jsr wf_desc
  bne @ppu
@byte:
  lda WF_CNT
  ora WF_CNT+1
  beq @end
  jsr read_byte
  bcs @bad
  sta WF_TMP
  jsr wf_count
  ldy #0
  lda (WF_SRC),y
  cmp WF_TMP
  bne @bad
  inc WF_SRC
  bne @byte
  inc WF_SRC+1
  jmp @byte
@ppu:
  lda WF_CNT
  ora WF_CNT+1
  beq @end
  jsr read_byte
  bcs @bad
  sta WF_TMP
  jsr wf_count
  lda PPUDATA
  cmp WF_TMP
  bne @bad
  jmp @ppu
@end:
  jsr wf_crc
  clc
  rts
@bad:
  sec
  rts
; Error codes by which template byte failed, in BCD, for the loader and the
; writer alike. Byte 9 giving $10 rather than $0A is what proves they are BCD.
lf_iderr:
  .byte $04, $05, $05, $05, $05, $06, $07, $08, $09, $10
wf_checks_end:
.assert wf_checks_end <= $F600

; ===========================================================================
; Vectors
; ===========================================================================
.org $FA00
; ===========================================================================
; $E32A: read the whole side and write a directory of it into RAM
; ===========================================================================
;
; 1 of the 114 corpus titles calls it and was stopped on it: Akumajou Dracula,
; once, from $720F, and its next instruction checks the side holds ten files.
; Measured by a helper with fault injection on the drive; never writes the disk.
;
; Called as `jsr $E32A` with one inline word, the buffer. It spins the side up
; and reads every block, every body byte included, and leaves at the buffer:
;
;   +0..+9      disk-info bytes $0F-$18 of block 1
;   +10         the file count from block 2
;   +11+9k      file k: its ID (header byte 2), then its 8-byte name
;   +11+9n      the total, HIGH byte first: the sum of size + 261 over files
;
; The total also goes to $02/$03, little-endian. $0A/$0B walks the buffer one
; byte behind where the next entry goes, so it ends on the last entry byte.
;
; Exits, a = x = the code on every one, D always as it came in, I cleared:
;
;   success           a=0, y=2, V and Z set, C clear
;   no disk at entry  a=1, y=[$0E], C set, V as it came; $F9/$FA untouched
;   flat battery      a=2, y=0, C set
;   disk pulled       a=1, y=0, C set, during the spin-up or between blocks
;   block 1 type      a=$22, y=$22, C from the compare against $01
;   magic byte i      a=$21, y=14-i, C from the compare
;   block 2/3/4 type  a=$23/$24/$25, y the same, C from the compare
;   end of head, CRC  a=$28/$27; y=0 C=0 at block 1, $23 C=1 at block 2,
;                     $09 C=1 at a header. A body's status is never checked.
;
; V is set once any attempt gets past the spin-up, and passes through until
; then. Any failure after the motor starts is retried once from the spin-up;
; $05 ends as 2 less the failed attempts. Every exit but the first sets bit 7
; of $F9 and leaves $FA = ($FA & 8) | $27. $08, $0E and $0101 are never
; written: we poll, so the byte IRQ the original arms through $0101 is not
; needed.
;
; Fuzzed by the helper against its model of the original, with faults keyed
; to the status read each block makes: 2100 disks, every path identical but
; one, and every case faster. The one is an end of head DURING a body, which
; the original never reads for and ours, polling, does. On our drive that
; only happens when the head truly runs off the side, a count larger than the
; files present, where the original waits forever and ours returns $28.
DI_PTR    = $00           ; 2: the buffer, from the inline word
DI_TOTAL  = $02           ; 2: the running total, low byte first
DI_FRAME  = $04           ; entry stack pointer minus 3
DI_TRIES  = $05           ; 2 less the failed attempts
DI_LEFT   = $06           ; files left; $05/$06 hold the return address first
DI_BLK    = $07           ; the block in hand
DI_HDR    = $09           ; $FF once a header's type has passed
DI_DST    = $0A           ; 2: one byte behind the next entry
DI_CNT    = $0C           ; 2: body bytes left, less one; ends $FFFF

disk_info:
  php                       ; V and D, for the exits that pass them through
  tsx
  lda $0102,x               ; our return address, under the P
  sta DI_TRIES
  clc
  adc #$02                  ; stepped over the inline word in place
  sta $0102,x
  lda $0103,x
  sta DI_LEFT
  adc #$00
  sta $0103,x
  dex
  dex
  stx DI_FRAME              ; S at entry minus 3
  ldy #$01
  lda (DI_TRIES),y
  sta DI_PTR
  iny
  lda (DI_TRIES),y
  sta DI_PTR+1
  lda #$00
  sta DI_TOTAL
  lda FDS_DRIVE
  lsr a
  bcc @disk
  lda #$01
  tax
  ldy $0E
  jmp di_flags
@disk:
  lda #$02
  sta DI_TRIES

@attempt:
  jsr di_spin
  bcs @failed
  pla                       ; past the spin-up: V from here on
  ora #$40
  pha
  jsr di_info
  bcs @failed
  jsr di_count
  bcs @failed
@file:
  lda DI_LEFT
  beq @done
  jsr di_header
  bcs @failed
  jsr di_body
  bcs @failed
  dec DI_LEFT
  jmp @file
@failed:
  dec DI_TRIES
  bne @attempt
  jmp di_exit
@done:
  ldy #$01
  lda DI_TOTAL+1
  sta (DI_DST),y
  iny
  lda DI_TOTAL
  sta (DI_DST),y
  lda #$00                  ; y is 2
  tax
  ; fall through

; In: a = the code, y as returned, x = the carry to return.
di_exit:
  pha
  lda LF_SHADOW
  and #$08
  ora #$27
  sta LF_SHADOW
  sta FDS_CTRL
  pla
di_flags:
  plp                       ; the caller's P, with V if a spin-up passed
  cli
  cpx #$01                  ; C as the exit wants it
  tax                       ; x = a, and N and Z from the code
  rts

; ---- the motor, the battery, the drive. Carry set: a = code, y, x --------
di_spin:
  jsr lf_base
  ora #$02
  sta FDS_CTRL
  lda $F9                   ; as lf_spin: marked, and sent to $4026
  ora #$80
  sta $F9
  sta FDS_EXT
  lda FDS_BATT
  bpl @flat
  jsr lf_base
  ora #$01
  sta FDS_CTRL
@wait:
  lda FDS_DRIVE
  lsr a
  bcs di_gone
  lsr a
  bcs @wait
  rts                       ; carry clear from the lsr
@flat:
  lda #$02
  bne di_io
di_gone:
  lda #$01
di_io:
  ldy #$00
  ldx #$01
  sec
  rts

; ---- the block-level helpers, all called straight from a block routine ----
;
; A fault leaves the BLOCK routine, not just the helper, so each block reads
; as a straight line. The y and carry a fault returns depend only on the
; block in hand: 0 and clear at block 1, $23 and set at block 2, $09 and set
; at a header. A body is never checked by the original; ours only faults
; there if the head runs off the side, and gives a header's answer.

; Open block a: its type byte, or leave with $21 + a, y the same, and the
; compare's carry.
di_open:
  sta DI_BLK
  jsr lf_next
  jsr read_byte
  bcs di_rd_eoh
  cmp DI_BLK
  bne @bad
  rts
@bad:
  pla                       ; leave the block routine; pla keeps the carry
  pla
  ldx #$00
  bcc @lt
  inx
@lt:
  lda DI_BLK
  clc
  adc #$21
  tay
  sec
  rts

; One byte, or leave the block routine with an end of head.
di_rd:
  jsr read_byte
  bcs di_rd_eoh
  rts
di_rd_eoh:
  pla
  pla
di_eoh:
  lda #$28
di_fault:
  ldx DI_BLK
  ldy di_fault_y-1,x
  cpx #$02                  ; C set from block 2 on
  ldx #$00
  bcc @lt
  inx
@lt:
  sec
  rts
di_fault_y:
  .byte $00, $23, $09, $09

; A header-type block's end: end of head, then its CRC, then the drive.
di_end:
  lda FDS_STATUS
  and #$40
  bne di_eoh
  jsr lf_base
  ora #$51                  ; CRC mode
  sta FDS_CTRL
  jsr read_byte
  bcs di_eoh
  lda FDS_STATUS
  and #$10
  beq di_off
  lda #$27
  bne di_fault
di_off:
  jsr lf_base
  ora #$01                  ; transfer off, motor on
  sta FDS_CTRL
  lda FDS_DRIVE
  lsr a                     ; carry clear: still in
  bcs di_gone
  rts

di_helpers_end:
.assert di_helpers_end <= $FB28

; The four block routines, in the free run at $EF07.
.org $EF07
; ---- block 1: the magic, then the ten info bytes into the buffer ----------
di_info:
  lda #$01
  jsr di_open
  ldy #$01
@magic:
  jsr di_rd
  cmp lf_magic-1,y
  bne @bad
  iny
  cpy #15
  bne @magic
  lda #$00
  sta DI_TOTAL
  sta DI_TOTAL+1
  tay
@copy:
  jsr di_rd
  sta (DI_PTR),y
  iny
  cpy #10
  bne @copy
  lda DI_PTR
  clc
  adc #10
  sta DI_DST
  lda DI_PTR+1
  adc #0
  sta DI_DST+1
  ldy #31                   ; the rest of block 1's 56 bytes
@rest:
  jsr di_rd
  dey
  bne @rest
  jmp di_end
@bad:
  ldx #$00                  ; the compare's carry
  bcc @lt
  inx
@lt:
  tya                       ; y = 14 - i
  eor #$FF
  sec
  adc #14
  tay
  lda #$21
  sec
  rts

; ---- block 2: the count, into $06 and, once it has checked, the buffer ----
di_count:
  lda #$02
  jsr di_open
  jsr di_rd
  sta DI_LEFT
  jsr di_end
  bcs @out
  ldy #$00
  lda DI_LEFT
  sta (DI_DST),y
  clc
@out:
  rts

; ---- block 3: an entry for the buffer, and the size for the total ---------
di_header:
  lda #$03
  jsr di_open
  lda #$FF
  sta DI_HDR
  jsr di_rd                 ; the file number, not wanted
  ldy #$01
@entry:
  jsr di_rd                 ; the ID, then the eight bytes of name
  sta (DI_DST),y
  iny
  cpy #10
  bne @entry
  jsr di_rd                 ; the load address, not wanted
  jsr di_rd
  jsr di_rd
  sta DI_CNT
  jsr di_rd
  sta DI_CNT+1
  jsr di_rd                 ; the kind, not wanted
  lda DI_DST                ; the entry is in: step past it
  clc
  adc #9
  sta DI_DST
  bcc @nc
  inc DI_DST+1
@nc:
  lda DI_TOTAL              ; total += size + 261
  clc
  adc DI_CNT
  tax
  lda DI_TOTAL+1
  adc DI_CNT+1
  tay
  txa
  clc
  adc #$05
  sta DI_TOTAL
  tya
  adc #$01
  sta DI_TOTAL+1
  lda DI_CNT                ; and count the body down from size - 1
  bne @lo
  dec DI_CNT+1
@lo:
  dec DI_CNT
  jmp di_end

; ---- block 4: every byte streamed and dropped, CRC included ---------------
di_body:
  lda #$04
  jsr di_open
@loop:
  jsr di_rd
  lda DI_CNT
  bne @lo
  dec DI_CNT+1
@lo:
  dec DI_CNT
  lda DI_CNT
  and DI_CNT+1
  cmp #$FF
  bne @loop
  jsr di_rd                 ; the CRC, as data
  jsr di_rd
  jmp di_off
disk_info_end:
.assert disk_info_end <= $F000

;
; ===========================================================================
; $E305: set the side's file count, by rewriting block 2
; ===========================================================================
;
; The title stopped on it is Ginga Denshou - Galaxy Odyssey, whose save drops
; the count from $0D to $0C and then appends the new save file through $E237.
; Tobidase Daisakusen calls it too. Measured by a helper and fuzzed against
; the original, 1700 cases for the model and 1300 for this code, with eject,
; write protect, battery, bad type and magic bytes and ID mismatches forced.
;
; `jsr $E305` with the disk-ID template after it, and a is the count. With a
; $FF there is a second word, copied to $02/$03, and ITS low byte is the
; count. One pass: block 1 is read and its ID matched as the loader does,
; then block 2 is written as $80 $02 count with its CRC where it was. No
; verify, and the old count is never read.
;
;   success           a=x=0, y=0, V and Z set; $05=2, $06=count, $07=2,
;                     $08 = the boot read file code
;   no disk, protect  a=x=1 or 3, y=[$0E], before the motor; V as it came
;   flat battery      a=x=2, y=0, C set, V clear
;   type, magic, ID   the loader's codes; y is the byte that failed and $08
;                     holds an ID code in binary
;
; Every failure after the motor moves is retried once, the battery included,
; which the loader and the writer do not do. Every exit after the motor sets
; bit 7 of $F9 and leaves $FA = ($FA & 8) | $27. The pre-motor exits are the
; only tight budgets, 128 and 139 cycles in the original; these take 125 and
; 129. The rest is the original's counted delays gone.
.org $E4FC
; $E301, 32 disks' save code: $E305 with the count one more than its operand,
; wrapping. Measured: every exit as $E305's but $07 = 1 where $E305 leaves 0
; (no disk, protect, battery), which is the flag this path carries in y until
; block 1 sets $07 for itself.
set_count1:
  ldy #$01
  bne set_count_c           ; always; ldy leaves a and V alone
set_count:
  ldy #$00
set_count_c:
  sty $07                   ; 0 for $E305, 1 for $E301, before y is used
  sta $02                   ; the count, or $FF for "the count is inline"
  tsx
  dex
  dex
  dex
  stx $04                   ; frame anchor, as $E1F8 and $E239
  pla
  sta $05
  pla
  sta $06                   ; $05/$06: the stacked return address
  ldy #1
  lda ($05),y
  sta $00
  iny
  lda ($05),y
  sta $01                   ; word 1: the disk-ID template
  ldx #2
  lda $02
  cmp #$FF
  bne @one
  iny
  lda ($05),y
  sta $02                   ; word 2, low byte: the count
  iny
  lda ($05),y
  sta $03
  ldx #4
@one:                       ; step the return address over x bytes; adc
  php                       ; would lose the caller's V, which an early exit
  txa                       ; hands back
  clc
  adc $05
  tax
  lda $06
  adc #0
  plp
  pha
  txa
  pha
  lda FDS_DRIVE
  lsr a
  bcc @disk
  lda #$01
  bne @early
@disk:
  lsr a
  lsr a
  bcc @go
  lda #$03
@early:
  tax
  ldy $0E                   ; measured: y is $0E as found
  cli
  clc
  txa
  rts
@go:
  lda $0101
  pha
  lda $09
  pha
  lda $02
  ldx $07
  beq @count
  clc
  adc #$01                  ; $E301: one more, and $FF wraps to 0
@count:
  sta $06
  lda #0
  sta $05                   ; the retry flag; every late exit leaves $00
@attempt:
  jsr lf_base
  ora #$02
  sta FDS_CTRL
  lda $F9
  ora #$80
  sta $F9
  sta FDS_EXT
  lda FDS_BATT
  bmi @batt_ok
  lda $05                   ; bit 0 is the retry, bit 7 $E2BB's reading pass
  and #$01
  bne @flat
  inc $05
  bne @attempt
@flat:
  lda $05                   ; $E2BB and $E2B7 keep V on a flat battery: the
  and #$C0                  ; caller's on the reading pass, the subtraction's
  bne @keep_v               ; on the writing one. $E305 and $E301 clear it.
  clv
@keep_v:
  lda #$02
  ldy #$00
  sty $05
  sec
  jmp @tail
@batt_ok:
  jsr lf_base
  ora #$01
  sta FDS_CTRL
@ready:
  lda FDS_DRIVE
  and #$03
  bne @ready
  lda #$01
  sta $07
  jsr lf_next
  jsr read_byte
  cmp #$01
  beq @type_ok
  lda #$22                  ; C is the compare against $01
  ldy #$22
  jmp @fail
@type_ok:
  ldx #0
  ldy #13
@magic:
  jsr read_byte
  cmp lf_magic,x
  bne @magic_bad
  inx
  dey
  bpl @magic
  ldy #0
@id:
  jsr read_byte
  sta $09
  lda ($00),y
  cmp #$FF
  beq @wild
  cmp $09
  bne @id_bad
@wild:
  iny
  cpy #10
  bne @id
  jsr read_byte
  sta $08                   ; disk-info byte $19, the boot read file code
  ldy #30
@rest:
  jsr read_byte
  dey
  bne @rest
  jsr wf_crc
  bit $05
  bpl @write
  jmp adjust_count@read     ; $E2BB's first pass reads block 2 instead
@write:
  lda $06
  jsr wf_put_count_a        ; $07 ends $02 from wf_open
  lda #$02
  sta $05
  lda #$00
  tay
  clc
  jmp @late
@magic_bad:
  lda #$21                  ; C is the compare of the disk byte
  jmp @fail
@id_bad:
  lda lf_iderr,y
  sta $08
  cmp #$10
  bne @bin
  lda #$0A
  sta $08
  lda #$10
@bin:
  clc
@fail:
  pha                       ; the code; carry is the exit's and stays
  lda $05
  and #$01
  bne @give_up_a
  pla
  inc $05
  jmp @attempt
@give_up_a:
  pla
@give_up:
  ldx #0
  stx $05
@late:
  bit lf_vflag              ; V set; C is whatever the exit decided
@tail:                      ; nothing below touches C or V
  tax
  pla
  sta $09
  pla
  sta $0101
  lda LF_SHADOW
  and #$08
  ora #$27
  sta LF_SHADOW
  sta FDS_CTRL
  cli
  txa
  rts
set_count_end:
.assert set_count_end <= $E68F

.org $F550
; $E4A0: header bytes 1 and 2, the ID against the list at ($02), then the
; eight name bytes. A match leaves $09 = 0 and counts it in $0E HERE, not when
; the body loads. A list's $FF ends it; a list STARTING with $FF means the
; boot set, ID <= the boot code in $08. Out: a = 0, x = the ID, y = where the
; list stopped, V and Z set, I clear, C from the compare.
pf_e4a0:
  jsr pf_rd                 ; the file number, not wanted
  jsr pf_rd
  tax                       ; the ID, to the end
  lda #$08
  sta $0101                 ; name bytes to go, as the original counts them
  ldy #$00
  lda (LF_P2),y
  cmp #$FF
  beq @boot
@scan:
  lda (LF_P2),y
  cmp #$FF
  beq @miss1
  txa
  cmp (LF_P2),y
  beq @hit1
  iny
  jmp @scan                 ; no end but the $FF, as measured
@boot:
  cpx LF_BOOT               ; C = ID >= the boot code
  beq @hit
  bcc @hit
  bcs @miss
@hit1:
  sec
@hit:
  lda #$00
  sta LF_SKIP
  inc LF_LOADED
  jmp @name
@miss1:
  sec
@miss:
  lda #$FF
  sta LF_SKIP
@name:
  php                       ; the carry, for the exit
@byte:
  jsr pf_rd                 ; seven of the eight
  dec $0101
  lda $0101
  cmp #$01
  bne @byte
  pla                       ; the exit flags are ready before the last byte
  and #$09                  ; D and C kept
  ora #$42                  ; V and Z
  pha
  dec $0101
  lda #$02
@last:
  bit FDS_STATUS            ; a tight poll: the call ends on the drive's last
  bvs @eoh                  ; byte, and the original's IRQ ends it within a
  beq @last                 ; few cycles of that
  lda FDS_DATA
  lda #$00
  plp
  rts
@eoh:
  lda #$28
  jmp pf_err_v
pf_match_end:
.assert pf_match_end <= vf_loop
.assert (>pf_e4a0) == (>pf_match_end)  ; one page: no branch pays a crossing


; --- $E8B3: read VRAM back into a table of 3-byte records ------------------
;
; Radical Bomber!! Jirai Kun reads the tile under its player with this. Each
; record at $0300+x is an address high, low, and a slot for the byte; y is the
; count, 0 meaning 256. One status read for the call, then per record the
; address, a priming read and the real one. Every index wraps inside page 3.
; 9 + 40y cycles, the original's. Exit a = the last byte, x += 3y, y = 0, Z set.
.org $E8B3
vram_read:
  lda PPUSTATUS
@rec:
  lda $0300,x
  sta PPUADDR
  inx
  lda $0300,x
  sta PPUADDR
  inx
  lda PPUDATA               ; primes the read buffer
  lda PPUDATA
  sta $0300,x
  inx
  dey
  bne @rec
  rts
vram_read_end:
.assert vram_read_end == $E8D2

; --- $E93C: step the queue index and check it against the buffer size ------
;
; Bishoujo Kachinuki Renju has its own copy of an $E8D2 append and calls this
; after every byte. Room: back with the compare's flags, 15 cycles. Full: the
; queue is cut back to its committed index with an $FF, our own return address
; is dropped and a = 1, C set, comes back to the caller's caller; 35 cycles.
; The same rollback $E8D2 makes at @full.
.org $E93C
queue_step:
  inx
  cpx BUF-2
  bcc @room
  ldx BUF-1
  lda #$FF
  sta BUF,x
  pla
  pla
  lda #$01
@room:
  rts

; --- $E94F: look an address up in record y, or put it there ----------------
;
; Jirai Kun again: it registers the address it wants read, and $E8B3 fills the
; byte in at the next vblank. y is the record number from 1 (0 means 256) in
; the table at $0300+x, and $00/$01 the address. A hit returns the cached byte
; with C clear, x = r+2, y = r; a miss writes the address in and returns
; a = [$01], C set, y = r+1, x = r or r+1 by which byte differed.
;
; V comes out of the walk, a 3 added per step, so it is set when the record is
; $80-$82; that falls out of finding it by adding. 2 cycles under the
; original's 54 + 9(y-1) hit, 56 + 9(y-1) and 67 + 9(y-1) misses: there was no
; byte left to spend them.
.org $E94F
record_find:
  txa
  sec
  sbc #$03
@walk:
  clc
  adc #$03
  dey
  bne @walk
  tax
  tay
  lda $0300,x
  cmp $00
  bne @miss
  inx
  lda $0300,x
  cmp $01
  bne @miss
  inx
  lda $0300,x
  clc
  rts
@miss:
  lda $00
  sta $0300,y
  iny
  lda $01
  sta $0300,y
  sec
  rts
record_find_end:
.assert record_find_end == $E97D
.assert (>record_find) == (>record_find_end)

; --- $E97D: pixel coordinates to a nametable address -----------------------
;
; The inverse of $E997, on its zero page: $02 = Y and $03 = X in, $00/$01 the
; address in nametable 0 out, a = $01. Cocona World and Jirai Kun. The low
; three bits of each coordinate are dropped; C ends as bit 2 of X. 48 cycles,
; and the sequence is the one the access log admits, exactly 26 bytes.
.org $E97D
pixel_to_tile:
  lda #$08
  sta $00
  lda $02
  asl a
  rol $00
  asl a
  rol $00
  and #$E0
  sta $01
  lda $03
  lsr a
  lsr a
  lsr a
  ora $01
  sta $01
  rts
pixel_to_tile_end:
.assert pixel_to_tile_end == $E997

; --- the $E86A loop, moved here whole so $E8B3 has its address -------------
;
; One page, like the old place, so no taken branch pays a crossing and the
; counts in the commentary still hold.
.org $F5B8
vf_loop:
vf_body:
  pha                       ; 3   kept for the $3F test after the data
  sta PPUADDR               ; 4
  iny                       ; 2
  lda BUF,y                 ; 4
  sta PPUADDR               ; 4
  iny                       ; 2
  ldx BUF,y                 ; 4   the count straight into x: all eight bits,
                            ;     and 0 means 256, which `dex`/`bne` gives
@byte:
  iny                       ; 2   step BEFORE the read, and once more after the
  lda BUF,y                 ; 4   entry: the same count as stepping after, but
  sta PPUDATA               ; 4   the pla and the palette writes then land on
  dex                       ; 2   the original's cycles, two sooner than they
  bne @byte                 ; 3   did
  pla                       ; 4   the address high from the top of the entry
  cmp #$3F                  ; 2   and this is also where carry comes from
  bne vf_next               ; 3 taken on the common path
  ; The palette-safe reset: point at $3F00, then at $0000, so the PPU is not
  ; left showing a palette entry in place of the backdrop. `a` is still $3F
  ; from the `pla` and `x` is $00 from the byte loop, so neither needs loading.
  sta PPUADDR               ; 4
  stx PPUADDR               ; 4
  stx PPUADDR               ; 4
  stx PPUADDR               ; 4
vf_next:
  iny                       ; 2   past the last data byte
vf_check:
  lda BUF,y                 ; 4   the next entry's address high, or the end
  bpl vf_body               ; 3 taken
  sta BUF                   ; 4   the terminator it found, echoed back
  lda #$00                  ; 2
  sta BUF-1                 ; 4   the game's write index
  rts                       ; 6
; 43 + 40 per entry + 15 per data byte + 15 per palette entry, exactly the
; original's, and +1 for any read whose address crosses $03FF, which absolute
; indexing gives for free.
vf_loop_end:
.assert (>vf_loop) == (>vf_loop_end)
.assert vf_loop_end <= $F600

.org $FFFA
  .word nmi
  .word reset
  .word irq

.org $F850
; ---- the PPU and the register shadows a game is handed --------------------
;
; Measured identical on every disk sampled under the original: PPUCTRL and $FF
; $10 (background tiles from $1000, where the font is), PPUMASK and $FE $06
; (rendering off, left columns shown), $FA $2F, $F9 $FF. Games lean on these
; without setting them: Zelda 2 never sets PPUCTRL bit 4 itself, so with $00
; here its whole screen drew from the wrong pattern table.
ho_ppu:
  lda #$10
  sta $FF
  sta PPUCTRL
  lda #$06
  sta $FE
  sta PPUMASK
  lda #$2F
  sta $FA
  lda #$FF
  sta $F9
  ; The VRAM queue as the original leaves it: empty, a $7D-byte buffer with
  ; its terminator in place. Big Challenge! Dogfight Spirit flushes it from its
  ; first NMI without filling it, and a zeroed page sent $E86A walking 29,000
  ; cycles of garbage into the next NMI until the stack ran out.
  lda #$7D
  sta BUF-2
  lda #$00
  sta BUF-1
  lda #$FF
  sta BUF
  rts

; One byte of block 1 for the boot load, y its offset. Bytes $15 and $16 are
; the side and the disk number, and the original will not boot anything but
; side A of disk 1 (ERR.07, ERR.08). The corpus holds two that claim otherwise,
; Kind Gal's Disk 2 and Tantei Jinguuji's Kouhen; both stop under it too.
; Carry set if the disk ended or the side is not one to boot.
info_byte:
  jsr read_byte
  bcs @out
  cpy #$15
  bcc @ok
  cpy #$17
  bcs @ok
  cmp #$00
  bne @refuse
@ok:
  clc
@out:
  rts
@refuse:
  sec
  rts

; Both nametables to the blank tile $24 with attributes $00, as the original
; leaves nametable A: error screens and licence files print over it.
blank_nt:
  bit PPUSTATUS
  lda #$20
  sta PPUADDR
  lda #$00
  sta PPUADDR
  ldx #4
@nt:
  ldy #$C0
  lda #$24
@t1:
  sta PPUDATA
  sta PPUDATA
  sta PPUDATA
  sta PPUDATA
  sta PPUDATA
  dey
  bne @t1
  lda #0
  ldy #64
@a:
  sta PPUDATA
  dey
  bne @a
  dex
  bne @nt
  rts

; The boot load runs with NMI selector $C0, the game's own vector, as the
; original's does. Dig Dug II's boot files include one that turns NMI on mid
; load and expects its own handler to take it; with $00 the NMI landed in
; ours and popped our loader's stack.
arm_c0:
  jsr clear_screen
  lda #$C0
  sta $0100
  ; What the original has set by the time it reads the first boot file. A
  ; disk whose boot file lands on $2000 turns NMI on mid-load and the game
  ; takes over there (Moero Yakyuuken), and it finds these, not our zeros:
  ; $FE decides whether the left column shows, $0101 how a BRK is taken.
  lda #$40
  sta $0101
  lda #$06
  sta $FE
  lda #$10
  sta $FF
  rts

.org $EB13
; ---- the warm boot -----------------------------------------------------------
;
; A game that jumps to reset with $0102/$0103 = $35/$AC (or the $35/$53 this
; leaves behind) goes straight back in through $DFFC with no disk read and no
; PPU memory touched; any other value is a cold boot. Dig Dug II's loader does
; this, and without it the disk reloaded forever. The order of the writes is
; the original's, measured from planted values; its $2E to $4025 (horizontal
; mirroring) matters, or Dig Dug draws into the wrong nametable. The original
; waits about 59,600 cycles; two vblanks stand in for it. Also where the APU
; frame counter is set, because reset has no spare byte for the call.
warm_check:
  lda #$40
  sta APUFRAME
  lda $0102
  cmp #$35
  bne @cold
  lda $0103
  cmp #$AC
  beq @warm
  cmp #$53
  beq @warm
@cold:
  rts
@warm:
  lda #$10
  sta PPUCTRL
  sta $FF
  lda #$06
  sta $FE
  sta PPUMASK
@wv1:
  bit PPUSTATUS
  bpl @wv1
@wv2:
  bit PPUSTATUS
  bpl @wv2
  lda #0
  sta $4022
  sta $4023
  lda #$83
  sta $4023
  lda #0
  sta $FD
  sta $FC
  sta $FB
  lda #$2E
  sta $FA
  sta $4025
  lda #$FF
  sta $F9
  sta $4026
  lda #$C0
  sta $0100
  lda #$80
  sta $0101
  lda #$53
  sta $0103
  lda #0
  sta $2005
  sta $2005
  lda #$10
  sta PPUCTRL
  ldx #$FF
  txs
  ldy #$00
  lda #$10
  sec
  cli
  jmp ($DFFC)

.org $E32D
; ---- $EBAF with its mode bits --------------------------------------------------
;
; Measured against the original with planted calls. The VRAM address is
; y:(a & $F0). Bit 0 picks the fill byte, $FF or $00, left in $03. Bits 3-2
; per unit: 00 sixteen source bytes; 01 eight source then eight fill; 10 eight
; fill then eight source; 11 eight of source EOR fill then eight source. Bit 1
; reads instead, after one dummy $2007 read, in the same shapes. Exit C = bit
; 1, x = y = 0, Z set; $00/$01 the source advanced 16 or 8 a unit, $02 = 0,
; $03 the fill, $04 = a. Section-Z expands the ROM font with a=$04.
vram_upload2:
  sta UP_LO
  stx UP_UNITS
  sty UP_HI
  tsx
  lda $0101,x
  sta UP_RET
  lda $0102,x
  sta UP_RET+1
  ldy #1
  lda (UP_RET),y
  sta UP_SRC
  iny
  lda (UP_RET),y
  sta UP_SRC+1
  lda UP_RET
  clc
  adc #2
  sta $0101,x
  lda UP_RET+1
  adc #0
  sta $0102,x
  bit PPUSTATUS
  lda $FF
  and #$FB
  sta $FF
  sta PPUCTRL
  lda UP_HI
  sta PPUADDR
  lda UP_LO
  and #$F0
  sta PPUADDR
  lda UP_LO
  lsr a
  lda #0
  bcc eb_f0
  lda #$FF
eb_f0:
  sta UP_HI
  lda UP_LO
  lsr a
  lsr a
  and #$03
  tax
  lda UP_LO
  and #$02
  beq eb_unit
  lda PPUDATA
eb_unit:
  lda UP_LO
  and #$02
  beq eb_write
  jmp eb_read
eb_write:
  cpx #0
  bne eb_w8
  ldy #0
eb_w16:
  lda (UP_SRC),y
  sta PPUDATA
  iny
  cpy #16
  bne eb_w16
  jmp eb_adv16
eb_w8:
  cpx #2
  bne eb_wlo
  jsr eb_fill8
  jsr eb_src8
  jmp eb_adv8
eb_wlo:
  cpx #1
  bne eb_wxor
  jsr eb_src8
  jsr eb_fill8
  jmp eb_adv8
eb_wxor:
  jsr eb_xor8
  jsr eb_src8
  jmp eb_adv8

.org $F959
eb_adv16:
  lda UP_SRC
  clc
  adc #16
  jmp eb_adv
eb_adv8:
  lda UP_SRC
  clc
  adc #8
eb_adv:
  sta UP_SRC
  lda UP_SRC+1
  adc #0
  sta UP_SRC+1
  dec UP_UNITS
  beq eb_done
  jmp eb_unit
eb_done:
  lda UP_LO
  lsr a
  lsr a
  lda UP_SRC+1
  ldx #0
  ldy #0
  rts

eb_read:
  cpx #0
  bne eb_r8
  ldy #0
eb_r16:
  lda PPUDATA
  sta (UP_SRC),y
  iny
  cpy #16
  bne eb_r16
  jmp eb_adv16
eb_r8:
  cpx #1
  bne eb_r2
  jsr eb_get8
  jsr eb_skip8
  jmp eb_adv8
eb_r2:
  cpx #2
  bne eb_r3
  jsr eb_skip8
  jsr eb_get8
  jmp eb_adv8
eb_r3:
  jsr eb_xor8
  jsr eb_get8
  jmp eb_adv8
eb_src8:
  ldy #0
eb_s8:
  lda (UP_SRC),y
  sta PPUDATA
  iny
  cpy #8
  bne eb_s8
  rts
eb_fill8:
  ldy #8
  lda UP_HI
eb_f8:
  sta PPUDATA
  dey
  bne eb_f8
  rts
eb_xor8:
  ldy #0
eb_x8:
  lda (UP_SRC),y
  eor UP_HI
  sta PPUDATA
  iny
  cpy #8
  bne eb_x8
  rts
eb_get8:
  ldy #0
eb_g8:
  lda PPUDATA
  sta (UP_SRC),y
  iny
  cpy #8
  bne eb_g8
  rts
eb_skip8:
  ldy #8
eb_k8:
  lda PPUDATA
  dey
  bne eb_k8
  rts
