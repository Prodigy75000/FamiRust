// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! An FDS disk boots with no firmware file anywhere in the path.
//!
//! That sentence is the whole point of `firmware/fds-hle`, so it is a test
//! rather than a claim in a README. Where a real `disksys.rom` is available
//! these also hold our boot against it, because the real BIOS is the oracle:
//! see `docs/notes/FDS-HLE.md`.
//!
//! The disks live in `dumps/fds/` and are not redistributable, so every test
//! that needs one skips when it is absent rather than failing. The tests that
//! need only the built-in BIOS always run.

use std::path::{Path, PathBuf};

const BIOS_BASE: u16 = 0xe000;
const PRG_RAM_LO: u16 = 0x6000;
const VEC_RESET: u16 = 0xdffc;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn disk(name: &str) -> Option<Vec<u8>> {
    let p = repo_root().join("dumps/fds").join(name);
    std::fs::read(p).ok()
}

fn real_bios() -> Option<Vec<u8>> {
    std::fs::read(repo_root().join("dumps/fds/disksys.rom")).ok()
}

/// What the machine looks like at handover: the instant the BIOS dispatches
/// through the $DFFC pseudo-vector. A cold boot does everything it does before
/// that point, so this instant is the whole of what the BIOS did.
struct Handover {
    reached: bool,
    entry: u16,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    p: u8,
    frames: u64,
    prg: Vec<u8>,
    chr: Vec<u8>,
    ciram: Vec<u8>,
}

fn boot(disk: &[u8], bios: &[u8], max_frames: u64) -> Handover {
    let mut nes = nes_core::Nes::from_fds(disk, bios).expect("disk should parse");
    let mut reached = false;
    // Handover is the BIOS dispatching through $DFFC, not merely the first time
    // the PC leaves the window. The real BIOS keeps NMI enabled while it loads,
    // so on a disk that installs an NMI handler early the first exit is an
    // interrupt being dispatched mid-load, and measuring that instead gave
    // Xevious an entry of $050A in an interrupt frame.
    while nes.dbg_frame() < max_frames {
        let pc = nes.dbg_pc();
        if pc < BIOS_BASE {
            let v = u16::from(nes.peek(VEC_RESET)) | (u16::from(nes.peek(VEC_RESET + 1)) << 8);
            if pc == v {
                reached = true;
                break;
            }
        }
        nes.step();
        if nes.dbg_halted() {
            break;
        }
    }
    let prg = (0..0x8000u32)
        .map(|i| nes.peek(PRG_RAM_LO.wrapping_add(i as u16)))
        .collect();
    Handover {
        reached,
        entry: nes.dbg_pc(),
        a: nes.cpu.a,
        x: nes.cpu.x,
        y: nes.cpu.y,
        sp: nes.cpu.sp,
        p: nes.cpu.p,
        frames: nes.dbg_frame(),
        prg,
        chr: nes.dbg_chr_ram().to_vec(),
        ciram: nes.dbg_ciram().to_vec(),
    }
}

fn skip(what: &Path) -> bool {
    if what.exists() {
        return false;
    }
    eprintln!("skipping: {} is not in this checkout", what.display());
    true
}

#[test]
fn the_built_in_bios_is_the_committed_file() {
    // `include_bytes!` resolves at compile time, so an image rebuilt without a
    // recompile would ship stale inside the core while the file on disk looked
    // correct. The other half of this, that the file matches its source, is in
    // nes-asm/tests/fds_hle_bios_reproduces.rs.
    let path = repo_root().join("firmware/fds-hle/fds-hle.bin");
    let on_disk = std::fs::read(&path).expect("the committed BIOS image");
    assert_eq!(nes_core::fds::HLE_BIOS, on_disk.as_slice());
    assert_eq!(nes_core::fds::HLE_BIOS.len(), nes_core::fds::BIOS_LEN);
}

#[test]
fn a_disk_boots_with_no_bios_file_at_all() {
    let path = repo_root().join("dumps/fds/zelda.fds");
    if skip(&path) {
        return;
    }
    let d = disk("zelda.fds").unwrap();
    let h = boot(&d, &[], 1500);
    assert!(
        h.reached,
        "never handed over: still at ${:04x} after {} frames",
        h.entry, h.frames
    );
    // Handing over to $0000 would also count as "left the BIOS window". The
    // entry has to be the address the disk's own loaded RESET vector names.
    let vec = u16::from(h.prg[(VEC_RESET - PRG_RAM_LO) as usize])
        | (u16::from(h.prg[(VEC_RESET - PRG_RAM_LO) as usize + 1]) << 8);
    assert_eq!(h.entry, vec, "entry is not the $DFFC pseudo-vector");
    assert!(h.entry >= PRG_RAM_LO, "entry ${:04x} is not in program RAM", h.entry);
}

