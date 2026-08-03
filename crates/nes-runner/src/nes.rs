//! Headless dev harness: load a `.nes`, run N frames, dump the last frame as a
//! PNG. Mirrors the `snes` / `n64` bins in the sibling cores. Reports pixel
//! statistics (the user judges the render; the harness reports measurable facts).
//!
//!   cargo run -p nes-runner --bin nes -- <rom.nes> [frames] [out.png]

use std::io::Write;
use std::process::ExitCode;

/// Write f32 samples (roughly -1..1) as a 16-bit PCM mono WAV at 44.1 kHz.
fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let sr: u32 = nes_core::SAMPLE_RATE;
    let data_len = (samples.len() * 2) as u32;
    let Ok(mut f) = std::fs::File::create(path).map(std::io::BufWriter::new) else {
        eprintln!("cannot write {}", path.display());
        return;
    };
    let _ = f.write_all(b"RIFF");
    let _ = f.write_all(&(36 + data_len).to_le_bytes());
    let _ = f.write_all(b"WAVEfmt ");
    let _ = f.write_all(&16u32.to_le_bytes()); // fmt chunk size
    let _ = f.write_all(&1u16.to_le_bytes()); // PCM
    let _ = f.write_all(&1u16.to_le_bytes()); // mono
    let _ = f.write_all(&sr.to_le_bytes());
    let _ = f.write_all(&(sr * 2).to_le_bytes()); // byte rate
    let _ = f.write_all(&2u16.to_le_bytes()); // block align
    let _ = f.write_all(&16u16.to_le_bytes()); // bits/sample
    let _ = f.write_all(b"data");
    let _ = f.write_all(&data_len.to_le_bytes());
    // Normalize a little and clamp; the mixer output is ~0..0.25 so apply gain.
    for &s in samples {
        let v = (s * 4.0).clamp(-1.0, 1.0);
        let i = (v * 32767.0) as i16;
        let _ = f.write_all(&i.to_le_bytes());
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: nes <rom.nes> [frames] [out.png]");
        return ExitCode::FAILURE;
    };
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let out = args.next().unwrap_or_else(|| "out/nes.png".to_string());
    // Optional 4th arg: comma-separated buttons to hold (e.g. "start,a") to
    // drive past menus, plus a flicker analysis of consecutive frames.
    let hold = args.next().unwrap_or_default();
    let mut buttons = 0u8;
    for b in hold.split(',') {
        buttons |= match b.trim().to_ascii_lowercase().as_str() {
            "a" => 0x01,
            "b" => 0x02,
            "select" => 0x04,
            "start" => 0x08,
            "up" => 0x10,
            "down" => 0x20,
            "left" => 0x40,
            "right" => 0x80,
            _ => 0,
        };
    }

    let rom = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut nes = match nes_core::Nes::from_rom(&rom) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("failed to load {path}: {e:?}");
            return ExitCode::FAILURE;
        }
    };

    // Optional 5th arg: a save-state file to load (must match this ROM).
    if let Some(state_path) = args.next() {
        match std::fs::read(&state_path) {
            Ok(bytes) => match nes.load_state(&bytes) {
                Ok(()) => {
                    println!("loaded state {state_path} ({} bytes)", bytes.len());
                    if std::env::var("MMC5_EXTATTR").is_ok() {
                        nes.dbg_force_ext_attr();
                        println!("(forced MMC5 extended-attribute mode)");
                    }
                    println!("PPUCTRL=${:02x}  {}", nes.dbg_ppu_ctrl(), nes.dbg_mapper());
                    println!("{}", nes.dbg_ppu_scroll());
                }
                Err(e) => {
                    eprintln!("state load failed: {e:?}");
                    return ExitCode::FAILURE;
                }
            },
            Err(e) => {
                eprintln!("cannot read state {state_path}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut last: Vec<u32> = Vec::new();
    let mut audio: Vec<f32> = Vec::new();
    for f in 0..frames {
        // Pulse the held buttons (press/release alternating so menus that need a
        // fresh edge advance) once we're a little past boot.
        if buttons != 0 && f > 20 {
            nes.set_buttons(0, if f % 8 < 4 { buttons } else { 0 });
        }
        last = nes.step_frame().to_vec();
        audio.extend(nes.take_audio());
    }

    // Flicker analysis (only when a 4th "hold" arg is given, i.e. a debug run):
    // render more consecutive frames and report, per scanline, how many pixels
    // changed vs the previous frame + the sprite-0 hit scanline + the HUD band.
    if !hold.is_empty() {
        let mut prev = last.clone();
        let analysis_frames = 40;
        println!("--- flicker analysis ({analysis_frames} frames) ---");
        for k in 0..analysis_frames {
            if buttons != 0 {
                nes.set_buttons(0, if k % 8 < 4 { buttons } else { 0 });
            }
            let fb = nes.step_frame().to_vec();
            audio.extend(nes.take_audio());
            // Per-scanline change counts; report rows with the most churn.
            let mut worst: Vec<(usize, u32)> = (0..240)
                .map(|y| {
                    let c = (0..256)
                        .filter(|&x| fb[y * 256 + x] != prev[y * 256 + x])
                        .count() as u32;
                    (y, c)
                })
                .filter(|&(_, c)| c > 0)
                .collect();
            worst.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
            let top: Vec<String> =
                worst.iter().take(4).map(|&(y, c)| format!("row{y}={c}")).collect();
            // HUD band = rows 190..239 (the bottom status bar).
            let hud: u32 = (190..240)
                .map(|y| (0..256).filter(|&x| fb[y * 256 + x] != prev[y * 256 + x]).count() as u32)
                .sum();
            // Separator probe: in rows 170..185, find the row that is most "white"
            // (near-white px count). Reports the white line's Y and how solid it is.
            let is_white = |p: u32| {
                let (r, g, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
                r > 0xd0 && g > 0xd0 && b > 0xd0
            };
            let (mut sep_y, mut sep_n) = (0usize, 0u32);
            for y in 168..186 {
                let n = (0..256).filter(|&x| is_white(fb[y * 256 + x])).count() as u32;
                if n > sep_n {
                    sep_n = n;
                    sep_y = y;
                }
            }
            // Also: the exact rows in 168..214 that changed vs prev, with count.
            let hud_rows: Vec<String> = (168..214)
                .filter_map(|y| {
                    let c = (0..256).filter(|&x| fb[y * 256 + x] != prev[y * 256 + x]).count();
                    (c > 0).then(|| format!("r{y}={c}"))
                })
                .collect();
            let flag = if sep_y != 176 || hud > 500 { " <<GLITCH" } else { "" };
            println!(
                "frame +{k} (abs f{}): s0_hit=({},{}), sep_y={sep_y}(white={sep_n}), HUD_px={hud}{flag}",
                nes.dbg_frame(),
                nes.dbg_sprite0_scanline(),
                nes.dbg_sprite0_dot(),
            );
            let _ = &hud_rows;
            let _ = (worst.len(), &top);
            // Dump frames where the HUD band churns hard (the flicker), plus the
            // clean frame right before it, so the HUD can be compared directly.
            if hud > 500 {
                let dir = std::path::Path::new(&out).parent().unwrap_or(std::path::Path::new("."));
                let dump = |name: String, buf: &[u32]| {
                    let mut rgba = Vec::with_capacity(buf.len() * 4);
                    for &px in buf {
                        rgba.push(((px >> 16) & 0xff) as u8);
                        rgba.push(((px >> 8) & 0xff) as u8);
                        rgba.push((px & 0xff) as u8);
                        rgba.push(0xff);
                    }
                    let gp = dir.join(name).to_string_lossy().to_string();
                    if let Ok(file) = std::fs::File::create(&gp) {
                        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 256, 240);
                        enc.set_color(png::ColorType::Rgba);
                        enc.set_depth(png::BitDepth::Eight);
                        if let Ok(mut wr) = enc.write_header() {
                            let _ = wr.write_image_data(&rgba);
                        }
                    }
                    println!("  -> dumped {gp}");
                };
                dump(format!("glitch_{k}_before.png"), &prev);
                dump(format!("glitch_{k}.png"), &fb);
                // Per-column HUD-band change signature (which x's move) to reveal a
                // uniform horizontal shift vs localized garbage.
                let cols: Vec<usize> = (0..256)
                    .filter(|&x| (190..240).any(|y| fb[y * 256 + x] != prev[y * 256 + x]))
                    .collect();
                println!("     HUD changed columns: {} of 256 (first {:?})", cols.len(), &cols[..cols.len().min(12)]);
            }
            prev = fb.clone();
            last = fb;
        }
    }

    // Write the audio track as a 16-bit PCM mono WAV next to the PNG.
    let wav_path = std::path::Path::new(&out).with_extension("wav");
    write_wav(&wav_path, &audio);
    let peak = audio.iter().fold(0f32, |m, &s| m.max(s.abs()));
    println!("audio: {} samples @44.1kHz, peak={:.3} -> {}", audio.len(), peak, wav_path.display());

    // Stats: how many pixels differ from the top-left (a proxy for "is anything
    // actually being drawn"), and the distinct-color count.
    let backdrop = last.first().copied().unwrap_or(0);
    let non_backdrop = last.iter().filter(|&&p| p != backdrop).count();
    let mut colors: Vec<u32> = last.clone();
    colors.sort_unstable();
    colors.dedup();
    println!(
        "ran {frames} frames of {path}\n\
         framebuffer 256x240, backdrop={:06X}, non-backdrop pixels={}, distinct colors={}",
        backdrop & 0xffffff,
        non_backdrop,
        colors.len()
    );

    // Write PNG (RGBA8888).
    if let Some(dir) = std::path::Path::new(&out).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut rgba = Vec::with_capacity(last.len() * 4);
    for &px in &last {
        rgba.push(((px >> 16) & 0xff) as u8);
        rgba.push(((px >> 8) & 0xff) as u8);
        rgba.push((px & 0xff) as u8);
        rgba.push(0xff);
    }
    let file = match std::fs::File::create(&out) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot write {out}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let w = std::io::BufWriter::new(file);
    let mut enc = png::Encoder::new(w, 256, 240);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(&rgba).unwrap();
    println!("wrote {out}");

    ExitCode::SUCCESS
}
