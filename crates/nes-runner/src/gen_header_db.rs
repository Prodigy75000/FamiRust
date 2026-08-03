//! Generate the `header_db::HEADER_FIXES` table by boot-verifying mapper
//! corrections against this core. For every ROM whose iNES header matches a
//! known-mislabel PATTERN, we render it both under the header's mapper and under
//! the proposed correction; we emit an entry ONLY when the correction renders and
//! the header does not. That keeps the table our own empirical measurement.
//!
//!   cargo run --release -p nes-runner --bin gen_header_db -- <rom_dir> [frames]
//!
//! Paste the printed `(0x…, N),` lines into crates/nes-core/src/header_db.rs.
//! Run with the DB table EMPTY (otherwise the header-mapper render would already
//! be corrected and the black-check would fail).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Peak non-backdrop pixel count across `frames` (0 if the ROM won't load).
fn peak_px(rom: &[u8], frames: u32) -> usize {
    let mut nes = match nes_core::Nes::from_rom(rom) {
        Ok(n) => n,
        Err(_) => return 0,
    };
    let mut peak = 0;
    for _ in 0..frames {
        let fb = nes.step_frame();
        let bd = fb.first().copied().unwrap_or(0);
        let px = fb.iter().filter(|&&p| p != bd).count();
        if px > peak {
            peak = px;
        }
    }
    peak
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().map(|x| x.eq_ignore_ascii_case("nes")).unwrap_or(false) {
            out.push(p);
        }
    }
}

/// A mislabel pattern: `(header_mapper, requires_chr_ram, corrected_mapper)`.
/// GxROM(66)-tagged carts with CHR-RAM are the big one — real GxROM always has
/// CHR ROM, so CHR-RAM + m66 is an UNROM(2) cart every time we've checked.
const PATTERNS: &[(u16, bool, u16)] = &[(66, true, 2)];

fn main() {
    let dir = std::env::args().nth(1).expect("usage: gen_header_db <rom_dir> [frames]");
    let frames: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(400);
    let thresh = 100usize;

    let mut roms = Vec::new();
    collect(Path::new(&dir), &mut roms);
    roms.sort();

    let mut fixes: BTreeMap<u32, (u16, String)> = BTreeMap::new(); // crc -> (mapper, name)
    let (mut tested, mut added, mut skipped) = (0, 0, 0);

    for p in &roms {
        let d = match std::fs::read(p) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if d.len() < 16 || &d[0..4] != b"NES\x1a" {
            continue;
        }
        if (d[7] & 0x0c) == 0x08 {
            continue; // trust curated NES 2.0 headers
        }
        let hmap = ((d[7] & 0xf0) | (d[6] >> 4)) as u16;
        let low = (d[6] >> 4) as u16;
        let chr_ram = d[5] == 0;
        // Two sources of a correction candidate:
        //  (a) a known mislabel PATTERN keyed on the naive (both-nibble) mapper;
        //  (b) DiskDude/archaic recovery: byte 7 is clobbered ('D'=0x44), so the
        //      core now parses mapper = low nibble only. That's right when the real
        //      mapper is <16, but wrong for >=16 carts (RAMBO-1=64, ...) where the
        //      high nibble was real. Recover it by testing 0x40|low (the value the
        //      corrupt 'D' nibble encodes) and keep it only if it boots and the
        //      default low-nibble parse does not.
        let archaic = (d[7] & 0x0c) != 0;
        let cm: u16 = if let Some(&(_, _, c)) =
            PATTERNS.iter().find(|&&(m, needs_ram, _)| m == hmap && (!needs_ram || chr_ram))
        {
            c
        } else if archaic && (0x40 | low) != low {
            0x40 | low
        } else {
            continue;
        };
        tested += 1;

        // Render under the correction (forced via a CLEAN header) and under the
        // core's current default parse of the raw bytes.
        let mut fixed = d.clone();
        fixed[6] = (fixed[6] & 0x0f) | (((cm as u8) & 0x0f) << 4);
        fixed[7] = (cm as u8) & 0xf0; // clean byte 7 (low bits 0 => reliable, no archaic drop)
        let corrected_px = peak_px(&fixed, frames);
        let header_px = peak_px(&d, frames);

        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if corrected_px > thresh && header_px <= thresh {
            let crc = nes_core::header_db::rom_data_crc32(&d);
            // Keep one entry per unique ROM data; note collisions.
            if let Some((prev_m, prev_n)) = fixes.get(&crc) {
                if *prev_m != cm {
                    eprintln!("! crc {crc:08x} collision: {prev_n}->{prev_m} vs {name}->{cm}");
                }
            }
            fixes.insert(crc, (cm, name.clone()));
            added += 1;
            eprintln!("+ {name}  m{hmap}->{cm}  crc={crc:08x}  ({corrected_px}px vs header {header_px}px)");
        } else {
            skipped += 1;
            eprintln!("- {name}  SKIP (corrected {corrected_px}px, header {header_px}px)");
        }
    }

    println!("    // {added} entries generated from {tested} candidates ({dir})");
    for (crc, (m, name)) in &fixes {
        println!("    (0x{crc:08x}, HeaderFix::mapper({m})), // {name}");
    }
    eprintln!("\n=== {added} added, {skipped} skipped, {tested} candidates, {} unique ROMs ===", fixes.len());
}
