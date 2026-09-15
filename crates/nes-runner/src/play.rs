// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Scripted input harness: drive a ROM with an exact sequence of button
//! presses, on either pad, and dump frames along the way.
//!
//! The `nes` harness pulses whatever buttons you name on a fixed four-on
//! four-off period, which is right for walking a menu and useless for anything
//! that has to be played: a jump needs the button down on one specific frame
//! and up eleven frames later, and a platformer that is a frame out is a
//! different game. This one takes the frames as part of the script.
//!
//!   cargo run -p nes-runner --bin play -- rom.nes "SCRIPT" [outdir] [flags]
//!
//! A script is a list of events separated by `;`, in any order:
//!
//!   `N=pad1/pad2`   from frame N on, hold exactly these
//!   `N#name`        write outdir/name.png at frame N
//!   `N?where=what`  at frame N, assert RAM holds that, or fail the run
//!   `N`             end the run at frame N (otherwise the last event ends it)
//!
//! Buttons are `a b select start up down left right`, joined with `+`. A
//! misspelled one stops the run rather than doing nothing, because a button
//! that silently never presses looks exactly like a game that ignores it.
//!
//!   "30=start;40=;60=right;120=right+a;170#leap;240"
//!
//! ## Two pads
//!
//! The pads are separated by `/`, pad 1 first. A field that is present, even
//! empty, means hold exactly that, so `40=` still lets go of pad 1 the way it
//! always did. A field that is absent, or written `.`, leaves that pad exactly
//! as it was:
//!
//!   `60=right`        pad 1 walks right, pad 2 untouched
//!   `60=right/left`   they walk into each other
//!   `60=./a`          pad 2 jumps, pad 1 carries on doing whatever it was
//!   `60=/`            both let go
//!
//! `.` earns its keep. In a two-player script the alternative is restating a
//! pad you did not mean to touch, and a pad restated wrong reads as a game bug
//! for as long as it takes to suspect the script.
//!
//! ## Assertions
//!
//! `N?where=what` reads the 2 KB internal RAM after frame N and fails the run
//! if it does not match, so a script can be a test instead of a pile of PNGs
//! somebody has to look at. Either side is a decimal number, a `$` hex number,
//! or a name from the assembler's symbol file, loaded from `<rom>.sym` beside
//! the ROM unless `--sym` says otherwise:
//!
//!   "300?hero_room=$04;300?p2_state=HS_ALIVE"
//!
//! Use the names. `$0350` is the right address until somebody declares a
//! variable above it, and from then on it is quietly asserting something else.
//!
//! ## Netplay delay
//!
//! `--delay N` makes the core see every input N frames after the script says
//! it, on both pads at once, with both pads reading zero for the first N
//! frames. That is precisely what a lockstep netplay peer's core consumes
//! (`InputDelayBuffer` on the Android side, four frames by default), so it is
//! worth one run before any two-player cart is called finished.
//!
//! It answers one question: does anything in this script have to land on an
//! exact frame. A script that plays the game at `--delay 0` and still plays it
//! at `--delay 4` holds no frame-critical input. It is not a test of reacting
//! under lag, because a script reacts to nothing; that one needs two humans
//! and two devices, and no harness is going to stand in for it.

use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

/// Controller ports the script can address. The core has exactly these two.
const PORTS: usize = 2;

const USAGE: &str = "usage: play <rom.nes> \"30=start;60=right/left;200?p2_alive=1;240\" \
                     [outdir] [--delay N] [--sym file.sym]";

fn parse_buttons(spec: &str) -> Result<u8, String> {
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
            other => return Err(format!("unknown button {other:?}")),
        };
    }
    Ok(mask)
}

/// One `N=...` event, as what each pad is told to do. `None` is "carry on".
fn parse_pads(spec: &str) -> Result<[Option<u8>; PORTS], String> {
    let fields: Vec<&str> = spec.split('/').collect();
    if fields.len() > PORTS {
        return Err(format!(
            "{spec:?} names {} pads and there are {PORTS}",
            fields.len()
        ));
    }
    let mut pads = [None; PORTS];
    for (i, f) in fields.iter().enumerate() {
        let f = f.trim();
        if f == "." {
            continue;
        }
        pads[i] = Some(parse_buttons(f)?);
    }
    Ok(pads)
}

fn parse_number(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('$') {
        return u32::from_str_radix(h, 16).ok();
    }
    if let Some(h) = s.strip_prefix("0x") {
        return u32::from_str_radix(h, 16).ok();
    }
    s.parse().ok()
}

/// A number, or a name out of the symbol file. Symbols carry both addresses
/// and constants, which is why the value side accepts one too: `HS_ALIVE`
/// says what the assertion means where `1` only says what it compares.
fn resolve(tok: &str, syms: &HashMap<String, u32>) -> Result<u32, String> {
    let tok = tok.trim();
    if let Some(v) = parse_number(tok) {
        return Ok(v);
    }
    syms.get(tok)
        .copied()
        .ok_or_else(|| format!("{tok:?} is not a number and not in the symbol file"))
}

