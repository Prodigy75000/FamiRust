// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! `fdstrace`: what an FDS game actually asks the BIOS to do.
//!
//! The first step towards an HLE BIOS is knowing which of the real one's
//! routines are load-bearing, and that has to be measured rather than read off
//! a disassembly. A static scan does not work: scanning the disk images for the
//! `JSR` opcode returned 164 distinct targets, most of them data bytes that
//! happen to read as `JSR` (`$FFFF` came up eight times). So this is dynamic.
//!
//! ## The primitive: control crossing the window boundary
//!
//! Rather than look for call opcodes, this watches for the PC entering
//! `$E000-$FFFF` from outside it. That is both simpler and strictly more
//! complete than a `JSR` census: it catches `JSR`, `JMP`, `JMP ()`, an `RTS`
//! landing in the BIOS, and the NMI/IRQ/RESET vector dispatch, without decoding
//! a single addressing mode. It is also the right definition for the job, since
//! an entry point *is* an address control arrives at from outside.
//!
//! ## Code, data, and the difference that matters
//!
//! Every read of the BIOS window is counted per address, split by whether the
//! CPU was executing inside the window at the time. The split is the point:
//!
//!   * a byte only the BIOS reads goes away with the routine that read it, and
//!     an HLE may lay out its own internals however it likes;
//!   * a byte the GAME reads out of the BIOS image is part of the published
//!     interface and has to still be there, byte for byte, whatever replaces
//!     the code around it.
//!
//! The interrupt vectors fall out of this for free: the CPU fetches `$FFFA`
//! while the PC is still at the interrupted address, so they land in the
//! outside bank on their own without being special-cased.
//!
//! ## Usage
//!
//!   cargo run -p nes-runner --bin fdstrace -- disk.fds [--frames N] [--mash]
//!                                              [--json out.json] [--png out.png]
//!
//! The BIOS comes from `$FDS_BIOS` or `disksys.rom` beside the disk. Nothing
//! here needs a real BIOS to be *correct*, but everything here needs one to
//! produce a census: the real BIOS is the oracle the HLE gets held against.

use std::collections::HashMap;
use std::process::ExitCode;

use serde_json::json;

/// Base of the BIOS window in CPU space.
const BIOS_BASE: u16 = 0xe000;
/// Frames at the end of the run that decide the verdict. A game spends its
/// first seconds legitimately inside the BIOS being loaded, so the split over
/// the whole run says nothing; the split once it has settled says everything.
const TAIL_FRAMES: u64 = 60;
/// Frames of "the game is doing nothing but BIOS" before `--autoswap` turns the
/// disk over. A whole side is ~5.5 seconds of physical read, so this has to be
/// comfortably longer than that as well as longer than the drive's own reinsert
/// settle (60 frames), or a swap lands in the middle of the load it was waiting
/// for. Measured: at 180 frames, auto-swapping turned a booting Bio Miracle
/// into NOLOAD by ejecting the disk mid-load, sixteen times running.
const STUCK_FRAMES: u64 = 420;
/// Cap on automatic swaps, so a disk that prompts forever cannot spin forever.
const MAX_SWAPS: u32 = 16;

/// What a run looked like, in a fixed vocabulary. Prose verdicts cannot be
/// aggregated across 114 titles, and a ledger of 114 paragraphs is not a
/// census. See `docs/notes/FDS-HLE.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The CPU jammed on a KIL/JAM opcode. Nothing after this means anything.
    Halt,
    /// Not one instruction ran outside the BIOS window. The disk never loaded.
    NoLoad,
    /// The tail is almost entirely the game's own code: it loaded and is running.
    Boot,
    /// The tail is almost entirely BIOS: stuck in an error loop or waiting on a
    /// disk that never satisfies it.
    BiosLoop,
    /// Neither. Still loading at the end of the run, or a game whose NMI handler
    /// leans on the BIOS heavily enough to keep the split even.
    Mixed,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Halt => "HALT",
            Verdict::NoLoad => "NOLOAD",
            Verdict::Boot => "BOOT",
            Verdict::BiosLoop => "BIOS-LOOP",
            Verdict::Mixed => "MIXED",
        }
    }
}

