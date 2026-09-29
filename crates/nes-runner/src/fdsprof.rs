// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! `fdsprof`: what one FDS BIOS routine does, from the outside.
//!
//! Step four of replacing `disksys.rom` is reimplementing the 37 entry points
//! the census found games calling but the HLE does not yet have. To write a
//! routine you need its interface: what it takes, what it touches, what it
//! leaves behind. This works that out by watching, because the one thing this
//! project may not do is read Nintendo's code.
//!
//! **A routine's interface is the set of addresses it reads and writes.** So
//! the machine is put under an access log for the length of one call and the
//! log is what gets reported: registers in, registers out, every byte of RAM
//! and every hardware register touched on the way, and what it cost.
//!
//! Only calls made FROM GAME CODE are profiled. The BIOS calls itself
//! constantly and those calls are somebody else's argument; what matters is the
//! published edge, which is the one a game crosses.
//!
//!   cargo run --release -p nes-runner --bin fdsprof -- disk.fds --routine EAEA
//!   cargo run --release -p nes-runner --bin fdsprof -- disk.fds --routine E1F8 --json p.json
//!
//! The BIOS comes from `$FDS_BIOS` or `disksys.rom` beside the disk. Unlike the
//! other tools here this one wants the REAL BIOS: it is measuring the thing
//! being reimplemented.
//!
//! ## One thing the cycle counts do not include
//!
//! Cycles come from the CPU's own counter, which ticks once per bus access the
//! CPU makes. A `$4014` write stalls the CPU for 513 cycles while the DMA unit
//! drives the bus instead, and the CPU makes no accesses during those, so they
//! are not counted. A routine that does an OAM DMA therefore reports about 18
//! cycles rather than about 530. The PPU and APU do advance correctly; it is
//! the reported number that is short, and only for DMA.

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde_json::json;

const BIOS_BASE: u16 = 0xe000;

/// One call to the routine, seen from outside.
struct Call {
    caller: u16,
    a_in: u8,
    x_in: u8,
    y_in: u8,
    p_in: u8,
    sp_in: u8,
    a_out: u8,
    x_out: u8,
    y_out: u8,
    p_out: u8,
    sp_out: u8,
    exit_pc: u16,
    cycles: u64,
    /// Accesses outside the BIOS window: RAM, the stack, hardware, program RAM.
    /// Everything inside it is the routine fetching itself.
    log: Vec<(u16, u8, bool)>,
    /// True if the stay ended because the routine ran away rather than returned.
    ran_away: bool,
}

impl Call {
    /// How control left. `rts` puts the stack back two bytes higher than a
    /// `jsr` left it; `jmp` leaves it alone.
    fn exit_kind(&self) -> &'static str {
        match self.sp_out.wrapping_sub(self.sp_in) {
            2 => "rts",
            0 => "jmp",
            _ => "?",
        }
    }
}