/// `nes-asm -s` writes `HHHH  NAME` per line. A missing file is not an error:
/// scripts that only use numbers never needed it.
fn load_symbols(path: &std::path::Path) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(addr), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        if let Ok(v) = u32::from_str_radix(addr, 16) {
            map.insert(name.to_string(), v);
        }
    }
    map
}

/// The script's intent, pad by pad, frame by frame. Built up front because `.`
/// merges into what came before rather than replacing it, and because --delay
/// then reads out of it N frames behind.
fn schedule(holds: &[(u32, [Option<u8>; PORTS])], end: u32) -> Vec<[u8; PORTS]> {
    let mut sched = Vec::with_capacity(end as usize + 1);
    let mut cur = [0u8; PORTS];
    let mut hi = 0usize;
    for f in 0..=end {
        while hi < holds.len() && holds[hi].0 <= f {
            for (slot, told) in cur.iter_mut().zip(holds[hi].1) {
                if let Some(v) = told {
                    *slot = v;
                }
            }
            hi += 1;
        }
        sched.push(cur);
    }
    sched
}

/// What the core sees on frame `f`: the script's intent from `delay` frames
/// back, or nothing at all while both peers are still inside the warmup window.
fn pads_at(sched: &[[u8; PORTS]], f: u32, delay: u32) -> [u8; PORTS] {
    if f >= delay {
        sched[(f - delay) as usize]
    } else {
        [0u8; PORTS]
    }
}

