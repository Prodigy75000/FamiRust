// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Each BIOS routine we implement, held against the real one that inspired it.
//!
//! The corpus check in `scripts/fds-hle-check.py` proves the boot. It cannot
//! prove a routine, because by the time a game calls one the two runs have
//! diverged for reasons that say nothing about the routine. So each routine
//! gets a purpose-built caller instead: a few bytes of 6502, planted in work RAM
//! at the instant of handover, that set up a known machine, call the routine,
//! and record what came back. The same program is run under our BIOS and under
//! the real one and the answers compared.
//!
//! ## Why the caller is planted rather than put on a disk
//!
//! The obvious rig was a synthetic disk carrying the test program, and it does
//! not work, for a reason worth knowing: **the real BIOS will not boot a disk
//! whose first file is not Nintendo's licence screen, byte for byte.** Measured
//! here by building disks and trying them: the real licence file boots, the
//! same file with a different name or file number boots, our own 224 bytes in
//! the same shape are refused, and the real content with ONE byte flipped is
//! refused. That is the Disk System's licensing gate.
//!
//! Our BIOS does not implement that check and will not: it is a licensing gate
//! rather than a technical requirement, the reference bytes are not ours to
//! ship, and removing gates is the whole point. The practical consequence for
//! testing is that the oracle can only ever be run on a real disk, so that is
//! what these do: boot a commercial disk to handover, then take the machine
//! over.
//!
//! Where `dumps/fds/disksys.rom` or the disks are absent the comparison is
//! skipped and the test asserts what we measured instead, so these still mean
//! something in a checkout with no firmware in it.

use std::path::PathBuf;

const BIOS_BASE: u16 = 0xe000;
const VEC_RESET: u16 = 0xdffc;
/// Where the test program is planted. Work RAM above $0600 is left alone by
/// both BIOSes: the real one's scratch stops at $0547 and ours clears
/// everything it borrows.
const PROG: u16 = 0x0600;
/// Where it leaves its answers: a, x, y, p, then a sentinel.
const OUT: u16 = 0x0700;
const OUT_SENTINEL: u16 = 0x0704;
/// Written last, so "the routine returned and we recorded" is distinguishable
/// from "the machine never got there", which otherwise both look like zeroes.
const DONE: u8 = 0x5a;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn real_bios() -> Option<Vec<u8>> {
    std::fs::read(repo_root().join("dumps/fds/disksys.rom")).ok()
}

fn a_disk() -> Option<Vec<u8>> {
    for name in ["zelda.fds", "fgp.fds", "biomiracle.fds"] {
        if let Ok(d) = std::fs::read(repo_root().join("dumps/fds").join(name)) {
            return Some(d);
        }
    }
    None
}

/// A tiny 6502 program, assembled by hand because the alternative is making
/// `nes-core`'s tests depend on the assembler.
#[derive(Default)]
struct Prog(Vec<u8>);

impl Prog {
    fn lda(&mut self, v: u8) -> &mut Self {
        self.0.extend_from_slice(&[0xa9, v]);
        self
    }
    fn ldx(&mut self, v: u8) -> &mut Self {
        self.0.extend_from_slice(&[0xa2, v]);
        self
    }
    fn ldy(&mut self, v: u8) -> &mut Self {
        self.0.extend_from_slice(&[0xa0, v]);
        self
    }
    /// `lda #v` then a store, for planting an argument a routine reads from
    /// memory rather than from a register.
    fn poke(&mut self, addr: u16, v: u8) -> &mut Self {
        self.lda(v);
        if addr < 0x100 {
            self.0.extend_from_slice(&[0x85, addr as u8]);
        } else {
            self.0.extend_from_slice(&[0x8d, addr as u8, (addr >> 8) as u8]);
        }
        self
    }
    /// Zero the first 128 bytes of the nametable.
    ///
    /// Without this a VRAM comparison between the two BIOSes compares their
    /// BOOT SCREENS: the real one leaves its blank tile $24 across the
    /// nametable and ours leaves zeroes, so every byte the test did not write
    /// differs and every byte it did write agrees. That looks like a broken
    /// routine and is a broken test.
    fn clear_vram(&mut self) -> &mut Self {
        self.0.extend_from_slice(&[
            0x2c, 0x02, 0x20, // bit $2002, reset the address latch
            0xa9, 0x20, 0x8d, 0x06, 0x20, // $2006 = $20
            0xa9, 0x00, 0x8d, 0x06, 0x20, // $2006 = $00
            0xa2, 0x80, // ldx #128
            0x8d, 0x07, 0x20, // sta $2007 with a still zero
            0xca, 0xd0, 0xfa, // dex : bne
        ]);
        self
    }
    /// Emit raw opcode bytes, for the handful of instructions with no helper.
    fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bytes);
        self
    }
    fn jsr(&mut self, addr: u16) -> &mut Self {
        self.0.extend_from_slice(&[0x20, addr as u8, (addr >> 8) as u8]);
        self
    }
    /// `jsr addr` followed by two little-endian words the routine reads as
    /// inline arguments and then steps its own return address over.
    fn jsr_with_args(&mut self, addr: u16, a: u16, b: u16) -> &mut Self {
        self.jsr(addr);
        self.0.extend_from_slice(&a.to_le_bytes());
        self.0.extend_from_slice(&b.to_le_bytes());
        self
    }
    /// Plant a run of bytes in memory before the call.
    fn bytes_at(&mut self, addr: u16, vals: &[u8]) -> &mut Self {
        for (i, v) in vals.iter().enumerate() {
            self.poke(addr + i as u16, *v);
        }
        self
    }
    /// Record a, x, y and the flags, then the sentinel, then spin forever.
    fn record_and_halt(&mut self) -> &mut Self {
        self.0.extend_from_slice(&[0x8d, OUT as u8, (OUT >> 8) as u8]);
        self.0.extend_from_slice(&[0x8e, (OUT + 1) as u8, (OUT >> 8) as u8]);
        self.0.extend_from_slice(&[0x8c, (OUT + 2) as u8, (OUT >> 8) as u8]);
        self.0.extend_from_slice(&[0x08, 0x68]); // php : pla
        self.0.extend_from_slice(&[0x8d, (OUT + 3) as u8, (OUT >> 8) as u8]);
        self.lda(DONE);
        self.0
            .extend_from_slice(&[0x8d, OUT_SENTINEL as u8, (OUT_SENTINEL >> 8) as u8]);
        let here = PROG + self.0.len() as u16;
        self.0.extend_from_slice(&[0x4c, here as u8, (here >> 8) as u8]);
        self
    }
}

/// What the test program recorded.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    reached: bool,
    a: u8,
    x: u8,
    y: u8,
    p: u8,
}

/// Boot `disk` under `bios` and stop the instant the BIOS hands the machine
/// over, with the machine left exactly there.
///
/// Handover is the BIOS dispatching through the `$DFFC` pseudo-vector, not
/// merely the first exit from the `$E000-$FFFF` window. The real BIOS leaves
/// NMI enabled while it loads, so its first exit is an interrupt dispatched
/// mid-load; taking that for handover made six Namco titles look broken. See
/// fds_hle_boot.rs.
fn boot_to_handover(disk: &[u8], bios: &[u8]) -> nes_core::Nes {
    let mut nes = nes_core::Nes::from_fds(disk, bios).expect("disk should parse");
    let deadline = nes.dbg_frame() + 1500;
    while nes.dbg_frame() < deadline {
        let pc = nes.dbg_pc();
        if pc < BIOS_BASE {
            let v = u16::from(nes.peek(VEC_RESET)) | (u16::from(nes.peek(VEC_RESET + 1)) << 8);
            if pc == v {
                break;
            }
        }
        nes.step();
    }
    assert!(nes.dbg_pc() < BIOS_BASE, "never reached handover");
    nes
}

