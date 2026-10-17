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
    run_poked(disk, bios, prog, frames, &[])
}

/// [`run`], with `pokes` written straight into memory at handover, for a cell
/// a program cannot plant without moving its own addresses.
fn run_poked(disk: &[u8], bios: &[u8], prog: &[u8], frames: u64, pokes: &[(u16, u8)])
    -> (Outcome, nes_core::Nes)
{
    let mut nes = boot_to_handover(disk, bios);
    for &(a, v) in pokes {
        nes.dbg_poke(a, v);
    }

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

/// Like [`both`], but also hands back the bytes at `watch` from each machine
/// and requires those to agree too.
///
/// Registers alone are not enough for a routine whose product is in memory. A
/// generator could return the right `A`, `X`, `Y` and flags while shifting the
/// wrong bits into the caller's buffer.
fn both_watching(prog: &[u8], frames: u64, watch: &[u16]) -> Option<(Outcome, Vec<u8>)> {
    let disk = a_disk()?;
    let (ours, mut nes) = run(&disk, &[], prog, frames);
    let mine: Vec<u8> = watch.iter().map(|a| nes.peek(*a)).collect();
    if let Some(real) = real_bios() {
        let (theirs, mut their_nes) = run(&disk, &real, prog, frames);
        assert!(
            theirs.reached,
            "the real BIOS never finished the test program, so this is measuring \
             the harness rather than the routine"
        );
        assert_eq!(ours, theirs, "ours (left) disagrees with the real BIOS (right)");
        let hers: Vec<u8> = watch.iter().map(|a| their_nes.peek(*a)).collect();
        assert_eq!(
            mine, hers,
            "the registers agree but the memory at {watch:04x?} does not: ours \
             (left) against the real BIOS (right)"
        );
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some((ours, mine))
}

/// Cycles spent inside one call to `addr`, made from a planted caller.
///
/// Measured rather than asserted from the source, because a delay routine's
/// whole product is its duration and a comment claiming 131 is not evidence.
fn cycles_of_call(disk: &[u8], bios: &[u8], addr: u16) -> u64 {
    cycles_of_call_after(disk, bios, addr, &[])
}

/// Cycles spent inside one call to `addr`, with `setup` run first.
///
/// For a routine whose cost depends on its arguments there is no such thing as
/// "the" cycle count, so the setup is how a test picks which one it is asking
/// about.
fn cycles_of_call_after(disk: &[u8], bios: &[u8], addr: u16, setup: &[u8]) -> u64 {
    let mut p = Prog::default();
    p.raw(setup).jsr(addr).record_and_halt();
    cycles_of_call_in(disk, bios, addr, &p.0)
}

/// Cycles spent inside one call to `addr`, from a caller the test builds whole.
///
/// Needed by anything whose call site is not simply `jsr addr`: a routine that
/// reads inline arguments cannot be measured by a harness that appends its own
/// bytes after the `jsr`, because those bytes become the arguments.
fn cycles_of_call_in(disk: &[u8], bios: &[u8], addr: u16, prog: &[u8]) -> u64 {
    let p = Prog(prog.to_vec());
    let mut nes = boot_to_handover(disk, bios);
    for (i, b) in p.0.iter().enumerate() {
        nes.dbg_poke(PROG + i as u16, *b);
    }
    nes.dbg_set_pc(PROG);
    // Deadlined like the loop below: a program that never reaches the routine
    // should fail this test rather than hang the whole suite.
    let reach_by = nes.dbg_cycles() + 2 * 29_781;
    while nes.dbg_pc() != addr {
        nes.step();
        assert!(nes.dbg_cycles() < reach_by, "the caller never reached ${addr:04x}");
    }
    let sp_in = nes.cpu.sp;
    let start = nes.dbg_cycles();
    // A deadline, because a routine that never returns would otherwise hang
    // the whole suite rather than fail one test. Two frames is far more than
    // any routine measured here takes.
    // Generous, because one of these routines is a delay: $E153 at y=0 waits
    // 256 ms, which is 468,544 cycles or about sixteen frames. The deadline is
    // here to turn a hang into a failed test rather than to police duration,
    // so it only has to be larger than anything legitimate.
    let give_up = start + 64 * 29_781;
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

// --- $E9B1, the shift-register random number generator ----------------------

/// Where an `$E9B1` test puts its seed. Far enough from `$00-$0E` that the
/// routine's own scratch is not part of the register.
const SEED: u16 = 0x0040;

/// `ldx #seed_lo` then `ldy #len` then call, which is how every real caller
/// enters: by `jmp`, with the register address in `x` and its length in `y`.
fn shift_call(seed: u8, len: u8) -> Prog {
    let mut p = Prog::default();
    p.ldx(seed).ldy(len).jsr(0xe9b1);
    p
}

#[test]
fn random_shift_at_e9b1_shifts_the_whole_register_and_matches_the_real_bios() {
    // A four-byte seed, worked out by hand so the test means something with no
    // oracle present, and held against the oracle when there is one.
    //
    //   $A5 $5A $3C $C3, so bit1($A5)=0 and bit1($5A)=1 and the feedback is 1
    //   $A5 ror with carry in  -> $D2, carry out 1
    //   $5A ror with carry in  -> $AD, carry out 0
    //   $3C ror                -> $1E, carry out 0
    //   $C3 ror                -> $61, carry out 1
    if a_disk().is_none() {
        return skipped();
    }
    let mut p = Prog::default();
    p.bytes_at(SEED, &[0xa5, 0x5a, 0x3c, 0xc3]);
    // $0044 is one past a four-byte register and $0001 is read as the dummy
    // half of `lda $01,x` but never written, so both must come back as planted.
    //
    // Planting them rather than leaving them out of the watch list is the
    // point: an unplanted byte neither BIOS wrote holds each BIOS's own
    // leftovers, so comparing it compares their boot scratch and fails for a
    // reason that has nothing to do with the routine. That is how the first
    // version of this test failed.
    p.bytes_at(SEED + 4, &[SCRIBBLE]);
    p.bytes_at(0x0001, &[SCRIBBLE]);
    p.raw(&shift_call(SEED as u8, 4).0).record_and_halt();
    let watch = [SEED, SEED + 1, SEED + 2, SEED + 3, SEED + 4, 0x0000, 0x0001];
    let Some((r, mem)) = both_watching(&p.0, 10, &watch) else { return skipped() };
    assert!(r.reached, "$E9B1 never returned");
    assert_eq!(mem[4], SCRIBBLE, "a four-byte register must not touch the fifth byte");
    assert_eq!(mem[6], SCRIBBLE, "$0001 is read but never written");
    assert_eq!(
        &mem[..4],
        &[0xd2, 0xad, 0x1e, 0x61],
        "the register should come back shifted right with the feedback in bit 7"
    );
    assert_eq!(r.a, 0x02, "a should be the feedback bit as $02");
    assert_eq!(r.x, SEED as u8 + 4, "x should end past the register");
    assert_eq!(r.y, 0x00, "y should count down to zero");
    assert_eq!(r.p & 0x01, 0x01, "carry should be bit 0 of the last byte, which was set");
    assert_eq!(mem[5], 0x00, "$0000 holds bit1 of the FIRST byte, which is clear");
}

#[test]
fn the_feedback_bit_is_bit_one_of_the_first_two_bytes_xored() {
    // Four combinations at the shortest useful length, so the only thing that
    // can move bit 7 of the result is the feedback. A routine that took the
    // feedback from carry, or from the wrong bit, or from the wrong byte,
    // fails at least one row.
    if a_disk().is_none() {
        return skipped();
    }
    for (first, tap, want, feedback) in [
        (0x00u8, 0x00u8, 0x00u8, 0x00u8),
        (0x00, 0x02, 0x80, 0x02),
        (0x02, 0x00, 0x81, 0x02),
        (0x02, 0x02, 0x01, 0x00),
    ] {
        let mut p = Prog::default();
        p.bytes_at(SEED, &[first, tap]);
        // `sec` first, so "the feedback is computed" is distinguishable from
        // "the feedback is whatever carry came in as".
        p.raw(&[0x38]);
        p.raw(&shift_call(SEED as u8, 1).0).record_and_halt();
        let Some((r, mem)) = both_watching(&p.0, 10, &[SEED]) else { return skipped() };
        assert!(r.reached);
        assert_eq!(
            mem[0], want,
            "first=${first:02x} tap=${tap:02x} should shift to ${want:02x}"
        );
        assert_eq!(r.a, feedback, "a should be the feedback bit for ${first:02x}/${tap:02x}");
    }
}

#[test]
fn the_tap_is_read_from_outside_the_register_when_the_length_is_one() {
    // At y=1 the register is one byte, and the tap at x+1 is past the end of
    // it. The real routine reads it anyway and its bit 1 changes the answer,
    // measured on two titles.
    //
    // This is the test that stops someone tidying the out-of-bounds read away.
    // It looks like a bug and it is the published behaviour, so a game that
    // seeded one byte and left junk next to it gets the sequence that junk
    // produces, and we have to produce the same one.
    if a_disk().is_none() {
        return skipped();
    }
    let mut out = Vec::new();
    for tap in [0x00u8, 0x02] {
        let mut p = Prog::default();
        p.bytes_at(SEED, &[0x00, tap]);
        p.raw(&shift_call(SEED as u8, 1).0).record_and_halt();
        let Some((r, mem)) = both_watching(&p.0, 10, &[SEED]) else { return skipped() };
        assert!(r.reached);
        out.push(mem[0]);
    }
    assert_eq!(
        out,
        vec![0x00, 0x80],
        "the byte past a one-byte register must still feed the shift"
    );
}

#[test]
fn the_tap_wraps_inside_zero_page_and_collides_with_the_scratch_at_x_is_ff() {
    // x=$FF puts the tap at ($FF + 1) & $FF = $0000, which the routine filled
    // with bit1 of the first byte two instructions earlier. So the feedback is
    // that bit xored with itself and is ALWAYS zero, whatever the seed.
    //
    // Two things fail this. An implementation using absolute indexing would
    // read $0100, the bottom of the stack, instead of wrapping. One that kept
    // its intermediate somewhere other than $0000 would not collide at all and
    // would produce a feedback of 1 for an odd-bit-1 seed.
    if a_disk().is_none() {
        return skipped();
    }
    for seed in [0x02u8, 0xff, 0x06] {
        let mut p = Prog::default();
        p.bytes_at(0x00ff, &[seed]);
        // $0100 gets the opposite bit 1, so reading it instead of wrapping
        // would give a different feedback and a different result.
        p.bytes_at(0x0100, &[!seed]);
        p.raw(&shift_call(0xff, 1).0).record_and_halt();
        let Some((r, mem)) = both_watching(&p.0, 10, &[0x00ff]) else { return skipped() };
        assert!(r.reached);
        assert_eq!(
            r.a, 0x00,
            "at x=$FF the tap collides with the scratch, so the feedback is always 0"
        );
        assert_eq!(
            mem[0],
            seed >> 1,
            "and the byte is a plain shift right with a zero coming in"
        );
    }
}

#[test]
fn a_length_of_zero_means_two_hundred_and_fifty_six_bytes() {
    // The loop is a do-while, so y=0 walks the whole of zero page and comes
    // back with x where it started. The real routine does exactly that and
    // returns normally rather than hanging or stopping early.
    //
    // Three planted bytes make the middle of the walk checkable. $007F fixes
    // the carry going into $0080, so $0080 and $0081 are then determined:
    //   $7F=$01, so carry into $80 is 1
    //   $80=$AA ror with carry in -> $D5, carry out 0
    //   $81=$55 ror              -> $2A
    if a_disk().is_none() {
        return skipped();
    }
    //
    // The feedback and the carry out have to be planted too, or they come from
    // whatever each BIOS happened to leave in zero page and the two runs
    // disagree for a reason that is not about the routine. The feedback reads
    // $0040 and $0041; the carry out is bit 0 of the LAST byte the walk
    // reaches, which starting from $0040 is $003F.
    let mut p = Prog::default();
    p.bytes_at(0x003f, &[0x01]);
    p.bytes_at(SEED, &[0x00, 0x00]);
    p.bytes_at(0x007f, &[0x01, 0xaa, 0x55]);
    p.raw(&shift_call(SEED as u8, 0).0).record_and_halt();
    let Some((r, mem)) = both_watching(&p.0, 20, &[0x0080, 0x0081]) else {
        return skipped();
    };
    assert!(r.reached, "y=0 never came back, so it is not a counted 256");
    assert_eq!(
        mem,
        vec![0xd5, 0x2a],
        "the walk should reach $0080 and $0081, a long way past the four bytes \
         a caller would normally ask for"
    );
    assert_eq!(r.x, SEED as u8, "x should wrap all the way round to where it began");
    assert_eq!(r.y, 0x00);
    assert_eq!(r.a, 0x00, "the feedback from the two planted zero bytes");
    assert_eq!(r.p & 0x01, 0x01, "carry is bit 0 of $003F, the last byte of the walk");
}

#[test]
fn the_generator_produces_the_same_sequence_as_the_real_bios() {
    // The product of a generator is its SEQUENCE, so one call proving correct
    // proves less than it looks. Eight in a row from a fixed seed is the thing
    // a game actually depends on, and the only way an off-by-one in the
    // feedback survives the earlier tests is by being right for one step.
    if a_disk().is_none() {
        return skipped();
    }
    //
    // The seed is picked so the feedback bit comes out 0, 0, 1, 0, 1, 1, 1, 1
    // over the eight steps. A seed that fed back the same bit every time would
    // pass against an implementation that ignored one of the two taps.
    let mut p = Prog::default();
    p.bytes_at(SEED, &[0xa5, 0x4d, 0xca, 0x18]);
    for _ in 0..8 {
        p.raw(&shift_call(SEED as u8, 4).0);
    }
    p.record_and_halt();
    let watch = [SEED, SEED + 1, SEED + 2, SEED + 3];
    let Some((r, mem)) = both_watching(&p.0, 20, &watch) else { return skipped() };
    assert!(r.reached, "eight calls did not finish");
    assert_eq!(
        mem,
        vec![0xf4, 0xa5, 0x4d, 0xca],
        "eight steps from $A5 $4D $CA $18"
    );
}

#[test]
fn the_generator_is_not_slower_than_the_real_bios() {
    // Ours is 25 + 13*y against the original's 28 + 13*y + feedback, so three
    // or four cycles quick. Safe here: the product is the bit sequence and not
    // the duration, unlike $E149 where the duration is the whole point.
    let Some(disk) = a_disk() else { return skipped() };
    for len in [1u8, 2, 4] {
        let setup = shift_call(SEED as u8, len);
        // Drop the `jsr`, which cycles_of_call_after adds itself.
        let setup = &setup.0[..setup.0.len() - 3];
        let ours = cycles_of_call_after(&disk, &[], 0xe9b1, setup);
        assert_eq!(
            ours,
            25 + 13 * u64::from(len),
            "our own cost model at y={len}"
        );
        if let Some(real) = real_bios() {
            let theirs = cycles_of_call_after(&disk, &real, 0xe9b1, setup);
            assert!(
                ours <= theirs,
                "at y={len} ours took {ours} against the real {theirs}, and slower is not safe"
            );
        }
    }
}

#[test]
fn the_generator_preserves_v_d_and_i_and_ignores_carry_and_a_coming_in() {
    // Measured on the real routine: N is always 0 and Z always 1, both from
    // the loop counter reaching zero rather than from anything about the
    // result, so Z=1 even when a comes back $02. V, D and I pass through.
    //
    // Carry IN is ignored, which is the one that needs forcing to mean
    // anything: the feedback is computed from the two taps, so an
    // implementation that seeded the rotate from the caller's carry instead
    // would agree with this test on every call that happened to arrive with
    // carry already matching. Entering with carry set and a zero feedback is
    // what separates them.
    if a_disk().is_none() {
        return skipped();
    }
    for (c_in, a_in) in [(false, 0x00u8), (true, 0x00), (true, 0xff), (false, 0x5a)] {
        let mut p = Prog::default();
        // Both taps clear, so the feedback is 0 and a carry that leaked in
        // from the caller would show up as bit 7 of the first byte.
        p.bytes_at(SEED, &[0x00, 0x00]);
        p.ldx(SEED as u8).ldy(0x02);
        p.lda(a_in);
        p.raw(&[0xf8]); // sed, so D=1 on the way in
        p.raw(&[0x24, 0xfd]); // bit $fd: bit 6 of a planted $40 sets V
        p.raw(&[if c_in { 0x38 } else { 0x18 }]); // sec or clc
        p.raw(&[0x78]); // sei
        p.jsr(0xe9b1).record_and_halt();
        let mut full = Prog::default();
        full.bytes_at(0x00fd, &[0x40]);
        let mut bytes = full.0;
        bytes.extend_from_slice(&p.0);
        let Some((r, mem)) = both_watching(&bytes, 10, &[SEED, SEED + 1]) else {
            return skipped();
        };
        assert!(r.reached, "never returned with c_in={c_in} a_in=${a_in:02x}");
        assert_eq!(
            mem,
            vec![0x00, 0x00],
            "carry in (={c_in}) must not become the feedback bit"
        );
        assert_eq!(r.a, 0x00, "a out is the feedback, not a in (${a_in:02x})");
        assert_eq!(r.p & 0x80, 0x00, "N should be clear from the loop counter");
        assert_eq!(r.p & 0x02, 0x02, "Z should be set from the loop counter");
        assert_eq!(r.p & 0x40, 0x40, "V should be preserved");
        assert_eq!(r.p & 0x08, 0x08, "D should be preserved");
        assert_eq!(r.p & 0x04, 0x04, "I should be preserved");
        assert_eq!(r.p & 0x01, 0x00, "carry out is bit 0 of the last byte, which was clear");
    }
}

#[test]
fn the_generator_handles_the_longest_register_a_real_caller_asks_for() {
    // Zelda asks for thirteen bytes and Nazo no Murasame-jou for eight, so the
    // short registers the other tests use are not the whole range. Thirteen
    // also crosses the point where a byte-count bug would show up as a short
    // or long walk rather than as wrong arithmetic.
    if a_disk().is_none() {
        return skipped();
    }
    let seed: [u8; 13] = [
        0xa5, 0x4d, 0xca, 0x18, 0x01, 0x80, 0xff, 0x00, 0x7e, 0x33, 0x99, 0x42, 0x0f,
    ];
    let mut p = Prog::default();
    p.bytes_at(SEED, &seed);
    // The fourteenth byte must come back untouched.
    p.bytes_at(SEED + 13, &[SCRIBBLE]);
    p.raw(&shift_call(SEED as u8, 13).0).record_and_halt();
    let watch: Vec<u16> = (0..14).map(|i| SEED + i).collect();
    let Some((r, mem)) = both_watching(&p.0, 10, &watch) else { return skipped() };
    assert!(r.reached);
    // feedback = bit1($A5) xor bit1($4D) = 0 xor 0 = 0, then a plain ripple.
    let mut want = Vec::new();
    let mut carry = 0u8;
    for v in seed {
        want.push((v >> 1) | (carry << 7));
        carry = v & 1;
    }
    want.push(SCRIBBLE);
    assert_eq!(mem, want, "thirteen bytes should ripple right once");
    assert_eq!(r.a, 0x00, "the feedback for this seed is 0");
    assert_eq!(r.x, SEED as u8 + 13);
    assert_eq!(r.y, 0x00);
}

// --- $EAFD, the jump-table dispatcher ---------------------------------------

/// A caller for `$EAFD`: `lda #index`, `jsr $EAFD`, then a table of `entries`
/// addresses, then one landing byte per entry.
///
/// Entry `i` points at its own `nop`, and the pad runs straight into a
/// `record_and_halt`, so whichever entry was taken falls through to the same
/// recorder. That matters because the routine leaves the target address in
/// `a` and `x`: the recorded registers name the entry that was dispatched,
/// which is the thing under test, without the targets needing to differ.
///
/// Returns the program and the address of the first landing byte.
fn dispatch_prog(index: u8, entries: usize) -> (Vec<u8>, u16) {
    dispatch_prog_at(index, entries, PROG)
}

/// As [`dispatch_prog`], but for a caller that will sit at `base` rather than
/// at [`PROG`]. The table holds absolute addresses, so a program with anything
/// in front of it has to be built knowing where it will land.
fn dispatch_prog_at(index: u8, entries: usize, base: u16) -> (Vec<u8>, u16) {
    let mut p = Prog::default();
    p.lda(index);
    p.jsr(0xeafd);
    let table_at = base + p.0.len() as u16;
    let pad_at = table_at + 2 * entries as u16;
    for i in 0..entries {
        p.0.extend_from_slice(&(pad_at + i as u16).to_le_bytes());
    }
    p.0.extend(std::iter::repeat(0xea).take(entries)); // nop per entry
    p.record_and_halt();
    (p.0, pad_at)
}

#[test]
fn the_dispatcher_at_eafd_jumps_to_the_entry_the_accumulator_picks() {
    // `a` is the index into a table of 16-bit addresses written inline after
    // the `jsr`. The routine doubles it, so entry `i` is at offset 2i+1 past
    // the return address.
    //
    // Each entry lands somewhere one byte apart, so the target address the
    // routine reports back in `a` and `x` says exactly which one it took. An
    // off-by-one in the doubling, or reading the table from the wrong base,
    // moves that address and fails here.
    if a_disk().is_none() {
        return skipped();
    }
    for i in 0..4u8 {
        let (prog, pad_at) = dispatch_prog(i, 4);
        let want = pad_at + u16::from(i);
        let Some((r, _)) = both(&prog, 10) else { return skipped() };
        assert!(r.reached, "entry {i} never arrived anywhere that records");
        assert_eq!(r.x, want as u8, "entry {i}: low byte of the target");
        assert_eq!(r.a, (want >> 8) as u8, "entry {i}: high byte of the target");
        assert_eq!(r.y, 2 * i + 2, "y should come out as 2a+2");
    }
}

#[test]
fn the_dispatcher_consumes_the_call_frame_so_the_target_tail_returns() {
    // Two `pla`s and no push: the stack comes back two bytes shallower and the
    // target runs as though it had been called by whoever called the caller.
    //
    // So a target that executes `rts` returns past the `jsr $EAFD` entirely,
    // to the instruction after the OUTER call. This is the test that says so:
    // an implementation that left the frame alone would send that `rts` into
    // the address table and off into the weeds.
    if a_disk().is_none() {
        return skipped();
    }
    let mut p = Prog::default();
    // The outer call, with its target filled in afterwards: the inner routine
    // sits past the recorder, and the recorder's length is the builder's
    // business rather than a number to guess at here.
    p.jsr(0x0000);
    let operand = p.0.len() - 2;
    p.record_and_halt(); // where a working tail call lands
    let inner_at = PROG + p.0.len() as u16;
    p.0[operand] = inner_at as u8;
    p.0[operand + 1] = (inner_at >> 8) as u8;
    // inner: lda #0 : jsr $EAFD : .word target ... target: rts
    p.lda(0x00);
    p.jsr(0xeafd);
    let target = PROG + p.0.len() as u16 + 2;
    p.0.extend_from_slice(&target.to_le_bytes());
    p.0.push(0x60); // rts

    let Some((r, _)) = both(&p.0, 10) else { return skipped() };
    assert!(
        r.reached,
        "the target's rts did not come back to the outer caller, so the call \
         frame was not consumed"
    );
    assert_eq!(r.a, (target >> 8) as u8, "a still names the dispatched target");
    assert_eq!(r.x, target as u8);
}

#[test]
fn the_dispatcher_leaves_the_target_address_in_zero_page() {
    // $0000 and $0001 are the vector it jumps through, and it does not tidy
    // them up afterwards. A game is entitled to find the target address there.
    if a_disk().is_none() {
        return skipped();
    }
    let (prog, pad_at) = dispatch_prog(2, 4);
    let want = pad_at + 2;
    let Some((r, mem)) = both_watching(&prog, 10, &[0x0000, 0x0001]) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(
        mem,
        vec![want as u8, (want >> 8) as u8],
        "$0000/$0001 should hold the dispatched address, little-endian"
    );
}

#[test]
fn the_dispatcher_carries_bit_seven_of_the_index_and_preserves_the_rest() {
    // Carry out is bit 7 of the index, which is the doubling leaking into the
    // flags rather than a status of any kind. It is reproduced because it is
    // what the original leaves, and a caller that happened to branch on carry
    // afterwards would see the original's answer.
    //
    // N and Z come from the target's high byte. V, D and I pass through.
    if a_disk().is_none() {
        return skipped();
    }
    for (index, want_carry) in [(0x00u8, 0u8), (0x01, 0), (0x02, 0)] {
        // Force the flags in, so "preserved" means something. These run before
        // the dispatcher's own `lda #index`, and the table holds absolute
        // addresses, so the dispatch half is built knowing it sits after them.
        let mut p = Prog::default();
        p.poke(0x00fd, 0x40);
        p.0.extend_from_slice(&[0x24, 0xfd]); // bit $fd, bit 6 sets V
        p.0.push(0xf8); // sed
        p.0.push(0x78); // sei
        p.0.push(0x38); // sec, so a cleared carry is visible
        let (prog, _) = dispatch_prog_at(index, 4, PROG + p.0.len() as u16);
        p.0.extend_from_slice(&prog);
        let Some((r, _)) = both(&p.0, 10) else { return skipped() };
        assert!(r.reached, "index {index} never arrived");
        assert_eq!(r.p & 0x01, want_carry, "carry for index ${index:02x}");
        assert_eq!(r.p & 0x40, 0x40, "V should be preserved");
        assert_eq!(r.p & 0x08, 0x08, "D should be preserved");
        assert_eq!(r.p & 0x04, 0x04, "I should be preserved");
        assert_eq!(
            r.p & 0x80 != 0,
            r.a & 0x80 != 0,
            "N should come from the target's high byte"
        );
    }
}

#[test]
fn an_index_with_bit_seven_set_wraps_and_is_not_range_checked() {
    // Measured on the original: forced $80 doubles to $00 and quietly
    // dispatches entry 0 with carry set. There is no bounds check anywhere,
    // and a reimplementation that added one would be kinder and wrong.
    if a_disk().is_none() {
        return skipped();
    }
    let (prog, pad_at) = dispatch_prog(0x80, 4);
    let Some((r, _)) = both(&prog, 10) else { return skipped() };
    assert!(r.reached, "$80 should dispatch entry 0 rather than fail");
    assert_eq!(r.x, pad_at as u8, "it should land on entry 0");
    assert_eq!(r.y, 0x02, "y = 2a+2 with the doubling wrapped");
    assert_eq!(r.p & 0x01, 0x01, "and carry carries bit 7 out of the doubling");
}

#[test]
fn the_dispatcher_takes_the_same_forty_five_cycles_as_the_real_bios() {
    // 45, min=max across every caller and every forced index. Ours is the same
    // 45, which was not aimed at: the count fell out of the instruction
    // sequence the access trace forced. Matching exactly is evidence the
    // sequence is right rather than merely equivalent.
    let Some(disk) = a_disk() else { return skipped() };
    for i in [0u8, 3] {
        let (prog, _) = dispatch_prog(i, 4);
        assert_eq!(
            cycles_of_call_in(&disk, &[], 0xeafd, &prog),
            45,
            "our own cost at index {i}"
        );
        if let Some(real) = real_bios() {
            assert_eq!(
                cycles_of_call_in(&disk, &real, 0xeafd, &prog),
                45,
                "the real one should agree at index {i}"
            );
        }
    }
}

// --- $E86A, the queued VRAM flush -------------------------------------------

/// The queue $E86A empties, and the game's write index just below it.
const QUEUE: u16 = 0x0302;
const QUEUE_INDEX: u16 = 0x0301;

/// Emit `ldx #len : lda #val : sta addr-1,x : dex : bne`, filling `len` bytes
/// from `addr` without costing five program bytes per byte planted.
///
/// `bytes_at` is fine for a header but not for a body: a 69-byte entry would
/// be 345 bytes of `lda`/`sta` pairs, and the program starts at $0600 with the
/// results area at $0700.
fn fill_run(p: &mut Prog, addr: u16, len: u8, val: u8) {
    let base = addr - 1;
    p.ldx(len);
    p.lda(val);
    p.0.extend_from_slice(&[0x9d, base as u8, (base >> 8) as u8]); // sta base,x
    p.0.extend_from_slice(&[0xca, 0xd0, 0xfa]); // dex : bne
}

/// A caller for `$E86A`: a known PPUCTRL shadow, a cleared nametable, the
/// queue planted, and a sentinel in the write index.
///
/// Bit 7 of the shadow is deliberately CLEAR. Bit 7 of PPUCTRL is the NMI
/// enable and this routine writes the shadow straight at $2000, so a shadow
/// with it set hands the next NMI to the game's handler and the test program
/// is never seen again. Bit 2 is set instead, because bit 2 is the one the
/// routine masks off.
fn flush_prog(queue: &[u8]) -> Prog {
    let mut p = Prog::default();
    p.poke(0x00ff, 0x34);
    p.clear_vram();
    p.bytes_at(QUEUE, queue);
    p.poke(QUEUE_INDEX, SCRIBBLE);
    p
}

#[test]
fn vram_flush_at_e86a_walks_the_queue_and_matches_the_real_bios() {
    // Two entries into the nametable, then the terminator. The queue is the
    // whole interface: nothing is passed in a register.
    if a_disk().is_none() {
        return skipped();
    }
    let mut p = flush_prog(&[
        0x20, 0x00, 0x02, 0xaa, 0xbb, // $2000: two bytes
        0x20, 0x10, 0x03, 0x11, 0x22, 0x33, // $2010: three bytes
        0xff, // end
    ]);
    p.jsr(0xe86a).record_and_halt();
    let Some((r, mem)) = both_watching(&p.0, 30, &[QUEUE_INDEX, QUEUE, 0x00ff]) else {
        return skipped();
    };
    assert!(r.reached, "$E86A never returned");
    assert_eq!(mem[0], 0x00, "the game's write index should be reset");
    assert_eq!(mem[1], 0xff, "the terminator it found is echoed back");
    assert_eq!(mem[2], 0x30, "the shadow keeps the masked value");
    assert_eq!(r.a, 0x00, "a is the $00 it stored into the write index");
    assert_eq!(r.x, 0x00, "x ends as the exhausted byte counter");
    assert_eq!(r.y, 11, "y ends at the terminator's offset from $0302");

    let Some((_, nes)) = both(&p.0, 30) else { return skipped() };
    let mut nes = nes;
    let nt = nes.dbg_ciram();
    assert_eq!(&nt[0..2], &[0xaa, 0xbb], "the first entry's bytes");
    assert_eq!(&nt[0x10..0x13], &[0x11, 0x22, 0x33], "the second entry's bytes");
    assert_eq!(nt[2], 0x00, "and nothing between them");
}

#[test]
fn the_flush_count_is_the_whole_byte_and_not_six_bits_like_e7bb() {
    // This is the one that matters most. $E7BB walks the same SHAPE of data
    // with a different length byte: there the low six bits are the count, bit
    // 6 means repeat-one-byte and bit 7 picks the VRAM increment. Here the
    // count is all eight bits.
    //
    // $45 is the discriminator. Read as $E7BB would, it is a repeat of five.
    // Read as $E86A does, it is a straight run of 69. Implementing this
    // routine by calling the other would quietly corrupt every entry of 64
    // bytes or more, and would pass any test that only used short entries.
    if a_disk().is_none() {
        return skipped();
    }
    let mut p = flush_prog(&[0x20, 0x00, 0x45]);
    fill_run(&mut p, QUEUE + 3, 69, 0x5a);
    p.bytes_at(QUEUE + 3 + 69, &[0xff]);
    p.jsr(0xe86a).record_and_halt();
    let Some((r, _)) = both(&p.0, 60) else { return skipped() };
    assert!(r.reached);
    assert_eq!(
        r.y,
        3 + 69,
        "y should end past 69 data bytes, not past 5 or 1"
    );
    let Some((_, mut nes)) = both(&p.0, 60) else { return skipped() };
    let nt = nes.dbg_ciram();
    assert!(nt[..69].iter().all(|&b| b == 0x5a), "69 bytes should land");
    assert_eq!(nt[69], 0x00, "and the seventieth should not");
}

#[test]
fn a_flush_count_of_zero_means_two_hundred_and_fifty_six() {
    // Not 64, which is what a zero count means in $E7BB, and not nothing. It
    // falls out of an eight-bit countdown rather than being a special case,
    // and it is how the original behaves.
    if a_disk().is_none() {
        return skipped();
    }
    // The index is eight bits against a sixteen-bit base, so 256 data bytes
    // consume offsets 3 through 258, which wraps: the last three come from
    // $0302-$0304, the entry's own header, and the next header is then read
    // back at offset 3. So the terminator for a 256-byte entry has to live
    // INSIDE the data, at $0305, and there is nowhere else it could go. A
    // buffer whose terminator would sit at offset $100 or beyond hangs the
    // real BIOS, which is why the obvious layout cannot be the test.
    let mut p = flush_prog(&[0x20, 0x00, 0x00]);
    fill_run(&mut p, QUEUE + 3, 251, 0x77); // $0305 through $03FF
    p.bytes_at(0x0400, &[0x77, 0x77]); // the two that carry into page 4
    p.bytes_at(QUEUE + 3, &[0xff]); // $0305: data byte 0, and the terminator
    p.jsr(0xe86a).record_and_halt();
    let Some((r, _)) = both(&p.0, 60) else { return skipped() };
    assert!(r.reached, "a zero count should terminate, not hang");
    assert_eq!(r.y, 3, "the index wraps back to the offset it started the data at");
    let Some((_, mut nes)) = both(&p.0, 60) else { return skipped() };
    let nt = nes.dbg_ciram().to_vec();
    assert_eq!(nt[0], 0xff, "the first data byte is the byte at $0305");
    assert_eq!(
        nt[100], 0x77,
        "byte 100 should have been written, so the count is 256 and not 64"
    );
    assert_eq!(
        &nt[253..256],
        &[0x20, 0x00, 0x00],
        "the last three bytes wrap round to the entry's own header"
    );
}

#[test]
fn the_flush_resets_the_address_for_a_palette_entry_and_only_for_exactly_3f() {
    // After a palette entry the routine points $2006 at $3F00 and then at
    // $0000, so the PPU is not left showing a palette entry instead of the
    // backdrop. The trigger is the address high byte being EXACTLY $3F: $3E
    // and $7F were both measured not to fire it, which is what makes it a
    // full-byte compare rather than a range or a mask.
    let Some(disk) = a_disk() else { return skipped() };
    for (hi, extra) in [(0x3fu8, 15u64), (0x3e, 0), (0x7f, 0), (0x20, 0)] {
        let mut p = flush_prog(&[hi, 0x00, 0x01, 0xaa, 0xff]);
        p.jsr(0xe86a).record_and_halt();
        let ours = cycles_of_call_in(&disk, &[], 0xe86a, &p.0);
        assert_eq!(
            ours,
            98 + extra,
            "high byte ${hi:02x} should cost the palette reset only when it is $3F"
        );
        if let Some(real) = real_bios() {
            assert_eq!(
                cycles_of_call_in(&disk, &real, 0xe86a, &p.0),
                ours,
                "and the real BIOS should agree for ${hi:02x}"
            );
        }
    }
}

#[test]
fn a_flush_entry_beginning_4c_is_an_address_and_not_a_call() {
    // $E7BB's little language reads $4C as "call" and $60 as "return". $E86A
    // has neither: it walks a fixed base with an eight-bit index and has no
    // pointer for a call to redirect, so $4C and $60 are ordinary VRAM
    // address high bytes.
    //
    // Measured, not inferred from silence: an entry starting $4C writes $4C to
    // $2006 and streams its data, with no stack traffic beyond the one push
    // every entry makes.
    let Some(disk) = a_disk() else { return skipped() };
    for hi in [0x4cu8, 0x60] {
        let mut p = flush_prog(&[hi, 0x40, 0x01, 0xaa, 0xff]);
        p.jsr(0xe86a).record_and_halt();
        let ours = cycles_of_call_in(&disk, &[], 0xe86a, &p.0);
        assert_eq!(ours, 98, "${hi:02x} should cost one ordinary one-byte entry");
        let writes = writes_inside_call(&disk, &[], &p.0, 0xe86a);
        let addr: Vec<u8> = writes
            .iter()
            .filter(|(a, _)| *a == 0x2006)
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(addr, vec![hi, 0x40], "$2006 should take the address as written");
        let pushes = writes
            .iter()
            .filter(|(a, _)| (0x0100..0x0200).contains(a))
            .count();
        assert_eq!(
            pushes, 1,
            "exactly the one push every entry makes; a call would push a position too"
        );
    }
}

#[test]
fn the_flush_preamble_masks_bit_two_of_the_ppuctrl_shadow() {
    // It forces the VRAM increment to +1 and keeps the masked value where the
    // game can see it, leaving every other bit alone. Nearly missed: with the
    // shadow at $30 the mask changes nothing, and $30 is what the first title
    // profiled happened to hold.
    if a_disk().is_none() {
        return skipped();
    }
    for (shadow, want) in [(0x34u8, 0x30u8), (0x04, 0x00), (0x7b, 0x7b), (0x7f, 0x7b)] {
        let mut p = Prog::default();
        p.poke(0x00ff, shadow);
        p.bytes_at(QUEUE, &[0xff]);
        p.jsr(0xe86a).record_and_halt();
        let Some((r, mem)) = both_watching(&p.0, 20, &[0x00ff]) else {
            return skipped();
        };
        assert!(r.reached);
        assert_eq!(
            mem[0], want,
            "a shadow of ${shadow:02x} should come back ${want:02x}"
        );
    }
}

#[test]
fn the_flush_echoes_back_whichever_terminator_it_found() {
    // Not a constant $FF: a queue ended with $80 comes back with $80 at $0302.
    // Any byte with bit 7 set ends the walk.
    if a_disk().is_none() {
        return skipped();
    }
    for end in [0xffu8, 0x80, 0xa0] {
        let mut p = flush_prog(&[end]);
        p.jsr(0xe86a).record_and_halt();
        let Some((r, mem)) = both_watching(&p.0, 20, &[QUEUE, QUEUE_INDEX]) else {
            return skipped();
        };
        assert!(r.reached, "${end:02x} should end the walk");
        assert_eq!(mem[0], end, "the terminator it read is what it writes back");
        assert_eq!(mem[1], 0x00, "and the write index is always cleared");
        assert_eq!(r.y, 0, "nothing was walked");
    }
}

#[test]
fn the_flush_preserves_v_d_and_i_and_takes_carry_from_the_last_entry() {
    // N and Z come from the $00 it stores into the write index, so they are
    // always 0 and 1. Carry is left over from the `cmp #$3F` that decides the
    // palette reset, which makes it (last entry's high byte >= $3F): not a
    // status, but it is what the original leaves. V, D and I pass through.
    if a_disk().is_none() {
        return skipped();
    }
    for (hi, want_carry) in [(0x20u8, 0x00u8), (0x3f, 0x01), (0x40, 0x01), (0x3e, 0x00)] {
        let mut p = flush_prog(&[hi, 0x00, 0x01, 0xaa, 0xff]);
        p.0.extend_from_slice(&[0x24, 0xfd]); // bit $fd, whose bit 6 sets V
        p.0.push(0xf8); // sed
        p.0.push(0x78); // sei
        p.jsr(0xe86a).record_and_halt();
        let mut full = Prog::default();
        full.bytes_at(0x00fd, &[0x40]);
        let mut bytes = full.0;
        bytes.extend_from_slice(&p.0);
        let Some((r, _)) = both(&bytes, 30) else { return skipped() };
        assert!(r.reached);
        assert_eq!(r.p & 0x01, want_carry, "carry after a ${hi:02x} entry");
        assert_eq!(r.p & 0x80, 0x00, "N comes from the final $00");
        assert_eq!(r.p & 0x02, 0x02, "Z comes from the final $00");
        assert_eq!(r.p & 0x40, 0x40, "V should be preserved");
        assert_eq!(r.p & 0x08, 0x08, "D should be preserved");
        assert_eq!(r.p & 0x04, 0x04, "I should be preserved");
    }
}

#[test]
fn the_flush_costs_exactly_what_the_real_bios_costs() {
    // 43 + 40 per entry + 15 per data byte + 15 per palette entry, and ours is
    // the same. Matching matters here rather than merely being quick: this
    // runs inside vblank, where a routine that takes LONGER than the original
    // overruns the window a game budgeted for and corrupts the picture.
    //
    // Getting there needed the loop's test at the bottom so the palette block
    // falls through into it. Branching back instead costs three cycles more on
    // every palette entry, which is the wrong side of the line.
    let Some(disk) = a_disk() else { return skipped() };
    let cases: [(&[u8], u64); 5] = [
        (&[0xff], 43),
        (&[0x20, 0x00, 0x01, 0xaa, 0xff], 98),
        (&[0x20, 0x00, 0x03, 0xaa, 0xbb, 0xcc, 0xff], 128),
        (&[0x3f, 0x00, 0x01, 0xaa, 0xff], 113),
        (&[0x20, 0x00, 0x01, 0xaa, 0x24, 0x40, 0x01, 0xbb, 0xff], 153),
    ];
    for (queue, want) in cases {
        let mut p = flush_prog(queue);
        p.jsr(0xe86a).record_and_halt();
        assert_eq!(
            cycles_of_call_in(&disk, &[], 0xe86a, &p.0),
            want,
            "our own cost for {queue:02x?}"
        );
        if let Some(real) = real_bios() {
            assert_eq!(
                cycles_of_call_in(&disk, &real, 0xe86a, &p.0),
                want,
                "the real BIOS should agree for {queue:02x?}"
            );
        }
    }
}

// --- $E153, the millisecond delay -------------------------------------------

/// `ldy #n`, the only argument `$E153` takes.
fn delay_setup(ms: u8) -> Vec<u8> {
    vec![0xa0, ms]
}

#[test]
fn the_delay_at_e153_waits_five_plus_1790_per_millisecond() {
    // 1790 cycles is 1.0001 ms on NTSC, which is what makes "milliseconds" the
    // right word rather than a coincidence.
    //
    // This is the second routine where our usual rule inverts. Everywhere else
    // being faster than the original is safe; here, as at $E149, the duration
    // IS the product and the count has to be exact in both directions. So this
    // asserts equality, not an inequality.
    let Some(disk) = a_disk() else { return skipped() };
    for ms in [1u8, 2, 5, 10] {
        let want = 5 + 1790 * u64::from(ms);
        assert_eq!(
            cycles_of_call_after(&disk, &[], 0xe153, &delay_setup(ms)),
            want,
            "y={ms} should take 5 + 1790*{ms}"
        );
        if let Some(real) = real_bios() {
            assert_eq!(
                cycles_of_call_after(&disk, &real, 0xe153, &delay_setup(ms)),
                want,
                "and the real BIOS should agree at y={ms}"
            );
        }
    }
}

#[test]
fn a_delay_of_zero_milliseconds_waits_two_hundred_and_fifty_six() {
    // The loop is a do-while on y, so zero is the long way round rather than
    // no wait at all: 458,245 cycles, about sixteen frames. A game asking for
    // "none" and getting a quarter of a second is the original's behaviour and
    // has to be ours.
    let Some(disk) = a_disk() else { return skipped() };
    let want = 5 + 1790 * 256;
    assert_eq!(
        cycles_of_call_after(&disk, &[], 0xe153, &delay_setup(0)),
        want,
        "y=0 should mean 256 milliseconds"
    );
    if let Some(real) = real_bios() {
        assert_eq!(
            cycles_of_call_after(&disk, &real, 0xe153, &delay_setup(0)),
            want
        );
    }
}

#[test]
fn the_delay_preserves_a_and_leaves_carry_as_a_against_zero_page_0() {
    // `a` survives, `x` and `y` both come out zero, N and Z follow the final
    // `dey`, and V, D and I pass through.
    //
    // Carry is the awkward one and took four goes. It is not cleared, not
    // set, and not preserved: it is the carry of `cmp $00`, a against the
    // byte at $0000, which is what the original's "dummy" reads of $0000 are.
    // It read as `a >= $20` for three rounds only because the real BIOS leaves
    // $20 at $00 at handover and no test planted it, so ours, leaving the same
    // by accident of its boot, agreed. Found by Super Lode Runner, which calls
    // it with a=$10 over a smaller $00. $00 is planted here for that reason.
    //
    // NMI is off for the duration and I is set. A 2 ms wait is 3585 cycles and
    // an interrupt can land inside it; when one does, the game's own handler
    // alters the P it pushed and the carry this test is about comes back
    // whatever that handler left. Two earlier readings of this routine's carry
    // were wrong for exactly that reason.
    if a_disk().is_none() {
        return skipped();
    }
    for (a_in, zp0, want_carry) in [
        (0x10u8, 0x00u8, 1u8),
        (0x10, 0x10, 1),
        (0x10, 0x11, 0),
        (0x10, 0xff, 0),
        (0x1f, 0x20, 0),
        (0x20, 0x20, 1),
        (0x20, 0x21, 0),
        (0x00, 0x00, 1),
        (0x80, 0x7f, 1),
        (0x7f, 0x80, 0),
    ] {
        let mut p = Prog::default();
        p.poke(0x2000, 0x00);
        p.poke(0x0000, zp0);
        p.poke(0x00fd, 0x40);
        p.ldx(0xaa);
        p.0.extend_from_slice(&[0x24, 0xfd]); // bit $fd, whose bit 6 sets V
        p.0.push(0x78); // sei
        // Carry in deliberately the OPPOSITE of what should come out, so
        // "preserved" cannot pass as "derived".
        p.0.push(if want_carry == 1 { 0x18 } else { 0x38 });
        p.lda(a_in);
        p.ldy(0x02);
        p.jsr(0xe153).record_and_halt();
        let Some((r, _)) = both(&p.0, 30) else { return skipped() };
        assert!(r.reached, "$E153 never came back with a=${a_in:02x}");
        assert_eq!(r.a, a_in, "a is the caller's and must survive");
        assert_eq!(r.x, 0x00, "x is clobbered to zero though it is not an argument");
        assert_eq!(r.y, 0x00, "y counts down to zero");
        assert_eq!(
            r.p & 0x01,
            want_carry,
            "carry should be (a >= [$00]) for a=${a_in:02x} over ${zp0:02x}, not the carry going in"
        );
        assert_eq!(r.p & 0x02, 0x02, "Z from the final dey");
        assert_eq!(r.p & 0x80, 0x00, "N from the final dey");
        assert_eq!(r.p & 0x40, 0x40, "V should be preserved");
        assert_eq!(r.p & 0x04, 0x04, "I should be preserved");
    }
}

#[test]
fn the_millisecond_delay_is_decimal_proof() {
    // Nothing in it adds, so decimal mode cannot change the count. This is the
    // test that stops someone "simplifying" the loop into arithmetic later,
    // the same way the one on $E149 does.
    let Some(disk) = a_disk() else { return skipped() };
    let mut setup = vec![0xf8]; // sed
    setup.extend_from_slice(&delay_setup(3));
    assert_eq!(
        cycles_of_call_after(&disk, &[], 0xe153, &setup),
        5 + 1790 * 3,
        "decimal mode must not move the count"
    );

    let mut p = Prog::default();
    // NMI off, as in the carry test above and for the same reason: an
    // interrupt landing mid-wait makes this a test of whose handler ran.
    p.poke(0x2000, 0x00);
    p.poke(0x0000, 0x20); // the carry compares against it; plant it
    p.0.push(0xf8); // sed
    p.0.push(0x78); // sei
    p.ldy(0x02);
    p.jsr(0xe153);
    p.record_and_halt();
    let Some((r, _)) = both(&p.0, 30) else { return skipped() };
    assert!(r.reached, "it never returned with decimal mode set");
    assert_eq!(r.p & 0x08, 0x08, "and D should still be set on the way out");
}

#[test]
fn the_delay_touches_nothing_at_all() {
    // No writes anywhere, and in particular no stack writes: there is no `pha`
    // in it, which is exactly why `a` cannot be borrowed as a counter and why
    // the loop has the shape it has. Its only access below $E000 is the
    // repeated read of $0000 the carry comes from.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.ldy(0x01).jsr(0xe153).record_and_halt();
    let writes = writes_inside_call(&disk, &[], &p.0, 0xe153);
    assert!(
        writes.is_empty(),
        "a delay should write nothing, got {writes:04x?}"
    );
    if let Some(real) = real_bios() {
        assert!(
            writes_inside_call(&disk, &real, &p.0, 0xe153).is_empty(),
            "and the real one writes nothing either, which is what we are matching"
        );
    }
}

// --- $E8D2, the producer side of the VRAM transfer queue --------------------

/// Where a queued block's source data goes: clear of the program at $0600, its
/// output at $0700 and the buffer at $0300.
const QSRC: u16 = 0x0760;

/// A caller for $E8D2: one block of `data` bound for VRAM `hi`/`lo`, with the
/// buffer's size at $0300 and its write index at $0301 planted first.
///
/// The buffer is filled with $AA before the call, so a byte the routine left
/// alone is distinguishable from one it wrote as zero. Without that, "it did
/// not get this far" and "it wrote a zero here" look the same and a truncated
/// copy passes.
fn queue_prog(hi: u8, lo: u8, count: u8, cap: u8, start: u8, data: &[u8]) -> Vec<u8> {
    let mut p = Prog::default();
    // NMI off. The routine never touches the PPU, but an interrupt landing
    // mid-call hands the machine to whatever handler the disk left in $DFF6,
    // and then this compares two games rather than one routine.
    p.poke(0x2000, 0x00);
    p.ldx(0x60).lda(0xaa);
    p.raw(&[0x9d, 0x00, 0x03, 0xca, 0xd0, 0xfa]); // sta $0300,x : dex : bne
    p.poke(0x0300, cap).poke(0x0301, start);
    p.bytes_at(QSRC, data);
    p.lda(hi).ldx(lo).ldy(count);
    p.jsr(0xe8d2);
    p.0.extend_from_slice(&QSRC.to_le_bytes());
    p.record_and_halt();
    p.0
}

/// Everything a caller of $E8D2 can see in memory: the zero page it borrows,
/// the buffer's size and index, and the buffer itself.
fn queue_watch() -> Vec<u16> {
    (0x0000..=0x0006u16).chain(0x0300..=0x0360u16).collect()
}

/// One watched byte, by address rather than by position in the vector.
fn watched(vals: &[u8], addr: u16) -> u8 {
    let i = match addr {
        0x0000..=0x0006 => addr as usize,
        0x0300..=0x0360 => 7 + (addr - 0x0300) as usize,
        _ => panic!("${addr:04x} is not in the watch list"),
    };
    vals[i]
}

#[test]
fn vram_queue_at_e8d2_appends_a_block_and_matches_the_real_bios() {
    // $E8D2, entered by 5 of the 114 corpus titles and the last routine four
    // of them are stopped on. a and x are the VRAM address, y the byte count,
    // and the word after the jsr points at the data.
    let data: Vec<u8> = (0..8u8).map(|i| i.wrapping_mul(17).wrapping_add(3)).collect();
    let prog = queue_prog(0x22, 0xcb, 8, 0x4e, 0x00, &data);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert!(r.reached, "the routine never returned");
    assert_eq!(r.a, 0xff, "a is the terminator it planted");
    assert_eq!(r.x, 0x0b, "x is the committed length");
    assert_eq!(r.y, 0x07, "y indexes the last byte copied, so count-1");
    assert_eq!(r.p & 0x01, 0x00, "carry clear: the block fitted");
    assert_eq!(watched(&v, 0x0301), 0x0b, "three header bytes and eight of data");
    assert_eq!(
        [watched(&v, 0x0302), watched(&v, 0x0303), watched(&v, 0x0304)],
        [0x22, 0xcb, 0x08],
        "address high, address low, count"
    );
    for (i, b) in data.iter().enumerate() {
        assert_eq!(watched(&v, 0x0305 + i as u16), *b, "data byte {i}");
    }
    assert_eq!(watched(&v, 0x030d), 0xff, "and the queue is ended");
    assert_eq!(watched(&v, 0x030e), 0xaa, "with nothing written past it");
}

#[test]
fn what_the_queue_appends_is_what_the_flush_writes() {
    // The two halves of one contract, run end to end. Worth doing because
    // $E8D2 and $E86A could agree with each other on a header order that
    // agrees with neither the real BIOS nor the hardware, and each routine's
    // own test would still pass.
    let Some(disk) = a_disk() else { return skipped() };
    let data: Vec<u8> = vec![0xde, 0xad, 0xbe, 0xef, 0x11, 0x22];
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x00ff, 0x00).clear_vram();
    p.poke(0x0300, 0x4e).poke(0x0301, 0x00).poke(0x0302, 0xff);
    p.bytes_at(QSRC, &data);
    p.lda(0x20).ldx(0x00).ldy(data.len() as u8);
    p.jsr(0xe8d2);
    p.0.extend_from_slice(&QSRC.to_le_bytes());
    p.jsr(0xe86a);
    p.record_and_halt();

    let (ours, mut nes) = run(&disk, &[], &p.0, 30);
    assert!(ours.reached, "the pair never returned");
    assert_eq!(&nes.dbg_ciram()[..data.len()], &data[..], "VRAM $2000");
    assert_eq!(nes.peek(0x0301), 0x00, "the flush resets the index");
    assert_eq!(nes.peek(0x0302), 0xff, "and leaves an empty queue");
    if let Some(real) = real_bios() {
        let (theirs, mut rn) = run(&disk, &real, &p.0, 30);
        assert!(theirs.reached, "the real BIOS never finished the test program");
        assert_eq!(
            nes.dbg_ciram()[..64],
            rn.dbg_ciram()[..64],
            "the two BIOSes put different bytes in video memory"
        );
    }
}

#[test]
fn the_queue_appends_rather_than_starting_over() {
    // $0301 is the game's index and it is READ here, not assumed zero. A
    // routine that ignored it would pass every test above and drop whatever
    // the game had already queued this frame.
    let data: Vec<u8> = vec![0x5a; 8];
    let prog = queue_prog(0x22, 0xcb, 8, 0x4e, 0x10, &data);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(watched(&v, 0x0312), 0x22, "the block starts at the index given");
    assert_eq!(watched(&v, 0x0311), 0xaa, "and not one byte before it");
    assert_eq!(watched(&v, 0x031d), 0xff, "terminated after $10 + $0B");
    assert_eq!(watched(&v, 0x0301), 0x1b, "and the index is $10 + $0B");
    assert_eq!(r.x, 0x1b, "x is the new length, not the block's own size");
}

#[test]
fn a_block_that_does_not_fit_rolls_the_buffer_back() {
    // The failure is reported, not papered over: $0301 keeps the length it
    // had, the terminator goes back where the call started, and the caller
    // gets a=$01 with carry set instead of a=$FF with carry clear.
    let data: Vec<u8> = (0..8u8).map(|i| i | 0x40).collect();
    let prog = queue_prog(0x22, 0xcb, 8, 0x0b, 0x00, &data);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert!(r.reached, "an overflow still has to return");
    assert_eq!(r.a, 0x01, "a is the error code");
    assert_eq!(r.p & 0x01, 0x01, "and carry is set");
    assert_eq!(r.x, 0x00, "x is the index the call started at");
    assert_eq!(watched(&v, 0x0301), 0x00, "the index is left alone");
    assert_eq!(watched(&v, 0x0302), 0xff, "and the queue reads as empty again");
    assert_eq!(watched(&v, 0x0002), 0xcb, "$02/$03 keep the address as passed");
    assert_eq!(watched(&v, 0x0003), 0x22);

    // One byte more of room and the same block fits, which is what makes the
    // test above about the boundary rather than about overflow in general.
    // The terminator needs a byte of its own: $0B bytes of block want $0C.
    let prog = queue_prog(0x22, 0xcb, 8, 0x0c, 0x00, &data);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(r.a, 0xff, "with room for the terminator it succeeds");
    assert_eq!(r.p & 0x01, 0x00, "carry clear");
    assert_eq!(watched(&v, 0x0301), 0x0b, "and commits");
}

#[test]
fn the_limit_is_checked_after_every_byte_not_once_up_front() {
    // A size check made once, before the copy, would either refuse the whole
    // block or write all of it. This writes as much as fits and then rolls
    // back, so the number of bytes it got through is the evidence.
    let data: Vec<u8> = vec![0x77; 8];
    for (cap, written, y) in [(0x01u8, 1usize, 0xffu8), (0x04, 4, 0x00), (0x05, 5, 0x01)] {
        let prog = queue_prog(0x22, 0xcb, 8, cap, 0x00, &data);
        let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
            return skipped();
        };
        assert_eq!(r.a, 0x01, "cap ${cap:02x} cannot fit the block");
        assert_eq!(r.y, y, "cap ${cap:02x}: y shows how far the copy got");
        // The rolled-back terminator sits at $0302, so the bytes that prove
        // how far it went start one later.
        for i in 1..written {
            assert_ne!(
                watched(&v, 0x0302 + i as u16),
                0xaa,
                "cap ${cap:02x}: byte {i} should have been written"
            );
        }
        assert_eq!(
            watched(&v, 0x0302 + written as u16),
            0xaa,
            "cap ${cap:02x}: and byte {written} should not"
        );
    }
}

#[test]
fn a_queued_count_of_zero_means_two_hundred_and_fifty_six() {
    // The count is the whole byte, as in $E86A. Zero reading as nothing would
    // look like a successful empty block; it is instead 256 bytes, which no
    // buffer in the corpus can hold, so it always overflows.
    let data: Vec<u8> = (0..8u8).map(|i| i.wrapping_add(0x90)).collect();
    let prog = queue_prog(0x22, 0xcb, 0, 0x4e, 0x00, &data);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(r.a, 0x01, "256 bytes cannot fit, so this is an overflow");
    assert_eq!(watched(&v, 0x0304), 0x00, "the count byte went in as passed");
    assert_eq!(watched(&v, 0x0305), data[0], "and the copy started");
    assert_ne!(
        watched(&v, 0x034f),
        0xaa,
        "a count of zero should run the copy to the end of the buffer"
    );
}

#[test]
fn the_queue_steps_over_its_inline_source_pointer() {
    // The pointer sits after the jsr and would be executed as opcodes if the
    // return address were not stepped on. $60 $07 is an `rts` followed by a
    // stray byte, so a routine that failed to step over it would return to the
    // caller's caller and never reach the recorder below.
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.poke(0x0300, 0x4e).poke(0x0301, 0x00);
    p.bytes_at(QSRC, &[0x11, 0x22]);
    p.lda(0x22).ldx(0x00).ldy(0x02);
    p.jsr(0xe8d2);
    p.0.extend_from_slice(&QSRC.to_le_bytes());
    p.lda(0x3d).record_and_halt();
    let (r, _) = run(&disk, &[], &p.0, 30);
    assert!(r.reached, "control did not resume after the inline pointer");
    assert_eq!(r.a, 0x3d, "execution resumed at the wrong place");
}

#[test]
fn the_queued_address_moves_on_by_one_row_of_thirty_two() {
    // $02/$03 come back pointing one nametable row further down, and the carry
    // the caller sees is that add's. A low byte of $F0 carries into the high
    // byte, and the `adc #$00` there consumes the carry, so the caller still
    // sees it clear: that is why this is two cases and not one.
    let data: Vec<u8> = vec![0x01; 4];
    for (lo, hi, want_lo, want_hi) in [(0xcbu8, 0x22u8, 0xebu8, 0x22u8), (0xf0, 0x22, 0x10, 0x23)] {
        let prog = queue_prog(hi, lo, 4, 0x4e, 0x00, &data);
        let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
            return skipped();
        };
        assert_eq!(r.a, 0xff, "lo ${lo:02x} should have fitted");
        assert_eq!(watched(&v, 0x0002), want_lo, "lo ${lo:02x}: $02");
        assert_eq!(watched(&v, 0x0003), want_hi, "lo ${lo:02x}: $03");
        assert_eq!(r.p & 0x01, 0x00, "lo ${lo:02x}: carry clear on the way out");
        assert_eq!(watched(&v, 0x0303), lo, "and the block kept the address given");
    }
}

