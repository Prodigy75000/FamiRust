//! Headless dev harness: load a `.nes`, run N frames, dump the last frame as a
//! PNG. Mirrors the `snes` / `n64` bins in the sibling cores. Reports pixel
//! statistics (the user judges the render; the harness reports measurable facts).
//!
//!   cargo run -p nes-runner --bin nes -- <rom.nes> [frames] [out.png]

use std::process::ExitCode;

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
    for _ in 0..frames {
        last = nes.step_frame().to_vec();
    }

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
