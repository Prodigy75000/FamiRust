// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! `fdsdiff`: hold one FDS BIOS against another at the moment it hands a game
//! its machine.
//!
//! The real BIOS is the oracle for the HLE (see `docs/notes/FDS-HLE.md`), and
//! this is how that is made to mean something concrete. Both BIOSes boot the
//! same disk, and each run stops at **handover**: the first instruction the CPU
//! executes outside `$E000-$FFFF`.
//!
//! Handover is a clean comparison point and the only one. A cold boot runs
//! entirely inside the BIOS window until it jumps to the game, so at that
//! instant RAM holds exactly what the BIOS put there and nothing a game has
//! done since. One frame later the two runs have diverged for reasons that
//! prove nothing, because the games are timing-sensitive and the BIOSes take
//! different numbers of cycles to get there.
//!
//! What gets compared is what a game can observe: the entry address, the
//! registers it is handed, the 2 KB of work RAM, the 32 KB of program RAM the
//! files loaded into, the 8 KB of pattern RAM, and the `$DFF6-$DFFF` pseudo
//! vectors. Cycles are reported but never compared, because an HLE taking a
//! different number of cycles to load a disk is expected.
//!
//!   cargo run -p nes-runner --bin fdsdiff -- disk.fds --bios real.rom
//!   cargo run -p nes-runner --bin fdsdiff -- disk.fds --bios real.rom --bios hle.bin

use std::process::ExitCode;

const BIOS_BASE: u16 = 0xe000;
const PRG_RAM_LO: u16 = 0x6000;
const PRG_RAM_HI: u16 = 0xdfff;
/// The pseudo-vectors the BIOS dispatches interrupts through, in program RAM.
const VECTORS: u16 = 0xdff6;
/// The RESET pseudo-vector: the address the BIOS hands the game control at.
const VEC_RESET: u16 = 0xdffc;

/// Everything a game can see at the instant it is handed control.
struct Handover {
    reached: bool,
    entry: u16,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    p: u8,
    cycles: u64,
    frames: u64,
    ram: Vec<u8>,
    prg: Vec<u8>,
    chr: Vec<u8>,
    ciram: Vec<u8>,
}

impl Handover {
    fn vectors(&self) -> [u16; 5] {
        let mut v = [0u16; 5];
        for (i, slot) in v.iter_mut().enumerate() {
            let off = (VECTORS - PRG_RAM_LO) as usize + i * 2;
            *slot = u16::from(self.prg[off]) | (u16::from(self.prg[off + 1]) << 8);
        }
        v
    }
}

/// Boot `disk` with `bios` and stop at handover, or at `max_frames` if the BIOS
/// never gets there.
fn boot_to_handover(
    disk: &[u8],
    bios: &[u8],
    max_frames: u64,
    at: Option<u64>,
) -> Result<Handover, String> {
    let mut nes = nes_core::Nes::from_fds(disk, bios).map_err(|e| format!("{e:?}"))?;
    let mut reached = false;
    // `--at N` stops at a frame instead of at handover, for looking at what the
    // BIOS has on the screen part way through rather than what it leaves behind.
    if let Some(f) = at {
        while nes.dbg_frame() < f && !nes.dbg_halted() {
            nes.step();
        }
        reached = true;
    } else {
        // Handover is the BIOS dispatching through the $DFFC pseudo-vector, and
        // it is NOT simply the first time the PC leaves the window.
        //
        // The real BIOS leaves NMI enabled while it loads, so on a disk whose
        // files install an NMI handler early, the first exit is an interrupt
        // being dispatched to the game mid-load and the machine at that instant
        // is an interrupt frame rather than a handover. Measuring that gave
        // Xevious an entry of $050A with a=$03 and sp=$F3, and made six Namco
        // titles look like they disagreed with us when they did not.
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
    }
    let entry = nes.dbg_pc();
    let (a, x, y, sp, p) = (nes.cpu.a, nes.cpu.x, nes.cpu.y, nes.cpu.sp, nes.cpu.p);
    let cycles = nes.dbg_cycles();
    let frames = nes.dbg_frame();
    let chr = nes.dbg_chr_ram().to_vec();
    let ciram = nes.dbg_ciram().to_vec();
    let mut ram = vec![0u8; 0x800];
    for (i, b) in ram.iter_mut().enumerate() {
        *b = nes.peek(i as u16);
    }
    let mut prg = vec![0u8; (PRG_RAM_HI - PRG_RAM_LO) as usize + 1];
    for (i, b) in prg.iter_mut().enumerate() {
        *b = nes.peek(PRG_RAM_LO + i as u16);
    }
    Ok(Handover { reached, entry, a, x, y, sp, p, cycles, frames, ram, prg, chr, ciram })
}

