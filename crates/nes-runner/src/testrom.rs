//! Blargg test-ROM harness (the `$6000` status protocol).
//!
//! Blargg's test ROMs report via battery RAM: `$6000` holds `$80` while running
//! and the result code (0 = pass) when done; the magic bytes `$DE $B0 $61` at
//! `$6001..$6003` confirm the protocol is active; a NUL-terminated message sits
//! at `$6004`. `$6000 == $81` asks the host to reset the CPU after a short delay.
//!
//!   cargo run -p nes-runner --bin testrom -- <rom.nes> [max_frames]

use std::process::ExitCode;

fn read_message(nes: &mut nes_core::Nes) -> String {
    let mut s = String::new();
    let mut addr = 0x6004u16;
    for _ in 0..256 {
        let b = nes.peek(addr);
        if b == 0 {
            break;
        }
        s.push(b as char);
        addr += 1;
    }
    s.trim().replace('\n', " / ")
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: testrom <rom.nes> [max_frames]");
        return ExitCode::FAILURE;
    };
    let max_frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1200);

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

    let mut protocol_seen = false;
    let mut reset_pending: Option<u32> = None;
    // Fallback for older (2005) blargg tests that report via zero-page $F8
    // (1 = pass, n>1 = fail code) and then loop forever: detect a stable value.
    let mut f8_last = 0u8;
    let mut f8_stable = 0u32;

    for frame in 0..max_frames {
        nes.step_frame();

        let f8 = nes.peek(0x00f8);
        if f8 == f8_last {
            f8_stable += 1;
        } else {
            f8_last = f8;
            f8_stable = 0;
        }

        if let Some(due) = reset_pending {
            if frame >= due {
                nes.reset();
                reset_pending = None;
            }
        }

        // Protocol active once the magic signature appears.
        if nes.peek(0x6001) == 0xde && nes.peek(0x6002) == 0xb0 && nes.peek(0x6003) == 0x61 {
            protocol_seen = true;
        }
        if protocol_seen {
            let status = nes.peek(0x6000);
            match status {
                0x80 => {} // still running
                0x81 => {
                    if reset_pending.is_none() {
                        reset_pending = Some(frame + 12); // ~200 ms
                    }
                }
                code => {
                    let msg = read_message(&mut nes);
                    let verdict = if code == 0 { "PASS" } else { "FAIL" };
                    println!("[{verdict}] code={code} ({} frames): {}", frame, msg);
                    return if code == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE };
                }
            }
        } else if f8_last != 0 && f8_stable >= 20 {
            // Older $F8 protocol: value settled -> the ROM is looping on its
            // result. 1 = pass; anything else is the failure code.
            if f8_last == 1 {
                println!("[PASS] $F8=1 ({} frames)", frame);
                return ExitCode::SUCCESS;
            } else {
                println!("[FAIL] $F8={} ({} frames)", f8_last, frame);
                return ExitCode::FAILURE;
            }
        }
    }

    let msg = read_message(&mut nes);
    println!(
        "[TIMEOUT] after {max_frames} frames; protocol_seen={protocol_seen} $6000={:02X}: {}",
        nes.peek(0x6000),
        msg
    );
    ExitCode::FAILURE
}
