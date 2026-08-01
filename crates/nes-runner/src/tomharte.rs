//! TomHarte `nes6502` single-step conformance harness.
//!
//! Each vector fully specifies an initial CPU + flat-memory state, runs exactly
//! one instruction, and gives the expected final state plus the exact per-cycle
//! bus trace (`[addr, value, "read"|"write"]`). Because our CPU does exactly one
//! bus access per cycle, a logging bus reproduces that trace directly — so we
//! check final registers, final memory, AND the cycle-by-cycle bus log.
//!
//! Vectors are vendored (gitignored) under
//! `FamiRust/tests/vendor/nes6502/v1/<opcode>.json`. Run:
//!   cargo run -p nes-runner --bin tomharte              # all files present
//!   cargo run -p nes-runner --bin tomharte -- a9 69 6c  # specific opcodes

use nes_core::cpu::{Cpu, CpuBus};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Test {
    name: String,
    initial: State,
    #[serde(rename = "final")]
    final_: State,
    cycles: Vec<(u16, u8, String)>,
}

#[derive(Deserialize)]
struct State {
    pc: u16,
    s: u8,
    a: u8,
    x: u8,
    y: u8,
    p: u8,
    ram: Vec<(u16, u8)>,
}

/// Flat 64 KiB memory that logs every access, in order, as the CPU makes it.
struct LogBus {
    mem: Box<[u8; 0x10000]>,
    log: Vec<(u16, u8, &'static str)>,
}
impl CpuBus for LogBus {
    fn read(&mut self, addr: u16) -> u8 {
        let v = self.mem[addr as usize];
        self.log.push((addr, v, "read"));
        v
    }
    fn write(&mut self, addr: u16, val: u8) {
        self.mem[addr as usize] = val;
        self.log.push((addr, val, "write"));
    }
}

#[derive(Default)]
struct Stats {
    total: u64,
    passed: u64,
    fail_regs: u64,
    fail_mem: u64,
    fail_cycles: u64,
    /// Per-P-bit mismatch tally, to diagnose whether failures are only in the
    /// B/unused bits (which would call for comparison normalization).
    p_bit_mismatch: [u64; 8],
    first_fail: Option<String>,
}

fn run_test(t: &Test, stats: &mut Stats) {
    let mut bus = LogBus {
        mem: Box::new([0u8; 0x10000]),
        log: Vec::with_capacity(16),
    };
    for &(addr, val) in &t.initial.ram {
        bus.mem[addr as usize] = val;
    }
    let mut cpu = Cpu::new();
    cpu.pc = t.initial.pc;
    cpu.sp = t.initial.s;
    cpu.a = t.initial.a;
    cpu.x = t.initial.x;
    cpu.y = t.initial.y;
    cpu.p = t.initial.p;
    cpu.cycles = 0;

    cpu.step(&mut bus);

    stats.total += 1;

    let f = &t.final_;
    let mut ok = true;

    // Registers.
    let regs_ok = cpu.pc == f.pc
        && cpu.sp == f.s
        && cpu.a == f.a
        && cpu.x == f.x
        && cpu.y == f.y
        && cpu.p == f.p;
    if !regs_ok {
        ok = false;
        stats.fail_regs += 1;
        let diff = cpu.p ^ f.p;
        for bit in 0..8 {
            if diff & (1 << bit) != 0 {
                stats.p_bit_mismatch[bit] += 1;
            }
        }
    }

    // Final memory.
    let mut mem_ok = true;
    for &(addr, val) in &f.ram {
        if bus.mem[addr as usize] != val {
            mem_ok = false;
        }
    }
    if !mem_ok {
        ok = false;
        stats.fail_mem += 1;
    }

    // Cycle-by-cycle bus trace.
    let mut cyc_ok = bus.log.len() == t.cycles.len();
    if cyc_ok {
        for (got, exp) in bus.log.iter().zip(t.cycles.iter()) {
            if got.0 != exp.0 || got.1 != exp.1 || got.2 != exp.2 {
                cyc_ok = false;
                break;
            }
        }
    }
    if !cyc_ok {
        ok = false;
        stats.fail_cycles += 1;
    }

    if ok {
        stats.passed += 1;
    } else if stats.first_fail.is_none() {
        stats.first_fail = Some(format!(
            "'{}': regs_ok={} mem_ok={} cyc_ok={}\n  got  pc={:04X} s={:02X} a={:02X} x={:02X} y={:02X} p={:02X} ({} cyc)\n  want pc={:04X} s={:02X} a={:02X} x={:02X} y={:02X} p={:02X} ({} cyc)",
            t.name, regs_ok, mem_ok, cyc_ok,
            cpu.pc, cpu.sp, cpu.a, cpu.x, cpu.y, cpu.p, bus.log.len(),
            f.pc, f.s, f.a, f.x, f.y, f.p, t.cycles.len()
        ));
    }
}

fn opcode_file(dir: &Path, stem: &str) -> Option<PathBuf> {
    for ext in ["json", "json.gz"] {
        let p = dir.join(format!("{stem}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn load_tests(path: &Path) -> Result<Vec<Test>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let json: Vec<u8> = if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut d = GzDecoder::new(&bytes[..]);
        let mut out = Vec::new();
        d.read_to_end(&mut out).map_err(|e| format!("gunzip: {e}"))?;
        out
    } else {
        bytes
    };
    serde_json::from_slice(&json).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/vendor/nes6502/v1");
    if !dir.exists() {
        eprintln!(
            "vectors not found at {}\n\
             Fetch the TomHarte nes6502 v1 set into that directory (gitignored), e.g.\n\
             curl -L -o <op>.json https://raw.githubusercontent.com/SingleStepTests/65x02/main/nes6502/v1/<op>.json",
            dir.display()
        );
        std::process::exit(2);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let stems: Vec<String> = if !args.is_empty() {
        args
    } else {
        (0u16..=0xff).map(|op| format!("{op:02x}")).collect()
    };

    let mut stats = Stats::default();
    let mut files_run = 0u32;
    let mut missing = 0u32;
    let mut worst: Vec<(String, u64, u64)> = Vec::new(); // (op, passed, total)

    for stem in &stems {
        let Some(path) = opcode_file(&dir, stem) else {
            missing += 1;
            continue;
        };
        let tests = match load_tests(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skip {stem}: {e}");
                continue;
            }
        };
        files_run += 1;
        let before = (stats.passed, stats.total);
        for t in &tests {
            run_test(t, &mut stats);
        }
        let op_passed = stats.passed - before.0;
        let op_total = stats.total - before.1;
        if op_passed != op_total {
            worst.push((stem.clone(), op_passed, op_total));
        }
    }

    println!("== TomHarte nes6502 ==");
    println!("files: {files_run} run, {missing} missing (of {} requested)", stems.len());
    println!(
        "tests: {}/{} passed ({:.2}%)",
        stats.passed,
        stats.total,
        if stats.total > 0 {
            100.0 * stats.passed as f64 / stats.total as f64
        } else {
            0.0
        }
    );
    println!(
        "failures: regs={} mem={} cycles={}",
        stats.fail_regs, stats.fail_mem, stats.fail_cycles
    );
    if stats.fail_regs > 0 {
        println!("P-bit mismatches (bit0..7): {:?}", stats.p_bit_mismatch);
    }
    if let Some(ff) = &stats.first_fail {
        println!("first failure:\n{ff}");
    }
    if !worst.is_empty() {
        worst.sort_by_key(|(_, p, t)| *t - *p); // most failures first
        worst.reverse();
        println!("opcodes with failures ({} of {files_run}):", worst.len());
        for (op, p, t) in worst.iter().take(24) {
            println!("  {op}: {p}/{t}");
        }
    }

    if stats.total > 0 && stats.passed == stats.total {
        println!("ALL GREEN");
    }
}