/// What the BIOS's interrupt dispatch does to the machine on its way through.
///
/// This is a hot path: it runs every frame of every game, and an HLE that takes
/// a different number of cycles to reach the game's handler moves every
/// raster-timed split the game draws. So it gets measured rather than assumed.
struct Dispatch {
    seen: bool,
    entry: u16,
    /// Where control went, which is the game's own handler.
    exit_to: u16,
    /// Stack pointer on arrival at the BIOS handler. The interrupt sequence has
    /// already pushed three bytes by then.
    sp_in: u8,
    /// Stack pointer on leaving it. Lower than `sp_in` means the BIOS pushed
    /// registers the game's handler is expected to pull back.
    sp_out: u8,
    cycles: u64,
    instructions: u32,
    /// The three NMI pseudo-vectors read at the instant of dispatch, NOT at
    /// handover. Games rewrite them while they run, so comparing a dispatch
    /// target against handover values attributes it to the wrong vector: that
    /// mistake made Doki Doki Panic look like it used vector 1.
    vecs: [u16; 3],
    /// Address of each instruction executed inside the window. Control flow,
    /// not code: it says which path was taken, which is what tells a
    /// data-driven dispatch from a branching one.
    trail: Vec<u16>,
    /// Work RAM as it was at the instant of dispatch. The handler's first
    /// instruction is an absolute load, so the byte that decides which vector
    /// gets used is in here; which byte is found by correlation rather than by
    /// reading the ROM.
    ram: Vec<u8>,
}

/// Run on from a handover state until the BIOS's handler at `target` is entered,
/// then follow it until control leaves the BIOS window.
fn probe_dispatch(
    disk: &[u8],
    bios: &[u8],
    target: u16,
    max_frames: u64,
) -> Result<Dispatch, String> {
    let mut nes = nes_core::Nes::from_fds(disk, bios).map_err(|e| format!("{e:?}"))?;
    let mut d = Dispatch {
        seen: false,
        entry: target,
        exit_to: 0,
        sp_in: 0,
        sp_out: 0,
        cycles: 0,
        instructions: 0,
        vecs: [0; 3],
        trail: Vec::new(),
        ram: Vec::new(),
    };
    // Phase 0: get past handover. The BIOS takes its own interrupts while it is
    // loading and handles them without leaving the window, so probing before
    // the game exists measures the loader rather than the dispatch.
    while nes.dbg_frame() < max_frames && nes.dbg_pc() >= BIOS_BASE {
        nes.step();
        if nes.dbg_halted() {
            return Ok(d);
        }
    }
    // Phase 1: reach the handler, from game code.
    while nes.dbg_frame() < max_frames && nes.dbg_pc() != target {
        nes.step();
        if nes.dbg_halted() {
            return Ok(d);
        }
    }
    if nes.dbg_pc() != target {
        return Ok(d);
    }
    // Phase 2: follow it out of the window.
    d.seen = true;
    d.sp_in = nes.cpu.sp;
    for (i, v) in d.vecs.iter_mut().enumerate() {
        let a = VECTORS + (i as u16) * 2;
        *v = u16::from(nes.peek(a)) | (u16::from(nes.peek(a + 1)) << 8);
    }
    d.ram = (0..0x800u16).map(|a| nes.peek(a)).collect();
    let start = nes.dbg_cycles();
    while nes.dbg_pc() >= BIOS_BASE {
        if d.trail.len() < 24 {
            d.trail.push(nes.dbg_pc());
        }
        nes.step();
        d.instructions += 1;
        if nes.dbg_halted() || d.instructions > 10_000 {
            break;
        }
    }
    d.sp_out = nes.cpu.sp;
    d.exit_to = nes.dbg_pc();
    d.cycles = nes.dbg_cycles() - start;
    Ok(d)
}

/// Contiguous runs of differing bytes between two equal-length blocks.
fn diff_runs(a: &[u8], b: &[u8], base: u16) -> Vec<(u16, u16)> {
    let mut out: Vec<(u16, u16)> = Vec::new();
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x == y {
            continue;
        }
        let addr = base + i as u16;
        match out.last_mut() {
            Some(last) if last.1 + 1 == addr => last.1 = addr,
            _ => out.push((addr, addr)),
        }
    }
    out
}