/// What every observed call agrees on, or `None` if they disagree.
fn constant<T: PartialEq + Copy>(vals: impl Iterator<Item = T>) -> Option<T> {
    let mut it = vals;
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

fn reg(name: &str, vals: impl Iterator<Item = u8>) -> String {
    match constant(vals) {
        Some(v) => format!("{name}=${v:02x}"),
        None => format!("{name}=.."),
    }
}

/// Profile up to `want` calls to `target` made from outside the BIOS window.
fn profile(
    disk: &[u8],
    bios: &[u8],
    target: u16,
    want: usize,
    max_frames: u64,
) -> Result<Vec<Call>, String> {
    let mut nes = nes_core::Nes::from_fds(disk, bios).map_err(|e| format!("{e:?}"))?;
    let mut calls = Vec::new();
    let mut prev = nes.dbg_pc();
    while nes.dbg_frame() < max_frames && calls.len() < want {
        let pc = nes.dbg_pc();
        // Entered at the routine, and from game code rather than from the BIOS
        // calling itself.
        if pc == target && prev < BIOS_BASE {
            let (a_in, x_in, y_in, p_in, sp_in) =
                (nes.cpu.a, nes.cpu.x, nes.cpu.y, nes.cpu.p, nes.cpu.sp);
            let start = nes.dbg_cycles();
            nes.dbg_log_start(1 << 16);
            let mut ran_away = false;
            let mut steps = 0u32;
            loop {
                nes.step();
                steps += 1;
                let now = nes.dbg_pc();
                // Out of the window AND the stack is not deeper than it was.
                // Without the second half, an NMI taken mid-routine and
                // dispatched to the game would read as the routine returning.
                if now < BIOS_BASE && nes.cpu.sp >= sp_in {
                    break;
                }
                if nes.dbg_halted() || steps > 8_000_000 {
                    ran_away = true;
                    break;
                }
            }
            let log: Vec<_> = nes
                .dbg_log_take()
                .into_iter()
                .filter(|(a, _, _)| *a < BIOS_BASE)
                .collect();
            calls.push(Call {
                caller: prev,
                a_in,
                x_in,
                y_in,
                p_in,
                sp_in,
                a_out: nes.cpu.a,
                x_out: nes.cpu.x,
                y_out: nes.cpu.y,
                p_out: nes.cpu.p,
                sp_out: nes.cpu.sp,
                exit_pc: nes.dbg_pc(),
                cycles: nes.dbg_cycles() - start,
                log,
                ran_away,
            });
            prev = nes.dbg_pc();
            continue;
        }
        prev = pc;
        nes.step();
        if nes.dbg_halted() {
            break;
        }
    }
    Ok(calls)
}

/// Per-address totals across every call.
#[derive(Default, Clone)]
struct Touch {
    reads: u32,
    writes: u32,
    /// Distinct values written, capped so a data copy does not print a novel.
    values: Vec<u8>,
}

fn fold(calls: &[Call]) -> BTreeMap<u16, Touch> {
    let mut m: BTreeMap<u16, Touch> = BTreeMap::new();
    for c in calls {
        for &(a, v, w) in &c.log {
            let t = m.entry(a).or_default();
            if w {
                t.writes += 1;
                if t.values.len() < 8 && !t.values.contains(&v) {
                    t.values.push(v);
                }
            } else {
                t.reads += 1;
            }
        }
    }
    m
}

/// Collapse a sorted address list into runs, so a 6 KiB copy is one line.
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

/// Which side of the machine an address is on. The point of the split is that
/// hardware and the pseudo-vectors are interface and work RAM is state.
fn region(a: u16) -> &'static str {
    match a {
        0x0000..=0x00ff => "zp",
        0x0100..=0x01ff => "stack",
        0x0200..=0x07ff => "ram",
        0x2000..=0x3fff => "ppu",
        0x4000..=0x401f => "apu",
        0x4020..=0x40ff => "fds",
        0xdff6..=0xdfff => "vectors",
        0x6000..=0xdff5 => "prgram",
        _ => "other",
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: fdsprof <disk.fds> --routine HEX [--calls N] [--frames N] [--json out]");
    eprintln!("  --routine A  the BIOS entry address to profile, in hex (e.g. EAEA)");
    eprintln!("  --calls N    how many calls to record (default 8)");
    eprintln!("  --trace      print the access log of the first call, in order");
    eprintln!("  --trace-max N  how much of it to print (default 80; a load is long)");
    eprintln!("  $FDS_BIOS    the real disksys.rom; this tool measures what it does");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path = String::new();
    let mut routine: Option<u16> = None;
    let mut calls_wanted = 8usize;
    let mut frames = 3000u64;
    let mut json_out: Option<String> = None;
    let mut trace = false;
    let mut trace_max = 80usize;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--routine" => {
                match args.next().and_then(|v| {
                    u16::from_str_radix(v.trim_start_matches('$').trim_start_matches("0x"), 16).ok()
                }) {
                    Some(v) => routine = Some(v),
                    None => return usage(),
                }
            }
            "--calls" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => calls_wanted = n,
                None => return usage(),
            },
            "--frames" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => frames = n,
                None => return usage(),
            },
            "--json" => match args.next() {
                Some(v) => json_out = Some(v),
                None => return usage(),
            },
            "--trace" => trace = true,
            "--trace-max" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => {
                    trace_max = n;
                    trace = true;
                }
                None => return usage(),
            },
            "-h" | "--help" => return usage(),
            other if other.starts_with("--") => {
                eprintln!("fdsprof: unknown option {other}");
                return usage();
            }
            other => path = other.to_string(),
        }
    }
    let (Some(target), false) = (routine, path.is_empty()) else {
        return usage();
    };

    let disk = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bios_path = std::env::var("FDS_BIOS").unwrap_or_else(|_| {
        std::path::Path::new(&path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("disksys.rom")
            .to_string_lossy()
            .into_owned()
    });
    let bios = std::fs::read(&bios_path).unwrap_or_default();
    if bios.is_empty() {
        eprintln!("fdsprof: no BIOS at {bios_path}. This tool profiles the REAL one.");
        return ExitCode::FAILURE;
    }

    let calls = match profile(&disk, &bios, target, calls_wanted, frames) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let title = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());

    println!("${target:04x}  {title}");
    if calls.is_empty() {
        println!("  never called from game code in {frames} frames");
        if let Some(p) = &json_out {
            let _ = std::fs::write(p, json!({"routine": target, "title": title, "calls": 0}).to_string());
        }
        return ExitCode::SUCCESS;
    }

    println!("  {} call(s) from game code", calls.len());
    println!(
        "  in    {} {} {} {} sp=${:02x}",
        reg("a", calls.iter().map(|c| c.a_in)),
        reg("x", calls.iter().map(|c| c.x_in)),
        reg("y", calls.iter().map(|c| c.y_in)),
        reg("p", calls.iter().map(|c| c.p_in)),
        calls[0].sp_in
    );
    println!(
        "  out   {} {} {} {}  kind={}  cycles {}..{}",
        reg("a", calls.iter().map(|c| c.a_out)),
        reg("x", calls.iter().map(|c| c.x_out)),
        reg("y", calls.iter().map(|c| c.y_out)),
        reg("p", calls.iter().map(|c| c.p_out)),
        constant(calls.iter().map(|c| c.exit_kind())).unwrap_or("mixed"),
        calls.iter().map(|c| c.cycles).min().unwrap(),
        calls.iter().map(|c| c.cycles).max().unwrap(),
    );
    if calls.iter().any(|c| c.ran_away) {
        println!("  NOTE: at least one call never returned");
    }

    let touched = fold(&calls);
    for (what, want_write) in [("reads ", false), ("writes", true)] {
        let mut by_region: BTreeMap<&str, Vec<u16>> = BTreeMap::new();
        for (&a, t) in &touched {
            let n = if want_write { t.writes } else { t.reads };
            if n > 0 {
                by_region.entry(region(a)).or_default().push(a);
            }
        }
        if by_region.is_empty() {
            continue;
        }
        println!("  {what}:");
        for (r, addrs) in by_region {
            let rs = runs(&addrs);
            let shown: Vec<String> = rs
                .iter()
                .take(10)
                .map(|(lo, hi)| {
                    if lo == hi {
                        let t = &touched[lo];
                        if want_write && !t.values.is_empty() {
                            let vals: Vec<String> =
                                t.values.iter().map(|v| format!("{v:02x}")).collect();
                            format!("${lo:04x}=${}", vals.join("/"))
                        } else {
                            format!("${lo:04x}")
                        }
                    } else {
                        format!("${lo:04x}-${hi:04x}")
                    }
                })
                .collect();
            let more = if rs.len() > 10 {
                format!(" (+{} more)", rs.len() - 10)
            } else {
                String::new()
            };
            println!("    {r:<8} {}{}", shown.join(" "), more);
        }
    }

    if trace {
        println!("  first call, in order ({} accesses):", calls[0].log.len());
        for (i, (a, v, w)) in calls[0].log.iter().take(trace_max).enumerate() {
            println!("    {i:>3} {} ${a:04x} = ${v:02x}", if *w { "W" } else { "R" });
        }
        if calls[0].log.len() > trace_max {
            println!("    ... {} more", calls[0].log.len() - trace_max);
        }
    }

    if let Some(p) = &json_out {
        let row = json!({
            "routine": target,
            "title": title,
            "calls": calls.len(),
            "callers": calls.iter().map(|c| c.caller).collect::<Vec<_>>(),
            "returns_to": calls.iter().map(|c| c.exit_pc).collect::<Vec<_>>(),
            "in": {
                "a": constant(calls.iter().map(|c| c.a_in)),
                "x": constant(calls.iter().map(|c| c.x_in)),
                "y": constant(calls.iter().map(|c| c.y_in)),
                "p": constant(calls.iter().map(|c| c.p_in)),
            },
            "out": {
                "a": constant(calls.iter().map(|c| c.a_out)),
                "x": constant(calls.iter().map(|c| c.x_out)),
                "y": constant(calls.iter().map(|c| c.y_out)),
                "p": constant(calls.iter().map(|c| c.p_out)),
            },
            "kind": constant(calls.iter().map(|c| c.exit_kind())),
            "cycles_min": calls.iter().map(|c| c.cycles).min(),
            "cycles_max": calls.iter().map(|c| c.cycles).max(),
            "touched": touched.iter().map(|(a, t)| json!({
                "addr": a,
                "region": region(*a),
                "reads": t.reads,
                "writes": t.writes,
                "values": t.values,
            })).collect::<Vec<_>>(),
        });
        if let Err(e) = std::fs::write(p, serde_json::to_string_pretty(&row).unwrap_or_default()) {
            eprintln!("cannot write {p}: {e}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_reports_agreement_and_disagreement() {
        assert_eq!(constant([1u8, 1, 1].into_iter()), Some(1));
        assert_eq!(constant([1u8, 2, 1].into_iter()), None);
        assert_eq!(constant(std::iter::empty::<u8>()), None);
    }

    #[test]
    fn runs_collapses_a_copy_into_one_line() {
        assert_eq!(
            runs(&[0x6000, 0x6001, 0x6002, 0x7000]),
            vec![(0x6000, 0x6002), (0x7000, 0x7000)]
        );
        assert_eq!(runs(&[]), vec![]);
    }

    #[test]
    fn the_pseudo_vectors_are_their_own_region() {
        // $DFF6-$DFFF sits inside program RAM but is interface, not state, and
        // a routine touching it means something quite different.
        assert_eq!(region(0xdff6), "vectors");
        assert_eq!(region(0xdfff), "vectors");
        assert_eq!(region(0xdff5), "prgram");
        assert_eq!(region(0x4025), "fds");
        assert_eq!(region(0x0100), "stack");
    }

    #[test]
    fn exit_kind_reads_the_stack_not_the_address() {
        let mk = |sp_in: u8, sp_out: u8| Call {
            caller: 0,
            a_in: 0, x_in: 0, y_in: 0, p_in: 0, sp_in,
            a_out: 0, x_out: 0, y_out: 0, p_out: 0, sp_out,
            exit_pc: 0, cycles: 0, log: Vec::new(), ran_away: false,
        };
        // A jsr left the stack two bytes down; an rts puts them back.
        assert_eq!(mk(0xfb, 0xfd).exit_kind(), "rts");
        assert_eq!(mk(0xfb, 0xfb).exit_kind(), "jmp");
        assert_eq!(mk(0xfb, 0xf0).exit_kind(), "?");
    }
}