/// Boot `disk` under `bios` to handover, then run `prog` instead of the game.
fn run(disk: &[u8], bios: &[u8], prog: &[u8], frames: u64) -> (Outcome, nes_core::Nes) {
    let mut nes = boot_to_handover(disk, bios);

    for (i, b) in prog.iter().enumerate() {
        nes.dbg_poke(PROG + i as u16, *b);
    }
    nes.dbg_poke(OUT_SENTINEL, 0);
    nes.dbg_set_pc(PROG);

    let stop = nes.dbg_frame() + frames;
    while nes.dbg_frame() < stop && !nes.dbg_halted() && nes.peek(OUT_SENTINEL) != DONE {
        nes.step();
    }
    let out = Outcome {
        reached: nes.peek(OUT_SENTINEL) == DONE,
        a: nes.peek(OUT),
        x: nes.peek(OUT + 1),
        y: nes.peek(OUT + 2),
        p: nes.peek(OUT + 3),
    };
    (out, nes)
}

/// Like [`run`], but with a button pattern held on pad 1 for the whole thing.
///
/// Buttons are in HARDWARE order here (A=$01 up to Right=$80). The BIOS shifts
/// them in the other way round, so what lands in `$F5` has A at bit 7 and
/// Right at bit 0. Getting that backwards makes a controller test pass on a
/// routine that reverses the pad.
fn run_holding(disk: &[u8], bios: &[u8], prog: &[u8], frames: u64, buttons: u8)
    -> (Outcome, nes_core::Nes)
{
    let mut nes = boot_to_handover(disk, bios);
    nes.set_buttons(0, buttons);
    for (i, b) in prog.iter().enumerate() {
        nes.dbg_poke(PROG + i as u16, *b);
    }
    nes.dbg_poke(OUT_SENTINEL, 0);
    nes.dbg_set_pc(PROG);
    let stop = nes.dbg_frame() + frames;
    while nes.dbg_frame() < stop && !nes.dbg_halted() && nes.peek(OUT_SENTINEL) != DONE {
        nes.step();
    }
    let out = Outcome {
        reached: nes.peek(OUT_SENTINEL) == DONE,
        a: nes.peek(OUT),
        x: nes.peek(OUT + 1),
        y: nes.peek(OUT + 2),
        p: nes.peek(OUT + 3),
    };
    (out, nes)
}