#[test]
fn the_queue_leaves_its_zero_page_scratch_where_the_real_bios_does() {
    // $00-$06 all belong to the routine while it runs, and a game that reads
    // them afterwards has to find what the real BIOS left. $05 is a block
    // counter the tail decrements, which is why it is 0 after a success and 1
    // after an overflow rather than simply unused.
    let data: Vec<u8> = vec![0x3c; 6];
    let prog = queue_prog(0x21, 0x40, 6, 0x4e, 0x00, &data);
    let Some((_, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(
        [watched(&v, 0x0000), watched(&v, 0x0001)],
        QSRC.to_le_bytes(),
        "$00/$01 keep the source pointer"
    );
    assert_eq!(watched(&v, 0x0004), 0x06, "$04 keeps the count as passed");
    assert_eq!(watched(&v, 0x0005), 0x00, "$05 reaches zero");
    assert_eq!(watched(&v, 0x0006), 0x00, "$06 counts the copy down to zero");

    let prog = queue_prog(0x21, 0x40, 6, 0x05, 0x00, &data);
    let Some((_, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(watched(&v, 0x0005), 0x01, "and an overflow never decrements it");
    // A cap of 5 leaves room for the three header bytes and two of the six
    // data bytes, so the countdown stops four short rather than one.
    assert_eq!(watched(&v, 0x0006), 0x04, "$06 keeps what was left to copy");
}

#[test]
fn the_queue_is_not_slower_than_the_real_bios() {
    // 182 + 28 per byte against the original's 224 + 41, so the margin grows
    // with the block. Hand-counted from the source rather than recorded from a
    // run, so the numbers can disagree with the implementation.
    let Some(disk) = a_disk() else { return skipped() };
    for count in [4u8, 8, 32] {
        let data: Vec<u8> = vec![0x5a; count as usize];
        let prog = queue_prog(0x22, 0x00, count, 0x4e, 0x00, &data);
        let ours = cycles_of_call_in(&disk, &[], 0xe8d2, &prog);
        assert_eq!(
            ours,
            182 + 28 * count as u64,
            "count {count}: ours should be 182 + 28 per byte"
        );
        if let Some(real) = real_bios() {
            let theirs = cycles_of_call_in(&disk, &real, 0xe8d2, &prog);
            assert_eq!(
                theirs,
                224 + 41 * count as u64,
                "count {count}: the original should be 224 + 41 per byte"
            );
            assert!(ours < theirs, "count {count}: {ours} against {theirs}");
        }
    }
}

// --- $E8E1, the same tail with the shape packed into the data ---------------

/// A caller for $E8E1. The shape byte goes in front of the data, where the
/// routine expects to find it, and `y` is set to something conspicuous
/// because it is not an argument here and must not become one.
fn packed_prog(hi: u8, lo: u8, shape: u8, cap: u8, start: u8, data: &[u8], y: u8) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.ldx(0x60).lda(0xaa);
    p.raw(&[0x9d, 0x00, 0x03, 0xca, 0xd0, 0xfa]); // sta $0300,x : dex : bne
    p.poke(0x0300, cap).poke(0x0301, start);
    p.poke(QSRC, shape);
    p.bytes_at(QSRC + 1, data);
    p.lda(hi).ldx(lo).ldy(y);
    p.jsr(0xe8e1);
    p.0.extend_from_slice(&QSRC.to_le_bytes());
    p.record_and_halt();
    p.0
}

#[test]
fn vram_queue_packed_at_e8e1_writes_several_blocks_and_matches_the_real_bios() {
    // Shape $23: two blocks of three bytes. The second block's VRAM address is
    // one row of 32 further down, so $2140 is followed by $2160.
    let data: Vec<u8> = vec![0xa0, 0xa1, 0xa2, 0xb0, 0xb1, 0xb2];
    let prog = packed_prog(0x21, 0x40, 0x23, 0x4e, 0x00, &data, 0x55);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert!(r.reached, "the routine never returned");
    assert_eq!(r.a, 0xff, "a is the terminator it planted");
    assert_eq!(r.x, 0x0c, "two blocks of three plus two headers");
    assert_eq!(r.y, 0x06, "y walks the source and ends on the last byte read");
    assert_eq!(watched(&v, 0x0301), 0x0c);
    let got: Vec<u8> = (0..13).map(|i| watched(&v, 0x0302 + i)).collect();
    assert_eq!(
        got,
        vec![0x21, 0x40, 0x03, 0xa0, 0xa1, 0xa2, 0x21, 0x60, 0x03, 0xb0, 0xb1, 0xb2, 0xff],
        "two headers, two runs of data, one terminator"
    );
}

#[test]
fn the_packed_nibbles_are_blocks_high_and_bytes_low() {
    // $23 and $32 are the same two digits and must not produce the same
    // buffer: two blocks of three is twelve bytes, three of two is fifteen.
    let data: Vec<u8> = (0..8u8).map(|i| 0xc0 | i).collect();
    for (shape, len) in [(0x23u8, 0x0cu8), (0x32, 0x0f)] {
        let prog = packed_prog(0x21, 0x40, shape, 0x4e, 0x00, &data, 0x01);
        let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
            return skipped();
        };
        assert_eq!(r.a, 0xff, "shape ${shape:02x} should have fitted");
        assert_eq!(watched(&v, 0x0301), len, "shape ${shape:02x}: committed length");
        assert_eq!(watched(&v, 0x0304), shape & 0x0f, "the count byte is the low nibble");
    }
}

#[test]
fn the_packed_form_ignores_y_entirely() {
    // Measured on Druid: forced to $00 and to $55 the call came out identical,
    // registers and memory. A routine that used y as the count, as $E8D2 does,
    // would differ on every byte.
    let data: Vec<u8> = vec![0x11, 0x22, 0x33, 0x44];
    let a = packed_prog(0x21, 0x40, 0x22, 0x4e, 0x00, &data, 0x00);
    let b = packed_prog(0x21, 0x40, 0x22, 0x4e, 0x00, &data, 0x55);
    let Some((ra, va)) = both_watching(&a, 30, &queue_watch()) else {
        return skipped();
    };
    let Some((rb, vb)) = both_watching(&b, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(ra, rb, "y should make no difference to what comes back");
    assert_eq!(va, vb, "nor to the buffer");
}

#[test]
fn the_packed_copy_index_does_not_restart_between_blocks() {
    // The source index walks on through every block. If it reset, block two
    // would be a copy of block one, which is a plausible implementation and
    // the wrong one.
    let data: Vec<u8> = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    let prog = packed_prog(0x21, 0x40, 0x23, 0x4e, 0x00, &data, 0x00);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(r.a, 0xff);
    assert_eq!(
        [watched(&v, 0x0305), watched(&v, 0x0306), watched(&v, 0x0307)],
        [0x01, 0x02, 0x03],
        "block one"
    );
    assert_eq!(
        [watched(&v, 0x030b), watched(&v, 0x030c), watched(&v, 0x030d)],
        [0x04, 0x05, 0x06],
        "and block two carries on from there rather than repeating"
    );
    assert_eq!(r.y, 0x06, "six bytes read in total");
}

#[test]
fn each_packed_block_commits_its_own_length() {
    // Three blocks of three with room for two and a bit. The rollback goes to
    // the SECOND block's commit, not to where the call started, which is the
    // only way to see that $0301 is written per block rather than once.
    let data: Vec<u8> = (0..9u8).map(|i| 0xd0 | i).collect();
    let prog = packed_prog(0x21, 0x40, 0x33, 0x0d, 0x00, &data, 0x00);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(r.a, 0x01, "the third block cannot fit");
    assert_eq!(r.x, 0x0c, "rolled back to two blocks, not to none");
    assert_eq!(watched(&v, 0x0301), 0x0c, "and that is what $0301 holds");
    assert_eq!(watched(&v, 0x030e), 0xff, "the terminator goes after block two");
    assert_eq!(watched(&v, 0x0305), 0xd0, "block one survives");
    assert_eq!(watched(&v, 0x030b), 0xd3, "and so does block two");
}

#[test]
fn a_packed_zero_in_either_nibble_means_two_hundred_and_fifty_six() {
    // Neither nibble reads a zero as nothing and neither reads it as sixteen.
    // Both are a `dec`/`bne` countdown, so both overflow any real buffer.
    let data: Vec<u8> = vec![0x5a; 16];
    for shape in [0x10u8, 0x01] {
        let prog = packed_prog(0x21, 0x40, shape, 0x4e, 0x00, &data, 0x00);
        let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
            return skipped();
        };
        assert_eq!(r.a, 0x01, "shape ${shape:02x} cannot fit");
        assert_ne!(
            watched(&v, 0x034f),
            0xaa,
            "shape ${shape:02x} should have run to the end of the buffer"
        );
    }

    // And sixteen in a nibble is not available to say it with: $0F is the most
    // either can carry, so fifteen of fifteen is the largest shape there is.
    let prog = packed_prog(0x21, 0x40, 0xff, 0x4e, 0x00, &data, 0x00);
    let Some((r, v)) = both_watching(&prog, 30, &queue_watch()) else {
        return skipped();
    };
    assert_eq!(r.a, 0x01, "fifteen blocks of fifteen overflow a 78-byte buffer");
    assert_eq!(watched(&v, 0x0304), 0x0f, "and the count is the low nibble only");
}

#[test]
fn the_packed_form_steps_over_its_inline_pointer() {
    let Some(disk) = a_disk() else { return skipped() };
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.poke(0x0300, 0x4e).poke(0x0301, 0x00);
    p.poke(QSRC, 0x12);
    p.bytes_at(QSRC + 1, &[0x11, 0x22]);
    p.lda(0x21).ldx(0x40).ldy(0x00);
    p.jsr(0xe8e1);
    p.0.extend_from_slice(&QSRC.to_le_bytes());
    p.lda(0x3d).record_and_halt();
    let (r, _) = run(&disk, &[], &p.0, 30);
    assert!(r.reached, "control did not resume after the inline pointer");
    assert_eq!(r.a, 0x3d, "execution resumed at the wrong place");
}

#[test]
fn the_packed_form_is_not_slower_than_the_real_bios() {
    // 117 + 80 a block + 28 a byte against the original's 119 + 120 + 41.
    // Hand-counted from the source, so the numbers can disagree with it.
    let Some(disk) = a_disk() else { return skipped() };
    for (blocks, bytes) in [(1u8, 1u8), (2, 1), (2, 14), (4, 3)] {
        let data: Vec<u8> = vec![0x5a; (blocks * bytes) as usize];
        let shape = (blocks << 4) | bytes;
        let prog = packed_prog(0x20, 0x00, shape, 0x7f, 0x00, &data, 0x00);
        let ours = cycles_of_call_in(&disk, &[], 0xe8e1, &prog);
        assert_eq!(
            ours,
            117 + blocks as u64 * (80 + 28 * bytes as u64),
            "shape ${shape:02x}: ours should be 117 + 80 a block + 28 a byte"
        );
        if let Some(real) = real_bios() {
            let theirs = cycles_of_call_in(&disk, &real, 0xe8e1, &prog);
            assert_eq!(
                theirs,
                119 + blocks as u64 * (120 + 41 * bytes as u64),
                "shape ${shape:02x}: the original should be 119 + 120 a block + 41 a byte"
            );
            assert!(ours < theirs, "shape ${shape:02x}: {ours} against {theirs}");
        }
    }
}

// --- $EC22, the metasprite blitter ------------------------------------------

/// Where these tests put $EC22's twelve-byte block, and the tile list.
const MS_BLOCK: u16 = 0x0500;
const MS_TILES: u16 = 0x0400;

/// The tile list every pointer-mode test reads: index i holds i ^ $5A, so
/// every index reads differently and none reads as its own number, which is
/// what tells the pointer path from the literal one.
fn ms_tile(i: u8) -> u8 {
    i ^ 0x5a
}

/// A caller for $EC22.
///
/// It paints pages 2 and 3 with $EE and the borrowed zero page with $CC
/// before the call, so "never written" is told apart from "written as zero",
/// and a routine that stopped early or wrote one byte too many shows up.
/// Page 3 is included because a grid that runs off the end of the sprite page
/// carries on into it.
fn ms_prog(block: [u8; 12], regs: (u8, u8, u8)) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00); // NMI off, as for the queue tests
    p.raw(&[
        0xa2, 0x00, // ldx #0
        0xa9, 0xee, // lda #$ee
        0x9d, 0x00, 0x02, // sta $0200,x
        0x9d, 0x00, 0x03, // sta $0300,x
        0x8a, // txa
        0x49, 0x5a, // eor #$5a
        0x9d, 0x00, 0x04, // sta $0400,x
        0xe8, // inx
        0xd0, 0xef, // bne, back to the lda
        0xa2, 0x0a, // ldx #$0a
        0xa9, 0xcc, // lda #$cc
        0x95, 0x02, // sta $02,x
        0xca, // dex
        0x10, 0xfb, // bpl
    ]);
    p.bytes_at(MS_BLOCK, &block);
    p.poke(0x0000, MS_BLOCK as u8).poke(0x0001, (MS_BLOCK >> 8) as u8);
    p.lda(regs.0).ldx(regs.1).ldy(regs.2).jsr(0xec22).record_and_halt();
    assert!(p.0.len() < 0x100, "the caller has grown into its own answers");
    p.0
}