/// Decide the verdict from the counts. Pure, so it can be tested without a
/// BIOS, a disk, or a CPU.
fn verdict(halted: bool, total_outside: u64, tail_inside: u64, tail_outside: u64) -> Verdict {
    if halted {
        return Verdict::Halt;
    }
    if total_outside == 0 {
        return Verdict::NoLoad;
    }
    let tail = tail_inside + tail_outside;
    if tail == 0 {
        return Verdict::Mixed;
    }
    let out = tail_outside as f64 / tail as f64;
    if out >= 0.90 {
        Verdict::Boot
    } else if out <= 0.05 {
        Verdict::BiosLoop
    } else {
        Verdict::Mixed
    }
}

/// Has the game run nothing of its own over this window? The BIOS waiting at a
/// "SET SIDE" prompt and the BIOS stuck in an error loop look identical from
/// here, which is fine: turning the disk over is the right answer to both.
fn stalled(win_inside: u64, win_outside: u64) -> bool {
    let tot = win_inside + win_outside;
    tot > 0 && (win_outside as f64 / tot as f64) < 0.05
}

/// The three hardware vectors, read out of the BIOS image at startup rather
/// than hardcoded. An entry whose target is one of these was almost certainly
/// an interrupt, and that beats guessing from the opcode: an NMI taken at an
/// instruction boundary leaves the PC pointing at an instruction that never
/// ran, so the opcode there is a red herring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vectors {
    nmi: u16,
    reset: u16,
    irq: u16,
}

/// Name the reason control entered the BIOS. `opcode` is `None` when the PC was
/// somewhere it would not be safe to peek (I/O space), which only happens on a
/// run that has already gone off the rails.
fn edge_kind(opcode: Option<u8>, target: u16, v: Vectors) -> &'static str {
    // Vector targets win over the opcode, for the reason in `Vectors`.
    if target == v.nmi {
        return "nmi";
    }
    if target == v.irq {
        return "irq";
    }
    if target == v.reset {
        return "reset";
    }
    match opcode {
        Some(0x20) => "jsr",
        Some(0x4c) => "jmp",
        Some(0x6c) => "jmp()",
        Some(0x60) => "rts",
        Some(0x40) => "rti",
        Some(0x00) => "brk",
        // Not a control-transfer opcode, so control did not move because of the
        // instruction at that address: an interrupt was serviced before it ran.
        Some(_) => "int",
        None => "?",
    }
}

/// Collapse a sorted, deduplicated address list into contiguous runs. A census
/// of 8192 addresses is unreadable as a list and obvious as a handful of ranges
/// ("$F800-$FBFF read from outside" says the font is part of the interface).
fn runs(addrs: &[u16]) -> Vec<(u16, u16)> {
    let mut out: Vec<(u16, u16)> = Vec::new();
    for &a in addrs {
        match out.last_mut() {
            Some(last) if last.1 + 1 == a => last.1 = a,
            _ => out.push((a, a)),
        }
    }
    out
}

/// One `(from, to)` control transfer into the BIOS window.
#[derive(Default, Clone)]
struct Edge {
    count: u64,
    /// CPU cycles spent inside the window after arriving here, summed over every
    /// arrival. Not "cycles the routine costs": the BIOS can call out to a game
    /// hook through the `$DFF6-$DFFF` RAM vectors, which closes the accounting
    /// early and opens a fresh edge when the hook returns. Read it as the cost
    /// of an uninterrupted stay, which is what an HLE has to charge for.
    cycles_in_window: u64,
    opcode: Option<u8>,
}

/// One BIOS entry point, with every call site that reaches it folded together.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    to: u16,
    kind: &'static str,
    count: u64,
    cycles: u64,
    /// How many distinct addresses control arrived from.
    callers: usize,
}