/// Run the same program under ours and, if both are present, under the real
/// BIOS and a real disk, and require the two to agree.
fn both(prog: &[u8], frames: u64) -> Option<(Outcome, nes_core::Nes)> {
    let disk = a_disk()?;
    let (ours, nes) = run(&disk, &[], prog, frames);
    if let Some(real) = real_bios() {
        let (theirs, _) = run(&disk, &real, prog, frames);
        assert!(
            theirs.reached,
            "the real BIOS never finished the test program, so this is measuring \
             the harness rather than the routine"
        );
        assert_eq!(ours, theirs, "ours (left) disagrees with the real BIOS (right)");
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some((ours, nes))
}

/// Cycles spent inside one call to `addr`, made from a planted caller.
///
/// Measured rather than asserted from the source, because a delay routine's
/// whole product is its duration and a comment claiming 131 is not evidence.
fn cycles_of_call(disk: &[u8], bios: &[u8], addr: u16) -> u64 {
    let mut p = Prog::default();
    p.jsr(addr).record_and_halt();
    let mut nes = boot_to_handover(disk, bios);
    for (i, b) in p.0.iter().enumerate() {
        nes.dbg_poke(PROG + i as u16, *b);
    }
    nes.dbg_set_pc(PROG);
    while nes.dbg_pc() != addr {
        nes.step();
    }
    let sp_in = nes.cpu.sp;
    let start = nes.dbg_cycles();
    // A deadline, because a routine that never returns would otherwise hang
    // the whole suite rather than fail one test. Two frames is far more than
    // any routine measured here takes.
    let give_up = start + 2 * 29_781;
    loop {
        nes.step();
        if nes.dbg_pc() < BIOS_BASE && nes.cpu.sp >= sp_in {
            break;
        }
        assert!(
            nes.dbg_cycles() < give_up,
            "${addr:04x} did not return within two frames"
        );
    }
    nes.dbg_cycles() - start
}

/// Every CPU-space write one call to `addr` makes, in order.
///
/// Logging starts at the routine's first instruction rather than at the
/// caller, so the `jsr`'s own two stack pushes are not in the result and an
/// empty stack range really means the routine pushed nothing.
fn writes_inside_call(disk: &[u8], bios: &[u8], prog: &[u8], addr: u16) -> Vec<(u16, u8)> {
    let mut nes = boot_to_handover(disk, bios);
    for (i, b) in prog.iter().enumerate() {
        nes.dbg_poke(PROG + i as u16, *b);
    }
    nes.dbg_poke(OUT_SENTINEL, 0);
    nes.dbg_set_pc(PROG);
    let deadline = nes.dbg_cycles() + 2 * 29_781;
    while nes.dbg_pc() != addr {
        nes.step();
        assert!(nes.dbg_cycles() < deadline, "never reached ${addr:04x}");
    }
    let sp_in = nes.cpu.sp;
    // Below $E000: RAM, the stack page and the hardware registers, but not the
    // routine fetching its own instructions, which would fill the buffer.
    nes.dbg_log_start(4096, BIOS_BASE);
    loop {
        nes.step();
        if nes.dbg_pc() < BIOS_BASE && nes.cpu.sp >= sp_in {
            break;
        }
        assert!(
            nes.dbg_cycles() < deadline,
            "${addr:04x} did not return within two frames"
        );
    }
    assert!(!nes.dbg_log_full(), "the access log filled, so this is a truncated set");
    nes.dbg_log_take()
        .into_iter()
        .filter(|(_, _, w)| *w)
        .map(|(a, v, _)| (a, v))
        .collect()
}

fn skipped() {
    eprintln!("skipping: no FDS disk in dumps/fds/ to run the harness on");
}

#[test]
fn the_harness_takes_the_machine_over_and_records() {
    // Before trusting any routine result, check the rig: a program that calls
    // nothing still has to run and report. Without this, a broken harness looks
    // exactly like two BIOSes agreeing.
    let mut p = Prog::default();
    p.lda(0x37).ldx(0x11).ldy(0x22).record_and_halt();
    let Some((r, _)) = both(&p.0, 60) else { return skipped() };
    assert!(r.reached, "the test program never ran");
    assert_eq!((r.a, r.x, r.y), (0x37, 0x11, 0x22));
}

#[test]
fn sprite_dma_at_e9c8_matches_the_real_bios() {
    // $E9C8, entered by 27 of the 114 corpus titles. Measured with `fdsprof`:
    // every caller writes $2003=$00 then $4014=$02 and reads $0200-$02FF,
    // returns by rts with a=$02, and costs exactly 18 CPU cycles.
    let mut p = Prog::default();
    p.lda(0xee).ldx(0x44).ldy(0x55).jsr(0xe9c8).record_and_halt();
    let Some((r, _)) = both(&p.0, 60) else { return skipped() };
    assert!(r.reached, "the routine never returned");
    assert_eq!(r.a, 0x02, "a should be the DMA page it wrote");
    // X and Y are the caller's and must survive. A routine that used them as
    // scratch would break every caller holding something across the call.
    assert_eq!((r.x, r.y), (0x44, 0x55));
}

#[test]
fn sprite_dma_actually_moves_the_page() {
    // The register check above would pass on a routine that did nothing but
    // `lda #2 : rts`. This one puts a pattern in page 2 and reads OAM back.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x0200, 0x3c)
        .poke(0x0201, 0x9d)
        .poke(0x02ff, 0x7e)
        .jsr(0xe9c8)
        .record_and_halt();
    let (r, nes) = run(&disk, &[], &p.0, 60);
    assert!(r.reached, "the routine never returned");
    assert_eq!(nes.dbg_oam()[0x00], 0x3c);
    assert_eq!(nes.dbg_oam()[0x01], 0x9d);
    assert_eq!(nes.dbg_oam()[0xff], 0x7e);
}

#[test]
fn the_real_bios_refuses_a_disk_without_the_licence_file_and_ours_does_not() {
    // The finding that shaped this whole file, kept as a test so it cannot
    // quietly stop being true. A disk carrying our own first file boots under
    // our BIOS and is refused by the real one. This is the difference being
    // deliberate: theirs is a licensing gate, and ours has no business having
    // one.
    const SIDE_LEN: usize = 65500;
    let mut s: Vec<u8> = Vec::with_capacity(SIDE_LEN);
    s.push(0x01);
    s.extend_from_slice(b"*NINTENDO-HVC*");
    s.push(0x00);
    s.extend_from_slice(b"TST ");
    while s.len() < 0x19 {
        s.push(0);
    }
    s.push(0xf0); // boot read file code: take everything
    while s.len() < 56 {
        s.push(0xff);
    }
    s.push(0x02);
    s.push(2);
    let prog: Vec<u8> = vec![0xa9, DONE, 0x8d, 0x00, 0x70, 0x4c, 0x05, 0x60];
    let file = |n: u8, addr: u16, body: &[u8], out: &mut Vec<u8>| {
        out.push(0x03);
        out.push(n);
        out.push(1);
        out.extend_from_slice(b"PROG    ");
        out.extend_from_slice(&addr.to_le_bytes());
        out.extend_from_slice(&(body.len() as u16).to_le_bytes());
        out.push(0);
        out.push(0x04);
        out.extend_from_slice(body);
    };
    file(0, 0x6000, &prog, &mut s);
    file(1, 0xdffc, &0x6000u16.to_le_bytes(), &mut s);
    s.resize(SIDE_LEN, 0);

    let boots = |bios: &[u8]| {
        let mut nes = nes_core::Nes::from_fds(&s, bios).expect("disk should parse");
        while nes.dbg_frame() < 900 && !nes.dbg_halted() {
            if nes.dbg_pc() == 0x6000 {
                return true;
            }
            nes.step();
        }
        false
    };
    assert!(boots(&[]), "our BIOS should run a disk that carries no licence file");
    if let Some(real) = real_bios() {
        assert!(
            !boots(&real),
            "the real BIOS booted a disk with no licence file, which contradicts \
             the measurement this test exists to record"
        );
    }
}

#[test]
fn set_scroll_at_eaea_matches_the_real_bios() {
    // $EAEA, 32 of 114 titles. $FD and $FC are the horizontal and vertical
    // scroll shadows and $FF the PPUCTRL shadow; the routine pushes all three
    // at the PPU and returns a = $FF with no mask applied.
    let mut p = Prog::default();
    p.poke(0x00fd, 0x11)
        .poke(0x00fc, 0x22)
        .poke(0x00ff, 0x14)
        .ldx(0x77)
        .ldy(0x88)
        .jsr(0xeaea)
        .record_and_halt();
    let Some((r, mut nes)) = both(&p.0, 60) else { return skipped() };
    assert!(r.reached);
    assert_eq!(r.a, 0x14, "a should be the PPUCTRL shadow, unmasked");
    assert_eq!((r.x, r.y), (0x77, 0x88), "x and y belong to the caller");
    assert_eq!(nes.dbg_ppu_ctrl(), 0x14, "PPUCTRL should be the shadow verbatim");
    // Writing $FF back would be $EA84's behaviour, not this one's.
    assert_eq!(nes.peek(0x00ff), 0x14, "the shadow is read, never written");
}

#[test]
fn vram_fill_at_ea84_fills_a_nametable_and_matches_the_real_bios() {
    // $EA84, 41 of 114 titles and the joint most-called after the NMI handler.
    // a = the page, x = the fill byte, y = the attribute byte when the page is
    // $20 or above.
    //
    // The shadow deliberately has bit 7 CLEAR. Bit 7 of PPUCTRL is the NMI
    // enable, and this routine writes the shadow straight at $2000: a first
    // draft used $FF=$FF, the routine wrote $FB, the NMI that followed went to
    // the game's handler through $DFFA and the test program was never seen
    // again. Bit 2 is set instead, because bit 2 is the one the routine masks.
    let mut p = Prog::default();
    p.poke(0x2000, 0x00)
        .poke(0x00ff, 0x34)
        .lda(0x20)
        .ldx(0xaa)
        .ldy(0x55)
        .jsr(0xea84)
        .record_and_halt();
    let Some((r, mut nes)) = both(&p.0, 60) else { return skipped() };
    assert!(r.reached);
    // The leftover low byte of the attribute address, measured on every caller.
    assert_eq!(r.a, 0xc0);
    assert_eq!((r.x, r.y), (0xaa, 0x55), "the arguments come back");
    // The three arguments are left in zero page on purpose: published state.
    assert_eq!((nes.peek(0), nes.peek(1), nes.peek(2)), (0x20, 0xaa, 0x55));
    // Bit 2 of the PPUCTRL shadow is forced off and the masked value persists.
    assert_eq!(nes.peek(0x00ff), 0x30, "$FF should keep the masked value");
    let nt = &nes.dbg_ciram()[..0x3c0];
    assert!(nt.iter().all(|&b| b == 0xaa), "the nametable should be filled");
    assert_eq!(&nes.dbg_ciram()[0x3c0..0x400], &[0x55u8; 64][..], "attributes");
}

#[test]
fn vram_fill_takes_a_page_count_below_page_twenty() {
    // Below $20 the y argument stops being an attribute byte and becomes a
    // count of 256-byte pages. The boundary is exactly $20, measured by
    // forcing a=$19 and a=$1F onto the short path and a=$20 onto the long one.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00)
        .poke(0x00ff, 0x00)
        .lda(0x00)
        .ldx(0x3c)
        .ldy(0x02)
        .jsr(0xea84)
        .record_and_halt();
    let (r, nes) = run(&disk, &[], &p.0, 60);
    assert!(r.reached);
    let chr = nes.dbg_chr_ram();
    assert!(chr[..0x200].iter().all(|&b| b == 0x3c), "two pages filled");
    assert!(chr[0x200..0x400].iter().any(|&b| b != 0x3c), "and no more than two");
}

#[test]
fn mem_fill_at_ead2_fills_the_pages_it_is_given() {
    // $EAD2, 27 of 114 titles. a = the byte, x = the first page, y = the last,
    // both inclusive. It touches no hardware at all.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.lda(0x5a).ldx(0x03).ldy(0x03).jsr(0xead2).record_and_halt();
    let (r, mut nes) = run(&disk, &[], &p.0, 60);
    assert!(r.reached);
    assert_eq!(r.a, 0x5a, "the fill byte survives the call");
    assert_eq!((r.x, r.y), (0x00, 0x00));
    for a in 0x0300..0x0400u16 {
        assert_eq!(nes.peek(a), 0x5a, "page 3 at ${a:04x}");
    }
    // Inclusive at both ends means one page here, and only one.
    assert_ne!(nes.peek(0x02ff), 0x5a, "page 2 should be untouched");
    assert_ne!(nes.peek(0x0400), 0x5a, "page 4 should be untouched");
    assert_eq!((nes.peek(0), nes.peek(1)), (0x00, 0x02), "$00=0, $01=x-1");
}

#[test]
fn read_pads_at_ea1f_reports_held_and_newly_pressed() {
    // $EA1F, 26 of 114 titles. It keeps the previous call's held state in
    // $F7/$F8 and returns "newly pressed" in $F5/$F6, so it has to be called
    // twice to mean anything. Bit order is A=$80 down to Right=$01.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    // First call establishes the "was held" baseline with nothing pressed,
    // then the second sees the buttons this test holds down.
    p.poke(0x00fb, 0x00).jsr(0xea1f).jsr(0xea1f).record_and_halt();
    let (r, mut nes) = run(&disk, &[], &p.0, 60);
    assert!(r.reached);
    assert_eq!(r.x, 0xff, "x is $FF on exit, whatever the data");
    // Nothing is pressed, so held and newly-pressed are both empty and the
    // two pads agree. This is the shape of the contract rather than its
    // content; `sees_a_button` below supplies the content.
    assert_eq!(nes.peek(0x00f5), 0x00);
    assert_eq!(nes.peek(0x00f7), 0x00);
    assert_eq!(r.a, nes.peek(0x00f5), "a is $F5");
    assert_eq!(r.y, nes.peek(0x00f7), "y is $F7");
}

/// Where a LoadFiles test puts the two things the routine reads through its
/// inline pointers. Work RAM, clear of the program at $0600 and the results
/// at $0700.
const LF_TEMPLATE: u16 = 0x0720;
const LF_LIST: u16 = 0x0740;
/// Written over a file's destination before the call, so "the bytes are there"
/// cannot be satisfied by the boot loader having put them there already.
const SCRIBBLE: u8 = 0xa5;

#[test]
fn load_files_at_e1f8_matches_the_real_bios() {
    // $E1F8, entered by 41 of the 114 corpus titles and the joint most-called
    // routine after the NMI handler. It cannot be reached through a game yet
    // because every title that calls it asks for something else first, so the
    // planted caller is the only way to test it at all.
    //
    // Ten $FF template bytes is "any disk", which is what makes the test work
    // on whichever disk the checkout happens to have. The list asks for one
    // file by ID and ends with $FF.
    let Some(disk) = a_disk() else { return skipped() };
    let (want, addr, body) = wanted_file(&disk);
    let mut p = Prog::default();
    // Scribble over the destination first. The boot loader already put this
    // file where it belongs, so without the scribble the check below passes on
    // a routine that loads nothing at all. Verified: it did.
    p.bytes_at(addr, &[SCRIBBLE; 32])
        .bytes_at(LF_TEMPLATE, &[0xff; 10])
        .bytes_at(LF_LIST, &[want, 0xff])
        .jsr_with_args(0xe1f8, LF_TEMPLATE, LF_LIST)
        .record_and_halt();

    let (ours, mut nes) = run(&disk, &[], &p.0, 900);
    assert!(ours.reached, "the routine never returned");
    assert_eq!(ours.a, 0x00, "a is the status and this load should succeed");
    assert_eq!(ours.x, ours.a, "x mirrors the status");
    assert!(ours.y >= 1, "y counts the files loaded, and one was asked for");
    assert_eq!(nes.peek(0x000e), ours.y, "$0E holds the same count as y");

    // The scribble has to be gone and the file's own bytes in its place.
    let got: Vec<u8> = (0..32)
        .map(|i| nes.peek(addr.wrapping_add(i as u16)))
        .collect();
    assert_eq!(
        got,
        body[..32],
        "the file was not written over the scribble at ${addr:04x}"
    );

    if let Some(real) = real_bios() {
        let (theirs, _) = run(&disk, &real, &p.0, 900);
        assert!(theirs.reached, "the real BIOS never finished the test program");
        assert_eq!(
            (ours.a, ours.x, ours.y, ours.p),
            (theirs.a, theirs.x, theirs.y, theirs.p),
            "ours (left) disagrees with the real BIOS (right) on a/x/y/p"
        );
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
}

#[test]
fn load_files_steps_its_return_address_over_the_inline_arguments() {
    // The two pointer words sit after the jsr, so a routine that returned
    // normally would execute them as opcodes. Reaching the recording code at
    // all is the proof; without the fix-up the machine would be somewhere in
    // the middle of a pointer.
    let Some(disk) = a_disk() else { return skipped() };
    let want = wanted_file_id(&disk);
    let mut p = Prog::default();
    p.bytes_at(LF_TEMPLATE, &[0xff; 10])
        .bytes_at(LF_LIST, &[want, 0xff])
        .jsr_with_args(0xe1f8, LF_TEMPLATE, LF_LIST)
        .lda(0x5c)
        .record_and_halt();
    let (r, _) = run(&disk, &[], &p.0, 900);
    assert!(r.reached, "control did not resume after the inline arguments");
    assert_eq!(r.a, 0x5c, "execution resumed at the wrong place");
}

#[test]
fn a_template_that_does_not_match_the_disk_is_an_error() {
    // A disk-ID mismatch returns a BCD code rather than loading anything, and
    // a caller checks it with beq. Byte 0 of the template maps to $04.
    let Some(disk) = a_disk() else { return skipped() };
    let mut bad = [0xffu8; 10];
    bad[0] = 0x5a; // no real disk has this maker code
    let mut p = Prog::default();
    p.bytes_at(LF_TEMPLATE, &bad)
        .bytes_at(LF_LIST, &[0x00, 0xff])
        .jsr_with_args(0xe1f8, LF_TEMPLATE, LF_LIST)
        .record_and_halt();
    let (ours, _) = run(&disk, &[], &p.0, 1800);
    assert!(ours.reached, "the routine never returned");
    assert_ne!(ours.a, 0x00, "a mismatch must not report success");
    assert_eq!(ours.a, 0x04, "byte 0 of the template maps to BCD $04");
    assert_eq!(ours.y, 0x00, "nothing should have loaded");
}

/// A file the disk in this checkout actually carries: its ID, where it loads,
/// and its bytes. Read out of the image rather than hardcoded, so the test does
/// not depend on which of the three disks is present.
///
/// It picks a file that lands in program RAM with a body worth comparing,
/// because a test that only checked the return value would pass on a routine
/// that returned the right numbers and loaded nothing at all.
fn wanted_file(disk: &[u8]) -> (u8, u16, Vec<u8>) {
    let side = if disk.starts_with(b"FDS") { &disk[16..] } else { disk };
    let mut p = 56usize;
    assert_eq!(side[p], 0x02, "block 2 should follow the disk info");
    let count = side[p + 1];
    p += 2;
    for _ in 0..count {
        assert_eq!(side[p], 0x03, "a file header should be here");
        let id = side[p + 2];
        let addr = u16::from_le_bytes([side[p + 11], side[p + 12]]);
        let size = u16::from_le_bytes([side[p + 13], side[p + 14]]) as usize;
        let kind = side[p + 15];
        let body = side[p + 17..p + 17 + size].to_vec();
        p += 16 + 1 + size;
        if kind == 0
            && size >= 64
            && addr >= 0x6000
            && body[..32].iter().any(|&b| b != SCRIBBLE)
        {
            return (id, addr, body);
        }
    }
    panic!("this disk has no program-RAM file big enough to check against");
}

fn wanted_file_id(disk: &[u8]) -> u8 {
    wanted_file(disk).0
}

#[test]
fn vram_upload_at_ebaf_matches_the_real_bios() {
    // $EBAF, entered by 29 of the 114 corpus titles. a and y are the VRAM
    // address, x the number of 16-byte units, and the source address is the
    // word inline after the jsr.
    //
    // NMI stays off: $FF has bit 7 clear, because this routine writes the
    // shadow straight at $2000 and an NMI would hand the machine to the game.
    const SRC: u16 = 0x0760;
    let Some(disk) = a_disk() else { return skipped() };
    let pattern: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(7).wrapping_add(3)).collect();
    let mut p = Prog::default();
    p.poke(0x2000, 0x00)
        .poke(0x00ff, 0x34) // bit 2 set, so the mask is observable
        .bytes_at(SRC, &pattern)
        .lda(0x00) // VRAM $2000
        .ldy(0x20)
        .ldx(0x02); // two units, 32 bytes
    // One inline word here, not two: this routine takes a single source
    // pointer where LoadFiles takes a template and a list.
    p.jsr(0xebaf);
    p.0.extend_from_slice(&SRC.to_le_bytes());
    p.record_and_halt();

    let (ours, mut nes) = run(&disk, &[], &p.0, 120);
    assert!(ours.reached, "the routine never returned");
    assert_eq!((ours.x, ours.y), (0x00, 0x00), "x and y are zero on exit");
    // a is the high byte of the first address past the source data.
    assert_eq!(ours.a, ((SRC as u32 + 32) >> 8) as u8, "a is the past-end high byte");
    // The mask is exactly $FB: bit 2 off, everything else kept.
    assert_eq!(nes.peek(0x00ff), 0x30, "the shadow keeps the masked value");
    assert_eq!(nes.dbg_ppu_ctrl(), 0x30, "and $2000 gets the same");
    assert_eq!(&nes.dbg_ciram()[..32], &pattern[..], "the bytes should be in VRAM");
    assert_eq!(nes.peek(0x0004), 0x00, "$04 keeps the VRAM low byte as passed");
    assert_eq!(nes.peek(0x0002), 0x00, "$02 counts down to zero");

    if let Some(real) = real_bios() {
        let (theirs, _) = run(&disk, &real, &p.0, 120);
        assert!(theirs.reached, "the real BIOS never finished the test program");
        assert_eq!(
            (ours.a, ours.x, ours.y),
            (theirs.a, theirs.x, theirs.y),
            "ours (left) disagrees with the real BIOS (right)"
        );
    }
}

#[test]
fn vram_upload_steps_over_its_inline_source_pointer() {
    // Same shape as LoadFiles: the pointer sits after the jsr and would be
    // executed as opcodes if the return address were not stepped on.
    const SRC: u16 = 0x0760;
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x00ff, 0x00).bytes_at(SRC, &[0x11; 16]);
    p.lda(0x00).ldy(0x20).ldx(0x01);
    p.jsr(0xebaf);
    p.0.extend_from_slice(&SRC.to_le_bytes());
    p.lda(0x3d).record_and_halt();
    let (r, _) = run(&disk, &[], &p.0, 120);
    assert!(r.reached, "control did not resume after the inline pointer");
    assert_eq!(r.a, 0x3d, "execution resumed at the wrong place");
}

