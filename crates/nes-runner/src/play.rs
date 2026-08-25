// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Scripted input harness: drive a ROM with an exact sequence of button
//! presses and dump frames along the way.
//!
//! The `nes` harness pulses whatever buttons you name on a fixed four-on
//! four-off period, which is right for walking a menu and useless for anything
//! that has to be played: a jump needs the button down on one specific frame
//! and up eleven frames later, and a platformer that is a frame out is a
//! different game. This one takes the frames as part of the script.
//!
//!   cargo run -p nes-runner --bin play -- rom.nes "SCRIPT" [outdir]
//!
//! A script is a list of events separated by `;`, in any order:
//!
//!   `N=buttons`   from frame N on, hold exactly these (empty = let go)
//!   `N#name`      write outdir/name.png at frame N
//!   `N`           end the run at frame N (otherwise it ends at the last event)
//!
//! Buttons are `a b select start up down left right`, joined with `+`.
//!
//!   "30=start;40=;60=right;120=right+a;170#leap;240"

use std::io::Write;
use std::process::ExitCode;

fn parse_buttons(spec: &str) -> u8 {
    let mut mask = 0u8;
    for b in spec.split('+') {
        let b = b.trim().to_ascii_lowercase();
        if b.is_empty() {
            continue;
        }
        mask |= match b.as_str() {
            "a" => 0x01,
            "b" => 0x02,
            "select" => 0x04,
            "start" => 0x08,
            "up" => 0x10,
            "down" => 0x20,
            "left" => 0x40,
            "right" => 0x80,
            other => {
                eprintln!("unknown button {other:?}");
                0
            }
        };
    }
    mask
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

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(script)) = (args.next(), args.next()) else {
        eprintln!("usage: play <rom.nes> \"30=start;60=right;120#shot;200\" [outdir]");
        return ExitCode::FAILURE;
    };
    let outdir = args.next().unwrap_or_else(|| "out".to_string());

    // frame -> held buttons, and frame -> screenshot name.
    let mut holds: Vec<(u32, u8)> = Vec::new();
    let mut shots: Vec<(u32, String)> = Vec::new();
    let mut end: u32 = 0;
    for ev in script.split(';') {
        let ev = ev.trim();
        if ev.is_empty() {
            continue;
        }
        let (frame, rest) = match ev.find(['=', '#']) {
            Some(i) => (&ev[..i], Some((ev.as_bytes()[i], &ev[i + 1..]))),
            None => (ev, None),
        };
        let Ok(frame) = frame.trim().parse::<u32>() else {
            eprintln!("event {ev:?} does not start with a frame number");
            return ExitCode::FAILURE;
        };
        end = end.max(frame);
        match rest {
            Some((b'=', spec)) => holds.push((frame, parse_buttons(spec))),
            Some((b'#', name)) => shots.push((frame, name.to_string())),
            _ => {}
        }
    }
    holds.sort_by_key(|&(f, _)| f);
    shots.sort_by_key(|&(f, _)| f);

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

    let mut hi = 0usize;
    let mut si = 0usize;
    let mut held = 0u8;
    let mut audio: Vec<f32> = Vec::new();
    for f in 0..=end {
        while hi < holds.len() && holds[hi].0 <= f {
            held = holds[hi].1;
            hi += 1;
        }
        nes.set_buttons(0, held);
        let fb = nes.step_frame().to_vec();
        audio.extend(nes.take_audio());
        while si < shots.len() && shots[si].0 == f {
            let p = std::path::Path::new(&outdir).join(format!("{}.png", shots[si].1));
            match write_png(&p, &fb) {
                Ok(()) => println!("f{f:<5} wrote {}", p.display()),
                Err(e) => eprintln!("f{f}: cannot write {}: {e}", p.display()),
            }
            si += 1;
        }
    }

    // A WAV of the run, and with $TRACE a per-frame energy trace beside it.
    // The trace is the useful half when a sound effect goes missing: an effect
    // that never fires and an effect that fires inaudibly sound identical from
    // the far side of a speaker, and are not the same bug.
    {
        let sr = nes_core::SAMPLE_RATE as usize;
        let wav = std::path::Path::new(&outdir).join("audio.wav");
        if let Some(d) = wav.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(f) = std::fs::File::create(&wav) {
            let mut f = std::io::BufWriter::new(f);
            let n = (audio.len() * 2) as u32;
            let _ = f.write_all(b"RIFF");
            let _ = f.write_all(&(36 + n).to_le_bytes());
            let _ = f.write_all(b"WAVEfmt ");
            let _ = f.write_all(&16u32.to_le_bytes());
            let _ = f.write_all(&1u16.to_le_bytes());
            let _ = f.write_all(&1u16.to_le_bytes());
            let _ = f.write_all(&(sr as u32).to_le_bytes());
            let _ = f.write_all(&(sr as u32 * 2).to_le_bytes());
            let _ = f.write_all(&2u16.to_le_bytes());
            let _ = f.write_all(&16u16.to_le_bytes());
            let _ = f.write_all(b"data");
            let _ = f.write_all(&n.to_le_bytes());
            for &s in &audio {
                let v = ((s * 4.0).clamp(-1.0, 1.0) * 32767.0) as i16;
                let _ = f.write_all(&v.to_le_bytes());
            }
            println!("wrote {}", wav.display());
        }
        let per_frame = audio.len() / (end as usize + 1).max(1);
        if std::env::var("TRACE").is_ok() && per_frame > 0 {
            println!("--- frames with any audio in them ---");
            for f in 0..=end as usize {
                let c = &audio[(f * per_frame).min(audio.len())
                    ..((f + 1) * per_frame).min(audio.len())];
                if c.is_empty() {
                    continue;
                }
                let r = (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt();
                if r > 0.002 {
                    println!("f{f:<5} rms {r:.4}");
                }
            }
        }
    }

    // A quiet channel that is holding a level rather than sitting at zero is
    // inaudible until the stream stops, so report both.
    let peak = audio.iter().fold(0f32, |a, &b| a.max(b.abs()));
    let rms = if audio.is_empty() {
        0.0
    } else {
        (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt()
    };
    println!("{} frames, {} samples, peak {peak:.4}, rms {rms:.4}", end + 1, audio.len());
    let _ = std::io::stdout().flush();
    ExitCode::SUCCESS
}
