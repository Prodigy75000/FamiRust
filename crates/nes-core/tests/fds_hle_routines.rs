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
fn both(prog: &[u8], frames: u64) -> Option<Outcome> {
    let disk = a_disk()?;
    let (ours, _) = run(&disk, &[], prog, frames);
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
    Some(ours)
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
    let Some(r) = both(&p.0, 60) else { return skipped() };
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
    let Some(r) = both(&p.0, 60) else { return skipped() };
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
