// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The boot loader's file-selection rule, pinned with disks built here.
//!
//! A file is loaded at boot if its ID is **at most** the boot read file code,
//! which is byte $19 of the disk-info block. That was measured against the real
//! BIOS (see `firmware/fds-hle/src/main.s`), and both halves of it are easy to
//! get wrong in a way no disk in this repository would notice:
//!
//!   * changing the rule to a strict `<` leaves every test in
//!     `fds_hle_boot.rs` passing, because none of the three disks in `dumps/`
//!     has a file whose ID equals its boot code. It breaks Falsion, which is in
//!     the corpus and not in this repository, by 6549 bytes of program RAM.
//!   * reading the code from byte $1A instead selects everything, because $1A
//!     is $FF on every disk, and that too is invisible on a disk whose files
//!     should all load anyway.
//!
//! So the disks here are built to order: small, synthetic, and shaped exactly
//! to ask the question. They need no firmware and no dumps, and they fail if
//! the rule moves in either direction.
//!
//! **These pin our behaviour; they cannot be held against the oracle.** The
//! real BIOS refuses to boot a disk whose first file is not Nintendo's licence
//! screen byte for byte, so none of these disks will run under it at all. That
//! measurement, and why our BIOS deliberately has no such check, is recorded in
//! `fds_hle_routines.rs`. The rule itself was measured on commercial disks
//! before being pinned here.

/// Bytes of block data per side in a `.fds` image.
const SIDE_LEN: usize = 65500;

/// One file to put on a synthetic disk.
struct File {
    id: u8,
    addr: u16,
    body: Vec<u8>,
}

/// Build a one-side `.fds` image with the given boot read file code and files.
///
/// Only the fields the loader reads are filled in; everything else is left as
/// the zero or $FF padding a real disk carries, which is also a small test in
/// itself that the loader does not depend on anything it should not.
fn make_disk(boot_code: u8, files: &[File]) -> Vec<u8> {
    let mut s: Vec<u8> = Vec::with_capacity(SIDE_LEN);

    // Block 1: 56 bytes of disk info.
    s.push(0x01);
    s.extend_from_slice(b"*NINTENDO-HVC*");
    s.push(0x00); // manufacturer
    s.extend_from_slice(b"TST "); // game name
    while s.len() < 0x19 {
        s.push(0);
    }
    s.push(boot_code); // $19: the byte the whole rule turns on
    while s.len() < 56 {
        s.push(0xff);
    }

    // Block 2: the file count.
    s.push(0x02);
    s.push(files.len() as u8);

    for (n, f) in files.iter().enumerate() {
        // Block 3: the 16-byte header.
        s.push(0x03);
        s.push(n as u8); // file number
        s.push(f.id); // file ID, which the rule compares
        s.extend_from_slice(b"FILE    "); // name
        s.extend_from_slice(&f.addr.to_le_bytes());
        s.extend_from_slice(&(f.body.len() as u16).to_le_bytes());
        s.push(0); // kind 0: program RAM
        // Block 4: the body.
        s.push(0x04);
        s.extend_from_slice(&f.body);
    }

    s.resize(SIDE_LEN, 0);
    s
}

/// Boot a synthetic disk on the built-in BIOS and return program RAM at
/// handover.
fn load(disk: &[u8]) -> Vec<u8> {
    let mut nes = nes_core::Nes::from_fds_hle(disk).expect("synthetic disk should parse");
    // These disks carry data and no code, so there is no reset pseudo-vector
    // and nothing to hand over to: the BIOS loads them and then jumps into a
    // zero. Running a fixed number of frames is therefore the right stop, and
    // 600 is several times what a handful of short files takes.
    while nes.dbg_frame() < 600 && !nes.dbg_halted() {
        nes.step();
    }
    (0..0x8000u32)
        .map(|i| nes.peek(0x6000u16.wrapping_add(i as u16)))
        .collect()
}

fn at(prg: &[u8], addr: u16, len: usize) -> Vec<u8> {
    let off = (addr - 0x6000) as usize;
    prg[off..off + len].to_vec()
}

#[test]
fn a_file_whose_id_equals_the_boot_code_is_loaded() {
    // This is the half Falsion settles on real hardware: its boot code is 15
    // and its FC_6 file has ID 15, and FC_6 is loaded. A strict `<` passes
    // every other test in this repository and fails this one.
    let disk = make_disk(
        15,
        &[File { id: 15, addr: 0x7000, body: vec![0xA5; 64] }],
    );
    assert_eq!(at(&load(&disk), 0x7000, 64), vec![0xA5; 64]);
}

#[test]
fn a_file_whose_id_is_above_the_boot_code_is_not_loaded() {
    // The other side of the same edge. Without this, "load everything" would
    // pass the test above.
    let disk = make_disk(
        15,
        &[File { id: 16, addr: 0x7000, body: vec![0xA5; 64] }],
    );
    assert_eq!(at(&load(&disk), 0x7000, 64), vec![0x00; 64]);
}

#[test]
fn selection_is_per_file_and_skipped_files_do_not_shift_the_rest() {
    // Bio Miracle is this shape: a boot code of 15 with files at 0, 1, 22, 23,
    // 31 and 39, of which only two load. A loader that stopped at the first
    // file it did not want, or that mislaid its place in the block chain after
    // skipping one, would load the wrong set.
    let disk = make_disk(
        10,
        &[
            File { id: 0, addr: 0x7000, body: vec![0x11; 32] },
            File { id: 40, addr: 0x7100, body: vec![0x22; 32] },
            File { id: 10, addr: 0x7200, body: vec![0x33; 32] },
            File { id: 11, addr: 0x7300, body: vec![0x44; 32] },
            File { id: 1, addr: 0x7400, body: vec![0x55; 32] },
        ],
    );
    let prg = load(&disk);
    assert_eq!(at(&prg, 0x7000, 32), vec![0x11; 32], "id 0 should load");
    assert_eq!(at(&prg, 0x7100, 32), vec![0x00; 32], "id 40 should not");
    assert_eq!(at(&prg, 0x7200, 32), vec![0x33; 32], "id 10 equals the code");
    assert_eq!(at(&prg, 0x7300, 32), vec![0x00; 32], "id 11 is above it");
    assert_eq!(
        at(&prg, 0x7400, 32),
        vec![0x55; 32],
        "a file after a skipped one still loads, so the block chain was kept"
    );
}

#[test]
fn a_later_file_overwrites_an_earlier_one_at_the_same_address() {
    // Files are loaded in the order they sit on the disk and the last write
    // wins. Several real disks rely on it: Bio Miracle loads four files to
    // $6000 in turn.
    let disk = make_disk(
        99,
        &[
            File { id: 1, addr: 0x7000, body: vec![0x11; 32] },
            File { id: 2, addr: 0x7000, body: vec![0x22; 32] },
        ],
    );
    assert_eq!(at(&load(&disk), 0x7000, 32), vec![0x22; 32]);
}

#[test]
fn the_boot_code_is_read_from_byte_19_and_not_byte_1a() {
    // Byte $1A is $FF on every real disk, which selects everything, so reading
    // the wrong one is invisible unless a disk is built to tell them apart.
    // Here $19 is 0 and $1A is $FF: reading $1A would load the ID-5 file.
    let mut disk = make_disk(
        0,
        &[File { id: 5, addr: 0x7000, body: vec![0xA5; 64] }],
    );
    assert_eq!(disk[0x1a], 0xff, "the test disk must have $FF at $1A to mean anything");
    disk[0x19] = 0;
    assert_eq!(at(&load(&disk), 0x7000, 64), vec![0x00; 64]);
}