/// The borrowed zero page, then pages 2 and 3.
fn ms_watch() -> Vec<u16> {
    (0x0002..=0x000cu16).chain(0x0200..=0x03ffu16).collect()
}

/// One watched byte, by address.
fn ms_at(vals: &[u8], addr: u16) -> u8 {
    match addr {
        0x0002..=0x000c => vals[(addr - 2) as usize],
        0x0200..=0x03ff => vals[11 + (addr - 0x0200) as usize],
        _ => panic!("${addr:04x} is not in the watch list"),
    }
}

/// The four bytes of the OAM entry at `offset` within page 2, as Y, tile,
/// attribute, X.
fn ms_entry(vals: &[u8], offset: u8) -> [u8; 4] {
    let a = 0x0200 + offset as u16;
    [ms_at(vals, a), ms_at(vals, a + 1), ms_at(vals, a + 2), ms_at(vals, a + 3)]
}

/// $EC22 against the real BIOS, with pages 2 and 3 and the zero page compared.
fn ms_both(block: [u8; 12]) -> Option<(Outcome, Vec<u8>)> {
    ms_both_regs(block, (0x05, 0x00, 0x00))
}

fn ms_both_regs(block: [u8; 12], regs: (u8, u8, u8)) -> Option<(Outcome, Vec<u8>)> {
    let r = both_watching(&ms_prog(block, regs), 30, &ms_watch());
    if let Some((o, _)) = &r {
        assert!(o.reached, "$EC22 never returned");
    }
    r
}