#[test]
fn vint_wait_at_e1b2_returns_within_a_frame_and_matches_the_real_bios() {
    // $E1B2, entered by 18 of 114 titles and the first thing Adian no Tsue
    // asks for. It waits for the next vblank NMI: no arguments, no failure
    // path, and a, x and y come back untouched.
    if a_disk().is_none() {
        return skipped();
    }
    let mut p = Prog::default();
    p.lda(0x3b).ldx(0x5a).ldy(0xa5).jsr(0xe1b2).record_and_halt();
    // Three frames is comfortably more than the one frame it can ever take.
    let Some((r, mut nes)) = both(&p.0, 3) else { return skipped() };
    assert!(r.reached, "it never came back, so the wait never ended");
    assert_eq!((r.a, r.x, r.y), (0x3b, 0x5a, 0xa5), "the registers are the caller's");
    // It leaves NMI off, in the shadow and in the register alike.
    assert_eq!(nes.peek(0x00ff) & 0x80, 0, "bit 7 of the shadow should be clear");
    assert_eq!(nes.dbg_ppu_ctrl() & 0x80, 0, "and NMI off in $2000");
}

#[test]
fn vint_wait_restores_the_nmi_selector_rather_than_resetting_it() {
    // $0100 is saved on the stack and put back. A version that reset it to the
    // boot default of $C0 would silently move a game off vector 1 or 2.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x0100, 0x80).jsr(0xe1b2).record_and_halt();
    let (r, mut nes) = run(&disk, &[], &p.0, 3);
    assert!(r.reached);
    assert_eq!(nes.peek(0x0100), 0x80, "the selector should come back as it was");
}