struct Check {
    frame: u32,
    addr: u16,
    want: u8,
    /// The assertion as it was typed, so a failure names what the author meant
    /// rather than an address they now have to go and look up.
    text: String,
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
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("play: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut delay: u32 = 0;
    let mut sym_arg: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--delay" => {
                let v = args.next().ok_or("--delay wants a frame count")?;
                delay = v
                    .parse()
                    .map_err(|_| format!("--delay {v:?} is not a frame count"))?;
            }
            "--sym" => sym_arg = Some(args.next().ok_or("--sym wants a path")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other if other.starts_with("--") => return Err(format!("unknown flag {other:?}")),
            other => positional.push(other.to_string()),
        }
    }
    let (Some(path), Some(script)) = (positional.first().cloned(), positional.get(1).cloned())
    else {
        return Err(USAGE.to_string());
    };
    let outdir = positional
        .get(2)
        .cloned()
        .unwrap_or_else(|| "out".to_string());

    let sym_path = sym_arg
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(&path).with_extension("sym"));
    let syms = load_symbols(&sym_path);

    let mut holds: Vec<(u32, [Option<u8>; PORTS])> = Vec::new();
    let mut shots: Vec<(u32, String)> = Vec::new();
    let mut checks: Vec<Check> = Vec::new();
    let mut end: u32 = 0;
    for ev in script.split(';') {
        let ev = ev.trim();
        if ev.is_empty() {
            continue;
        }
        let (frame, rest) = match ev.find(['=', '#', '?']) {
            Some(i) => (&ev[..i], Some((ev.as_bytes()[i], &ev[i + 1..]))),
            None => (ev, None),
        };
        let frame: u32 = frame
            .trim()
            .parse()
            .map_err(|_| format!("event {ev:?} does not start with a frame number"))?;
        end = end.max(frame);
        match rest {
            Some((b'=', spec)) => holds.push((
                frame,
                parse_pads(spec).map_err(|e| format!("in {ev:?}: {e}"))?,
            )),
            Some((b'#', name)) => shots.push((frame, name.to_string())),
            Some((b'?', cond)) => {
                let (lhs, rhs) = cond
                    .split_once('=')
                    .ok_or_else(|| format!("assertion {ev:?} wants the form N?where=what"))?;
                let addr = resolve(lhs, &syms).map_err(|e| format!("in {ev:?}: {e}"))?;
                let want = resolve(rhs, &syms).map_err(|e| format!("in {ev:?}: {e}"))?;
                if addr >= 0x0800 {
                    return Err(format!(
                        "in {ev:?}: ${addr:04X} is outside the 2 KB internal RAM this can read"
                    ));
                }
                if want > 0xff {
                    return Err(format!("in {ev:?}: {want} does not fit in a byte"));
                }
                checks.push(Check {
                    frame,
                    addr: addr as u16,
                    want: want as u8,
                    text: cond.trim().to_string(),
                });
            }
            _ => {}
        }
    }
    holds.sort_by_key(|&(f, _)| f);
    shots.sort_by_key(|&(f, _)| f);
    checks.sort_by_key(|c| c.frame);

    let sched = schedule(&holds, end);

    let rom = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut nes =
        nes_core::Nes::from_rom(&rom).map_err(|e| format!("failed to load {path}: {e:?}"))?;

    // Under delay the tail of the script has not reached the core yet at the
    // frame the script ends on, so the run outlasts it by exactly the delay.
    let run_end = end + delay;
    let mut si = 0usize;
    let mut ci = 0usize;
    let mut failures = 0usize;
    let mut audio: Vec<f32> = Vec::new();
    for f in 0..=run_end {
        let pads = pads_at(&sched, f, delay);
        for (p, &held) in pads.iter().enumerate() {
            nes.set_buttons(p, held);
        }
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
        while ci < checks.len() && checks[ci].frame == f {
            let c = &checks[ci];
            let got = nes.system_ram()[c.addr as usize];
            if got == c.want {
                println!("f{f:<5} ok    {}", c.text);
            } else {
                println!(
                    "f{f:<5} FAIL  {} : ${:04X} is ${got:02X}, want ${:02X}",
                    c.text, c.addr, c.want
                );
                failures += 1;
            }
            ci += 1;
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
        let per_frame = audio.len() / (run_end as usize + 1).max(1);
        if std::env::var("TRACE").is_ok() && per_frame > 0 {
            println!("--- frames with any audio in them ---");
            for f in 0..=run_end as usize {
                let c = &audio
                    [(f * per_frame).min(audio.len())..((f + 1) * per_frame).min(audio.len())];
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
    let delayed = if delay > 0 {
        format!(", {delay}-frame input delay")
    } else {
        String::new()
    };
    println!(
        "{} frames, {} samples, peak {peak:.4}, rms {rms:.4}{delayed}",
        run_end + 1,
        audio.len()
    );
    if !checks.is_empty() {
        println!("{} assertions, {failures} failed", checks.len());
    }
    let _ = std::io::stdout().flush();
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u8 = 0x01;
    const LEFT: u8 = 0x40;
    const RIGHT: u8 = 0x80;

    /// The distinction the whole two-pad syntax rests on: a field that is
    /// there is an instruction, a field that is missing is silence. Collapse
    /// the two and every `60=./a` starts letting go of pad 1 as a side effect.
    #[test]
    fn an_empty_field_releases_and_an_absent_one_does_not() {
        assert_eq!(parse_pads("").unwrap(), [Some(0), None]);
        assert_eq!(parse_pads(".").unwrap(), [None, None]);
        assert_eq!(parse_pads("/").unwrap(), [Some(0), Some(0)]);
        assert_eq!(parse_pads("./a").unwrap(), [None, Some(A)]);
        assert_eq!(parse_pads("right").unwrap(), [Some(RIGHT), None]);
        assert_eq!(parse_pads("right/left").unwrap(), [Some(RIGHT), Some(LEFT)]);
    }

    #[test]
    fn a_misspelled_button_or_a_third_pad_is_an_error() {
        assert!(parse_pads("jump").is_err());
        assert!(parse_pads("a/b/c").is_err());
        assert!(parse_pads("right+jump").is_err());
    }

    /// Pad 1 is told something on frame 0 and never mentioned again; pad 2 is
    /// told something on frame 2. Pad 1 has to still be holding it afterwards.
    #[test]
    fn a_hold_on_one_pad_leaves_the_other_where_it_was() {
        let holds = vec![(0, [Some(RIGHT), None]), (2, [None, Some(LEFT)])];
        let sched = schedule(&holds, 3);
        assert_eq!(
            sched,
            vec![[RIGHT, 0], [RIGHT, 0], [RIGHT, LEFT], [RIGHT, LEFT]]
        );
    }

    #[test]
    fn an_empty_field_in_the_script_really_does_let_go() {
        let holds = vec![(0, [Some(RIGHT), Some(LEFT)]), (2, [Some(0), None])];
        let sched = schedule(&holds, 2);
        assert_eq!(sched, vec![[RIGHT, LEFT], [RIGHT, LEFT], [0, LEFT]]);
    }

    /// Both pads read zero through the warmup window, then the core runs the
    /// script exactly N frames behind, which is what `InputDelayBuffer` hands
    /// a lockstep peer.
    #[test]
    fn delay_is_a_warmup_window_and_then_the_script_n_frames_back() {
        let sched = schedule(&[(0, [Some(RIGHT), Some(LEFT)])], 2);
        for f in 0..4 {
            assert_eq!(
                pads_at(&sched, f, 4),
                [0, 0],
                "frame {f} is inside the warmup"
            );
        }
        assert_eq!(pads_at(&sched, 4, 4), [RIGHT, LEFT]);
        assert_eq!(
            pads_at(&sched, 0, 0),
            [RIGHT, LEFT],
            "no delay is no warmup"
        );
    }

    #[test]
    fn numbers_are_decimal_hex_or_a_symbol() {
        let mut syms = HashMap::new();
        syms.insert("lives".to_string(), 0x46u32);
        assert_eq!(resolve("10", &syms).unwrap(), 10);
        assert_eq!(resolve("$10", &syms).unwrap(), 16);
        assert_eq!(resolve("0x10", &syms).unwrap(), 16);
        assert_eq!(resolve("lives", &syms).unwrap(), 0x46);
        assert!(
            resolve("lifes", &syms).is_err(),
            "a typo must not resolve to anything"
        );
    }
}