/// A block, laid out as the routine reads it.
#[allow(clippy::too_many_arguments)]
fn ms_block(control: u8, y: u8, x: u8, frame: u8, src: u16, flips: u8, attr: u8, shape: u8, at: u8)
    -> [u8; 12]
{
    [control, y, 0x00, x, 0x00, frame, (src >> 8) as u8, src as u8, flips, attr, shape, at]
}

#[test]
fn metasprite_at_ec22_draws_a_grid_and_matches_the_real_bios() {
    // Youkai Yashiki's shape: six rows of four strips from the start of the
    // page, attribute $02, at Y $70 and X $E0. Its own tile list lives in its
    // PRG RAM; this one points at the test's.
    let block = ms_block(0x00, 0x70, 0xe0, 0x00, MS_TILES, 0x00, 0x02, 0x64, 0x00);
    let Some((r, v)) = ms_both(block) else { return skipped() };
    // The exit Youkai's own call came back with.
    assert_eq!((r.a, r.x, r.y), (0x61, 0x00, 0x18), "a = $02, x = 0, y = 24 sprites");
    assert_eq!(ms_entry(&v, 0x00), [0x70, ms_tile(0), 0x02, 0xe0], "top of the left strip");
    assert_eq!(ms_entry(&v, 0x04), [0x78, ms_tile(1), 0x02, 0xe0], "one row down");
    assert_eq!(ms_entry(&v, 0x14), [0x98, ms_tile(5), 0x02, 0xe0], "foot of the strip");
    assert_eq!(ms_entry(&v, 0x18), [0x70, ms_tile(6), 0x02, 0xe8], "top of the next strip");
    assert_eq!(ms_entry(&v, 0x5c), [0x98, ms_tile(23), 0x02, 0xf8], "the last sprite");
    assert_eq!(ms_at(&v, 0x0260), 0xee, "and nothing after it");
    assert_eq!(
        (0x02..=0x0cu16).map(|a| ms_at(&v, a)).collect::<Vec<_>>(),
        [0x61, 0x02, 0x06, 0x04, 0x00, 0x00, 0x04, 0x02, 0x00, 0x00, 0x06],
        "$02-$0C on the way out"
    );
}

#[test]
fn druids_call_draws_literal_tiles_from_a_frame() {
    // Druid's block exactly as its game passes it: a zero source high byte,
    // so +07 is a base tile, moved on by frame $0B of two tiles each. $8A +
    // 22 is $A0, and the notes measured tiles $A0 and $A1 at $0239 and $023D.
    let block = ms_block(0x00, 0x00, 0x82, 0x0b, 0x008a, 0x00, 0x03, 0x21, 0x38);
    let Some((r, v)) = ms_both(block) else { return skipped() };
    assert_eq!((r.a, r.y), (0x41, 0xa2), "a = $38 + 8 + 1, y = base + 2");
    assert_eq!(ms_entry(&v, 0x38), [0x00, 0xa0, 0x03, 0x82]);
    assert_eq!(ms_entry(&v, 0x3c), [0x08, 0xa1, 0x03, 0x82]);
    assert_eq!([ms_at(&v, 0x0007), ms_at(&v, 0x0008)], [0xa0, 0x00], "the moved source");
    assert_eq!(ms_at(&v, 0x000a), 0xff, "$0A says literal");
}