#[test]
fn vint_wait_keeps_every_bit_of_the_ppuctrl_shadow_but_the_top_one() {
    // The rule is exactly `ora #$80` going in and `and #$7F` coming out.
    // Anything coarser would quietly change the sprite size, the pattern table
    // or the VRAM increment on its way past.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x00ff, 0x7f).jsr(0xe1b2).record_and_halt();
    let (r, mut nes) = run(&disk, &[], &p.0, 3);
    assert!(r.reached);
    assert_eq!(nes.peek(0x00ff), 0x7f, "all seven low bits should survive");
}

#[test]
fn delay_at_e149_takes_exactly_131_cycles() {
    // $E149 is the most-called routine in the corpus, 1,054,357 calls across
    // 17 titles, and 15 titles were stopped on it. It does nothing but take
    // time, so the time IS the contract.
    //
    // This is the one routine where our usual rule inverts. Everywhere else
    // being faster than the original is safe, because a caller that budgeted
    // for the slow version still fits. Here a game counting these to pace a
    // PPU access would get a shorter wait than it asked for.
    let Some(disk) = a_disk() else { return skipped() };
    assert_eq!(cycles_of_call(&disk, &[], 0xe149), 131);
    if let Some(real) = real_bios() {
        assert_eq!(
            cycles_of_call(&disk, &real, 0xe149),
            131,
            "the real BIOS should agree, or the number above is the wrong one"
        );
    }
}

