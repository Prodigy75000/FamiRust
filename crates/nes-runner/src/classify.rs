//! Batch outlier classifier: run every `.nes` in a directory headless and sort
//! the ones that DON'T boot into labelled subfolders, so the owner can Nestopia-
//! check them and prune bad dumps from the true-outlier list.
//!
//!   cargo run --release -p nes-runner --bin classify -- <rom_dir> <out_dir> [frames]
//!
//! Writes <out_dir>/manifest.csv and copies each non-booting ROM into
//!   unsupported_mapper/   -- mapper we don't implement yet (our TODO)
//!   supported_but_black/  -- mapper we DO support but nothing renders (Nestopia-check these)
//!   supported_but_panic/  -- the core panicked on this ROM (likely our bug or a wild dump)
//! Booting ROMs are left out of the folder entirely.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// iNES mapper number straight from the 16-byte header (works even when the core
/// refuses to load the ROM), incl. the NES 2.0 high nibble.
fn header_mapper(d: &[u8]) -> Option<u16> {
    if d.len() < 16 || &d[0..4] != b"NES\x1a" {
        return None;
    }
    let mut m = ((d[6] >> 4) as u16) | ((d[7] & 0xf0) as u16);
    // NES 2.0 (byte 7 bits 2-3 == 2) extends the mapper with byte 8's low nibble.
    if (d[7] & 0x0c) == 0x08 {
        m |= ((d[8] & 0x0f) as u16) << 8;
    }
    Some(m)
}

/// Run a ROM for `frames` frames and report the peak non-backdrop pixel count and
/// peak distinct-colour count seen across the whole run (peak, so a game that
/// draws a logo then blanks still counts as "renders"). None => panicked.
fn run_peaks(rom: &[u8], frames: u32) -> Option<(usize, usize)> {
    catch_unwind(AssertUnwindSafe(|| {
        let mut nes = match nes_core::Nes::from_rom(rom) {
            Ok(n) => n,
            Err(_) => return (0usize, 0usize),
        };
        let mut peak_px = 0usize;
        let mut peak_col = 0usize;
        for _ in 0..frames {
            let fb = nes.step_frame();
            let backdrop = fb.first().copied().unwrap_or(0);
            let px = fb.iter().filter(|&&p| p != backdrop).count();
            if px > peak_px {
                peak_px = px;
                let mut c: Vec<u32> = fb.to_vec();
                c.sort_unstable();
                c.dedup();
                peak_col = c.len();
            }
        }
        (peak_px, peak_col)
    }))
    .ok()
}

/// Recursively collect every `.nes` under `dir` (the ROM set is split into
/// region subfolders: USA/, Europe/, Japan/, …).
fn collect_nes(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_nes(&p, out);
        } else if p.extension().map(|x| x.eq_ignore_ascii_case("nes")).unwrap_or(false) {
            out.push(p);
        }
    }
}

fn main() {
    // Silence per-ROM panic backtraces; catch_unwind still reports them to us.
    std::panic::set_hook(Box::new(|_| {}));

    let mut args = std::env::args().skip(1);
    let rom_dir = args.next().expect("usage: classify <rom_dir> <out_dir> [frames]");
    let out_dir = args.next().expect("usage: classify <rom_dir> <out_dir> [frames]");
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600);
    // Peak non-backdrop pixels at/under this => "renders nothing".
    let black_threshold = 100usize;

    let out = Path::new(&out_dir);
    for sub in ["unsupported_mapper", "supported_but_black", "supported_but_panic"] {
        std::fs::create_dir_all(out.join(sub)).unwrap();
    }

    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    collect_nes(Path::new(&rom_dir), &mut entries);
    entries.sort();

    let mut csv = String::from("filename,mapper,status,peak_pixels,peak_colors\n");
    let (mut n_ok, mut n_unsup, mut n_black, mut n_panic, mut n_badhdr) = (0, 0, 0, 0, 0);
    let total = entries.len();

    let root = Path::new(&rom_dir);
    for (i, path) in entries.iter().enumerate() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Path relative to the ROM root (e.g. "USA/Punch-Out.nes") for the manifest,
        // and a collision-proof copy name prefixed with the region folder.
        let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
        let region = path.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let copy_name = format!("{region}__{name}");
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mapper = header_mapper(&bytes);
        // Does the core accept this mapper? (parse only; cheap)
        let supported = nes_core::Nes::from_rom(&bytes).is_ok();

        let (status, px, col, dest): (&str, usize, usize, Option<&str>) = if mapper.is_none() {
            ("BAD_HEADER", 0, 0, Some("supported_but_panic"))
        } else if !supported {
            ("UNSUPPORTED_MAPPER", 0, 0, Some("unsupported_mapper"))
        } else {
            match run_peaks(&bytes, frames) {
                None => ("PANIC", 0, 0, Some("supported_but_panic")),
                Some((px, col)) if px <= black_threshold => {
                    ("BLACK", px, col, Some("supported_but_black"))
                }
                Some((px, col)) => ("OK", px, col, None),
            }
        };

        let m = mapper.map(|m| m.to_string()).unwrap_or_else(|| "?".into());
        csv.push_str(&format!("{rel:?},{m},{status},{px},{col}\n"));
        match status {
            "OK" => n_ok += 1,
            "UNSUPPORTED_MAPPER" => n_unsup += 1,
            "BLACK" => n_black += 1,
            "PANIC" => n_panic += 1,
            _ => n_badhdr += 1,
        }
        if let Some(sub) = dest {
            let _ = std::fs::copy(path, out.join(sub).join(&copy_name));
        }
        if i % 100 == 0 || i + 1 == total {
            eprintln!("[{}/{}] {name} -> {status}", i + 1, total);
        }
    }

    std::fs::write(out.join("manifest.csv"), &csv).unwrap();
    let summary = format!(
        "classified {total} ROMs @{frames} frames (black<= {black_threshold}px):\n  \
         OK(boots)            {n_ok}\n  \
         UNSUPPORTED_MAPPER   {n_unsup}  -> unsupported_mapper/\n  \
         BLACK(renders none)  {n_black}  -> supported_but_black/\n  \
         PANIC                {n_panic}  -> supported_but_panic/\n  \
         BAD_HEADER           {n_badhdr}\n"
    );
    std::fs::write(out.join("SUMMARY.txt"), &summary).unwrap();
    eprintln!("\n{summary}\nmanifest + SUMMARY written to {out_dir}");
}