#[test]
fn the_mode_is_picked_after_the_frame_moves_the_source() {
    // A literal base of $10 moved on by $AB frames of six is $0412: the high
    // byte is no longer zero, so it is now a POINTER, into the test's list.
    // Deciding the mode before adding the frame would draw $10 upwards.
    let block = ms_block(0x00, 0x40, 0x40, 0xab, 0x0010, 0x00, 0x00, 0x32, 0x00);
    let Some((r, v)) = ms_both(block) else { return skipped() };
    assert_eq!(ms_entry(&v, 0x00)[1], ms_tile(0x12), "first tile from the list at $0412");
    assert_eq!(ms_entry(&v, 0x14)[1], ms_tile(0x17), "and the sixth");
    assert_eq!([ms_at(&v, 0x0007), ms_at(&v, 0x0008)], [0x12, 0x04]);
    assert_eq!(ms_at(&v, 0x000a), 0x00, "$0A says pointer");
    assert_eq!(r.y, 0x06, "pointer mode counts from 0");
}

#[test]
fn the_control_byte_hides_on_bit_7_and_otherwise_draws_nothing() {
    let draw = |control| ms_block(control, 0x70, 0xe0, 0x00, MS_TILES, 0x00, 0x02, 0x32, 0x00);

    let Some((_, v)) = ms_both(draw(0x80)) else { return skipped() };
    for k in 0..6u8 {
        assert_eq!(ms_entry(&v, 4 * k)[0], 0xf4, "sprite {k}: bit 7 parks every row at $F4");
    }
    assert_eq!(ms_entry(&v, 0x0c)[3], 0xe8, "but X and the rest are still drawn");

    for control in [0x01u8, 0x08, 0x7f] {
        let Some((r, v)) = ms_both_regs(draw(control), (0x11, 0x22, 0x33)) else {
            return skipped();
        };
        assert_eq!((r.a, r.x, r.y), (control, 0x70, 0x00), "a = +00, x = +01, y = 0");
        assert_eq!(ms_at(&v, 0x0200), 0xee, "control ${control:02x}: nothing drawn");
        assert_eq!(ms_at(&v, 0x0006), 0xcc, "and nothing past the shape touched");
    }
}

#[test]
fn a_zero_in_either_nibble_draws_nothing_and_leaves_x_alone() {
    for (shape, rows_written) in [(0x02u8, false), (0x30, true), (0x00, false)] {
        let block = ms_block(0x00, 0x70, 0xe0, 0x00, MS_TILES, 0x00, 0x02, shape, 0x0c);
        let Some((r, v)) = ms_both_regs(block, (0x11, 0x22, 0x33)) else {
            return skipped();
        };
        assert_eq!((r.a, r.x, r.y), (0x00, 0x22, 0x0a), "shape ${shape:02x}");
        assert_eq!(r.p & 0x02, 0x02, "shape ${shape:02x}: Z set");
        assert_eq!(ms_at(&v, 0x0002), 0x0c, "the OAM pointer is set before the check");
        assert_eq!(ms_at(&v, 0x0004) != 0xcc, rows_written, "shape ${shape:02x}: $04");
        assert_eq!(ms_at(&v, 0x0005), 0xcc, "shape ${shape:02x}: $05 never reached");
        assert_eq!(ms_at(&v, 0x020c), 0xee, "shape ${shape:02x}: nothing drawn");
    }
}

#[test]
fn flips_walk_the_tiles_backwards_and_never_move_a_sprite() {
    // Three rows of two strips. Vertical reverses the order within a strip,
    // horizontal the order of the strips, and every sprite keeps its place.
    let cases: [(u8, u8, [u8; 6]); 4] = [
        (0x00, 0x02, [0, 1, 2, 3, 4, 5]),
        (0x01, 0x82, [2, 1, 0, 5, 4, 3]),
        (0x10, 0x42, [3, 4, 5, 0, 1, 2]),
        (0x11, 0xc2, [5, 4, 3, 2, 1, 0]),
    ];
    for (flips, attr, order) in cases {
        let block = ms_block(0x00, 0x70, 0xe0, 0x00, MS_TILES, flips, 0x02, 0x32, 0x00);
        let Some((_, v)) = ms_both(block) else { return skipped() };
        for (k, idx) in order.iter().enumerate() {
            let want = [0x70 + 8 * (k as u8 % 3), ms_tile(*idx), attr, 0xe0 + 8 * (k as u8 / 3)];
            assert_eq!(ms_entry(&v, 4 * k as u8), want, "flips ${flips:02x}, sprite {k}");
        }
    }
}

#[test]
fn the_attribute_keeps_palette_and_priority_and_takes_flips_from_08() {
    // +09 is masked with $23, and +08 contributes only its two flip bits.
    // $0F/$F0 is the probe that refused to fit the first, summed reading.
    for (flips, attr, want) in [(0x00u8, 0xffu8, 0x23u8), (0x00, 0xe3, 0x23), (0x0f, 0xf0, 0xa0),
                                (0xfe, 0x00, 0x40), (0xff, 0xff, 0xe3)] {
        let block = ms_block(0x00, 0x70, 0xe0, 0x00, MS_TILES, flips, attr, 0x11, 0x00);
        let Some((_, v)) = ms_both(block) else { return skipped() };
        assert_eq!(ms_at(&v, 0x0202), want, "+08 ${flips:02x}, +09 ${attr:02x}");
        assert_eq!(ms_at(&v, 0x0009), want, "and $09 keeps it");
    }
}

#[test]
fn a_row_at_f4_stays_there_rather_than_wrapping_back_on_screen() {
    // $F4 is the off-screen Y. A strip that reaches it exactly stops moving;
    // one that steps over it wraps like any byte. This is also all the hide
    // bit is: it starts every strip at $F4.
    for (top, want) in [
        (0xecu8, [0xecu8, 0xf4, 0xf4, 0xf4, 0xf4]),
        (0xe5, [0xe5, 0xed, 0xf5, 0xfd, 0x05]),
        (0xf8, [0xf8, 0x00, 0x08, 0x10, 0x18]),
    ] {
        let block = ms_block(0x00, top, 0x10, 0x00, MS_TILES, 0x00, 0x00, 0x51, 0x00);
        let Some((_, v)) = ms_both(block) else { return skipped() };
        let got: Vec<u8> = (0..5u8).map(|k| ms_entry(&v, 4 * k)[0]).collect();
        assert_eq!(got, want, "a strip starting at ${top:02x}");
    }
}

#[test]
fn a_grid_past_the_page_end_spills_y_attr_and_x_but_wraps_its_tiles() {
    // Two passes, two addressing modes. From $F8, four sprites: the third
    // and fourth have their Y, attribute and X at $0300-$0307, while their
    // tiles wrap round to $0201 and $0205.
    let block = ms_block(0x00, 0x70, 0xe0, 0x00, MS_TILES, 0x00, 0x02, 0x41, 0xf8);
    let Some((r, v)) = ms_both(block) else { return skipped() };
    assert_eq!(ms_entry(&v, 0xf8), [0x70, ms_tile(0), 0x02, 0xe0]);
    assert_eq!(
        [ms_at(&v, 0x0300), ms_at(&v, 0x0301), ms_at(&v, 0x0302), ms_at(&v, 0x0303)],
        [0x80, 0xee, 0x02, 0xe0],
        "the third sprite, with no tile byte in page 3"
    );
    assert_eq!(ms_at(&v, 0x0201), ms_tile(2), "its tile wrapped into page 2");
    assert_eq!(ms_at(&v, 0x0205), ms_tile(3));
    assert_eq!(ms_at(&v, 0x0200), 0xee, "and nothing else there was touched");
    assert_eq!(r.a, 0x09, "$F8 + 16 + 1, in eight bits");
}

#[test]
fn the_exit_flags_come_from_the_last_sum_each_walk_does() {
    // Carry and overflow on the way out are not a single rule: each flip
    // setting leaves the flags of a different sum. These blocks are picked so
    // each sum overflows, which a routine that cleared V, or kept the
    // caller's, or took it from another add, would get wrong.
    //
    //   none, both  (off + 4n - 3) + 4     off $7E, n 1:  $7F + 4
    //   vertical    (base + n - r) + r     base $7F, 1x1: $7F + 1
    //   horizontal  (base + r - 1) - r     base $80, 1x1: $80 - 1, carry set
    // Z is set and N clear on every drawn path, from the x = 0 on the way out.
    let cases = [
        (0x00u8, 0x0000u16, 0x7eu8, 0x42u8),
        (0x11, 0x0000, 0x7e, 0x42),
        (0x01, 0x007f, 0x00, 0x42),
        (0x10, 0x0080, 0x00, 0x43),
    ];
    for (flips, src, at, p) in cases {
        let block = ms_block(0x00, 0x70, 0xe0, 0x00, src, flips, 0x00, 0x11, at);
        let Some((r, _)) = ms_both(block) else { return skipped() };
        assert_eq!(r.p & 0xc3, p, "flips ${flips:02x}: N V Z C");
    }
    // And the same sums NOT overflowing, so a routine that always set V
    // cannot pass either: one sprite further along clears it.
    for (flips, src, at) in [(0x00u8, 0x0000u16, 0x7au8), (0x01, 0x007e, 0x00), (0x10, 0x0081, 0x00)] {
        let block = ms_block(0x00, 0x70, 0xe0, 0x00, src, flips, 0x00, 0x11, at);
        let Some((r, _)) = ms_both(block) else { return skipped() };
        assert_eq!(r.p & 0x40, 0x00, "flips ${flips:02x}: no overflow this time");
    }
}

/// A fixed pseudo-random sequence, so the sweep below is the same every run.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.0 >> 16) as u8
    }
}

#[test]
fn a_sweep_of_forced_blocks_matches_the_real_bios() {
    // The cases above each pin one behaviour. This one is for the
    // interactions nobody thought to write down: forty blocks with every
    // field varied at once, compared byte for byte with the original. Only
    // meaningful with the oracle present, so it skips without one.
    if real_bios().is_none() {
        eprintln!("note: no dumps/fds/disksys.rom, so the sweep has nothing to compare with");
        return;
    }
    let mut g = Lcg(0xec22);
    for case in 0..40 {
        let control = [0x00, 0x00, 0x00, 0x80, 0x01][g.next() as usize % 5];
        let shape = ((g.next() % 15 + 1) << 4) | (g.next() % 15 + 1);
        let n = (shape >> 4) as u16 * (shape & 15) as u16;
        // A source has to stay inside memory the two runs agree on. RAM
        // outside the planted list differs between the BIOSes, and a frame
        // big enough can carry a literal base into the PPU registers, whose
        // reads differ too. So literal bases keep their frame small enough to
        // stay literal, and pointers stay in the list.
        let (frame, src) = if g.next() & 1 == 0 {
            let lo = g.next() as u16;
            ((g.next() as u16 % ((0xff - lo) / n + 1)) as u8, lo)
        } else {
            let lo = g.next() as u16 % (0x100 - n.min(0xff));
            (0x00, MS_TILES + lo)
        };
        let block = ms_block(control, g.next(), g.next(), frame, src, g.next(), g.next(), shape,
                             g.next());
        let regs = (g.next(), g.next(), g.next());
        eprintln!("case {case}: {block:02x?}");
        let _ = ms_both_regs(block, regs);
    }
}

#[test]
fn the_metasprite_is_not_slower_than_the_real_bios() {
    // About 192 + 87 a sprite + 21 a strip against the original's 206 + 131
    // a sprite, so the margin grows with the grid. Held at the three corpus
    // callers' own shapes (Druid's literal two with its frame, Kaettekita's
    // six, Youkai's 24), at the largest grid there is, and at a lone sprite,
    // where it is thinnest: hidden, flipped and carried into pointer mode by
    // its frame, which a sweep put at 3 cycles to spare.
    let Some(disk) = a_disk() else { return skipped() };
    let Some(real) = real_bios() else { return };
    let shapes = [
        (0x00u8, 0x21u8, 0x008au16, 0x0bu8),
        (0x00, 0x32, MS_TILES, 0x00),
        (0x00, 0x64, MS_TILES, 0x00),
        (0x00, 0x32, 0x0040, 0x01),
        (0x00, 0xff, MS_TILES, 0x00),
        (0x00, 0x11, MS_TILES, 0x00),
        (0x00, 0x11, 0x0040, 0x00),
        (0x80, 0x11, 0x00f7, 0x10),
        (0x80, 0x12, 0x00f7, 0x10),
    ];
    for (control, shape, src, frame) in shapes {
        for flips in [0x00u8, 0x01, 0x10, 0x11] {
            let block = ms_block(control, 0x70, 0xe0, frame, src, flips, 0x02, shape, 0x00);
            let prog = ms_prog(block, (0, 0, 0));
            let ours = cycles_of_call_in(&disk, &[], 0xec22, &prog);
            let theirs = cycles_of_call_in(&disk, &real, 0xec22, &prog);
            assert!(
                ours < theirs,
                "control ${control:02x} shape ${shape:02x} flips ${flips:02x}: {ours} against {theirs}"
            );
        }
    }
}

// --- $E9D3, the zero-page timers --------------------------------------------

/// A caller for $E9D3: NMI off, the WHOLE zero page filled with `fill` so the
/// two BIOSes start from the same bytes (a walk that wraps visits all of
/// them), then `plant` on top, then the call.
fn timers_prog(a: u8, x: u8, y: u8, fill: u8, plant: &[(u8, u8)]) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    // lda #fill : ldx #0 : sta $00,x : inx : bne
    p.raw(&[0xa9, fill, 0xa2, 0x00, 0x95, 0x00, 0xe8, 0xd0, 0xfb]);
    for (addr, v) in plant {
        p.poke(*addr as u16, *v);
    }
    p.lda(a).ldx(x).ldy(y).jsr(0xe9d3).record_and_halt();
    p.0
}

fn zero_page() -> Vec<u16> {
    (0x0000..=0x00ffu16).collect()
}

fn timers_both(a: u8, x: u8, y: u8, fill: u8, plant: &[(u8, u8)]) -> Option<(Outcome, Vec<u8>)> {
    let r = both_watching(&timers_prog(a, x, y, fill, plant), 30, &zero_page());
    if let Some((o, _)) = &r {
        assert!(o.reached, "$E9D3 never returned");
    }
    r
}

#[test]
fn timers_at_e9d3_count_the_fast_ones_down_and_match_the_real_bios() {
    // Bubble Bobble's own registers: prescaler at $2B, fast timers up to $2F,
    // slow ones up to $31. The prescaler is at 5, so this is a fast tick.
    let plant = [(0x2b, 0x05), (0x2c, 0x03), (0x2d, 0x00), (0x2e, 0x80), (0x2f, 0x01),
                 (0x30, 0x07), (0x31, 0x07)];
    let Some((r, z)) = timers_both(0x2f, 0x2b, 0x31, 0x00, &plant) else { return skipped() };
    assert_eq!(z[0x2b], 0x04, "the prescaler counts down");
    assert_eq!(&z[0x2c..=0x2f], &[0x02, 0x00, 0x7f, 0x00], "fast timers tick, zero stays zero");
    assert_eq!(&z[0x30..=0x31], &[0x07, 0x07], "and the slow ones wait");
    assert_eq!(z[0x00], 0x2b, "$00 holds where the walk stops");
    assert_eq!((r.a, r.x, r.y), (0x03, 0x2b, 0x31), "a is the last timer loaded, before its dec");
    assert_eq!(r.p & 0x03, 0x03, "Z and C from the final compare");
}

#[test]
fn a_prescaler_underflow_reloads_nine_and_ticks_the_slow_timers() {
    let plant = [(0x2b, 0x00), (0x2c, 0x03), (0x2f, 0x01), (0x30, 0x07), (0x31, 0x00)];
    let Some((r, z)) = timers_both(0x2f, 0x2b, 0x31, 0x00, &plant) else { return skipped() };
    assert_eq!(z[0x2b], 0x09, "0 - 1 reloads to 9, so the slow timers tick every tenth call");
    assert_eq!(&z[0x2c..=0x31], &[0x02, 0x00, 0x00, 0x00, 0x06, 0x00]);
    assert_eq!(r.a, 0x03, "the walk runs down to x+1 and $2C was loaded last");
    // Any negative counts as an underflow, not just $FF.
    let Some((_, z)) = timers_both(0x2f, 0x2b, 0x31, 0x00, &[(0x2b, 0xe0), (0x31, 0x02)]) else {
        return skipped();
    };
    assert_eq!((z[0x2b], z[0x31]), (0x09, 0x01), "$E0 - 1 is negative too");
}

#[test]
fn the_walk_stops_at_00_not_at_x_so_a_below_x_wraps_the_zero_page() {
    // a below x: the index walks down through $00, decrementing every
    // nonzero byte on the way, $00 itself included. With $00 one lower the
    // walk ends a byte late and takes the prescaler a second time: $49 to $47.
    let Some((r, z)) = timers_both(0x34, 0x3c, 0x3d, 0x01, &[(0x3c, 0x49)]) else {
        return skipped();
    };
    assert_eq!(z[0x3c], 0x47, "the prescaler, decremented twice");
    assert_eq!(z[0x00], 0x3b, "$00 was a timer too");
    assert_eq!(z[0x80], 0x00, "and so was everything else the walk passed");
    assert_eq!(z[0x3d], 0x00, "round the top and down to x again");
    assert_eq!(z[0x3b], 0x01, "and only the new stop, $3B, is spared");
    assert_eq!(r.x, 0x3b, "x stops where $00 now says");
}

#[test]
fn the_timers_are_cycle_exact() {
    // 19 + 15 a zero timer + 20 a nonzero one on a fast tick, 7 more when the
    // prescaler reloads. Hand-counted from the source, then held against the
    // original, wrapping walk included.
    let Some(disk) = a_disk() else { return skipped() };
    let cases: [(u8, u8, u8, &[(u8, u8)], u64); 4] = [
        (0x2f, 0x2b, 0x31, &[(0x2b, 0x05)], 19 + 15 * 4),
        (0x2f, 0x2b, 0x31, &[(0x2b, 0x00)], 26 + 15 * 6),
        (0x2f, 0x2b, 0x31, &[(0x2b, 0x05), (0x2c, 0x09), (0x2e, 0x01)], 19 + 15 * 2 + 20 * 2),
        (0x34, 0x3c, 0x3d, &[(0x3c, 0x49)], 0),
    ];
    // The last case wraps the zero page, filled with $01 so every byte it
    // passes is a live timer; it has no hand count, only the original.
    for (a, x, y, plant, want) in cases {
        let fill = if want == 0 { 0x01 } else { 0x00 };
        let prog = timers_prog(a, x, y, fill, plant);
        let ours = cycles_of_call_in(&disk, &[], 0xe9d3, &prog);
        if want != 0 {
            assert_eq!(ours, want, "a={a:02x} x={x:02x}: the hand count");
        }
        if let Some(real) = real_bios() {
            let theirs = cycles_of_call_in(&disk, &real, 0xe9d3, &prog);
            assert_eq!(ours, theirs, "a={a:02x} x={x:02x}: ours against the original");
        }
    }
}

// --- $E844, an inline pointer for the caller's caller -----------------------

/// Where the test's own `G` lives: `jsr $E844 : rts`, the shape a game
/// subroutine that takes an inline pointer has.
const INLINE_G: u16 = 0x06c0;