#[test]
fn the_delay_preserves_everything_and_clears_carry_and_overflow() {
    // a, x and y come back untouched, N and Z follow a, and C and V come out
    // clear whatever went in: the real one returns p=$20 for an entry of $61.
    if a_disk().is_none() {
        return skipped();
    }
    let mut p = Prog::default();
    // `sec` puts carry in before the call, so "comes out clear" means something.
    p.lda(0x3c).ldx(0x5a).ldy(0xa5);
    p.0.push(0x38); // sec
    p.jsr(0xe149).record_and_halt();
    let Some((r, _)) = both(&p.0, 10) else { return skipped() };
    assert!(r.reached);
    assert_eq!((r.a, r.x, r.y), (0x3c, 0x5a, 0xa5), "the registers are the caller's");
    assert_eq!(r.p & 0x01, 0, "carry should come out clear");
    assert_eq!(r.p & 0x40, 0, "overflow should come out clear");
}

#[test]
fn the_delay_is_decimal_proof() {
    // Forced into the real routine with D=1 it still took 131, so whatever
    // counts inside it is not an adc/sbc chain. Ours shifts, which cannot
    // care, and this is the test that stops someone "simplifying" it into
    // arithmetic later.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.0.push(0xf8); // sed
    p.jsr(0xe149);
    p.0.push(0xd8); // cld, so the harness's own recording is unaffected
    p.record_and_halt();
    let (r, _) = run(&disk, &[], &p.0, 10);
    assert!(r.reached, "it never returned with decimal mode set");
    // And the count itself must not move. Measured separately because the
    // program above cannot report its own cycles.
    assert_eq!(cycles_of_call(&disk, &[], 0xe149), 131);
}

/// Where a `$E7BB` test builds its structure.
const STRUCT_AT: u16 = 0x0760;

/// Run a hand-built VRAM structure through `$E7BB` under both BIOSes and
/// return what ours did, plus its machine.
///
/// Comparing this routine by watching a game does not work: by the time a title
/// reaches it the two runs have taken different paths and are pointing it at
/// different structures. Building the structure here is the only way to ask
/// both BIOSes the same question.
fn walk_struct(bytes: &[u8]) -> Option<(Outcome, nes_core::Nes)> {
    let disk = a_disk()?;
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x00ff, 0x00).clear_vram();
    p.bytes_at(STRUCT_AT, bytes);
    p.jsr(0xe7bb);
    p.0.extend_from_slice(&STRUCT_AT.to_le_bytes());
    p.record_and_halt();

    let (ours, nes) = run(&disk, &[], &p.0, 30);
    if let Some(real) = real_bios() {
        let (theirs, mut rn) = run(&disk, &real, &p.0, 30);
        assert!(theirs.reached, "the real BIOS never finished the test program");
        assert_eq!(
            (ours.a, ours.y),
            (theirs.a, theirs.y),
            "ours (left) disagrees with the real BIOS (right) on a/y"
        );
        let mut ours_nes = nes;
        assert_eq!(
            ours_nes.dbg_ciram()[..64],
            rn.dbg_ciram()[..64],
            "the two BIOSes wrote different video memory"
        );
        assert_eq!(ours_nes.peek(0x00ff), rn.peek(0x00ff), "the PPUCTRL shadow");
        return Some((ours, ours_nes));
    }
    Some((ours, nes))
}

#[test]
fn vram_struct_at_e7bb_writes_a_plain_entry() {
    // Address high, address low, a length byte, then that many bytes.
    let Some((r, mut nes)) =
        walk_struct(&[0x20, 0x00, 0x04, 0xde, 0xad, 0xbe, 0xef, 0xff])
    else {
        return skipped();
    };
    assert!(r.reached, "the routine never returned");
    assert_eq!(r.a, 0xff, "a is the terminator byte");
    assert_eq!(r.y, 0x00, "y is zero on exit");
    assert_eq!(&nes.dbg_ciram()[..4], &[0xde, 0xad, 0xbe, 0xef]);
}

#[test]
fn vram_struct_repeats_one_byte_when_bit_six_is_set() {
    // bit 6 of the length byte means "one data byte, written count times".
    let Some((r, mut nes)) = walk_struct(&[0x20, 0x00, 0x46, 0x5a, 0xff]) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(&nes.dbg_ciram()[..6], &[0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a]);
    assert_ne!(nes.dbg_ciram()[6], 0x5a, "six, and no more than six");
}

#[test]
fn a_count_of_zero_means_sixty_four() {
    // Zero in bits 0-5 is sixty-four, not nothing. Reading it as nothing would
    // silently drop a whole row of a nametable.
    let Some((r, mut nes)) = walk_struct(&[0x20, 0x00, 0x40, 0x3c, 0xff]) else {
        return skipped();
    };
    assert!(r.reached);
    assert!(nes.dbg_ciram()[..64].iter().all(|&b| b == 0x3c), "64 bytes");
    assert_ne!(nes.dbg_ciram()[64], 0x3c, "and not 65");
}

#[test]
fn any_byte_with_bit_seven_set_ends_the_structure() {
    // Not just $FF. $80 and $BE both terminate, and the byte itself comes back.
    for end in [0xffu8, 0x80, 0xbe] {
        let Some((r, _)) = walk_struct(&[0x20, 0x00, 0x02, 0x11, 0x22, end]) else {
            return skipped();
        };
        assert!(r.reached, "terminator ${end:02x} did not end the walk");
        assert_eq!(r.a, end, "a should be the terminator that ended it");
    }
}

#[test]
fn a_struct_can_call_and_return() {
    // $4C pushes the current position and jumps; $60 pops and resumes three
    // bytes on, past the $4C and its operand.
    const SUB: u16 = 0x0790;
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x00ff, 0x00).clear_vram();
    // main: call SUB, then write $77 twice, then end.
    p.bytes_at(STRUCT_AT, &[0x4c, SUB as u8, (SUB >> 8) as u8,
                           0x20, 0x08, 0x02, 0x77, 0x77, 0xff]);
    // sub: write $11 $22 at $2000, then return.
    p.bytes_at(SUB, &[0x20, 0x00, 0x02, 0x11, 0x22, 0x60]);
    p.jsr(0xe7bb);
    p.0.extend_from_slice(&STRUCT_AT.to_le_bytes());
    p.record_and_halt();

    let (ours, mut nes) = run(&disk, &[], &p.0, 30);
    assert!(ours.reached, "the routine never returned");
    assert_eq!(&nes.dbg_ciram()[..2], &[0x11, 0x22], "the called struct ran");
    assert_eq!(&nes.dbg_ciram()[8..10], &[0x77, 0x77], "and it came back");
    if let Some(real) = real_bios() {
        let (theirs, rn) = run(&disk, &real, &p.0, 30);
        assert!(theirs.reached);
        assert_eq!(nes.dbg_ciram()[..16], rn.dbg_ciram()[..16]);
        assert_eq!(ours.a, theirs.a);
    }
}

#[test]
fn bit_seven_of_the_length_byte_sets_the_vram_increment_and_keeps_it() {
    // It writes (shadow & ~$04) with bit 2 put back when the flag is set, and
    // the shadow KEEPS the result, so the stepping outlives the call.
    let Some((r, mut nes)) = walk_struct(&[0x20, 0x00, 0x82, 0x11, 0x22, 0xff]) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(nes.peek(0x00ff) & 0x04, 0x04, "bit 2 should be left set");
    assert_eq!(nes.dbg_ppu_ctrl() & 0x04, 0x04, "and $2000 should agree");
}

