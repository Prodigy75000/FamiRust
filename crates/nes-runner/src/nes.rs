// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

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
    let parse_buttons = |spec: &str| {
        let mut mask = 0u8;
        for b in spec.split(',') {
            mask |= match b.trim().to_ascii_lowercase().as_str() {
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
        mask
    };
    let buttons = parse_buttons(&hold);

    let rom = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // FDS disk images are not cartridges: they need the 8 KiB BIOS (disksys.rom,
    // user-supplied). Look it up next to the disk or via $FDS_BIOS.
    let mut nes = if nes_core::Nes::is_fds(&rom) {
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
        match nes_core::Nes::from_fds(&rom, &bios) {
            Ok(n) => {
                println!("FDS disk: {} side(s), BIOS {} bytes", n.fds_side_count(), bios.len());
                n
            }
            Err(e) => {
                eprintln!("failed to load FDS {path}: {e:?}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match nes_core::Nes::from_rom(&rom) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("failed to load {path}: {e:?}");
                return ExitCode::FAILURE;
            }
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

    let fds_pc = std::env::var("FDS_PC").is_ok();
    // Optional headless FDS side-flip: at frame $FDS_FLIP_AT, replay the exact
    // libretro swap (eject then insert side $FDS_FLIP_SIDE) to test disk-change
    // detection from a save state parked at an "insert side B" prompt.
    let flip_at: i64 = std::env::var("FDS_FLIP_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(-1);
    let flip_side: usize = std::env::var("FDS_FLIP_SIDE").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    // Optional: stop pulsing the held buttons after this frame. START drives the
    // pre-roll past title/intro screens, but a pause-toggling button left pressed
    // during the flicker analysis blinks the HUD in and out on its own period and
    // masks the thing being measured. -1 (default) = pulse for the whole run.
    let hold_stop: i64 = std::env::var("HOLD_STOP_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(-1);
    // Optional: swap to a different button set at $HOLD_STOP_AT instead of just
    // letting go -- "start until we are in the level, then hold right" is how a
    // scrolling scene gets measured without START pausing the game.
    let after_buttons = std::env::var("HOLD_AFTER").map(|s| parse_buttons(&s)).unwrap_or(0);
    // Buttons to pulse on frame `f`: the hold set before $HOLD_STOP_AT, the
    // $HOLD_AFTER set from it on (0 = nothing held).
    let held_at = |f: i64| {
        if hold_stop < 0 || f < hold_stop {
            buttons
        } else {
            after_buttons
        }
    };
    let any_buttons = buttons != 0 || after_buttons != 0;
    let mut last: Vec<u32> = Vec::new();
    let mut audio: Vec<f32> = Vec::new();
    for f in 0..frames {
        if flip_at >= 0 && f as i64 == flip_at {
            println!("--- FDS flip: eject -> insert side {flip_side} (frame {f}) ---");
            nes.fds_eject();
            nes.fds_insert_side(flip_side);
        }
        if fds_pc && f % 10 == 0 {
            println!("f{f}: pc=${:04x}  {}", nes.dbg_pc(), nes.dbg_mapper());
        }
        // Pulse the held buttons (press/release alternating so menus that need a
        // fresh edge advance) once we're a little past boot.
        if any_buttons && f > 20 {
            nes.set_buttons(0, if f % 8 < 4 { held_at(f as i64) } else { 0 });
        }
        last = nes.step_frame().to_vec();
        audio.extend(nes.take_audio());
    }

    // Flicker analysis (only when a 4th "hold" arg is given, i.e. a debug run):
    // render more consecutive frames and report, per scanline, how many pixels
    // changed vs the previous frame + the sprite-0 hit scanline + the HUD band.
    if !hold.is_empty() {
        let mut prev = last.clone();
        // The glitch this harness was built for shows on ~1 frame in 16, so 40 is
        // enough to see it; $ANALYSIS_FRAMES widens the window for a longer soak.
        let analysis_frames: u32 =
            std::env::var("ANALYSIS_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
        println!("--- flicker analysis ({analysis_frames} frames) ---");
        for k in 0..analysis_frames {
            if any_buttons {
                let abs = (frames + k) as i64;
                nes.set_buttons(0, if k % 8 < 4 { held_at(abs) } else { 0 });
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

    // Dev: disassembly peek -- dump raw bytes around an address (hex) to decode a
    // stuck loop. $FDS_PEEK=a350 prints [a350..a380).
    if let Ok(hx) = std::env::var("FDS_PEEK") {
        if let Ok(base) = u16::from_str_radix(hx.trim_start_matches("0x"), 16) {
            print!("peek ${base:04x}:");
            for i in 0..48u16 {
                print!(" {:02x}", nes.peek(base.wrapping_add(i)));
            }
            println!();
        }
    }

    // Dev: dump mapper-internal state (FDS drive position, IRQ flags, etc.).
    if std::env::var("DBG_MAPPER").is_ok() {
        println!("mapper: {}", nes.dbg_mapper());
        println!("pc=${:04x} halted={}", nes.dbg_pc(), nes.dbg_halted());
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