/// A caller for $E844, two levels deep. A stub at `at` does `jsr G` with
/// `word` after it and then jumps to the recorder, so the recorder is only
/// reached if G's return address was stepped over exactly two bytes. The
/// stack pointer is pinned first, because x comes back as the stack pointer
/// and the two BIOSes need not hand over the same one.
fn inline_prog(at: u16, word: u16, y: u8) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.raw(&[0xa2, 0xef, 0x9a]); // ldx #$ef : txs
    p.bytes_at(INLINE_G, &[0x20, 0x44, 0xe8, 0x60]);
    // The stub's jmp target is the recorder, whose address is only known once
    // the rest is laid out: everything below is fixed-size, so count it.
    let stub_len = 8u16;
    let tail = 5 * stub_len as usize + 2 + 3; // the stub's pokes, ldy, jmp
    let record = PROG + (p.0.len() + tail) as u16;
    let stub = [
        0x20, INLINE_G as u8, (INLINE_G >> 8) as u8,
        word as u8, (word >> 8) as u8,
        0x4c, record as u8, (record >> 8) as u8,
    ];
    p.bytes_at(at, &stub);
    p.ldy(y);
    p.raw(&[0x4c, at as u8, (at >> 8) as u8]);
    assert_eq!(PROG + p.0.len() as u16, record, "the recorder moved");
    p.record_and_halt();
    p.0
}

fn inline_watch() -> Vec<u16> {
    vec![0x0000, 0x0001, 0x0005, 0x0006]
}

#[test]
fn inline_ptr_at_e844_hands_the_callers_pointer_over_and_matches_the_real_bios() {
    let Some((r, v)) = both_watching(&inline_prog(0x0570, 0xbeef, 0x77), 30, &inline_watch())
    else {
        return skipped();
    };
    assert!(r.reached, "G did not return past its pointer, or never returned");
    assert_eq!([v[0], v[1]], [0xef, 0xbe], "$00/$01 = the word after G's jsr");
    assert_eq!([v[2], v[3]], [0x72, 0x05], "$05/$06 = G's return address as it was");
    // G's stub jsr is at $0570, so its return address is $0572 and the new
    // one $0574: a is the high byte, x the stack pointer at entry, y 2.
    assert_eq!((r.a, r.x, r.y), (0x05, 0xeb, 0x02));
}

#[test]
fn the_inline_step_carries_into_the_high_byte() {
    // G's jsr at $04FC: its return address is $04FE and the step lands on
    // $0500, so the +2 carries. The pointer itself straddles the page too.
    let Some((r, v)) = both_watching(&inline_prog(0x04fc, 0x1234, 0x00), 30, &inline_watch())
    else {
        return skipped();
    };
    assert!(r.reached, "the carry was dropped and G returned into $04xx");
    assert_eq!([v[0], v[1]], [0x34, 0x12]);
    assert_eq!(r.a, 0x05, "the stepped high byte");
}

#[test]
fn inline_ptr_is_cycle_exact() {
    // 64, as on both corpus callers, and 65 when the second byte of the
    // pointer is across a page from G's return address: ($05),y pays for the
    // crossing, and the original pays it too.
    let Some(disk) = a_disk() else { return skipped() };
    for (at, want) in [(0x0570u16, 64u64), (0x04fc, 65)] {
        let prog = inline_prog(at, 0xbeef, 0x00);
        let ours = cycles_of_call_in(&disk, &[], 0xe844, &prog);
        assert_eq!(ours, want, "G at ${at:04x}");
        if let Some(real) = real_bios() {
            assert_eq!(ours, cycles_of_call_in(&disk, &real, 0xe844, &prog), "G at ${at:04x}");
        }
    }
}

// --- $E997, a nametable address to pixel coordinates ------------------------

fn tile_prog(hi: u8, lo: u8) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00).poke(0x0000, hi).poke(0x0001, lo);
    p.poke(0x0002, 0xcc).poke(0x0003, 0xcc);
    p.lda(0x5a).ldx(0x11).ldy(0x22).jsr(0xe997).record_and_halt();
    p.0
}

#[test]
fn tile_to_pixel_at_e997_matches_the_real_bios() {
    // Knight Move's own call: $21D9 is row 14, column 25.
    let watch = [0x0000u16, 0x0001, 0x0002, 0x0003];
    let Some((r, v)) = both_watching(&tile_prog(0x21, 0xd9), 30, &watch) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!([v[2], v[3]], [0x70, 0xc8], "Y = 14 * 8, X = 25 * 8");
    assert_eq!([v[0], v[1]], [0x21, 0xd9], "the address is left as it was");
    assert_eq!((r.a, r.x, r.y), (0x70, 0x11, 0x22), "a = Y, x and y untouched");
}

#[test]
fn tile_to_pixel_takes_ten_bits_and_ignores_the_rest() {
    // The top six bits of the high byte are not part of the tile, so $2xxx,
    // $6xxx and $Exxx all give the same answer; the carry out is bit 1 of the
    // low byte, from the second ror.
    let watch = [0x0002u16, 0x0003];
    for (hi, lo, y, x, c) in [
        (0x20u8, 0x00u8, 0x00u8, 0x00u8, 0u8),
        (0x23, 0xbf, 0xe8, 0xf8, 1),
        (0xe3, 0xbf, 0xe8, 0xf8, 1),
        (0x62, 0x41, 0x90, 0x08, 0),
    ] {
        let Some((r, v)) = both_watching(&tile_prog(hi, lo), 30, &watch) else {
            return skipped();
        };
        assert_eq!([v[0], v[1]], [y, x], "${hi:02x}{lo:02x}");
        assert_eq!(r.p & 0x01, c, "${hi:02x}{lo:02x}: carry");
    }
}

#[test]
fn tile_to_pixel_is_cycle_exact() {
    let Some(disk) = a_disk() else { return skipped() };
    let prog = tile_prog(0x21, 0xd9);
    let ours = cycles_of_call_in(&disk, &[], 0xe997, &prog);
    assert_eq!(ours, 49);
    if let Some(real) = real_bios() {
        assert_eq!(ours, cycles_of_call_in(&disk, &real, 0xe997, &prog));
    }
}

// --- $E239 and $E237, writing a file to the disk ----------------------------

const WF_TEMPLATE: u16 = 0x0500;
const WF_DESC: u16 = 0x0510;
const WF_SOURCE: u16 = 0x0540;
const WF_LOADED: u16 = 0x0580;
/// The file ID the tests write, and the size of its body.
const WF_ID: u8 = 0x42;
const WF_SIZE: u8 = 0x20;

/// The byte the test writes at offset i, and expects to read back.
fn wf_byte(i: u8) -> u8 {
    i ^ 0x5a
}

/// A caller that writes one file through `entry` with `a` = `index`, then, if
/// `read_back`, loads it again through $E1F8 into WF_LOADED and records THAT.
/// The template is all wildcards, except a $77 at `bad_at` to make it the
/// wrong disk; the descriptor says: ID $42, "TESTSAVE",
/// load at WF_LOADED, $20 bytes, kind 0, from WF_SOURCE in CPU RAM, or from
/// the PPU at $0000 if `ppu`.
fn wf_prog(entry: u16, index: u8, read_back: bool, bad_at: Option<u8>, ppu: bool) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    // $00-$0F painted, so cells a path leaves alone compare equal rather than
    // comparing what each BIOS happened to leave there at boot.
    p.raw(&[0xa2, 0x0f, 0xa9, 0xcc, 0x95, 0x00, 0xca, 0x10, 0xfb]);
    p.raw(&[0xa9, 0xff, 0xa2, 0x09, 0x9d, 0x00, 0x05, 0xca, 0x10, 0xfa]); // template
    p.raw(&[0xa2, WF_SIZE - 1, 0x8a, 0x49, 0x5a, 0x9d, 0x40, 0x05, 0xca, 0x10, 0xf7]); // data
    if let Some(i) = bad_at {
        p.poke(WF_TEMPLATE + i as u16, 0x77); // nothing on Zelda's side reads $77
    }
    let src = if ppu { 0x0000 } else { WF_SOURCE };
    let mut desc = vec![WF_ID];
    desc.extend_from_slice(b"TESTSAVE");
    desc.extend_from_slice(&WF_LOADED.to_le_bytes());
    desc.extend_from_slice(&[WF_SIZE, 0x00, 0x00]);
    desc.extend_from_slice(&src.to_le_bytes());
    desc.push(ppu as u8);
    p.bytes_at(WF_DESC, &desc);
    p.poke(0x05f0, WF_ID).poke(0x05f1, 0xff); // the file list $E1F8 reads back
    p.lda(index);
    p.jsr_with_args(entry, WF_TEMPLATE, WF_DESC);
    if read_back {
        p.jsr_with_args(0xe1f8, WF_TEMPLATE, 0x05f0);
    }
    p.record_and_halt();
    assert!(p.0.len() < 0x100, "the caller has grown into its own answers");
    p.0
}

/// Run a write under ours and, if present, the real BIOS, and require the
/// registers, the zero page it works in and the WHOLE disk side to agree.
fn wf_both(prog: &[u8]) -> Option<(Outcome, Vec<u8>, Vec<u8>, Vec<u8>)> {
    let disk = a_disk()?;
    let frames = 3000; // a write is about 650 frames of disk under the original
    let (ours, mut nes) = run(&disk, &[], prog, frames);
    let zp: Vec<u8> = (0x00..=0x0fu16).map(|a| nes.peek(a)).collect();
    let loaded: Vec<u8> = (0..WF_SIZE as u16).map(|i| nes.peek(WF_LOADED + i)).collect();
    let image = nes.dbg_fds_image(0).to_vec();
    if let Some(real) = real_bios() {
        let (theirs, mut their_nes) = run(&disk, &real, prog, frames);
        assert!(theirs.reached, "the real BIOS never finished the test program");
        assert_eq!(ours, theirs, "ours (left) disagrees with the real BIOS (right)");
        let their_zp: Vec<u8> = (0x00..=0x0fu16).map(|a| their_nes.peek(a)).collect();
        assert_eq!(zp, their_zp, "$00-$0F: ours (left) against the real BIOS (right)");
        let theirs_img = their_nes.dbg_fds_image(0);
        assert_eq!(image.len(), theirs_img.len());
        let first = image.iter().zip(theirs_img).position(|(a, b)| a != b);
        assert_eq!(first, None, "the disk images part company at this byte");
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some((ours, zp, loaded, image))
}

#[test]
fn write_file_at_e239_appends_a_file_that_loads_back_and_matches_the_real_bios() {
    let Some((r, zp, loaded, _)) = wf_both(&wf_prog(0xe239, 0xff, true, None, false)) else {
        return skipped();
    };
    assert!(r.reached, "the write or the load never returned");
    assert_eq!(r.a, 0x00, "and the load found it");
    assert_eq!(r.y, 0x01, "exactly one file loaded");
    let want: Vec<u8> = (0..WF_SIZE).map(wf_byte).collect();
    assert_eq!(loaded, want, "what $E239 wrote is what $E1F8 reads");
    let _ = zp;
}

#[test]
fn write_file_reports_success_and_leaves_its_working_set() {
    let Some((r, zp, _, _)) = wf_both(&wf_prog(0xe239, 0xff, false, None, false)) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!((r.a, r.x, r.y), (0x00, 0x00, 0x00));
    assert_eq!(r.p & 0xc3, 0x42, "V and Z set, N and C clear");
    assert_eq!(r.p & 0x04, 0x00, "and interrupts enabled on the way out");
    assert_eq!(&zp[0..4], &[0x00, 0x05, 0x10, 0x05], "the two inline pointers");
    assert_eq!([zp[0x05], zp[0x07], zp[0x08], zp[0x09]], [0x02, 0x04, 0x00, 0xff]);
    assert_eq!([zp[0x0a], zp[0x0b]], [0x60, 0x05], "the source, one past its end");
    assert_eq!([zp[0x0c], zp[0x0d], zp[0x0e]], [0xff, 0xff, 0xff], "and a as it came in");
}

#[test]
fn e237_is_the_append_entry_whatever_a_says() {
    // `lda #$FF` and fall into $E239: the same disk, whatever a is.
    let Some((_, _, _, via_e237)) = wf_both(&wf_prog(0xe237, 0x00, false, None, false)) else {
        return skipped();
    };
    let Some((_, _, _, via_e239)) = wf_both(&wf_prog(0xe239, 0xff, false, None, false)) else {
        return skipped();
    };
    assert!(via_e237 == via_e239, "$E237 with a=0 should append exactly as $E239 with a=$FF");
}

#[test]
fn write_file_at_an_index_hides_the_files_after_it() {
    // Index 1: block 2 ends up saying two files, so whatever followed file 1
    // is still on the medium but no longer counted.
    let Some((r, _, _, image)) = wf_both(&wf_prog(0xe239, 0x01, false, None, false)) else {
        return skipped();
    };
    assert_eq!(r.a, 0x00);
    let count_at = image.windows(2).position(|w| w == [0x80, 0x02]).expect("block 2") + 2;
    assert_eq!(image[count_at], 0x02, "the count is the index plus one");
}

#[test]
fn write_file_from_ppu_memory_leaves_y_at_16() {
    let Some((r, _, _, _)) = wf_both(&wf_prog(0xe239, 0xff, false, None, true)) else {
        return skipped();
    };
    assert_eq!((r.a, r.y), (0x00, 0x10));
}

#[test]
fn write_file_with_the_wrong_disk_writes_nothing() {
    // The loader's codes, by template byte, and y says which byte. $08 holds
    // the code in BINARY, which only shows at byte 9: $10 in a, $0A in $08.
    let Some(disk) = a_disk() else { return skipped() };
    let before = nes_core::Nes::from_fds_hle(&disk).unwrap().dbg_fds_image(0).to_vec();
    for (at, code, binary) in [(0u8, 0x04u8, 0x04u8), (9, 0x10, 0x0a)] {
        let Some((r, zp, _, image)) = wf_both(&wf_prog(0xe239, 0xff, false, Some(at), false))
        else {
            return skipped();
        };
        assert_eq!((r.a, r.x, r.y), (code, code, at), "template byte {at}");
        assert_eq!(r.p & 0x41, 0x40, "V set, C clear");
        assert_eq!([zp[0x05], zp[0x07], zp[0x08]], [0x00, 0x01, binary], "template byte {at}");
        assert!(image == before, "and not a byte of the disk changed");
    }
}

// --- $E3E7, the caller's inline arguments and a disk check ------------------

/// Where the test's own `G` lives: `jsr $E3E7 : sta FETCH_BACK : rts`. The
/// store marks the way home: a disk returns to G and writes it, no disk skips
/// G and leaves it as planted. sta keeps the flags, so the recorder still sees
/// the routine's own.
const FETCH_G: u16 = 0x06c0;
const FETCH_BACK: u16 = 0x0714;

/// A caller for $E3E7, two levels deep: a stub at `at` does `jsr G` with
/// `args` after it, then jumps to the recorder, which is only reached if G's
/// return address was stepped over exactly those bytes. Everything compared
/// is planted first, and the stack pinned, so the two boots' leftovers stay
/// out of it.
fn fetch_prog(at: u16, a: u8, args: &[u8]) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.raw(&[0xa2, 0xef, 0x9a]); // ldx #$ef : txs
    for zp in 0x00..=0x06u16 {
        p.poke(zp, 0xcc);
    }
    p.poke(0x000e, 0x3c).poke(FETCH_BACK, 0xcc).poke(0x01eb, 0xcc);
    p.bytes_at(FETCH_G, &[0x20, 0xe7, 0xe3, 0x8d, FETCH_BACK as u8, (FETCH_BACK >> 8) as u8, 0x60]);
    // The stub's jmp target is the recorder, known once the fixed-size rest
    // is counted: five bytes a poke, the stub's own, the lda/ldx/ldy and jmp.
    let stub_len = 6 + args.len();
    let tail = 5 * stub_len + 6 + 4;
    let record = PROG + (p.0.len() + tail) as u16;
    let mut stub = vec![0x20, FETCH_G as u8, (FETCH_G >> 8) as u8];
    stub.extend_from_slice(args);
    stub.extend_from_slice(&[0x4c, record as u8, (record >> 8) as u8]);
    p.bytes_at(at, &stub);
    p.lda(a).ldx(0x55).ldy(0x66);
    p.raw(&[0x78, 0x4c, at as u8, (at >> 8) as u8]); // sei : jmp, so a cli shows
    assert_eq!(PROG + p.0.len() as u16, record, "the recorder moved");
    p.record_and_halt();
    p.0
}

fn fetch_watch() -> Vec<u16> {
    // $00-$06, the way home, and the byte the original's php leaves below the
    // stack: the stub's jsr takes SP to $ED and G's to $EB, so it lands at
    // $01EB.
    let mut w: Vec<u16> = (0x00..=0x06).collect();
    w.extend([FETCH_BACK, 0x01eb]);
    w
}