#[test]
fn a_palette_entry_leaves_the_vram_address_at_zero() {
    // After an entry whose address high byte is $3F, and only then, the
    // routine writes $2006 four more times with $3F,$00 and then $00,$00.
    // That is the palette-safe reset: a PPU left addressing inside the palette
    // shows that palette entry in place of the backdrop, which is a visible
    // coloured band across the screen.
    //
    // The effect is observable as where the NEXT $2007 write lands. Without
    // the reset the address is still in the palette; with it, at $0000, which
    // is pattern memory. This test exists because dropping the reset entirely
    // was the one mutation the rest of the suite did not notice.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x00ff, 0x00).clear_vram();
    p.bytes_at(STRUCT_AT, &[0x3f, 0x00, 0x04, 0x0f, 0x11, 0x22, 0x33, 0xff]);
    p.jsr(0xe7bb);
    p.0.extend_from_slice(&STRUCT_AT.to_le_bytes());
    // Now write a sentinel through $2007 and see where the PPU put it.
    p.lda(0x5a).raw(&[0x8d, 0x07, 0x20]);
    p.record_and_halt();

    let (ours, nes) = run(&disk, &[], &p.0, 30);
    assert!(ours.reached, "the routine never returned");
    assert_eq!(
        nes.dbg_chr_ram()[0],
        0x5a,
        "the next write should have landed at $0000, so the address was reset"
    );
    if let Some(real) = real_bios() {
        let (theirs, rn) = run(&disk, &real, &p.0, 30);
        assert!(theirs.reached);
        assert_eq!(
            nes.dbg_chr_ram()[0], rn.dbg_chr_ram()[0],
            "ours and the real BIOS should leave the address in the same place"
        );
    }
}

#[test]
fn a_non_palette_entry_does_not_reset_the_address() {
    // The other half: $2F and $3E were both measured NOT to trigger it, so the
    // test is `== $3F` and not "anything palette-ish". Without this, a reset
    // applied unconditionally would pass the test above.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x00ff, 0x00).clear_vram();
    p.bytes_at(STRUCT_AT, &[0x20, 0x00, 0x02, 0x11, 0x22, 0xff]);
    p.jsr(0xe7bb);
    p.0.extend_from_slice(&STRUCT_AT.to_le_bytes());
    p.lda(0x5a).raw(&[0x8d, 0x07, 0x20]);
    p.record_and_halt();

    let (ours, nes) = run(&disk, &[], &p.0, 30);
    assert!(ours.reached);
    // The address should have carried on from $2002, not jumped to $0000.
    assert_eq!(nes.dbg_ciram()[2], 0x5a, "the write should continue in the nametable");
    assert_ne!(nes.dbg_chr_ram()[0], 0x5a, "and not have been reset to $0000");
}

#[test]
fn verifying_pad_read_at_ea4c_matches_the_real_bios() {
    // $EA4C is $EA1F with a verification pass: read both pads, read again, and
    // only believe an answer that repeats. 17 titles call it and twelve were
    // stopped on it.
    //
    // The trap here is a test that proves nothing. With no buttons held and no
    // previous state, every byte involved is zero and the test passes against a
    // routine that simply zeroes $F5-$F8. So: hold a real pattern, and plant a
    // previous-held value so "newly pressed" has to be computed rather than
    // copied.
    let Some(disk) = a_disk() else { return skipped() };
    const HELD_HW: u8 = 0x05; // A | Select, in hardware order
    const HELD_BIOS: u8 = 0xa0; // the same two, as the BIOS stores them
    let mut p = Prog::default();
    p.poke(0x00fb, 0x00) // strobe byte
        .poke(0x00f7, 0x20) // pad 1 was already holding Select
        .poke(0x00f8, 0x00)
        .jsr(0xea4c)
        .record_and_halt();

    let (ours, mut nes) = run_holding(&disk, &[], &p.0, 10, HELD_HW);
    assert!(ours.reached, "the routine never returned");
    assert_eq!(nes.peek(0x00f7), HELD_BIOS, "$F7 should hold what is pressed now");
    assert_eq!(
        nes.peek(0x00f5),
        HELD_BIOS & !0x20,
        "newly pressed is what is held now and was not held before"
    );
    assert_eq!(ours.a, nes.peek(0x00f5), "a is the new $F5");
    assert_eq!(ours.y, HELD_BIOS, "y is the new $F7");
    assert_eq!(ours.x, 0xff, "x is $FF on exit");
    assert_eq!(ours.p & 0x01, 0x01, "carry is set by the compare that passed");

    if let Some(real) = real_bios() {
        let (theirs, mut rn) = run_holding(&disk, &real, &p.0, 10, HELD_HW);
        assert!(theirs.reached, "the real BIOS never finished the test program");
        assert_eq!(
            (ours.a, ours.x, ours.y, ours.p),
            (theirs.a, theirs.x, theirs.y, theirs.p),
            "ours (left) disagrees with the real BIOS (right)"
        );
        for cell in [0x00f5u16, 0x00f6, 0x00f7, 0x00f8] {
            assert_eq!(
                nes.peek(cell),
                rn.peek(cell),
                "${cell:04x} differs between the two BIOSes"
            );
        }
    }
}

#[test]
fn the_verifying_read_really_reads_twice() {
    // The real routine costs 873 cycles, constant across all 17 callers, and
    // it is long because it reads the pads TWICE rather than because it drives
    // anything. Ours costs 832: the same two passes, slightly cheaper.
    //
    // Asserting 873 exactly would be asserting an implementation detail of
    // somebody else's merge step. What actually matters is that the second
    // read happens at all, because a single-pass version would be a $EA1F with
    // a longer name and no verification in it. So compare against $EA1F: the
    // difference has to be most of another read pass.
    //
    // Being faster is safe here, as everywhere in this BIOS except $E149,
    // where the duration is the product.
    let Some(disk) = a_disk() else { return skipped() };
    let once = cycles_of_call(&disk, &[], 0xea1f);
    let twice = cycles_of_call(&disk, &[], 0xea4c);
    assert!(
        twice > once + 300,
        "$EA4C took {twice} against $EA1F's {once}, which is not a second read"
    );
    if let Some(real) = real_bios() {
        let theirs = cycles_of_call(&disk, &real, 0xea4c);
        assert_eq!(theirs, 873, "the oracle's number, in case it ever moves");
        assert!(twice <= theirs, "ours ({twice}) must not be slower than {theirs}");
    }
}

#[test]
fn the_plain_pad_read_still_works_after_sharing_its_read_pass() {
    // $EA1F and $EA4C now share one strobe-and-shift pass. This is here so a
    // change to the shared half cannot quietly break the simpler caller.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x00fb, 0x00).poke(0x00f7, 0x00).jsr(0xea1f).record_and_halt();
    let (ours, mut nes) = run_holding(&disk, &[], &p.0, 10, 0x05);
    assert!(ours.reached);
    assert_eq!(nes.peek(0x00f7), 0xa0, "held, with A at bit 7");
    assert_eq!(nes.peek(0x00f5), 0xa0, "all of it newly pressed");
    assert_eq!(ours.x, 0xff);
}

// --- $E161 and the rest of the PPUMASK family -------------------------------

/// The five entry points, with the result measured for a forced shadow of
/// `$00` and of `$FF`.
///
/// Those two columns are the whole reason this is a table of numbers rather
/// than a loop over a transform. Any one of the five is indistinguishable
/// from a routine that stores a constant until you run it twice from two
/// different shadows, and the real BIOS was run twice from both.
const MASK_FAMILY: [(u16, u8, u8, &str); 5] = [
    (0xe161, 0x00, 0xe7, "screen off"),
    (0xe16b, 0x18, 0xff, "screen on"),
    (0xe171, 0x00, 0xef, "sprites off"),
    (0xe17e, 0x00, 0xf7, "background off"),
    (0xe185, 0x08, 0xff, "background on"),
];

