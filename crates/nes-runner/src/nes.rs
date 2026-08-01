//! Headless dev harness: load a `.nes`, run N frames, dump the last frame as a
//! PNG. Mirrors the `snes` / `n64` bins in the sibling cores. Reports pixel
//! statistics (the user judges the render; the harness reports measurable facts).
//!
//!   cargo run -p nes-runner --bin nes -- <rom.nes> [frames] [out.png]

use std::io::Write;
use std::process::ExitCode;

/// Write f32 samples (roughly -1..1) as a 16-bit PCM mono WAV at 44.1 kHz.
fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let sr: u32 = 44_100;
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

    let mut last: Vec<u32> = Vec::new();
    let mut audio: Vec<f32> = Vec::new();
    for _ in 0..frames {
        last = nes.step_frame().to_vec();
        audio.extend(nes.take_audio());
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