/// [`both_watching`], with the disk taken out after boot when `eject` is set.
fn fetch_both(prog: &[u8], eject: bool) -> Option<(Outcome, Vec<u8>)> {
    let disk = a_disk()?;
    let go = |bios: &[u8]| {
        let mut nes = boot_to_handover(&disk, bios);
        if eject {
            nes.fds_eject();
        }
        for (i, b) in prog.iter().enumerate() {
            nes.dbg_poke(PROG + i as u16, *b);
        }
        nes.dbg_poke(OUT_SENTINEL, 0);
        nes.dbg_set_pc(PROG);
        let stop = nes.dbg_frame() + 30;
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
        let mem: Vec<u8> = fetch_watch().iter().map(|a| nes.peek(*a)).collect();
        (out, mem)
    };
    let ours = go(&[]);
    if let Some(real) = real_bios() {
        let theirs = go(&real);
        assert!(theirs.0.reached, "the real BIOS never finished the test program");
        assert_eq!(ours, theirs, "ours (left) against the real BIOS (right)");
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some(ours)
}

#[test]
fn fetch_args_at_e3e7_takes_four_bytes_when_a_is_ff() {
    // Tobidase Daisakusen's call. G's jsr is at $0570, so its return address
    // is $0572, and the four bytes after it go to $00-$03.
    let prog = fetch_prog(0x0570, 0xff, &[0x11, 0x22, 0x33, 0x44]);
    let Some((r, m)) = fetch_both(&prog, false) else { return skipped() };
    assert!(r.reached, "G did not return past its four bytes");
    assert_eq!(m[0..4], [0x11, 0x22, 0x33, 0x44], "$00-$03 = the bytes after G's jsr");
    assert_eq!([m[4], m[5], m[6]], [0xea, 0x72, 0x05], "$04 = SP-1, $05/$06 = G's return");
    assert_eq!(m[7], 0x00, "it came back through G");
    assert_eq!((r.a, r.x, r.y), (0x00, 0x01, 0x04));
    assert_eq!(r.p & 0xc3, 0x03, "C and Z set, N and V clear: the a=0 of a disk");
}

#[test]
fn fetch_args_takes_two_bytes_otherwise_and_keeps_a_in_02() {
    // $FE as well: only $FF itself takes four.
    for a in [0x37u8, 0xfe] {
        let prog = fetch_prog(0x0570, a, &[0x11, 0x22]);
        let Some((r, m)) = fetch_both(&prog, false) else { return skipped() };
        assert!(r.reached, "a=${a:02x}: G did not return past its two bytes");
        assert_eq!(m[0..4], [0x11, 0x22, a, 0xcc], "$02 = a, $03 untouched");
        assert_eq!((r.a, r.y), (0x00, 0x02));
    }
}

#[test]
fn fetch_args_with_no_disk_returns_past_g_with_a_1() {
    // G is skipped: our return address is dropped and the one under it has
    // already been stepped, so G's caller gets the error past G's arguments.
    let prog = fetch_prog(0x0570, 0xff, &[0x11, 0x22, 0x33, 0x44]);
    let Some((r, m)) = fetch_both(&prog, true) else { return skipped() };
    assert!(r.reached, "the error did not land past G's arguments");
    assert_eq!(m[7], 0xcc, "and G never got control back");
    assert_eq!((r.a, r.x, r.y), (0x01, 0x01, 0x3c), "y comes from $0E");
    assert_eq!(r.p & 0x04, 0x00, "I cleared, though the call came in with it set");
}

#[test]
fn fetch_args_carries_into_the_high_byte() {
    // G's return address $04FD, plus four, is $0501.
    let prog = fetch_prog(0x04fb, 0xff, &[0x11, 0x22, 0x33, 0x44]);
    let Some((r, m)) = fetch_both(&prog, false) else { return skipped() };
    assert!(r.reached, "the carry was dropped and G returned into $04xx");
    assert_eq!(m[0..4], [0x11, 0x22, 0x33, 0x44]);
}

#[test]
fn fetch_args_is_cycle_exact() {
    // 107, +21 for four bytes, +1 for a pointer read across a page.
    let Some(disk) = a_disk() else { return skipped() };
    for (at, a, n, want) in [
        (0x0570u16, 0x00u8, 2usize, 107u64),
        (0x0570, 0xff, 4, 128),
        (0x04fb, 0xff, 4, 130),
    ] {
        let prog = fetch_prog(at, a, &[0x11, 0x22, 0x33, 0x44][..n]);
        let ours = cycles_of_call_in(&disk, &[], 0xe3e7, &prog);
        assert_eq!(ours, want, "G at ${at:04x}, a=${a:02x}");
        if let Some(real) = real_bios() {
            assert_eq!(ours, cycles_of_call_in(&disk, &real, 0xe3e7, &prog), "G at ${at:04x}");
        }
    }
}

// --- $F9, which every routine that moves the motor marks --------------------

/// Bit 7 of $F9 is ORed in by the spin-up of the loader and the writer, and
/// that value, not $FF, is what goes to $4026. Measured by a helper on three
/// loader callers and two writer callers, with $F9 planted at $00, $11 and
/// $7F: an OR, never a store.
#[test]
fn the_spin_up_marks_bit_7_of_f9() {
    let Some(disk) = a_disk() else { return skipped() };
    let want = wanted_file_id(&disk);
    let mut load = Prog::default();
    load.bytes_at(LF_TEMPLATE, &[0xff; 10])
        .bytes_at(LF_LIST, &[want, 0xff])
        .jsr_with_args(0xe1f8, LF_TEMPLATE, LF_LIST)
        .record_and_halt();
    let write = wf_prog(0xe239, 0xff, false, None, false);
    for (what, prog, frames) in [("$E1F8", load.0, 900u64), ("$E239", write, 3000)] {
        let (r, mut nes) = run_poked(&disk, &[], &prog, frames, &[(0x00f9, 0x11)]);
        assert!(r.reached, "{what} never returned");
        assert_eq!(nes.peek(0x00f9), 0x91, "{what}: $F9 is ORed with $80");
        if let Some(real) = real_bios() {
            let (_, mut theirs) = run_poked(&disk, &real, &prog, frames, &[(0x00f9, 0x11)]);
            assert_eq!(theirs.peek(0x00f9), 0x91, "{what}: and the original agrees");
        }
    }
}

// --- $E32A, a directory of the whole side -----------------------------------

const DI_BUF: u16 = 0x0300;

/// `jsr $E32A` with the buffer inline, after painting $00-$0F and the
/// buffer's page so every byte compared is one the test planted.
fn di_prog() -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.raw(&[0xa2, 0x0f, 0xa9, 0xcc, 0x95, 0x00, 0xca, 0x10, 0xfb]); // $00-$0F
    p.raw(&[0xa2, 0x00, 0xa9, 0xa5, 0x9d, 0x00, 0x03, 0xe8, 0xd0, 0xfa]); // $0300-$03FF
    p.poke(0x000e, 0x3c);
    p.raw(&[0x78]); // sei, so the cli on the way out shows
    p.jsr(0xe32a);
    p.raw(&DI_BUF.to_le_bytes());
    p.record_and_halt();
    p.0
}

/// What a directory of side 0 should hold, worked out from the .fds file
/// itself rather than from either BIOS: info bytes, count, ID and name per
/// file, and the total of size + 261, high byte first.
fn di_expected(disk: &[u8]) -> Vec<u8> {
    let side = if disk.starts_with(b"FDS\x1a") { &disk[16..] } else { disk };
    let mut out = side[15..25].to_vec(); // block 1 bytes $0F-$18, the type byte being 0
    let mut at = 56;
    assert_eq!(side[at], 0x02, "block 2 follows block 1");
    let n = side[at + 1];
    out.push(n);
    at += 2;
    let mut total: u16 = 0;
    for _ in 0..n {
        assert_eq!(side[at], 0x03, "a header");
        out.extend_from_slice(&side[at + 2..at + 11]);
        let size = u16::from_le_bytes([side[at + 13], side[at + 14]]);
        total = total.wrapping_add(size).wrapping_add(261);
        at += 16 + 1 + size as usize;
    }
    out.extend_from_slice(&total.to_be_bytes());
    out
}

fn di_both(eject: bool) -> Option<(Outcome, Vec<u8>, Vec<u8>)> {
    let disk = a_disk()?;
    let prog = di_prog();
    let go = |bios: &[u8]| {
        let mut nes = boot_to_handover(&disk, bios);
        if eject {
            nes.fds_eject();
        }
        nes.dbg_poke(0x00f9, 0x11);
        nes.dbg_poke(0x00fa, 0x0d); // each BIOS boots to its own; $2F expected
        for (i, b) in prog.iter().enumerate() {
            nes.dbg_poke(PROG + i as u16, *b);
        }
        nes.dbg_poke(OUT_SENTINEL, 0);
        nes.dbg_set_pc(PROG);
        let stop = nes.dbg_frame() + 1500; // a whole side under the original
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
        let mut zp: Vec<u8> = (0x00..=0x0fu16).map(|a| nes.peek(a)).collect();
        zp.extend([nes.peek(0x00f9), nes.peek(0x00fa), nes.peek(0x0101)]);
        let buf: Vec<u8> = (0..0x100u16).map(|i| nes.peek(DI_BUF + i)).collect();
        (out, zp, buf)
    };
    let ours = go(&[]);
    if let Some(real) = real_bios() {
        let theirs = go(&real);
        assert!(theirs.0.reached, "the real BIOS never finished the test program");
        assert_eq!(ours.0, theirs.0, "registers: ours (left) against the real BIOS (right)");
        assert_eq!(ours.1, theirs.1, "$00-$0F, $F9, $FA, $0101: ours (left), real (right)");
        assert_eq!(ours.2, theirs.2, "the buffer: ours (left) against the real BIOS (right)");
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some(ours)
}

#[test]
fn disk_info_at_e32a_writes_a_directory_of_the_side_and_matches_the_real_bios() {
    let Some(disk) = a_disk() else { return skipped() };
    let Some((r, zp, buf)) = di_both(false) else { return skipped() };
    assert!(r.reached, "it never returned past its inline word");
    let want = di_expected(&disk);
    assert_eq!(buf[..want.len()], want[..], "the directory, against the .fds file");
    assert!(buf[want.len()..].iter().all(|&b| b == 0xa5), "and nothing past it");
    assert_eq!((r.a, r.x, r.y), (0x00, 0x00, 0x02));
    assert_eq!(r.p & 0xc7, 0x42, "V and Z set, N, C and I clear");
    let n = want[10] as u16;
    let last = DI_BUF + 10 + 9 * n;
    assert_eq!([zp[0x00], zp[0x01]], DI_BUF.to_le_bytes(), "$00/$01 = the buffer");
    let total = u16::from_be_bytes([want[want.len() - 2], want[want.len() - 1]]);
    assert_eq!([zp[0x02], zp[0x03]], total.to_le_bytes(), "$02/$03 = the total");
    assert_eq!([zp[0x05], zp[0x06]], [0x02, 0x00], "no retry, no files left");
    assert_eq!([zp[0x0a], zp[0x0b]], last.to_le_bytes(), "$0A/$0B on the last entry byte");
    assert_eq!([zp[0x0c], zp[0x0d], zp[0x0e]], [0xff, 0xff, 0x3c]);
    assert_eq!([zp[0x10], zp[0x11]], [0x91, 0x2f], "$F9 marked, $FA coasting, mirroring kept");
}

#[test]
fn disk_info_with_no_disk_touches_nothing_but_its_frame() {
    let Some((r, zp, buf)) = di_both(true) else { return skipped() };
    assert!(r.reached, "the error did not land past the inline word");
    assert_eq!((r.a, r.x, r.y), (0x01, 0x01, 0x3c), "y comes from $0E");
    assert_eq!(r.p & 0x05, 0x01, "C set, I clear");
    assert_eq!(zp[0x02], 0x00, "$02 is cleared at entry");
    assert_eq!(&zp[0x07..0x0e], &[0xcc; 7], "and $07-$0D are not touched");
    assert_eq!([zp[0x10], zp[0x11]], [0x11, 0x0d], "nor $F9 or $FA: the motor never moved");
    assert!(buf.iter().all(|&b| b == 0xa5), "nor the buffer");
}

#[test]
fn disk_info_is_no_slower_than_the_original() {
    // A whole side is millions of cycles under the original, far past what
    // cycles_of_call_in waits for, so this counts on its own deadline.
    let Some(disk) = a_disk() else { return skipped() };
    let Some(real) = real_bios() else { return skipped() };
    let prog = di_prog();
    let count = |bios: &[u8]| {
        let mut nes = boot_to_handover(&disk, bios);
        for (i, b) in prog.iter().enumerate() {
            nes.dbg_poke(PROG + i as u16, *b);
        }
        nes.dbg_set_pc(PROG);
        while nes.dbg_pc() != 0xe32a {
            nes.step();
        }
        let sp_in = nes.cpu.sp;
        let start = nes.dbg_cycles();
        loop {
            nes.step();
            if nes.dbg_pc() < BIOS_BASE && nes.cpu.sp >= sp_in {
                break;
            }
            assert!(nes.dbg_cycles() - start < 1500 * 29_781, "it never returned");
        }
        nes.dbg_cycles() - start
    };
    let (ours, theirs) = (count(&[]), count(&real));
    assert!(ours <= theirs, "ours {ours} against the original's {theirs}");
}

// --- $E305, setting the side's file count -----------------------------------

/// `jsr $E305` with the any-disk template (or one byte of it spoiled) and a
/// as given; when a is $FF the second word, `word2`, follows.
fn sc_prog(a: u8, word2: u16, bad_at: Option<u8>) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.raw(&[0xa2, 0x0f, 0xa9, 0xcc, 0x95, 0x00, 0xca, 0x10, 0xfb]); // $00-$0F
    p.raw(&[0xa9, 0xff, 0xa2, 0x09, 0x9d, 0x00, 0x05, 0xca, 0x10, 0xfa]); // template
    if let Some(i) = bad_at {
        p.poke(WF_TEMPLATE + i as u16, 0x77);
    }
    p.poke(0x000e, 0x3c);
    p.lda(a);
    p.raw(&[0x78]); // sei, so the cli on the way out shows
    p.jsr(0xe305);
    p.raw(&WF_TEMPLATE.to_le_bytes());
    if a == 0xff {
        p.raw(&word2.to_le_bytes());
    }
    p.record_and_halt();
    p.0
}

/// Registers, $00-$0F with $F9 and $FA after them, and side 0 whole, under
/// ours and the real BIOS, which have to agree on all of it.
fn sc_both(prog: &[u8], eject: bool) -> Option<(Outcome, Vec<u8>, Vec<u8>)> {
    let disk = a_disk()?;
    let go = |bios: &[u8]| {
        let mut nes = boot_to_handover(&disk, bios);
        if eject {
            nes.fds_eject();
        }
        nes.dbg_poke(0x00f9, 0x11);
        nes.dbg_poke(0x00fa, 0x0d); // each BIOS boots to its own
        for (i, b) in prog.iter().enumerate() {
            nes.dbg_poke(PROG + i as u16, *b);
        }
        nes.dbg_poke(OUT_SENTINEL, 0);
        nes.dbg_set_pc(PROG);
        let stop = nes.dbg_frame() + 600; // about 60 frames a try under the original
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
        let mut zp: Vec<u8> = (0x00..=0x0fu16).map(|a| nes.peek(a)).collect();
        zp.extend([nes.peek(0x00f9), nes.peek(0x00fa)]);
        (out, zp, nes.dbg_fds_image(0).to_vec())
    };
    let ours = go(&[]);
    if let Some(real) = real_bios() {
        let theirs = go(&real);
        assert!(theirs.0.reached, "the real BIOS never finished the test program");
        assert_eq!(ours.0, theirs.0, "registers: ours (left) against the real BIOS (right)");
        assert_eq!(ours.1, theirs.1, "$00-$0F, $F9, $FA: ours (left), real (right)");
        let first = ours.2.iter().zip(&theirs.2).position(|(a, b)| a != b);
        assert_eq!(first, None, "the disk images part company at this byte");
        assert_eq!(ours.2.len(), theirs.2.len());
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some(ours)
}

/// Side 0's file count and boot read file code, from the .fds file.
fn sc_side(disk: &[u8]) -> (u8, u8) {
    let side = if disk.starts_with(b"FDS\x1a") { &disk[16..] } else { disk };
    (side[57], side[25])
}

/// The count block 2 holds on a drive image: the byte after its $80 $02.
fn sc_count_on(image: &[u8]) -> u8 {
    image[image.windows(2).position(|w| w == [0x80, 0x02]).expect("block 2") + 2]
}

#[test]
fn set_count_at_e305_rewrites_block_2_and_matches_the_real_bios() {
    let Some(disk) = a_disk() else { return skipped() };
    let (n, boot) = sc_side(&disk);
    let Some((r, zp, image)) = sc_both(&sc_prog(n - 1, 0, None), false) else {
        return skipped();
    };
    assert!(r.reached, "it never returned past its template word");
    assert_eq!(sc_count_on(&image), n - 1, "block 2 says one file fewer");
    let before = nes_core::Nes::from_fds_hle(&disk).unwrap().dbg_fds_image(0).to_vec();
    let changed = image.iter().zip(&before).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 1, "and that byte is the only one the medium changed in");
    assert_eq!((r.a, r.x, r.y), (0x00, 0x00, 0x00));
    assert_eq!(r.p & 0xc7, 0x42, "V and Z set, N, C and I clear");
    assert_eq!(&zp[0..3], &[0x00, 0x05, n - 1], "$00/$01 the template, $02 the count");
    assert_eq!(&zp[5..9], &[0x02, n - 1, 0x02, boot], "$05-$08");
    assert_eq!([zp[0x10], zp[0x11]], [0x91, 0x2f], "$F9 marked, $FA coasting");
}

#[test]
fn set_count_takes_its_count_from_a_second_word_when_a_is_ff() {
    // A count of 2, because $02 is the JAM opcode: a return two bytes early,
    // over the first word only, lands on it and halts.
    let Some(disk) = a_disk() else { return skipped() };
    assert!(sc_side(&disk).0 > 2, "this needs a side with more than two files");
    let Some((r, zp, image)) = sc_both(&sc_prog(0xff, 0x5a02, None), false) else {
        return skipped();
    };
    assert!(r.reached, "it did not step over both words");
    assert_eq!(r.a, 0x00);
    assert_eq!(sc_count_on(&image), 0x02, "the low byte of word 2 is the count");
    assert_eq!([zp[2], zp[3]], [0x02, 0x5a], "and word 2 is left in $02/$03");
}

#[test]
fn set_count_with_the_wrong_disk_writes_nothing() {
    // The loader's codes, by template byte; $08 holds the code in binary,
    // which only differs at byte 9.
    let Some(disk) = a_disk() else { return skipped() };
    let before = nes_core::Nes::from_fds_hle(&disk).unwrap().dbg_fds_image(0).to_vec();
    for (at, code, binary) in [(0u8, 0x04u8, 0x04u8), (9, 0x10, 0x0a)] {
        let Some((r, zp, image)) = sc_both(&sc_prog(0x01, 0, Some(at)), false) else {
            return skipped();
        };
        assert_eq!((r.a, r.x, r.y), (code, code, at), "template byte {at}");
        assert_eq!(r.p & 0x41, 0x40, "V set, C clear");
        assert_eq!([zp[0x05], zp[0x08]], [0x00, binary], "template byte {at}");
        assert!(image == before, "and not a byte of the disk changed");
    }
}

#[test]
fn set_count_with_no_disk_returns_before_the_motor() {
    let Some((r, zp, _)) = sc_both(&sc_prog(0x01, 0, None), true) else { return skipped() };
    assert!(r.reached);
    assert_eq!((r.a, r.x, r.y), (0x01, 0x01, 0x3c), "y comes from $0E");
    assert_eq!(r.p & 0x05, 0x00, "C and I clear");
    assert_eq!(zp[0x07], 0x00, "$07 cleared");
    assert_eq!([zp[0x10], zp[0x11]], [0x11, 0x0d], "$F9 and $FA as found");
}

// --- $E445 $E484 $E68F $E4A0 $E4F9 $E778, the loader in pieces --------------

/// Tobidase Daisakusen's own loader, rebuilt at $06A0: G takes the template
/// and list inline through $E3E7, pushes $0101 and calls G2, which chains the
/// primitives over every file block 2 counts. An error unwinds to the frame
/// $E3E7 left, which is G2's return into G, exactly as in the game.
const PF_G: u16 = 0x06a0;
const PF_TEMPLATE: u16 = 0x0500;
const PF_LIST: u16 = 0x0510;

/// `list` is the file list: one ID then $FF, or $FF alone for the boot set.
/// With `whole` clear, G2 stops after $E445 and $E484, so their own exit
/// state is what gets compared rather than $E778's.
fn pf_prog(list: &[u8], addr: u16, bad_at: Option<u8>, whole: bool) -> Vec<u8> {
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.raw(&[0xa2, 0x0f, 0xa9, 0xcc, 0x95, 0x00, 0xca, 0x10, 0xfb]); // $00-$0F
    p.raw(&[0xa9, 0xff, 0xa2, 0x09, 0x9d, 0x00, 0x05, 0xca, 0x10, 0xfa]); // template
    if let Some(i) = bad_at {
        p.poke(PF_TEMPLATE + i as u16, 0x77);
    }
    p.bytes_at(PF_LIST, list);
    // Scribble the destination, so a load that stores nothing cannot pass.
    p.raw(&[0xa2, 0x1f, 0xa9, SCRIBBLE, 0x9d, addr as u8, (addr >> 8) as u8, 0xca, 0x10, 0xfa]);
    p.poke(0x000e, 0x00).poke(0x0101, 0x80);
    p.jsr(PF_G);
    p.raw(&PF_TEMPLATE.to_le_bytes());
    p.raw(&PF_LIST.to_le_bytes());
    p.record_and_halt();
    assert!(p.0.len() <= (PF_G - PROG) as usize, "the caller has grown into G");
    p.0.resize((PF_G - PROG) as usize, 0x00);
    p.raw(&[
        0xa9, 0xff, 0x20, 0xe7, 0xe3, // G: lda #$FF : jsr $E3E7
        0xad, 0x01, 0x01, 0x48, // lda $0101 : pha
        0x20, 0xc0, 0x06, // jsr G2
        0x68, 0x8d, 0x01, 0x01, // pla : sta $0101
        0xa4, 0x0e, 0x8a, 0x60, // ldy $0E : txa : rts
    ]);
    p.0.resize((0x06c0 - PROG) as usize, 0x00);
    if !whole {
        p.raw(&[0x20, 0x45, 0xe4, 0x20, 0x84, 0xe4, 0x60]); // G2: $E445, $E484, rts
        return p.0;
    }
    p.raw(&[
        0x20, 0x45, 0xe4, 0x20, 0x84, 0xe4, // G2: jsr $E445 : jsr $E484
        0xa9, 0x03, 0x20, 0x8f, 0xe6, // lda #3 : jsr $E68F
        0x20, 0xa0, 0xe4, 0x20, 0xf9, 0xe4, // jsr $E4A0 : jsr $E4F9
        0xc6, 0x06, 0xd0, 0xf1, // dec $06 : bne back to the lda #3
        0x20, 0x78, 0xe7, 0x60, // jsr $E778 : rts
    ]);
    p.0
}