/// `ldx $FE`, so a program can report the shadow without disturbing `A`.
const LDX_SHADOW: [u8; 2] = [0xa6, 0xfe];

#[test]
fn the_ppumask_family_transforms_the_shadow_and_matches_the_real_bios() {
    if a_disk().is_none() {
        return skipped();
    }
    for (entry, from_zero, from_ones, what) in MASK_FAMILY {
        for (shadow, want) in [(0x00u8, from_zero), (0xffu8, from_ones)] {
            let mut p = Prog::default();
            p.poke(0x00fe, shadow).jsr(entry);
            // The result is in a; the shadow goes through x, so one call
            // reports both and both are held against the oracle.
            p.raw(&LDX_SHADOW).record_and_halt();
            let Some((r, _)) = both(&p.0, 10) else { return skipped() };
            assert!(r.reached, "${entry:04x} ({what}) never returned");
            assert_eq!(
                r.a, want,
                "${entry:04x} ({what}) from a shadow of ${shadow:02x} should give ${want:02x}"
            );
            assert_eq!(
                r.x, want,
                "${entry:04x} ({what}) should leave the same value in the $FE shadow"
            );
        }
    }
}

#[test]
fn the_ppumask_family_masks_rather_than_replaces() {
    // Emphasis bits 5-7 and the left-column bits 0-2 pass straight through.
    // Bubble Bobble's natural $26 comes back $26 from $E161, which is what
    // first proved these are masks and not constants.
    //
    // The per-entry results are not written down here: `both` holds every one
    // of them against the real BIOS, which is the only authority on a value
    // no measurement note recorded.
    if real_bios().is_none() {
        eprintln!("note: no oracle, so this test has nothing to compare against");
        return skipped();
    }
    for (entry, _, _, what) in MASK_FAMILY {
        let mut p = Prog::default();
        p.poke(0x00fe, 0x26).jsr(entry).raw(&LDX_SHADOW).record_and_halt();
        let Some((r, _)) = both(&p.0, 10) else { return skipped() };
        assert!(r.reached, "${entry:04x} ({what}) never returned");
        assert_eq!(r.a & 0xe0, 0x20, "${entry:04x} ({what}) dropped an emphasis bit");
        assert_eq!(r.a & 0x07, 0x06, "${entry:04x} ({what}) dropped a left-column bit");
    }
}

#[test]
fn the_ppumask_family_writes_the_shadow_then_the_register_and_nothing_else() {
    // The complete bus activity of one real call is: read $FE, write $FE,
    // write $2001, then the rts reads. Two writes, shadow first.
    //
    // Shadow-first is not cosmetic. A handler that interrupts between the two
    // writes sees a shadow that already agrees with where the hardware is
    // going, and that order is observable to anything that reads $FE.
    //
    // The emptiness of the stack range is the other half: unlike $E149 these
    // do not even `pha`, so the bytes below the returned stack pointer come
    // back exactly as the caller left them.
    let Some(disk) = a_disk() else { return skipped() };
    for (entry, _, from_ones, what) in MASK_FAMILY {
        let mut p = Prog::default();
        p.poke(0x00fe, 0xff).jsr(entry).record_and_halt();
        let ours = writes_inside_call(&disk, &[], &p.0, entry);
        assert_eq!(
            ours,
            vec![(0x00fe, from_ones), (0x2001, from_ones)],
            "${entry:04x} ({what}) should write the shadow then $2001 and nothing else"
        );
        if let Some(real) = real_bios() {
            assert_eq!(
                writes_inside_call(&disk, &real, &p.0, entry),
                ours,
                "${entry:04x} ({what}) disagrees with the real BIOS on what it writes"
            );
        }
    }
}

#[test]
fn the_ppumask_family_preserves_the_registers_and_every_flag_but_n_and_z() {
    // No arguments in any register, x and y preserved, and C, V, D and I all
    // survive. Only N and Z move, and they follow the result.
    //
    // Forcing the flags in is what makes this bite: the real routine returned
    // p=$61 for an entry of $61, so a version that cleared carry on the way
    // out would look identical against a caller that entered with carry
    // already clear.
    if a_disk().is_none() {
        return skipped();
    }
    for (entry, from_zero, _, what) in MASK_FAMILY {
        let mut p = Prog::default();
        // $FD is a scroll shadow, borrowed here as somewhere to put a value
        // whose bit 6 the `bit` below can lift into V.
        p.poke(0x00fd, 0x40);
        p.poke(0x00fe, 0x00);
        p.ldx(0xaa).ldy(0x55);
        p.lda(0xff); // a is an argument to nothing; it comes back as the result
        p.raw(&[0x38]); // sec
        p.raw(&[0x78]); // sei
        p.raw(&[0xf8]); // sed
        p.raw(&[0x24, 0xfd]); // bit $fd, whose bit 6 sets V
        p.jsr(entry).record_and_halt();
        let Some((r, _)) = both(&p.0, 10) else { return skipped() };
        assert!(r.reached, "${entry:04x} ({what}) never returned");
        assert_eq!(r.a, from_zero, "${entry:04x} ({what}) should return the result in a");
        assert_eq!(r.x, 0xaa, "${entry:04x} ({what}) clobbered x");
        assert_eq!(r.y, 0x55, "${entry:04x} ({what}) clobbered y");
        assert_eq!(r.p & 0x01, 0x01, "${entry:04x} ({what}) lost carry");
        assert_eq!(r.p & 0x40, 0x40, "${entry:04x} ({what}) lost overflow");
        assert_eq!(r.p & 0x04, 0x04, "${entry:04x} ({what}) lost the interrupt disable");
        assert_eq!(r.p & 0x08, 0x08, "${entry:04x} ({what}) lost decimal mode");
        // N and Z from the result. Three of the five return $00 from a zero
        // shadow and two do not, so this separates "from the result" from
        // "from the shadow it read".
        assert_eq!(
            r.p & 0x02 != 0,
            r.a == 0,
            "${entry:04x} ({what}) set Z from something other than the result"
        );
        assert_eq!(
            r.p & 0x80 != 0,
            r.a & 0x80 != 0,
            "${entry:04x} ({what}) set N from something other than the result"
        );
    }
}

#[test]
fn the_ppumask_family_takes_the_same_cycles_as_the_real_bios() {
    // Eighteen for $E161 and twenty-one for the other four, min=max across
    // every caller and every forced state including D=1.
    //
    // Being faster is safe nearly everywhere in this BIOS, but not here. These
    // write $2001 and never read $2002, so games call them mid-frame, and a
    // version that arrived early would move a raster split by however many
    // cycles it saved. That puts the duration in the interface, the same way
    // it is for $E149 and for interrupt dispatch.
    let Some(disk) = a_disk() else { return skipped() };
    for (entry, _, _, what) in MASK_FAMILY {
        let want = if entry == 0xe161 { 18 } else { 21 };
        assert_eq!(cycles_of_call(&disk, &[], entry), want, "${entry:04x} ({what})");
        if let Some(real) = real_bios() {
            assert_eq!(
                cycles_of_call(&disk, &real, entry),
                want,
                "the real ${entry:04x} ({what}) should agree, or {want} is the wrong number"
            );
        }
    }
}