fn total(runs: &[(u16, u16)]) -> u32 {
    runs.iter().map(|(lo, hi)| u32::from(*hi - *lo) + 1).sum()
}

fn show(name: &str, h: &Handover) {
    if !h.reached {
        println!(
            "{name}: NEVER HANDED OVER. Still at ${:04x} after {} frames ({} cycles).",
            h.entry, h.frames, h.cycles
        );
        return;
    }
    let v = h.vectors();
    println!(
        "{name}: entry ${:04x} after {} frames / {} cycles",
        h.entry, h.frames, h.cycles
    );
    println!(
        "  a=${:02x} x=${:02x} y=${:02x} sp=${:02x} p=${:02x}",
        h.a, h.x, h.y, h.sp, h.p
    );
    println!(
        "  $DFF6 nmi1=${:04x} nmi2=${:04x} nmi3=${:04x} reset=${:04x} irq=${:04x}",
        v[0], v[1], v[2], v[3], v[4]
    );
    let prg_set = h.prg.iter().filter(|&&b| b != 0).count();
    let chr_set = h.chr.iter().filter(|&&b| b != 0).count();
    println!("  loaded: {prg_set} non-zero PRG-RAM bytes, {chr_set} non-zero CHR-RAM bytes");
    if h.entry != v[3] {
        println!("  note: entry is not the $DFFC reset vector, so this is not a handover");
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: fdsdiff <disk.fds> --bios A [--bios B] [--frames N] [--verbose]");
    eprintln!("  one --bios  report the handover state that BIOS produces");
    eprintln!("  two --bios  diff them, and exit non-zero if they disagree");
    eprintln!("  --frames N  give up if no handover by frame N (default 1200)");
    eprintln!("  --dump P    write P-<bios>.ram/.prg/.chr so the load can be picked apart");
    eprintln!("  --probe A   report what the handler at hex address A does on its way through");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path = String::new();
    let mut bioses: Vec<String> = Vec::new();
    let mut frames = 1200u64;
    let mut verbose = false;
    let mut dump: Option<String> = None;
    let mut probe: Option<u16> = None;
    let mut at: Option<u64> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bios" => match args.next() {
                Some(v) => bioses.push(v),
                None => return usage(),
            },
            "--frames" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => frames = n,
                None => return usage(),
            },
            "--verbose" => verbose = true,
            "--at" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => at = Some(n),
                None => return usage(),
            },
            "--probe" => match args.next().and_then(|v| {
                u16::from_str_radix(v.trim_start_matches('$').trim_start_matches("0x"), 16).ok()
            }) {
                Some(a) => probe = Some(a),
                None => return usage(),
            },
            "--dump" => match args.next() {
                Some(v) => dump = Some(v),
                None => return usage(),
            },
            "-h" | "--help" => return usage(),
            other if other.starts_with("--") => {
                eprintln!("fdsdiff: unknown option {other}");
                return usage();
            }
            other => path = other.to_string(),
        }
    }
    if path.is_empty() || bioses.is_empty() || bioses.len() > 2 {
        return usage();
    }

    let disk = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut runs = Vec::new();
    let mut loaded: Vec<Vec<u8>> = Vec::new();
    for b in &bioses {
        let bytes = match std::fs::read(b) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("cannot read {b}: {e}");
                return ExitCode::FAILURE;
            }
        };
        match boot_to_handover(&disk, &bytes, frames, at) {
            Ok(h) => runs.push((b.clone(), h)),
            Err(e) => {
                eprintln!("{b}: {e}");
                return ExitCode::FAILURE;
            }
        }
        loaded.push(bytes);
    }

    println!("{path}");
    for (name, h) in &runs {
        show(name, h);
        if let Some(prefix) = &dump {
            let tag = std::path::Path::new(name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "bios".into());
            for (ext, bytes) in [("ram", &h.ram), ("prg", &h.prg), ("chr", &h.chr),
                                 ("ciram", &h.ciram)] {
                let p = format!("{prefix}-{tag}.{ext}");
                if let Err(e) = std::fs::write(&p, bytes) {
                    eprintln!("cannot write {p}: {e}");
                } else {
                    println!("  dumped {p}");
                }
            }
        }
    }
    if let Some(target) = probe {
        for (name, b) in bioses.iter().zip(loaded.iter()) {
            match probe_dispatch(&disk, b, target, frames * 2) {
                Ok(d) if d.seen => {
                    let which: Vec<usize> = (0..3)
                        .filter(|&i| d.vecs[i] == d.exit_to)
                        .map(|i| i + 1)
                        .collect();
                    println!(
                        "{name}: ${:04x} -> ${:04x} in {} instructions / {} cycles,                          sp ${:02x} -> ${:02x} ({} pushed)",
                        d.entry, d.exit_to, d.instructions, d.cycles, d.sp_in, d.sp_out,
                        d.sp_in.wrapping_sub(d.sp_out)
                    );
                    println!(
                        "  vectors at dispatch: ${:04x} ${:04x} ${:04x} -> used {which:?}",
                        d.vecs[0], d.vecs[1], d.vecs[2]
                    );
                    let trail: Vec<String> =
                        d.trail.iter().map(|a| format!("${a:04x}")).collect();
                    println!("  path: {}", trail.join(" "));
                    if let Some(prefix) = &dump {
                        let p = format!("{prefix}.dispatch");
                        let _ = std::fs::write(&p, &d.ram);
                        println!("  used-vector {} ram at dispatch -> {p}",
                                 which.first().copied().unwrap_or(0));
                    }
                }
                Ok(_) => println!("{name}: ${target:04x} was never entered"),
                Err(e) => println!("{name}: {e}"),
            }
        }
    }

    if runs.len() < 2 {
        return ExitCode::SUCCESS;
    }

    let (na, a) = &runs[0];
    let (nb, b) = &runs[1];
    println!("\n{na}  vs  {nb}");
    let mut bad = false;
    if !a.reached || !b.reached {
        println!("  HANDOVER: {} / {}", a.reached, b.reached);
        bad = true;
    }
    if a.entry != b.entry {
        println!("  ENTRY differs: ${:04x} vs ${:04x}", a.entry, b.entry);
        bad = true;
    }
    for (what, x, y) in [
        ("a", a.a, b.a),
        ("x", a.x, b.x),
        ("y", a.y, b.y),
        ("sp", a.sp, b.sp),
        ("p", a.p, b.p),
    ] {
        if x != y {
            println!("  {what} differs: ${x:02x} vs ${y:02x}");
            bad = true;
        }
    }
    for (what, base, x, y) in [
        ("work RAM", 0x0000u16, &a.ram, &b.ram),
        ("PRG RAM", PRG_RAM_LO, &a.prg, &b.prg),
        ("CHR RAM", 0x0000u16, &a.chr, &b.chr),
        ("nametable RAM", 0x2000u16, &a.ciram, &b.ciram),
    ] {
        if x.len() != y.len() {
            println!("  {what} sizes differ: {} vs {}", x.len(), y.len());
            bad = true;
            continue;
        }
        let d = diff_runs(x, y, base);
        if d.is_empty() {
            println!("  {what}: identical");
            continue;
        }
        bad = true;
        println!("  {what}: {} bytes differ in {} run(s)", total(&d), d.len());
        let show_n = if verbose { d.len() } else { 12.min(d.len()) };
        for (lo, hi) in &d[..show_n] {
            println!("    ${lo:04x}-${hi:04x}");
        }
        if show_n < d.len() {
            println!("    ... {} more (--verbose for all)", d.len() - show_n);
        }
    }
    if a.vectors() != b.vectors() {
        println!("  pseudo-vectors differ");
        bad = true;
    }
    println!(
        "  cycles to handover: {} vs {} (not a failure; an HLE may take its own time)",
        a.cycles, b.cycles
    );

    if bad {
        println!("\nDIFFER");
        ExitCode::FAILURE
    } else {
        println!("\nIDENTICAL at handover");
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_runs_finds_nothing_in_identical_blocks() {
        assert_eq!(diff_runs(&[1, 2, 3], &[1, 2, 3], 0x6000), vec![]);
    }

    #[test]
    fn diff_runs_merges_adjacent_and_splits_gaps() {
        let a = [0u8, 0, 0, 0, 0, 0];
        let b = [1u8, 1, 0, 0, 9, 0];
        assert_eq!(
            diff_runs(&a, &b, 0x6000),
            vec![(0x6000, 0x6001), (0x6004, 0x6004)]
        );
        assert_eq!(total(&diff_runs(&a, &b, 0x6000)), 3);
    }

    #[test]
    fn diff_runs_reports_addresses_not_offsets() {
        // An offset-based report would send somebody looking at $0002 for a
        // difference that is really at $DFF8.
        let a = [0u8, 0, 0];
        let b = [0u8, 0, 7];
        assert_eq!(diff_runs(&a, &b, 0xdff6), vec![(0xdff8, 0xdff8)]);
    }
}