/// Registers, $00-$0F with $F9, $FA and $0101 after them, and the 32 bytes at
/// `addr`, under ours and the real BIOS, which have to agree on all of it.
fn pf_both(prog: &[u8], addr: u16) -> Option<(Outcome, Vec<u8>, Vec<u8>)> {
    let disk = a_disk()?;
    let go = |bios: &[u8]| {
        let (out, mut nes) =
            run_poked(&disk, bios, prog, 1500, &[(0x00f9, 0x11), (0x00fa, 0x0d)]);
        let mut zp: Vec<u8> = (0x00..=0x0fu16).map(|a| nes.peek(a)).collect();
        zp.extend([nes.peek(0x00f9), nes.peek(0x00fa), nes.peek(0x0101)]);
        let got: Vec<u8> = (0..32u16).map(|i| nes.peek(addr.wrapping_add(i))).collect();
        (out, zp, got)
    };
    let ours = go(&[]);
    if let Some(real) = real_bios() {
        let theirs = go(&real);
        assert!(theirs.0.reached, "the real BIOS never finished the test program");
        assert_eq!(ours.0, theirs.0, "registers: ours (left) against the real BIOS (right)");
        assert_eq!(ours.1, theirs.1, "$00-$0F, $F9, $FA, $0101: ours (left), real (right)");
        assert_eq!(ours.2, theirs.2, "the loaded bytes: ours (left), real (right)");
    } else {
        eprintln!("note: no dumps/fds/disksys.rom, so this is not held against the oracle");
    }
    Some(ours)
}

#[test]
fn the_loader_in_pieces_loads_a_file_as_tobidase_does_and_matches_the_real_bios() {
    let Some(disk) = a_disk() else { return skipped() };
    let (want, addr, body) = wanted_file(&disk);
    let Some((r, zp, got)) = pf_both(&pf_prog(&[want, 0xff], addr, None, true), addr) else {
        return skipped();
    };
    assert!(r.reached, "the chain never came back through G");
    assert_eq!(got, body[..32], "the file is over the scribble at ${addr:04x}");
    assert_eq!(r.a, 0x00, "$E778's x, through G's txa");
    assert!(r.y >= 1, "y = $0E, counted by $E4A0 as it matched");
    assert_eq!(zp[0x06], 0x00, "every counted file walked");
    assert_eq!(zp[0x07], 0x04, "the last block a body");
    assert_eq!(zp[0x11], 0x2f, "$E778 left $FA coasting: ($0D & 9) | $26");
    assert_eq!(zp[0x12], 0x80, "and G put $0101 back");
}

#[test]
fn a_wrong_disk_unwinds_to_the_frame_with_the_loaders_code() {
    // $E445 has no retry, and its error does not return to G2: S goes back to
    // $04 and the rts lands in G after its jsr G2, with a = x = the code.
    let Some(disk) = a_disk() else { return skipped() };
    let (want, addr, _) = wanted_file(&disk);
    for (at, code) in [(0u8, 0x04u8), (9, 0x10)] {
        let Some((r, zp, got)) = pf_both(&pf_prog(&[want, 0xff], addr, Some(at), true), addr) else {
            return skipped();
        };
        assert!(r.reached, "template byte {at}: the error did not land back in G");
        assert_eq!(r.a, code, "template byte {at}");
        assert_eq!(zp[0x07], 0x01, "stopped in block 1");
        assert!(got.iter().all(|&b| b == SCRIBBLE), "and nothing was loaded");
    }
}

#[test]
fn the_loader_in_pieces_takes_the_boot_set_when_the_list_starts_with_ff() {
    // $E4A0's boot mode: a file loads if its ID is at most the boot read file
    // code that $E445 left in $08.
    let Some(disk) = a_disk() else { return skipped() };
    let (_, addr, _) = wanted_file(&disk);
    let Some((r, zp, _)) = pf_both(&pf_prog(&[0xff], addr, None, true), addr) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(r.a, 0x00);
    assert!(zp[0x0e] >= 1, "the boot set is at least one file");
}

#[test]
fn spin_up_and_count_leave_their_own_exit_state() {
    // Stopping after $E484 compares what $E445 and $E484 leave, $FA among
    // it, rather than what $E778 then coasts over.
    let Some(disk) = a_disk() else { return skipped() };
    let (want, addr, _) = wanted_file(&disk);
    let Some((r, zp, _)) = pf_both(&pf_prog(&[want, 0xff], addr, None, false), addr) else {
        return skipped();
    };
    assert!(r.reached);
    assert_eq!(zp[0x07], 0x02, "block 2 the last opened");
    assert_eq!(zp[0x11], 0x2d, "$FA disarmed: ($0D & 8) | $25, then $40 and $80 off");
    assert_eq!(r.y, 0x00, "$0E: nothing loaded yet");
}

#[test]
fn coast_at_e778_is_cycle_exact() {
    let Some(disk) = a_disk() else { return skipped() };
    let ours = cycles_of_call(&disk, &[], 0xe778);
    assert_eq!(ours, 29);
    if let Some(real) = real_bios() {
        assert_eq!(ours, cycles_of_call(&disk, &real, 0xe778));
    }
}

// --- what the BIOS leaves for the game at handover --------------------------

/// The PPU and its shadows as the original hands them over, the same on every
/// disk sampled. Games lean on them: Zelda 2 takes PPUCTRL bit 4 from $FF and
/// never sets it, so under a BIOS that left $00 its whole screen drew from the
/// wrong pattern table; error screens print with tile $24 as the blank.
#[test]
fn handover_leaves_the_ppu_and_its_shadows_as_the_original_does() {
    let Some(disk) = a_disk() else { return skipped() };
    let look = |bios: &[u8]| {
        let mut nes = boot_to_handover(&disk, bios);
        let zp: Vec<u8> = [0xf9u16, 0xfa, 0xfc, 0xfd, 0xfe, 0xff].iter().map(|a| nes.peek(*a)).collect();
        (zp, nes.dbg_ppu_ctrl(), nes.dbg_ppu_mask(), nes.dbg_ciram()[..0x400].to_vec())
    };
    let (zp, ctrl, mask, nt) = look(&[]);
    assert_eq!(zp, [0xff, 0x2f, 0x00, 0x00, 0x06, 0x10], "$F9 $FA $FC $FD $FE $FF");
    assert_eq!((ctrl, mask), (0x10, 0x06), "background from $1000, rendering off");
    assert!(nt[..960].iter().all(|&t| t == 0x24), "nametable A is all the blank tile");
    assert!(nt[960..].iter().all(|&t| t == 0x00), "with attributes zero");
    if let Some(real) = real_bios() {
        assert_eq!(look(&real), (zp, ctrl, mask, nt), "ours (left) against the original");
    }
}

/// The font is read straight out of the ROM: $E001-$E148, 41 glyphs, one
/// plane, in the licence-file order. The art is ours, so the glyph bytes are
/// NOT compared with the original's; where the glyphs sit is.
#[test]
fn the_rom_font_sits_where_games_read_it() {
    let Some(disk) = a_disk() else { return skipped() };
    let rom = |bios: &[u8]| -> Vec<u8> {
        let mut nes = boot_to_handover(&disk, bios);
        (0xe000..0xe149u16).map(|a| nes.peek(a)).collect()
    };
    let ours = rom(&[]);
    let glyph = |t: usize| -> Vec<u8> { ours[1 + t * 8..9 + t * 8].to_vec() };
    assert_eq!(ours[0], 0x00, "$E000 reads zero: Super Lode Runner sums it");
    assert!(glyph(0x24).iter().all(|&b| b == 0), "tile $24 is the blank");
    let shapes: Vec<Vec<u8>> = (0..0x29).filter(|&t| t != 0x24).map(glyph).collect();
    assert!(shapes.iter().all(|g| g.iter().any(|&b| b != 0)), "every other glyph has ink");
    for (i, a) in shapes.iter().enumerate() {
        assert!(shapes[i + 1..].iter().all(|b| b != a), "glyph {i} is drawn twice");
    }
    if let Some(real) = real_bios() {
        let theirs = rom(&real);
        assert_eq!(theirs[0], ours[0], "$E000 as the original's");
        assert!(theirs[1 + 0x24 * 8..9 + 0x24 * 8].iter().all(|&b| b == 0), "its $24 is blank too");
    }
}

// --- $EBAF's mode bits -------------------------------------------------------

/// $EBAF with every low nibble of a, into pattern RAM at $1000 so both reads
/// and writes are observable. Section-Z calls it with a=$04 to expand a 1-bit
/// font into two planes; the old routine ignored the mode bits and copied 16
/// bytes regardless, and the old test only ever passed a=$00.
#[test]
fn vram_upload_honours_its_mode_bits_as_the_original_does() {
    const SRC: u16 = 0x0760;
    let Some(disk) = a_disk() else { return skipped() };
    let Some(real) = real_bios() else { return skipped() };
    let pattern: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(29).wrapping_add(5)).collect();
    for low in 0..16u8 {
        let a = 0x40 | low; // VRAM $1040
        let mut p = Prog::default();
        p.poke(0x2000, 0x00).poke(0x00ff, 0x00);
        p.raw(&[0xa2, 0x07, 0xa9, 0xcc, 0x95, 0x00, 0xca, 0x10, 0xfb]); // $00-$07
        p.bytes_at(SRC, &pattern).lda(a).ldy(0x10).ldx(0x01);
        p.jsr(0xebaf);
        p.raw(&SRC.to_le_bytes());
        p.record_and_halt();
        let go = |bios: &[u8]| {
            let (r, mut nes) = run(&disk, bios, &p.0, 120);
            let zp: Vec<u8> = (0..8u16).map(|i| nes.peek(i)).collect();
            let src: Vec<u8> = (0..16u16).map(|i| nes.peek(SRC + i)).collect();
            (r, zp, src, nes.dbg_chr_ram()[0x1040..0x1050].to_vec())
        };
        let ours = go(&[]);
        assert!(ours.0.reached, "a=${a:02x}: it never returned");
        assert_eq!(ours, go(&real), "a=${a:02x}: ours (left) against the original (right)");
    }
}

// --- the IRQ handler, by $0101 ----------------------------------------------

/// `jsr wait`, where wait pushes a fake interrupt frame returning to RESUME
/// and jumps into $E1C7, as a byte IRQ would arrive while a game's loader
/// waits. $40-$7F hands the byte back and returns from `wait` itself; the
/// others rti to RESUME. Super Lode Runner loads every byte this way.
#[test]
fn the_irq_handler_serves_every_range_of_0101_as_the_original_does() {
    let Some(disk) = a_disk() else { return skipped() };
    let Some(real) = real_bios() else { return skipped() };
    const WAIT: u16 = 0x06c0;
    for sel in [0x00u8, 0x01, 0x3f, 0x40, 0x7f, 0x80, 0xbf] {
        let mut p = Prog::default();
        p.poke(0x2000, 0x00).poke(0x0101, sel);
        p.raw(&[0x78]); // sei: only the planted interrupt
        p.ldx(0x66).ldy(0x77);
        p.jsr(WAIT);
        // Both ways home land here: $40 returns from WAIT to just after its
        // jsr, the rest rti to RESUME, which is this same address.
        let resume = PROG + p.0.len() as u16;
        p.record_and_halt();
        p.0.resize((WAIT - PROG) as usize, 0x00);
        p.raw(&[0xa9, (resume >> 8) as u8, 0x48, 0xa9, resume as u8, 0x48]);
        p.raw(&[0xa9, 0x5a, 0x08, 0x4c, 0xc7, 0xe1]); // lda #$5A : php : jmp $E1C7
        let go = |bios: &[u8]| {
            let (r, mut nes) = run(&disk, bios, &p.0, 30);
            (r, nes.peek(0x0101), nes.cpu.sp)
        };
        let (mut ours, mut theirs) = (go(&[]), go(&real));
        assert!(ours.0.reached, "$0101=${sel:02x}: it never came back");
        if sel & 0xc0 == 0x40 {
            // The byte handed back is whatever $4031 holds, and with no
            // transfer running that is each boot's leftover. Each side has to
            // hand it back in both a and x; then it and its N and Z are set
            // aside and everything else must agree.
            for (who, r) in [("ours", &mut ours), ("original", &mut theirs)] {
                assert_eq!(r.0.a, r.0.x, "$0101=${sel:02x}, {who}: the byte in both a and x");
                (r.0.a, r.0.x, r.0.p) = (0, 0, r.0.p & !0x82);
            }
        }
        assert_eq!(ours, theirs, "$0101=${sel:02x}: ours (left), original (right)");
    }
}

// --- the warm boot ------------------------------------------------------------

/// A game that jumps to reset with $0102/$0103 = $35/$AC is let straight back
/// in through $DFFC with no disk read; Dig Dug II's loader does exactly this.
/// Any other signature is a cold boot, which reads the disk again and takes
/// far longer than three frames.
#[test]
fn reset_with_the_warm_signature_returns_to_the_game_without_the_disk() {
    let Some(disk) = a_disk() else { return skipped() };
    let back_within = |bios: &[u8], sig: u8| -> Option<u64> {
        let mut nes = boot_to_handover(&disk, bios);
        let entry = u16::from(nes.peek(VEC_RESET)) | (u16::from(nes.peek(VEC_RESET + 1)) << 8);
        nes.dbg_poke(0x0102, 0x35);
        nes.dbg_poke(0x0103, sig);
        nes.dbg_set_pc(0xee24);
        let start = nes.dbg_frame();
        while nes.dbg_frame() < start + 3 {
            nes.step();
            if nes.dbg_pc() == entry {
                return Some(nes.dbg_frame() - start);
            }
        }
        None
    };
    assert!(back_within(&[], 0xac).is_some(), "the warm signature should skip the disk");
    assert!(back_within(&[], 0x53).is_some(), "and so should $53, what it leaves behind");
    assert!(back_within(&[], 0x00).is_none(), "anything else is a cold boot");
    if let Some(real) = real_bios() {
        for sig in [0xacu8, 0x53, 0x00] {
            assert_eq!(
                back_within(&[], sig).is_some(),
                back_within(&real, sig).is_some(),
                "signature ${sig:02x}: ours against the original"
            );
        }
    }
}

// --- the disk as a battery save ----------------------------------------------

/// The `.fds` side the battery holds for side 0, and where a file of `id`
/// sits in it: its header, then its body.
fn battery_file(battery: &[u8], id: u8) -> Option<Vec<u8>> {
    let side = &battery[..65500];
    let mut at = 58; // block 1 is 56 bytes, block 2 is 2
    while at + 17 < side.len() && side[at] == 0x03 {
        let size = u16::from_le_bytes([side[at + 13], side[at + 14]]) as usize;
        if side[at + 2] == id && &side[at + 3..at + 11] == b"TESTSAVE" {
            return Some(side[at + 17..at + 17 + size].to_vec());
        }
        at += 16 + 1 + size;
    }
    None
}

/// Run `prog` from handover under our BIOS with whole frames, as a frontend
/// does: the battery buffer is synced at each frame start.
fn run_frames(nes: &mut nes_core::Nes, prog: &[u8], frames: u64) -> bool {
    for (i, b) in prog.iter().enumerate() {
        nes.dbg_poke(PROG + i as u16, *b);
    }
    nes.dbg_poke(OUT_SENTINEL, 0);
    nes.dbg_set_pc(PROG);
    for _ in 0..frames {
        nes.step_frame();
        if nes.peek(OUT_SENTINEL) == DONE {
            nes.step_frame(); // one more frame start, which takes the copy
            return true;
        }
    }
    false
}

#[test]
fn a_file_written_to_the_disk_is_in_the_battery_save() {
    let Some(disk) = a_disk() else { return skipped() };
    let mut nes = boot_to_handover(&disk, &[]);
    let before = nes.battery_save().expect("an FDS disk has a battery save").to_vec();
    // Absolute, not a multiple: frontends restore a save only at exactly the
    // size reported, so this length is what keeps saves already on devices.
    assert_eq!(before.len(), 65500 * nes.fds_side_count(), "65,500 bytes a side");
    assert!(battery_file(&before, WF_ID).is_none(), "the disk has no such file yet");
    assert!(run_frames(&mut nes, &wf_prog(0xe239, 0xff, false, None, false), 3000));
    let after = nes.battery_save().unwrap().to_vec();
    let want: Vec<u8> = (0..WF_SIZE).map(wf_byte).collect();
    assert_eq!(battery_file(&after, WF_ID), Some(want), "the saved file, in the .fds side");
}

#[test]
fn a_battery_save_handed_back_is_the_disk_the_game_finds() {
    // Write a file, take the battery buffer as the frontend would, then give
    // it to a fresh machine before its first frame and have the BIOS load the
    // file back off that machine's disk.
    let Some(disk) = a_disk() else { return skipped() };
    let mut first = boot_to_handover(&disk, &[]);
    assert!(run_frames(&mut first, &wf_prog(0xe239, 0xff, false, None, false), 3000));
    let saved = first.battery_save().unwrap().to_vec();

    let mut fresh = nes_core::Nes::from_fds_hle(&disk).unwrap();
    fresh.battery_save().unwrap().copy_from_slice(&saved);
    fresh.step_frame(); // the first frame start takes the save
    let mut p = Prog::default();
    p.poke(0x2000, 0x00);
    p.raw(&[0xa9, 0xff, 0xa2, 0x09, 0x9d, 0x00, 0x05, 0xca, 0x10, 0xfa]); // any disk
    p.poke(0x05f0, WF_ID).poke(0x05f1, 0xff);
    p.bytes_at(WF_LOADED, &[0xa5; WF_SIZE as usize]);
    p.jsr_with_args(0xe1f8, WF_TEMPLATE, 0x05f0);
    p.record_and_halt();
    // Handover first, by stepping as the boot test does, then the load.
    let deadline = fresh.dbg_frame() + 1500;
    while fresh.dbg_frame() < deadline {
        let pc = fresh.dbg_pc();
        let v = u16::from(fresh.peek(VEC_RESET)) | (u16::from(fresh.peek(VEC_RESET + 1)) << 8);
        if pc < BIOS_BASE && pc == v {
            break;
        }
        fresh.step();
    }
    assert!(run_frames(&mut fresh, &p.0, 900), "the load never returned");
    let got: Vec<u8> = (0..WF_SIZE as u16).map(|i| fresh.peek(WF_LOADED + i)).collect();
    let want: Vec<u8> = (0..WF_SIZE).map(wf_byte).collect();
    assert_eq!(fresh.peek(OUT), 0x00, "the load succeeded");
    assert_eq!(got, want, "and found the file the other machine saved");
}

#[test]
fn loading_a_savestate_brings_its_disk_into_the_battery_save() {
    let Some(disk) = a_disk() else { return skipped() };
    let mut first = boot_to_handover(&disk, &[]);
    assert!(run_frames(&mut first, &wf_prog(0xe239, 0xff, false, None, false), 3000));
    let state = first.save_state();
    let mut other = boot_to_handover(&disk, &[]);
    other.step_frame();
    other.load_state(&state).expect("the state loads");
    other.step_frame();
    let want: Vec<u8> = (0..WF_SIZE).map(wf_byte).collect();
    assert_eq!(battery_file(other.battery_save().unwrap(), WF_ID), Some(want));
}