#[test]
fn the_registers_at_handover_are_the_ones_we_chose() {
    // These came from the real BIOS, which hands Zelda, Bio Miracle and Famicom
    // Grand Prix exactly this. They are NOT a contract, and the comment that
    // first stood here saying they were was wrong: Xevious is handed x=$FF
    // y=$00 p=$21 by the same BIOS. They are whatever its last instruction left
    // behind, they vary by disk, and so nothing can depend on them.
    //
    // This is therefore a test of OUR invariant, which is worth having anyway.
    // p=$20 is the part that matters and is not incidental: interrupts enabled,
    // decimal mode off, and a stack pointer that has been reset.
    let path = repo_root().join("dumps/fds/zelda.fds");
    if skip(&path) {
        return;
    }
    let h = boot(&disk("zelda.fds").unwrap(), &[], 1500);
    assert!(h.reached);
    assert_eq!(
        (h.a, h.x, h.y, h.sp, h.p),
        (0x10, 0x00, 0xff, 0xff, 0x20),
        "handover registers"
    );
}

#[test]
fn every_disk_in_the_checkout_loads_the_same_program_ram_as_the_real_bios() {
    // The real BIOS is the oracle. Program RAM is the load itself: the right
    // files, chosen by the right rule, at the right addresses. Work RAM is NOT
    // compared, because the real BIOS leaves about 700 bytes of undocumented
    // scratch and reproducing that byte for byte would mean deriving our
    // implementation from theirs.
    let Some(real) = real_bios() else {
        eprintln!("skipping: dumps/fds/disksys.rom is not in this checkout");
        return;
    };
    let mut checked = 0;
    for name in ["zelda.fds", "biomiracle.fds", "fgp.fds"] {
        let Some(d) = disk(name) else { continue };
        checked += 1;
        let a = boot(&d, &real, 1500);
        let b = boot(&d, &[], 1500);
        assert!(a.reached && b.reached, "{name}: one of the two never handed over");
        assert_eq!(a.entry, b.entry, "{name}: entry address");
        let differ = a.prg.iter().zip(&b.prg).filter(|(x, y)| x != y).count();
        assert_eq!(differ, 0, "{name}: {differ} bytes of program RAM differ");
    }
    assert!(checked > 0, "no FDS disks in this checkout to compare");
}

#[test]
fn a_file_of_kind_one_or_two_lands_where_the_real_bios_puts_it() {
    // Pattern RAM and nametable RAM are written through $2007, and getting
    // either the rendering state or the mirroring wrong scatters them. Both
    // went wrong during development: Zelda lost 5648 bytes of pattern data to
    // writes made while rendering was on, and its licence screen landed in the
    // wrong nametable bank until the loader was made to boot horizontal.
    let Some(real) = real_bios() else {
        eprintln!("skipping: dumps/fds/disksys.rom is not in this checkout");
        return;
    };
    let Some(d) = disk("zelda.fds") else {
        eprintln!("skipping: dumps/fds/zelda.fds is not in this checkout");
        return;
    };
    let a = boot(&d, &real, 1500);
    let b = boot(&d, &[], 1500);
    // Zelda loads a whole 8 KB pattern file, so this compares in full.
    assert_eq!(
        a.chr.iter().zip(&b.chr).filter(|(x, y)| x != y).count(),
        0,
        "pattern RAM differs"
    );
    // Its licence screen is a 224-byte kind-2 file at PPU $2800, which under
    // the mirroring the BIOS boots with is the second nametable bank.
    assert_eq!(&a.ciram[0x400..0x400 + 224], &b.ciram[0x400..0x400 + 224]);
    assert!(
        b.ciram[0x400..0x400 + 224].iter().any(|&x| x != 0),
        "the kind-2 file did not load at all, so matching proves nothing"
    );
}

#[test]
fn ours_is_not_slower_than_the_disk_it_reads() {
    // Loading is physical: the drive delivers a byte every 149 cycles, and no
    // BIOS can beat that. This guards the other direction, that we have not
    // accidentally made loading take longer than the real BIOS does, which is
    // the shape a polling bug takes.
    let Some(real) = real_bios() else {
        eprintln!("skipping: dumps/fds/disksys.rom is not in this checkout");
        return;
    };
    let Some(d) = disk("zelda.fds") else { return };
    let a = boot(&d, &real, 1500);
    let b = boot(&d, &[], 1500);
    assert!(a.reached && b.reached);
    assert!(
        b.frames <= a.frames,
        "ours took {} frames to hand over, the real BIOS took {}",
        b.frames,
        a.frames
    );
}
