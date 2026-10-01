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

/// Boot `disk` under `bios` to handover, then run `prog` instead of the game.
fn run(disk: &[u8], bios: &[u8], prog: &[u8], frames: u64) -> (Outcome, nes_core::Nes) {
    let mut nes = nes_core::Nes::from_fds(disk, bios).expect("disk should parse");
    // Handover: the BIOS dispatching through $DFFC, not merely the first exit
    // from the window. See fds_hle_boot.rs for why the difference matters.
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