/// Fold the `(from, to)` edges into one row per target. When the same target is
/// reached two ways, the kind reported is the one that happened most, because a
/// routine reached 248 times by `JSR` and once by an interrupt is a called
/// routine with an interrupt landing on it, not an interrupt vector.
fn group_by_target(ledger: &[((u16, u16), Edge)], v: Vectors) -> Vec<Target> {
    let mut by: HashMap<u16, Target> = HashMap::new();
    let mut best: HashMap<u16, u64> = HashMap::new();
    for ((from, to), e) in ledger {
        let _ = from;
        let kind = edge_kind(e.opcode, *to, v);
        let t = by.entry(*to).or_insert(Target {
            to: *to,
            kind,
            count: 0,
            cycles: 0,
            callers: 0,
        });
        t.count += e.count;
        t.cycles += e.cycles_in_window;
        t.callers += 1;
        let b = best.entry(*to).or_insert(0);
        if e.count > *b {
            *b = e.count;
            t.kind = kind;
        }
    }
    by.into_values().collect()
}

/// Safe to `peek`? RAM and cartridge space are plain array reads; `$2000-$401F`
/// is not, and on the FDS reading `$4031` advances the disk head. A tracer that
/// perturbs the machine it is measuring is worse than no tracer.
fn peekable(addr: u16) -> bool {
    matches!(addr, 0x0000..=0x1fff | 0x6000..=0xffff)
}

fn peek16(nes: &mut nes_core::Nes, addr: u16) -> u16 {
    u16::from(nes.peek(addr)) | (u16::from(nes.peek(addr.wrapping_add(1))) << 8)
}

