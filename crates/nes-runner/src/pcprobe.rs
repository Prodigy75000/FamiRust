//! Where is a hanging ROM stuck? Histogram the CPU PC into 256-byte buckets.
//!   cargo run --release -p nes-runner --bin pcprobe -- <rom.nes> [steps]
use std::collections::HashMap;

fn main() {
    let mut a = std::env::args().skip(1);
    let rom = std::fs::read(a.next().expect("rom")).unwrap();
    let mut nes = nes_core::Nes::from_rom(&rom).unwrap();
    let steps: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(30_000_000);
    // Optional 3rd arg "start" -> pulse the Start button so we reach gameplay.
    let press_start = a.next().map(|s| s == "start").unwrap_or(false);
    let mut frame_cyc = 0u64;
    let mut hist: HashMap<u16, u64> = HashMap::new();
    let mut last = 0u16;
    let mut ring = [0u16; 16];
    let mut ri = 0usize;
    let mut caught_crash = false;
    for _ in 0..steps {
        last = nes.dbg_pc();
        // First time the PC enters the $2000-$5FFF window (PPU/APU regs, not code),
        // dump the trail that led there -- that's the bad jump.
        if !caught_crash && (0x2000..0x6000).contains(&last) {
            caught_crash = true;
            eprintln!("CRASH: PC entered {last:04X} (register space). Trail:");
            for k in 0..16 {
                eprintln!("  {:04X}", ring[(ri + k) % 16]);
            }
        }
        ring[ri % 16] = last;
        ri += 1;
        *hist.entry(last & 0xff00).or_insert(0) += 1;
        // ~29780 CPU cyc/frame; pulse Start every other "frame" once past boot.
        if press_start {
            frame_cyc += 1;
            if frame_cyc > 30_000 * 60 {
                nes.set_buttons(0, if (frame_cyc / 90_000) % 2 == 0 { 0x08 } else { 0 });
            }
        }
        nes.step();
    }
    eprintln!("final ppu_mask={:02X} rendering={} CPU_HALTED={}", nes.dbg_ppu_mask(), nes.dbg_ppu_mask() & 0x18 != 0, nes.dbg_halted());
    eprint!("last 16 PCs:");
    for k in 0..16 {
        eprint!(" {:04X}", ring[(ri + k) % 16]);
    }
    eprintln!();
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("top PC buckets over {steps} steps (final pc={:04X}):", last);
    for (bucket, count) in v.into_iter().take(8) {
        eprintln!("  {:04X}xx : {}", bucket >> 8, count);
    }
    // Dump the live (currently-banked) bytes around the stuck PC for disassembly.
    let start = last.wrapping_sub(12);
    eprint!("bytes @{:04X}:", start);
    for i in 0..48u16 {
        eprint!(" {:02X}", nes.peek(start.wrapping_add(i)));
    }
    eprintln!();
    eprintln!("ppuctrl(dbg via mask n/a) mask={:02X}", nes.dbg_ppu_mask());
}