fn usage() -> ExitCode {
    eprintln!("usage: fdstrace <disk.fds> [--frames N] [--mash] [--json out.json] [--png out.png]");
    eprintln!("  --frames N   how long to run (default 1800, ~30s; a whole side is ~5.5s of disk)");
    eprintln!("  --mash       tap START every 2s from frame 240, to get past a title screen");
    eprintln!("  --autoswap   turn the disk over when the game stalls on a SET SIDE prompt");
    eprintln!("  --json PATH  write the machine-readable census row");
    eprintln!("  --png PATH   write the final frame, so a human can check the verdict");
    eprintln!("  $FDS_BIOS    the 8 KiB disksys.rom (else looked for beside the disk)");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path = String::new();
    let mut frames: u64 = 1800;
    let mut mash = false;
    let mut autoswap = false;
    let mut json_out: Option<String> = None;
    let mut png_out: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => frames = n,
                None => return usage(),
            },
            "--mash" => mash = true,
            "--autoswap" => autoswap = true,
            "--json" => match args.next() {
                Some(v) => json_out = Some(v),
                None => return usage(),
            },
            "--png" => match args.next() {
                Some(v) => png_out = Some(v),
                None => return usage(),
            },
            "-h" | "--help" => return usage(),
            other if other.starts_with("--") => {
                eprintln!("fdstrace: unknown option {other}");
                return usage();
            }
            other => path = other.to_string(),
        }
    }
    if path.is_empty() {
        return usage();
    }

    let disk = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !nes_core::Nes::is_fds(&disk) {
        eprintln!("{path} is not an FDS image (this tool traces the FDS BIOS)");
        return ExitCode::FAILURE;
    }
    let bios_path = std::env::var("FDS_BIOS").unwrap_or_else(|_| {
        std::path::Path::new(&path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("disksys.rom")
            .to_string_lossy()
            .into_owned()
    });
    let bios = match std::fs::read(&bios_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("FDS image needs a BIOS; cannot read {bios_path}: {e}");
            eprintln!("(set $FDS_BIOS or place disksys.rom next to the .fds)");
            return ExitCode::FAILURE;
        }
    };
    let mut nes = match nes_core::Nes::from_fds(&disk, &bios) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("failed to load FDS {path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    let sides = nes.fds_side_count();
    nes.dbg_census_on();

    let vectors = Vectors {
        nmi: peek16(&mut nes, 0xfffa),
        reset: peek16(&mut nes, 0xfffc),
        irq: peek16(&mut nes, 0xfffe),
    };

    // ---- the trace ----
    let mut exec = vec![0u32; nes_core::fds::BIOS_LEN];
    let mut entries: HashMap<(u16, u16), Edge> = HashMap::new();
    let mut exits: HashMap<u16, u64> = HashMap::new();
    let mut ins_inside = 0u64;
    let mut ins_outside = 0u64;
    let mut tail_inside = 0u64;
    let mut tail_outside = 0u64;
    let mut dw_bios = 0u64;
    let mut dw_game = 0u64;
    // Open stay in the window: the edge it arrived through, and the cycle it
    // arrived on. `None` while control is outside.
    let mut stay: Option<((u16, u16), u64)> = None;

    // Multi-side games stop at a BIOS "SET SIDE n" prompt and wait for a person
    // to turn the disk over, and a census that never gets past that prompt is a
    // census of the loader rather than of the library. The prompt looks exactly
    // like a stall (the BIOS polls, the game runs nothing), so the same signal
    // the verdict uses drives the swap.
    let mut win_inside = 0u64;
    let mut win_outside = 0u64;
    let mut win_start = 0u64;
    let mut swaps = 0u32;
    let mut side = 0usize;

    let tail_from = frames.saturating_sub(TAIL_FRAMES);
    let mut halted = false;

    while nes.dbg_frame() < frames {
        let frame = nes.dbg_frame();
        // Unattended input. Two seconds apart and eight frames long is longer
        // than any title-screen debounce and shorter than any menu repeat, and
        // it is what a person does to a game that will not start.
        if mash {
            let held = if frame >= 240 && (frame % 120) < 8 { 0x08 } else { 0 };
            nes.set_buttons(0, held);
        }

        let pc = nes.dbg_pc();
        let inside = pc >= BIOS_BASE;
        nes.dbg_census_ctx(inside);
        if inside {
            exec[(pc - BIOS_BASE) as usize] = exec[(pc - BIOS_BASE) as usize].saturating_add(1);
            ins_inside += 1;
            if frame >= tail_from {
                tail_inside += 1;
            }
            win_inside += 1;
        } else {
            ins_outside += 1;
            if frame >= tail_from {
                tail_outside += 1;
            }
            win_outside += 1;
        }
        if frame >= win_start + STUCK_FRAMES {
            // Never before the game has run something of its own. Until then,
            // "no game code" is what a load in progress looks like, not a
            // prompt, and swapping discards the load.
            let booted = ins_outside > 0;
            if autoswap && booted && sides > 1 && swaps < MAX_SWAPS && stalled(win_inside, win_outside)
            {
                side = (side + 1) % sides;
                nes.fds_insert_side(side);
                swaps += 1;
            }
            win_start = frame;
            win_inside = 0;
            win_outside = 0;
        }

        let dw_before = nes.dbg_diskreg_writes();
        nes.step();
        // Every write during an instruction came from that instruction, so the
        // PC at its start attributes it. This is the question "does the game
        // drive the drive itself", and it decides whether the hardware transfer
        // engine stays load-bearing under an HLE.
        let dw = nes.dbg_diskreg_writes() - dw_before;
        if inside {
            dw_bios += dw;
        } else {
            dw_game += dw;
        }

        let npc = nes.dbg_pc();
        let now_inside = npc >= BIOS_BASE;
        if now_inside && !inside {
            let opcode = peekable(pc).then(|| nes.peek(pc));
            let key = (pc, npc);
            let e = entries.entry(key).or_default();
            e.count += 1;
            e.opcode = opcode;
            stay = Some((key, nes.dbg_cycles()));
        } else if inside && !now_inside {
            *exits.entry(pc).or_insert(0) += 1;
            if let Some((key, at)) = stay.take() {
                if let Some(e) = entries.get_mut(&key) {
                    e.cycles_in_window += nes.dbg_cycles().saturating_sub(at);
                }
            }
        }

        if nes.dbg_halted() {
            halted = true;
            break;
        }
    }

    // ---- the report ----
    let cycles = nes.dbg_cycles();
    let ran_frames = nes.dbg_frame();
    let v = verdict(halted, ins_outside, tail_inside, tail_outside);

    let reads = nes.dbg_census_reads().to_vec();
    let mut read_in: Vec<u16> = Vec::new();
    let mut read_out: Vec<u16> = Vec::new();
    let mut read_out_total = 0u64;
    for (i, c) in reads.iter().enumerate() {
        let a = BIOS_BASE + i as u16;
        if c[0] > 0 {
            read_in.push(a);
        }
        if c[1] > 0 {
            read_out.push(a);
            read_out_total += u64::from(c[1]);
        }
    }
    let exec_addrs: Vec<u16> = exec
        .iter()
        .enumerate()
        .filter(|(_, &c)| c > 0)
        .map(|(i, _)| BIOS_BASE + i as u16)
        .collect();

    let mut ledger: Vec<((u16, u16), Edge)> = entries.into_iter().collect();
    ledger.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(&b.0)));
    let mut exit_list: Vec<(u16, u64)> = exits.into_iter().collect();
    exit_list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let title = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());

    println!("{title}");
    println!(
        "  {} side(s), ran {ran_frames} frames / {cycles} cycles -> {}",
        sides,
        v.as_str()
    );
    println!(
        "  vectors: nmi ${:04x}  reset ${:04x}  irq ${:04x}",
        vectors.nmi, vectors.reset, vectors.irq
    );
    let tot = ins_inside + ins_outside;
    println!(
        "  instructions: {ins_inside} in BIOS, {ins_outside} outside ({:.1}% outside); \
         tail {tail_inside}/{tail_outside}",
        if tot == 0 { 0.0 } else { 100.0 * ins_outside as f64 / tot as f64 }
    );
    println!("  $4024/$4025 writes: {dw_bios} by BIOS, {dw_game} by the game");
    if autoswap {
        println!("  disk swaps: {swaps} (ended on side {side})");
    }
    println!(
        "  BIOS bytes executed: {} of {}",
        exec_addrs.len(),
        nes_core::fds::BIOS_LEN
    );
    // Lower bound on the vectors specifically: an NMI that interrupts BIOS code
    // fetches $FFFA with the PC still inside, so that fetch lands in the inside
    // bank. The set of addresses is right; the counts for $FFFA-$FFFF are not.
    println!(
        "  BIOS bytes read from outside: {} ({read_out_total} reads) in {} range(s)",
        read_out.len(),
        runs(&read_out).len()
    );
    for (lo, hi) in runs(&read_out) {
        println!("    ${lo:04x}-${hi:04x}");
    }
    // Grouped by target, because the target is the routine and the caller is
    // detail: a game that enters $E18B from two addresses is one entry point,
    // not two, and counting it as two inflates every roll-up built on this.
    let mut by_target: Vec<Target> = group_by_target(&ledger, vectors);
    by_target.sort_by(|a, b| b.count.cmp(&a.count).then(a.to.cmp(&b.to)));
    println!("  entry points ({}):", by_target.len());
    for t in by_target.iter().take(40) {
        println!(
            "    ${:04x}  {:>6} x  {:<5} from {} site(s)  ~{} cyc/stay",
            t.to,
            t.count,
            t.kind,
            t.callers,
            t.cycles.checked_div(t.count).unwrap_or(0)
        );
    }
    if by_target.len() > 40 {
        println!("    ... {} more", by_target.len() - 40);
    }
    println!("  {}", nes.dbg_mapper());

    if let Some(p) = &png_out {
        let fb = nes.step_frame().to_vec();
        if let Err(e) = write_png(std::path::Path::new(p), &fb) {
            eprintln!("cannot write {p}: {e}");
        } else {
            println!("  png: {p}");
        }
    }

    if let Some(p) = &json_out {
        let row = json!({
            "title": title,
            "sides": sides,
            "frames_requested": frames,
            "frames_ran": ran_frames,
            "cycles": cycles,
            "mash": mash,
            "autoswap": autoswap,
            "swaps": swaps,
            "end_side": side,
            "verdict": v.as_str(),
            "vectors": { "nmi": vectors.nmi, "reset": vectors.reset, "irq": vectors.irq },
            "instructions": {
                "inside": ins_inside,
                "outside": ins_outside,
                "tail_inside": tail_inside,
                "tail_outside": tail_outside,
            },
            "diskreg_writes": { "by_bios": dw_bios, "by_game": dw_game },
            "exec_ranges": runs(&exec_addrs),
            "exec_bytes": exec_addrs.len(),
            "read_inside_bytes": read_in.len(),
            "read_inside_ranges": runs(&read_in),
            "read_outside_bytes": read_out.len(),
            "read_outside_reads": read_out_total,
            "read_outside_ranges": runs(&read_out),
            "entries": ledger.iter().map(|((from, to), e)| json!({
                "to": to,
                "from": from,
                "kind": edge_kind(e.opcode, *to, vectors),
                "count": e.count,
                "cycles_in_window": e.cycles_in_window,
            })).collect::<Vec<_>>(),
            "targets": by_target.iter().map(|t| json!({
                "to": t.to,
                "kind": t.kind,
                "count": t.count,
                "callers": t.callers,
                "cycles_in_window": t.cycles,
            })).collect::<Vec<_>>(),
            "exits": exit_list.iter().map(|(a, c)| json!({"from": a, "count": c}))
                .collect::<Vec<_>>(),
            "mapper": nes.dbg_mapper(),
        });
        match serde_json::to_string_pretty(&row) {
            Ok(s) => {
                if let Some(dir) = std::path::Path::new(p).parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(p, s) {
                    eprintln!("cannot write {p}: {e}");
                    return ExitCode::FAILURE;
                }
                println!("  json: {p}");
            }
            Err(e) => {
                eprintln!("cannot encode json: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

fn write_png(path: &std::path::Path, fb: &[u32]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 256, 240);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    let mut rgb = Vec::with_capacity(256 * 240 * 3);
    for &p in fb {
        rgb.push((p >> 16) as u8);
        rgb.push((p >> 8) as u8);
        rgb.push(p as u8);
    }
    w.write_image_data(&rgb)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: Vectors = Vectors {
        nmi: 0xe18b,
        reset: 0xee24,
        irq: 0xe1c7,
    };

    #[test]
    fn runs_collapses_contiguous_and_splits_gaps() {
        assert_eq!(runs(&[]), vec![]);
        assert_eq!(runs(&[0xe000]), vec![(0xe000, 0xe000)]);
        assert_eq!(
            runs(&[0xe000, 0xe001, 0xe002, 0xe007, 0xe008, 0xf800]),
            vec![(0xe000, 0xe002), (0xe007, 0xe008), (0xf800, 0xf800)]
        );
    }

    #[test]
    fn a_vector_target_beats_the_opcode_at_the_pc() {
        // An NMI taken at an instruction boundary leaves the PC on an
        // instruction that never ran, so the byte there means nothing. If this
        // lost to the opcode, every NMI during a JSR would be logged as a call.
        assert_eq!(edge_kind(Some(0x20), V.nmi, V), "nmi");
        assert_eq!(edge_kind(Some(0xa9), V.irq, V), "irq");
        assert_eq!(edge_kind(Some(0xea), V.reset, V), "reset");
    }

    #[test]
    fn call_opcodes_are_named_and_a_non_transfer_opcode_is_an_interrupt() {
        assert_eq!(edge_kind(Some(0x20), 0xe1f8, V), "jsr");
        assert_eq!(edge_kind(Some(0x4c), 0xe1f8, V), "jmp");
        assert_eq!(edge_kind(Some(0x6c), 0xe1f8, V), "jmp()");
        assert_eq!(edge_kind(Some(0x60), 0xe1f8, V), "rts");
        // LDA #imm cannot move the PC, so something else did.
        assert_eq!(edge_kind(Some(0xa9), 0xe1f8, V), "int");
        assert_eq!(edge_kind(None, 0xe1f8, V), "?");
    }

    #[test]
    fn verdict_reads_the_tail_not_the_whole_run() {
        // A booting game spends its first seconds legitimately inside the BIOS.
        // Judging the whole run would call every game BIOS-LOOP.
        assert_eq!(verdict(false, 900_000, 5, 95), Verdict::Boot);
        assert_eq!(verdict(false, 900_000, 99, 1), Verdict::BiosLoop);
        assert_eq!(verdict(false, 900_000, 50, 50), Verdict::Mixed);
    }

    #[test]
    fn halt_and_noload_outrank_the_tail_split() {
        // A jammed CPU stops stepping, so its tail is whatever it was when it
        // jammed; reporting that as BOOT would hide the jam.
        assert_eq!(verdict(true, 900_000, 5, 95), Verdict::Halt);
        // Never left the BIOS at all: the disk did not load, whatever the tail.
        assert_eq!(verdict(false, 0, 100, 0), Verdict::NoLoad);
        assert_eq!(verdict(false, 0, 0, 0), Verdict::NoLoad);
    }

    fn edge(count: u64, cycles: u64, opcode: u8) -> Edge {
        Edge {
            count,
            cycles_in_window: cycles,
            opcode: Some(opcode),
        }
    }

    #[test]
    fn grouping_folds_call_sites_into_one_entry_point() {
        // Zelda enters $E18B from two addresses because the NMI can land on
        // either of two instructions. That is one entry point. Counting it as
        // two inflates the entry-point count for every title in the census.
        let ledger = vec![
            ((0x637b, V.nmi), edge(129, 1677, 0xa9)),
            ((0x637d, V.nmi), edge(119, 1547, 0xa9)),
            ((0x63e1, 0xe9c8), edge(248, 4464, 0x20)),
        ];
        let mut got = group_by_target(&ledger, V);
        got.sort_by_key(|t| t.to);
        assert_eq!(
            got,
            vec![
                Target { to: V.nmi, kind: "nmi", count: 248, cycles: 3224, callers: 2 },
                Target { to: 0xe9c8, kind: "jsr", count: 248, cycles: 4464, callers: 1 },
            ]
        );
    }

    #[test]
    fn the_majority_kind_wins_a_target_reached_two_ways() {
        // A routine called 248 times and interrupted into once is a called
        // routine, not an interrupt vector.
        let ledger = vec![
            ((0x8000, 0xe1f8), edge(248, 0, 0x20)),
            ((0x9000, 0xe1f8), edge(1, 0, 0xa9)),
        ];
        let got = group_by_target(&ledger, V);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, "jsr");
        assert_eq!(got[0].count, 249);
    }

    #[test]
    fn stall_is_the_window_running_no_game_code() {
        assert!(stalled(1000, 0));
        assert!(stalled(1000, 10));
        assert!(!stalled(1000, 500));
        // An empty window is not evidence of a stall; it is evidence of nothing.
        assert!(!stalled(0, 0));
    }

    #[test]
    fn io_space_is_not_peekable_but_ram_and_cart_space_are() {
        // Reading $4031 advances the disk head. A tracer that moves the head is
        // measuring a machine it perturbed.
        assert!(!peekable(0x4031));
        assert!(!peekable(0x2002));
        assert!(peekable(0x0000));
        assert!(peekable(0x1fff));
        assert!(peekable(0x6000));
        assert!(peekable(0xffff));
    }
}
