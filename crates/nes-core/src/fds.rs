// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Family Computer Disk System (FDS) support: the disk-image container, the
//! RAM Adapter (the "mapper 20" that fronts every FDS game), and the disk-drive
//! controller.
//!
//! Clean-room, from the public NESdev hardware documentation only (no emulator
//! source). The FDS is not a cartridge: a RAM Adapter plugs into the cart slot
//! carrying 32 KiB of program RAM ($6000-$DFFF), 8 KiB of character RAM, and an
//! 8 KiB BIOS ROM at $E000-$FFFF (the copyrighted `disksys.rom`, supplied by the
//! user, never shipped). A separate drive streams a floppy one byte at a time
//! into that RAM under BIOS control.
//!
//! ## The disk container
//!
//! A `.fds` image is one or more 65500-byte *sides*, optionally prefixed by a
//! 16-byte fwNES header (`FDS\x1a` + side count). Each side is a back-to-back
//! run of variable-length *blocks*, stored WITHOUT the gap bytes, start marks,
//! or CRC footers that exist on the physical medium:
//!
//!   * block 1  (56 bytes, type `$01`): disk info, begins `\x01*NINTENDO-HVC*`
//!   * block 2  (2 bytes,  type `$02`): file count
//!   * per file: block 3 (16 bytes, type `$03`) file header incl. a 2-byte size,
//!               then block 4 (1 + size bytes, type `$04`) the file body.
//!
//! ## Why we reconstruct the physical layout
//!
//! The real BIOS reads a block, then reads its 2-byte CRC, then re-arms the
//! drive for the next block. A gapless `.fds` has no CRC bytes, so a naive
//! byte-for-byte stream desynchronises by two bytes per block. We therefore
//! rebuild each side into the on-medium shape: every block is followed by two
//! (faked, always-valid) CRC bytes, and we record each block's start offset.
//! When the BIOS releases the drive's transfer-reset to begin a new block, we
//! snap the head to the next recorded block start, which is exactly the "skip
//! the gap, latch on the start mark" the hardware shift register does. The
//! drive itself then stays a dumb sequential reader, which keeps timing simple
//! and deterministic (and therefore save-state friendly).

use crate::cart::Mirroring;
use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

/// Bytes of raw block data per side in a `.fds` image (the classic Mitsumi
/// Quick Disk capacity as truncated by the fwNES format).
const SIDE_LEN: usize = 65500;

/// FDS BIOS size ($E000-$FFFF).
pub const BIOS_LEN: usize = 8 * 1024;

/// CPU cycles between one disk byte and the next. The drive turns at a fixed
/// speed, delivering ~96.4 kbit/s; at the NTSC 2A03 clock that is ~149 cycles
/// per byte. Games time nothing off this directly (the BIOS drives it), so the
/// exact value only has to be close enough for the transfer IRQ to pace loads.
const BYTE_CYCLES: u32 = 149;

/// Cycles the drive reports "not ready" after the motor spins up, before the
/// first byte can transfer. Real spin-up is tens of milliseconds; the BIOS
/// polls $4032 and waits, so a short delay is enough to satisfy the handshake.
const SPINUP_CYCLES: u32 = 3 * BYTE_CYCLES;

/// After the frontend swaps to a new side, the drive reports the disk *absent*
/// for this many CPU cycles before presenting the new side. A libretro disk swap
/// is atomic (eject -> set index -> insert, with no emulation frames between), so
/// without this the game never observes the disk leaving and games that require
/// seeing the removal (Zelda no Densetsu, Famicom Grand Prix, ...) keep waiting
/// at the "turn the disk over" prompt. Games debounce the removal over many
/// frames (Famicom Grand Prix needs well over 3), so we mimic a real ~1-second
/// physical flip: long enough for any disk-swap debounce, short enough to feel
/// responsive. (NTSC frame ~= 29781 CPU cycles.)
const REINSERT_CYCLES: u32 = 60 * 29_781;

/// Two faked CRC bytes appended to every reconstructed block (see module docs).
const CRC_FOOTER: usize = 2;

/// The block start mark. On the physical medium each block is preceded by a gap
/// of zero bits terminated by a set bit; in this byte-level model that is a $80
/// byte right before the block's type byte. The BIOS *writes* this mark when it
/// rewrites a block, so the reconstructed layout carries it too — otherwise a
/// game's on-disk save (Zelda no Densetsu) writes a mark into a plain gap and the
/// next read latches on it as a bogus block start (BIOS "ERR.24").
const START_MARK: u8 = 0x80;

/// Zero-byte gap the drive skips before the first block of a side. The BIOS
/// releases transfer-reset and waits for the drive to scan across this lead-in
/// before the block start latches; it needs to be long enough that the BIOS has
/// armed its read by the time the first block byte arrives.
const LEAD_IN_GAP: usize = 200;

/// Zero-byte gap between one block's CRC and the next block's start mark. Kept to
/// a single byte so it matches the gap the BIOS emits when it writes a block:
/// a write lands the game's [gap, mark, data] exactly over the reconstructed
/// [gap, mark, data], keeping reads and writes on the same block positions.
const BLOCK_GAP: usize = 1;

// ---------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------

/// A parsed multi-side FDS disk image. Each entry in `sides` is one 65500-byte
/// side of raw block data (fwNES header stripped, all sides normalised to
/// `SIDE_LEN`).
pub struct FdsDisk {
    pub sides: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FdsError {
    /// Not an FDS image (no fwNES magic and no `\x01*NINTENDO-HVC*` block 1).
    NotFds,
    /// Image length is not a whole number of sides.
    BadLength,
    /// No sides at all.
    Empty,
}

impl FdsDisk {
    /// True if `data` looks like an FDS image (fwNES header or a bare side whose
    /// first block is the standard disk-info block).
    pub fn is_fds(data: &[u8]) -> bool {
        if data.len() >= 4 && &data[0..4] == b"FDS\x1a" {
            return true;
        }
        // Headerless: a side begins with block 1, `\x01*NINTENDO-HVC*`.
        data.len() >= 15 && data[0] == 0x01 && &data[1..15] == b"*NINTENDO-HVC*"
    }

    /// Parse a `.fds` image into normalised sides.
    pub fn parse(data: &[u8]) -> Result<Self, FdsError> {
        if !Self::is_fds(data) {
            return Err(FdsError::NotFds);
        }
        // Strip the optional 16-byte fwNES header. Its byte 4 is the side count,
        // but we trust the actual length so a wrong count can't truncate a disk.
        let body = if data.len() >= 4 && &data[0..4] == b"FDS\x1a" {
            &data[16..]
        } else {
            data
        };
        if body.is_empty() {
            return Err(FdsError::Empty);
        }
        // Sides are 65500 bytes. Tolerate a trailing partial side by padding it
        // (some dumps round the last side up or down); reject only nonsense.
        let full = body.len() / SIDE_LEN;
        let rem = body.len() % SIDE_LEN;
        let n_sides = full + usize::from(rem != 0);
        if n_sides == 0 {
            return Err(FdsError::Empty);
        }
        // A stray handful of bytes is corruption, not a side; but a large
        // remainder is a real (short-dumped) side we pad out.
        if rem != 0 && rem < 16 {
            return Err(FdsError::BadLength);
        }
        let mut sides = Vec::with_capacity(n_sides);
        for i in 0..n_sides {
            let start = i * SIDE_LEN;
            let end = (start + SIDE_LEN).min(body.len());
            let mut side = vec![0u8; SIDE_LEN];
            side[..end - start].copy_from_slice(&body[start..end]);
            sides.push(side);
        }
        Ok(FdsDisk { sides })
    }
}

// ---------------------------------------------------------------------------
// Reconstructed side (physical-medium view the drive streams over)
// ---------------------------------------------------------------------------

/// One side rebuilt into the on-medium byte order the drive streams over: a
/// lead-in gap, then every `.fds` block as (zero gap, block data, two faked CRC
/// bytes). The drive finds a block by scanning across the zero gap to the first
/// non-zero byte, which is the block's type byte, exactly as the real drive
/// latches on the start mark after the gap. No explicit start-mark byte is
/// needed: every block begins with a non-zero type byte ($01-$04).
struct MediumSide {
    /// Gaps + block data + per-block CRC footers, laid out contiguously.
    image: Vec<u8>,
    /// Whether this side has been written to since load (needs battery persist).
    dirty: bool,
}

impl MediumSide {
    /// Walk the `.fds` block chain and rebuild the medium image with gaps.
    /// Unknown or truncated tails stop the walk; whatever parsed so far streams
    /// (the BIOS just CRC-fails past valid data).
    fn build(raw: &[u8]) -> Self {
        let dbg = std::env::var("FDS_LAYOUT").is_ok();
        let mut image = Vec::with_capacity(raw.len() + 512);
        image.resize(LEAD_IN_GAP, 0); // lead-in gap before block 1
        let push_block = |image: &mut Vec<u8>, bytes: &[u8]| {
            image.push(START_MARK); // block start mark (consumed by the gap scan)
            image.extend_from_slice(bytes);
            image.extend_from_slice(&[0u8; CRC_FOOTER]); // faked CRC (reported valid)
            image.resize(image.len() + BLOCK_GAP, 0); // inter-block gap
        };

        let mut pos = 0usize;
        let mut any = false;
        // Block 1: disk info, 56 bytes, type $01.
        if raw.first() == Some(&0x01) && raw.len() >= 56 {
            push_block(&mut image, &raw[pos..pos + 56]);
            pos += 56;
            any = true;
        }
        // Block 2: file amount, 2 bytes, type $02. Byte 1 is the file count.
        let mut files = 0u16;
        if raw.get(pos) == Some(&0x02) && pos + 2 <= raw.len() {
            files = raw[pos + 1] as u16;
            push_block(&mut image, &raw[pos..pos + 2]);
            pos += 2;
        }
        if dbg {
            eprintln!("[layout] block1@0 ok, block2 file_count={files}");
        }
        // Per file: block 3 (16-byte header, type $03) then block 4 (1 + size).
        for f in 0..files {
            if raw.get(pos) != Some(&0x03) || pos + 16 > raw.len() {
                if dbg {
                    eprintln!(
                        "[layout] BREAK before file {f}/{files}: @{pos} got ${:02x} (want $03)",
                        raw.get(pos).copied().unwrap_or(0)
                    );
                }
                break;
            }
            // File size is a little-endian u16 at header offset 13.
            let size = u16::from_le_bytes([raw[pos + 13], raw[pos + 14]]) as usize;
            let fid = raw[pos + 1];
            let ldaddr = u16::from_le_bytes([raw[pos + 11], raw[pos + 12]]);
            let ftype = raw[pos + 15];
            push_block(&mut image, &raw[pos..pos + 16]);
            pos += 16;
            let body = 1 + size; // type byte + data
            if raw.get(pos) != Some(&0x04) || pos + body > raw.len() {
                if dbg {
                    eprintln!(
                        "[layout] file {f}: hdr id={fid} size={size} type={ftype} ld=${ldaddr:04x} -> BAD block4 @{pos} got ${:02x} (want $04){}",
                        raw.get(pos).copied().unwrap_or(0),
                        if pos + body > raw.len() { " [overruns side]" } else { "" }
                    );
                }
                break;
            }
            if dbg {
                eprintln!("[layout] file {f}: id={fid} size={size} type={ftype} ld=${ldaddr:04x} (data@{pos})");
            }
            push_block(&mut image, &raw[pos..pos + body]);
            pos += body;
        }
        if dbg {
            eprintln!("[layout] parsed to pos={pos} (side {} bytes)", raw.len());
        }
        // Guard against an empty parse (e.g. blank/formatted side): expose the
        // whole raw side after the lead-in so reads still return something sane.
        if !any {
            image.extend_from_slice(raw);
        }
        MediumSide {
            image,
            dirty: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Expansion audio (RP2C33 wavetable + modulation channel)
// ---------------------------------------------------------------------------

/// The FDS's built-in extra sound channel: a 64-step wavetable oscillator with a
/// volume envelope and a frequency-modulation unit, on top of the 2A03 APU.
/// Registers $4040-$408A / $4090-$4092. Clean-room from the NESdev FDS-audio
/// hardware reference; the main oscillator and mod unit tick every 16 CPU cycles.
///
/// Mod counter -> pitch is the documented "complicated" formula (see `wave_pitch`).
#[derive(Clone)]
struct FdsAudio {
    // ---- main wavetable oscillator ----
    wave: [u8; 64], // 6-bit samples ($4040-$407F)
    wave_accum: u32, // 20-bit phase; top 6 bits index `wave`
    wave_pos: u8,    // latched output position (0..63)
    pitch: u16,      // 12-bit ($4082 + $4083 low nibble)
    wave_write: bool, // $4089 bit7: wavetable write-enable + holds output
    env_halt: bool,  // $4083 bit6: halt volume + mod envelopes
    env_4x: bool,    // $4083 bit7: 4x envelope speed + halt mod accumulator

    // ---- volume envelope ----
    vol_gain: u8,    // current gain 0..63 (clamped to 32 at output)
    vol_speed: u8,   // $4080 bits0-5
    vol_dir_up: bool, // $4080 bit6
    vol_env_off: bool, // $4080 bit7 (1 => fixed gain, no sweep)
    vol_timer: u32,

    // ---- modulation unit ----
    mod_table: [u8; 32], // 3-bit entries
    mod_accum: u32,      // 18-bit: [addr5][ghost1][freq12]
    mod_counter: i32,    // 7-bit signed (-64..63)
    mod_freq: u16,       // 12-bit ($4086 + $4087 low nibble)
    mod_gain: u8,        // current mod gain 0..63
    mod_speed: u8,       // $4084 bits0-5
    mod_dir_up: bool,    // $4084 bit6
    mod_env_off: bool,   // $4084 bit7
    mod_timer: u32,

    env_speed: u8,   // $408A envelope clock multiplier (0 => envelopes off)
    master_vol: u8,  // $4089 bits0-1
    enabled: bool,   // $4023 bit1 (sound I/O enable)
    div16: u8,       // 16-CPU-cycle divider for the oscillators
    out: f32,        // cached, low-pass-filtered output for the mixer (0..~1)
    lp: f32,         // 1-pole low-pass state (~2 kHz, per the hardware DAC filter)
    dbg_peak: f32,   // max |out| seen (diagnostic; not serialized)
}

impl FdsAudio {
    fn new() -> Self {
        FdsAudio {
            wave: [0; 64],
            wave_accum: 0,
            wave_pos: 0,
            pitch: 0,
            wave_write: false,
            env_halt: false,
            env_4x: false,
            vol_gain: 0,
            vol_speed: 0,
            vol_dir_up: false,
            vol_env_off: true,
            vol_timer: 0,
            mod_table: [0; 32],
            mod_accum: 0,
            mod_counter: 0,
            mod_freq: 0,
            mod_gain: 0,
            mod_speed: 0,
            mod_dir_up: false,
            mod_env_off: true,
            mod_timer: 0,
            env_speed: 0,
            master_vol: 0,
            enabled: false,
            div16: 0,
            out: 0.0,
            lp: 0.0,
            dbg_peak: 0.0,
        }
    }

    /// Reload value for an envelope timer: 8 * (speed+1) * (env_speed+1), quartered
    /// when $4083's 4x bit is set. In CPU cycles.
    fn env_reload(&self, speed: u8) -> u32 {
        let base = 8 * (speed as u32 + 1) * (self.env_speed as u32 + 1);
        if self.env_4x {
            base / 4
        } else {
            base
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x4040..=0x407f => {
                // Wavetable RAM is writable only while write-enabled ($4089 bit7).
                if self.wave_write {
                    self.wave[(addr - 0x4040) as usize] = val & 0x3f;
                }
            }
            0x4080 => {
                self.vol_env_off = val & 0x80 != 0;
                self.vol_dir_up = val & 0x40 != 0;
                self.vol_speed = val & 0x3f;
                if self.vol_env_off {
                    self.vol_gain = self.vol_speed; // fixed gain
                }
                self.vol_timer = self.env_reload(self.vol_speed);
            }
            0x4082 => self.pitch = (self.pitch & 0x0f00) | val as u16,
            0x4083 => {
                self.pitch = (self.pitch & 0x00ff) | ((val as u16 & 0x0f) << 8);
                self.env_halt = val & 0x40 != 0;
                self.env_4x = val & 0x80 != 0;
                if self.env_4x {
                    // High bit resets the main oscillator phase.
                    self.wave_accum = 0;
                    self.wave_pos = 0;
                }
            }
            0x4084 => {
                self.mod_env_off = val & 0x80 != 0;
                self.mod_dir_up = val & 0x40 != 0;
                self.mod_speed = val & 0x3f;
                if self.mod_env_off {
                    self.mod_gain = self.mod_speed;
                }
                self.mod_timer = self.env_reload(self.mod_speed);
            }
            0x4085 => {
                // Directly set the 7-bit signed mod counter.
                self.mod_counter = ((val & 0x7f) as i32) << 25 >> 25;
            }
            0x4086 => self.mod_freq = (self.mod_freq & 0x0f00) | val as u16,
            0x4087 => {
                self.mod_freq = (self.mod_freq & 0x00ff) | ((val as u16 & 0x0f) << 8);
                if val & 0x80 != 0 {
                    // Reset the mod accumulator (bits 0-12 cleared).
                    self.mod_accum &= !0x1fff;
                }
            }
            0x4088 => {
                // Mod-table write: replaces the entry at the current position and
                // advances by one entry (two of the 64 sub-steps).
                let idx = ((self.mod_accum >> 13) & 0x1f) as usize;
                self.mod_table[idx] = val & 0x07;
                self.mod_accum = (self.mod_accum + 0x2000) & 0x3ffff;
            }
            0x4089 => {
                self.master_vol = val & 0x03;
                self.wave_write = val & 0x80 != 0;
            }
            0x408a => self.env_speed = val,
            _ => {}
        }
    }

    fn read(&self, addr: u16) -> u8 {
        match addr {
            // Write-disabled wavetable reads return the value at the current pos.
            0x4040..=0x407f => {
                if self.wave_write {
                    self.wave[(addr - 0x4040) as usize] | 0x40
                } else {
                    self.wave[self.wave_pos as usize] | 0x40
                }
            }
            0x4090 => 0x40 | (self.vol_gain & 0x3f),
            0x4092 => 0x40 | (self.mod_gain & 0x3f),
            _ => 0,
        }
    }

    /// Advance one envelope (volume or mod). Returns the new gain.
    fn clock_env(timer: &mut u32, reload: u32, gain: &mut u8, dir_up: bool) {
        if *timer == 0 {
            *timer = reload;
            if dir_up {
                if *gain < 32 {
                    *gain += 1;
                }
            } else if *gain > 0 {
                *gain -= 1;
            }
        } else {
            *timer -= 1;
        }
    }

    /// The documented mod-counter -> 20-bit modulated pitch transform.
    fn wave_pitch(&self) -> u32 {
        let mut temp: i32 = self.mod_counter * self.mod_gain as i32;
        if (temp & 0x0f) != 0 && (temp & 0x800) == 0 {
            temp += 0x20;
        }
        temp += 0x400;
        temp = (temp >> 4) & 0xff;
        (self.pitch as i32 as u32).wrapping_mul(temp as u32) & 0xf_ffff
    }

    /// One CPU cycle. Envelopes run per-cycle (gated by their timers); the
    /// oscillator + mod unit run every 16 cycles.
    fn clock(&mut self) {
        if !self.enabled {
            return;
        }
        // Envelopes (unless halted by $4083 bit6 or disabled by $408A == 0).
        if !self.env_halt && self.env_speed != 0 {
            if !self.vol_env_off {
                let reload = self.env_reload(self.vol_speed);
                Self::clock_env(&mut self.vol_timer, reload, &mut self.vol_gain, self.vol_dir_up);
            }
            if !self.mod_env_off {
                let reload = self.env_reload(self.mod_speed);
                Self::clock_env(&mut self.mod_timer, reload, &mut self.mod_gain, self.mod_dir_up);
            }
        }

        self.div16 = self.div16.wrapping_add(1);
        if self.div16 < 16 {
            return;
        }
        self.div16 = 0;

        // Modulation unit: advance the mod accumulator; on a carry out of bit 11,
        // step the mod table position and apply its entry to the mod counter.
        if !self.env_4x && self.mod_freq != 0 {
            let before = self.mod_accum;
            self.mod_accum = (self.mod_accum + self.mod_freq as u32) & 0x3ffff;
            // Carry out of the 12-bit frequency field (bit 11 -> bit 12).
            if (before & 0x1000) != (self.mod_accum & 0x1000) {
                let idx = ((self.mod_accum >> 13) & 0x1f) as usize;
                match self.mod_table[idx] {
                    0 => {}
                    1 => self.mod_counter += 1,
                    2 => self.mod_counter += 2,
                    3 => self.mod_counter += 4,
                    4 => self.mod_counter = 0,
                    5 => self.mod_counter -= 4,
                    6 => self.mod_counter -= 2,
                    _ => self.mod_counter -= 1,
                }
                // Wrap to 7-bit signed (-64..63).
                self.mod_counter = ((self.mod_counter & 0x7f) << 25) >> 25;
            }
        }

        // Main oscillator: accumulate the modulated pitch; the top 6 bits of the
        // 20-bit accumulator are the wave position.
        if !self.wave_write {
            self.wave_accum = (self.wave_accum + self.wave_pitch()) & 0xf_ffff;
            self.wave_pos = (self.wave_accum >> 14) as u8 & 0x3f;
        }

        // Output = wave sample * min(volume,32) * master, normalised to ~0..1.
        let sample = self.wave[self.wave_pos as usize] as f32; // 0..63
        let vol = self.vol_gain.min(32) as f32; // 0..32
        let master = match self.master_vol {
            0 => 1.0,
            1 => 2.0 / 3.0,
            2 => 2.0 / 4.0,
            _ => 2.0 / 5.0,
        };
        let raw = sample * vol * master / (63.0 * 32.0);
        // The real FDS DAC output passes through a low-pass that rolls off the
        // highs (approx. 1-pole, ~2 kHz cutoff), which is a big part of the
        // channel's soft/warm character. Sampled at the 16-cycle oscillator rate
        // (~111.86 kHz), a 2 kHz cutoff gives alpha ~= 0.10.
        const LP_ALPHA: f32 = 0.10;
        self.lp += LP_ALPHA * (raw - self.lp);
        self.out = self.lp;
        if self.out > self.dbg_peak {
            self.dbg_peak = self.out;
        }
    }

    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.wave);
        w.u32(self.wave_accum);
        w.u8(self.wave_pos);
        w.u16(self.pitch);
        w.bool(self.wave_write);
        w.bool(self.env_halt);
        w.bool(self.env_4x);
        w.u8(self.vol_gain);
        w.u8(self.vol_speed);
        w.bool(self.vol_dir_up);
        w.bool(self.vol_env_off);
        w.u32(self.vol_timer);
        w.bytes(&self.mod_table);
        w.u32(self.mod_accum);
        w.i32(self.mod_counter);
        w.u16(self.mod_freq);
        w.u8(self.mod_gain);
        w.u8(self.mod_speed);
        w.bool(self.mod_dir_up);
        w.bool(self.mod_env_off);
        w.u32(self.mod_timer);
        w.u8(self.env_speed);
        w.u8(self.master_vol);
        w.bool(self.enabled);
        w.u8(self.div16);
        // `out`/`lp`/`dbg_peak` are audio output-path floats (like the APU's
        // high-pass state) — never serialized, keeping the state integer-only.
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.wave)?;
        self.wave_accum = r.u32()?;
        self.wave_pos = r.u8()?;
        self.pitch = r.u16()?;
        self.wave_write = r.bool()?;
        self.env_halt = r.bool()?;
        self.env_4x = r.bool()?;
        self.vol_gain = r.u8()?;
        self.vol_speed = r.u8()?;
        self.vol_dir_up = r.bool()?;
        self.vol_env_off = r.bool()?;
        self.vol_timer = r.u32()?;
        r.bytes(&mut self.mod_table)?;
        self.mod_accum = r.u32()?;
        self.mod_counter = r.i32()?;
        self.mod_freq = r.u16()?;
        self.mod_gain = r.u8()?;
        self.mod_speed = r.u8()?;
        self.mod_dir_up = r.bool()?;
        self.mod_env_off = r.bool()?;
        self.mod_timer = r.u32()?;
        self.env_speed = r.u8()?;
        self.master_vol = r.u8()?;
        self.enabled = r.bool()?;
        self.div16 = r.u8()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RAM Adapter (the FDS "mapper")
// ---------------------------------------------------------------------------

/// Which way the head is moving data.
#[derive(Clone, Copy, PartialEq, Eq)]
enum XferMode {
    Read,
    Write,
}

/// The FDS RAM Adapter + disk drive: PRG-RAM, CHR-RAM, BIOS, the $402x/$403x
/// register file, the timer IRQ, and the byte-at-a-time disk transfer engine.
pub struct Fds {
    // ---- memory ----
    prg_ram: Vec<u8>, // 32 KiB, $6000-$DFFF
    chr_ram: Vec<u8>, // 8 KiB
    bios: Vec<u8>,    // 8 KiB, $E000-$FFFF

    // ---- disk ----
    sides: Vec<MediumSide>,
    current_side: usize,
    inserted: bool,
    /// Cycles left in the post-swap "disk absent" settle window (see
    /// `REINSERT_CYCLES`). While non-zero the drive reports no disk even though a
    /// side is selected, so the game observes the flip.
    reinsert_cycles: u32,

    // ---- timer IRQ ($4020-$4022) ----
    irq_reload: u16,
    irq_counter: u16,
    irq_repeat: bool,
    irq_timer_enabled: bool,
    timer_irq_flag: bool,

    // ---- master I/O enable ($4023) ----
    io_enabled: bool,
    sound_io_enabled: bool,

    // ---- disk control ($4024/$4025) ----
    write_reg: u8,
    ctrl: u8, // last $4025 value
    motor_on: bool,
    transfer_armed: bool, // $4025 bit6: transfer active (skips the gap to a block)
    mode: XferMode,
    mirroring: Mirroring,
    xfer_irq_enabled: bool, // $4025 bit7: IRQ on byte transfer

    // ---- transfer engine ----
    head: usize,       // byte offset into the current medium side
    in_block: bool,    // false = scanning the gap for a block start
    delay: u32,        // CPU cycles until the next byte event
    spinning_up: bool, // motor just turned on, not ready yet
    read_reg: u8,      // last byte the drive delivered ($4031)
    byte_ready: bool,  // a byte just transferred, unread ($4030 bit1)
    end_of_head: bool, // ran off the end of the side ($4030 bit6)

    // ---- FDS expansion audio ($4040-$408A) ----
    audio: FdsAudio,

    // ---- debug ----
    dbg_bytes_read: u64,
    dbg_trace: bool,
    dbg_events: u32,
    dbg_last_4030: u8,
    dbg_last_4032: u8,
    dbg_cycle: u64,
    dbg_first_bytes: u8,
}

impl Fds {
    /// Build the RAM Adapter from a parsed disk and the 8 KiB BIOS image.
    pub fn new(disk: FdsDisk, bios: Vec<u8>) -> Self {
        let sides: Vec<MediumSide> = disk.sides.iter().map(|s| MediumSide::build(s)).collect();
        let mut bios = bios;
        bios.resize(BIOS_LEN, 0);
        Fds {
            prg_ram: vec![0u8; 32 * 1024],
            chr_ram: vec![0u8; 8 * 1024],
            bios,
            sides,
            current_side: 0,
            inserted: true,
            reinsert_cycles: 0,
            irq_reload: 0,
            irq_counter: 0,
            irq_repeat: false,
            irq_timer_enabled: false,
            timer_irq_flag: false,
            io_enabled: false,
            sound_io_enabled: false,
            write_reg: 0,
            ctrl: 0,
            motor_on: false,
            transfer_armed: false,
            mode: XferMode::Read,
            mirroring: Mirroring::Horizontal,
            xfer_irq_enabled: false,
            head: 0,
            in_block: false,
            delay: 0,
            spinning_up: false,
            read_reg: 0,
            byte_ready: false,
            end_of_head: false,
            audio: FdsAudio::new(),
            dbg_bytes_read: 0,
            dbg_trace: std::env::var("FDS_TRACE").is_ok(),
            dbg_events: 0,
            dbg_last_4030: 0xff,
            dbg_last_4032: 0xff,
            dbg_cycle: 0,
            dbg_first_bytes: 0,
        }
    }

    fn trace(&mut self, msg: std::fmt::Arguments) {
        if self.dbg_trace && self.dbg_events < 400 {
            self.dbg_events += 1;
            eprintln!("[fds {:>3} @{}] {}", self.dbg_events, self.dbg_cycle, msg);
        }
    }

    /// Number of disk sides (for the frontend's disk-control interface).
    pub fn side_count(&self) -> usize {
        self.sides.len()
    }

    /// Currently loaded side index (255 if the disk is ejected).
    pub fn current_side(&self) -> usize {
        if self.inserted {
            self.current_side
        } else {
            255
        }
    }

    /// Eject the disk (frontend requests a swap): the drive reports no disk.
    pub fn eject(&mut self) {
        self.set_ejected();
    }

    /// Insert `side` (0-based). Selecting a side also inserts the disk. The
    /// drive then reports the disk absent for a short settle window so the
    /// running game observes the swap (see `REINSERT_CYCLES`).
    pub fn insert_side(&mut self, side: usize) {
        if side < self.sides.len() {
            self.current_side = side;
            self.inserted = true;
            self.reinsert_cycles = REINSERT_CYCLES;
            self.head = 0;
            self.end_transfer();
        }
    }

    /// Whether the drive currently presents a readable disk: a side is inserted
    /// and the post-swap settle window has elapsed.
    fn present(&self) -> bool {
        self.inserted && self.reinsert_cycles == 0
    }

    fn end_transfer(&mut self) {
        self.byte_ready = false;
        self.spinning_up = false;
        self.delay = 0;
    }

    /// Eject helper used by the frontend; clears the settle window.
    fn set_ejected(&mut self) {
        self.inserted = false;
        self.reinsert_cycles = 0;
        self.end_transfer();
    }

    /// Level of the combined FDS IRQ line. The timer IRQ latches until read; the
    /// disk-transfer IRQ is level-sensitive: asserted whenever a byte is waiting
    /// to be read and transfer interrupts are enabled (so enabling the IRQ after
    /// a byte has already latched still fires it).
    pub fn irq_line(&self) -> bool {
        self.timer_irq_flag || (self.byte_ready && self.xfer_irq_enabled)
    }

    // ---- $4025 decode ----
    // Bit layout (RP2C33): bit0 = motor stop, bit1 = motor start (both active
    // low); bit2 = 1:read / 0:write; bit3 = mirroring; bit4 = CRC control;
    // bit6 = transfer reset (arms the transfer, skipping the gap to the next
    // block start and resetting the CRC accumulator); bit7 = IRQ on byte transfer.
    fn write_ctrl(&mut self, val: u8) {
        let prev_motor = self.motor_on;
        let prev_armed = self.transfer_armed;
        let head = self.head;
        self.trace(format_args!(
            "W $4025 = ${val:02x} [mstop={} mstart={} {} mir={} crc={} xfer6={} irq7={}] head={head}",
            val & 1,
            (val >> 1) & 1,
            if val & 4 != 0 { "READ" } else { "WRITE" },
            (val >> 3) & 1,
            (val >> 4) & 1,
            (val >> 6) & 1,
            (val >> 7) & 1,
        ));
        self.ctrl = val;
        // Motor: start (bit1) and stop (bit0) are active low. Starting wins; if
        // neither is asserted the motor holds its current state.
        if val & 0x02 == 0 {
            self.motor_on = true;
        } else if val & 0x01 == 0 {
            self.motor_on = false;
        }
        self.mode = if val & 0x04 != 0 {
            XferMode::Read
        } else {
            XferMode::Write
        };
        // bit3: mirroring (1 = horizontal, 0 = vertical).
        self.mirroring = if val & 0x08 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        self.transfer_armed = val & 0x40 != 0;
        self.xfer_irq_enabled = val & 0x80 != 0;

        // Motor just turned on: rewind to the start of the side and spin up.
        if self.motor_on && !prev_motor {
            self.head = 0;
            self.in_block = false;
            self.end_of_head = false;
            self.spinning_up = true;
            self.delay = SPINUP_CYCLES;
            self.trace(format_args!("motor ON -> rewind, spin-up"));
        }
        if !self.motor_on {
            self.spinning_up = false;
        }
        // Transfer armed (bit6 0 -> 1): reset the CRC accumulator and restart the
        // gap scan so the drive latches on the next block start (see the doc:
        // "waits for first set bit before accumulating serial data").
        if self.transfer_armed && !prev_armed {
            self.in_block = false;
            self.byte_ready = false;
            if self.motor_on && !self.spinning_up {
                self.delay = BYTE_CYCLES;
            }
            self.dbg_first_bytes = 3;
            self.trace(format_args!(
                "transfer ARM: scan for block at head {head} mode={} irq7={}",
                if self.mode == XferMode::Read { 'R' } else { 'W' },
                self.xfer_irq_enabled as u8,
            ));
        }
    }

    /// One CPU cycle of the disk drive + timer IRQ (called from `tick_cpu`).
    fn clock(&mut self) {
        self.dbg_cycle = self.dbg_cycle.wrapping_add(1);
        // Count down the post-swap "disk absent" settle window.
        if self.reinsert_cycles > 0 {
            self.reinsert_cycles -= 1;
        }

        // Expansion audio runs every CPU cycle (its own /16 divider inside).
        self.audio.clock();
        // ---- timer IRQ ($4020-$4022) ----
        if self.irq_timer_enabled {
            if self.irq_counter == 0 {
                self.timer_irq_flag = true;
                if self.irq_repeat {
                    self.irq_counter = self.irq_reload;
                } else {
                    self.irq_timer_enabled = false;
                }
            } else {
                self.irq_counter -= 1;
            }
        }

        // ---- disk drive ----
        // The platter spins whenever the motor is on, independent of whether a
        // transfer is armed. Spin-up must therefore complete even while the BIOS
        // holds transfer-reset (it spins the motor, polls $4032 for ready, then
        // releases reset to start reading).
        if !self.motor_on || !self.present() {
            return;
        }
        if self.spinning_up {
            if self.delay > 0 {
                self.delay -= 1;
            }
            if self.delay == 0 {
                self.spinning_up = false; // disk now ready ($4032 clears not-ready)
                self.delay = BYTE_CYCLES; // first byte one period later
            }
            return;
        }
        // Spun up but transfer held in reset (or I/O disabled): idle, ready to go.
        if !self.transfer_active() {
            return;
        }
        if self.delay > 0 {
            self.delay -= 1;
        }
        if self.delay == 0 {
            self.transfer_byte();
            self.delay = BYTE_CYCLES;
        }
    }

    fn transfer_active(&self) -> bool {
        self.present() && self.motor_on && self.transfer_armed && self.io_enabled
    }

    fn transfer_byte(&mut self) {
        // No overrun: while a delivered byte is still unread by the CPU, the head
        // does not advance to the next byte. This is what keeps the head parked at
        // the block start during the BIOS's spin-up delay (it releases reset, then
        // delays without reading) instead of freewheeling deep into the disk.
        if self.byte_ready {
            return;
        }
        let side = &mut self.sides[self.current_side];
        if self.head >= side.image.len() {
            self.end_of_head = true;
            return;
        }
        // Gap scan: while not yet synced to a block, skip zero (gap) bytes at the
        // disk rate without signalling the CPU. A $80 start mark terminates the
        // gap; consume it (not delivered) and the block's type byte follows.
        if !self.in_block {
            if self.mode == XferMode::Read {
                if side.image[self.head] == 0 {
                    self.head += 1; // still in the gap
                    return;
                }
                if side.image[self.head] == START_MARK {
                    self.head += 1; // consume the start mark; block type follows
                    if self.head >= side.image.len() {
                        self.end_of_head = true;
                        return;
                    }
                }
            }
            self.in_block = true;
        }
        match self.mode {
            XferMode::Read => {
                self.read_reg = side.image[self.head];
                self.dbg_bytes_read += 1;
            }
            XferMode::Write => {
                side.image[self.head] = self.write_reg;
                side.dirty = true;
            }
        }
        if self.mode == XferMode::Read && self.dbg_first_bytes > 0 {
            self.dbg_first_bytes -= 1;
            let (h, v) = (self.head, self.read_reg);
            self.trace(format_args!("  deliver byte@{h} = ${v:02x} (irq7={})", self.xfer_irq_enabled as u8));
        }
        self.head += 1;
        self.byte_ready = true; // asserts the disk IRQ (level) if irq7 is enabled
    }

    // ---- register file ----
    fn read_disk_reg(&mut self, addr: u16) -> u8 {
        match addr {
            // $4030: disk status. Reading clears the two IRQ/transfer flags.
            0x4030 => {
                let mut s = 0u8;
                if self.timer_irq_flag {
                    s |= 0x01;
                }
                if self.byte_ready {
                    s |= 0x02;
                }
                // bit4 CRC error: always report OK (faked CRC). bit6 end-of-head.
                if self.end_of_head {
                    s |= 0x40;
                }
                s |= 0x80; // RW-enable / always-set marker the BIOS expects
                self.timer_irq_flag = false;
                self.byte_ready = false;
                if s != self.dbg_last_4030 {
                    self.dbg_last_4030 = s;
                    let h = self.head;
                    self.trace(format_args!("$4030 status -> ${s:02x} (head {h})"));
                }
                s
            }
            // $4031: read data. Returns the last byte; clears the ready flag.
            0x4031 => {
                self.byte_ready = false;
                let v = self.read_reg;
                let h = self.head;
                self.trace(format_args!("$4031 read -> ${v:02x} (head now {h})"));
                v
            }
            // $4032: drive status.
            0x4032 => {
                // All three drive-status bits are active low. Present + ready +
                // writable therefore reads as bits clear; the top bits float to
                // the open-bus $40 the BIOS tolerates.
                let mut s = 0x40u8;
                // During the post-swap settle window the disk reads as absent, so
                // the game sees the flip (removed -> re-inserted).
                let present = self.present();
                if !present {
                    s |= 0x01; // bit0: disk not inserted (active low)
                }
                if !present || self.spinning_up || !self.motor_on {
                    s |= 0x02; // bit1: drive not ready (active low)
                }
                // bit2 write-protect (active low): our disks are writable = clear.
                if s != self.dbg_last_4032 {
                    self.dbg_last_4032 = s;
                    self.trace(format_args!("$4032 drive -> ${s:02x}"));
                }
                s
            }
            // $4033: external connector. bit7 = battery good.
            0x4033 => 0x80,
            _ => 0,
        }
    }

    fn write_disk_reg(&mut self, addr: u16, val: u8) {
        if addr != 0x4025 {
            // $4025 is traced in write_ctrl with decoded fields; log the rest.
            self.trace(format_args!("W ${addr:04x} = ${val:02x}"));
        }
        match addr {
            0x4020 => self.irq_reload = (self.irq_reload & 0xff00) | val as u16,
            0x4021 => self.irq_reload = (self.irq_reload & 0x00ff) | ((val as u16) << 8),
            0x4022 => {
                self.irq_repeat = val & 0x01 != 0;
                self.irq_timer_enabled = val & 0x02 != 0;
                if self.irq_timer_enabled {
                    self.irq_counter = self.irq_reload;
                } else {
                    self.timer_irq_flag = false;
                }
            }
            0x4023 => {
                self.io_enabled = val & 0x01 != 0;
                self.sound_io_enabled = val & 0x02 != 0;
                self.audio.enabled = self.sound_io_enabled;
                if !self.io_enabled {
                    // Disabling the disk I/O masks and clears its interrupts.
                    self.irq_timer_enabled = false;
                    self.timer_irq_flag = false;
                    self.byte_ready = false;
                }
            }
            0x4024 => self.write_reg = val,
            0x4025 => self.write_ctrl(val),
            0x4026 => {} // external connector output (unused here)
            _ => {}
        }
    }
}

impl crate::cart::Mapper for Fds {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x4030..=0x4033 => self.read_disk_reg(addr),
            0x4040..=0x407f | 0x4090 | 0x4092 => self.audio.read(addr),
            0x6000..=0xdfff => self.prg_ram[(addr - 0x6000) as usize],
            0xe000..=0xffff => self.bios[(addr - 0xe000) as usize],
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x4020..=0x4026 => self.write_disk_reg(addr, val),
            0x4040..=0x407f | 0x4080..=0x408a => self.audio.write(addr, val),
            0x6000..=0xdfff => self.prg_ram[(addr - 0x6000) as usize] = val,
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr_ram[(addr & 0x1fff) as usize]
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        if addr < 0x2000 {
            self.chr_ram[(addr & 0x1fff) as usize] = val;
        }
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    fn irq(&self) -> bool {
        self.irq_line()
    }

    fn tick_cpu(&mut self) {
        self.clock();
    }

    fn audio_sample(&self) -> f32 {
        self.audio.out
    }

    fn save_ram(&mut self) -> Option<&mut [u8]> {
        // FDS "saves" are the writable disk itself, not $6000 work RAM, so we do
        // not expose PRG-RAM here (it is scratch that the BIOS reloads).
        None
    }

    fn fds_side_count(&self) -> usize {
        self.side_count()
    }
    fn fds_insert_side(&mut self, side: usize) {
        self.insert_side(side);
    }
    fn fds_eject(&mut self) {
        self.eject();
    }
    fn fds_current_side(&self) -> usize {
        self.current_side()
    }

    fn debug_dump(&self) -> String {
        format!(
            "FDS side {}/{} head={} io={} motor={} armed={} mode={} irq(timer={} disk={}) ctrl=${:02x} bytes_read={} aud(en={} pitch={} vol={} out={:.3})",
            self.current_side,
            self.sides.len(),
            self.head,
            self.io_enabled as u8,
            self.motor_on as u8,
            self.transfer_armed as u8,
            if self.mode == XferMode::Read { 'R' } else { 'W' },
            self.timer_irq_flag as u8,
            (self.byte_ready && self.xfer_irq_enabled) as u8,
            self.ctrl,
            self.dbg_bytes_read,
            self.audio.enabled as u8,
            self.audio.pitch,
            self.audio.vol_gain,
            self.audio.dbg_peak,
        )
    }
}

impl SaveState for Fds {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        w.bytes(&self.chr_ram);
        // Disk contents (writable): serialize every side's medium image so an
        // in-progress save on disk round-trips byte-identically.
        w.u8(self.sides.len() as u8);
        for s in &self.sides {
            w.u32(s.image.len() as u32);
            w.bytes(&s.image);
            w.bool(s.dirty);
        }
        w.u8(self.current_side as u8);
        w.bool(self.inserted);
        w.u32(self.reinsert_cycles);
        w.u16(self.irq_reload);
        w.u16(self.irq_counter);
        w.bool(self.irq_repeat);
        w.bool(self.irq_timer_enabled);
        w.bool(self.timer_irq_flag);
        w.bool(self.io_enabled);
        w.bool(self.sound_io_enabled);
        w.u8(self.write_reg);
        w.u8(self.ctrl);
        w.bool(self.motor_on);
        w.bool(self.transfer_armed);
        w.bool(self.mode == XferMode::Read);
        w.u8(crate::cart::mirroring_code(self.mirroring));
        w.bool(self.xfer_irq_enabled);
        w.u32(self.head as u32);
        w.bool(self.in_block);
        w.u32(self.delay);
        w.bool(self.spinning_up);
        w.u8(self.read_reg);
        w.bool(self.byte_ready);
        w.bool(self.end_of_head);
        self.audio.save(w);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.prg_ram)?;
        r.bytes(&mut self.chr_ram)?;
        let n = r.u8()? as usize;
        if n != self.sides.len() {
            return Err(LoadError::BadValue("fds side count"));
        }
        // Dev only: keep the freshly-built disk image (skip the stored one) so a
        // state saved under an older medium layout can still be loaded to
        // reproduce a bug — valid only for states with no on-disk writes yet.
        let keep_disk = std::env::var("FDS_STATE_KEEP_DISK").is_ok();
        for s in &mut self.sides {
            let len = r.u32()? as usize;
            if keep_disk {
                r.skip(len)?;
                let _ = r.bool()?; // dirty flag
                continue;
            }
            if len != s.image.len() {
                return Err(LoadError::BadValue("fds side length"));
            }
            r.bytes(&mut s.image)?;
            s.dirty = r.bool()?;
        }
        self.current_side = r.u8()? as usize;
        self.inserted = r.bool()?;
        self.reinsert_cycles = r.u32()?;
        self.irq_reload = r.u16()?;
        self.irq_counter = r.u16()?;
        self.irq_repeat = r.bool()?;
        self.irq_timer_enabled = r.bool()?;
        self.timer_irq_flag = r.bool()?;
        self.io_enabled = r.bool()?;
        self.sound_io_enabled = r.bool()?;
        self.write_reg = r.u8()?;
        self.ctrl = r.u8()?;
        self.motor_on = r.bool()?;
        self.transfer_armed = r.bool()?;
        self.mode = if r.bool()? { XferMode::Read } else { XferMode::Write };
        self.mirroring = crate::cart::mirroring_from_code(r.u8()?)?;
        self.xfer_irq_enabled = r.bool()?;
        self.head = r.u32()? as usize;
        self.in_block = r.bool()?;
        self.delay = r.u32()?;
        self.spinning_up = r.bool()?;
        self.read_reg = r.u8()?;
        self.byte_ready = r.bool()?;
        self.end_of_head = r.bool()?;
        self.audio.load(r)?;
        if self.current_side >= self.sides.len() {
            return Err(LoadError::BadValue("fds current side"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic one-side disk: block1 (56) + block2 (2) + one file
    /// header(16)+body(1+4). Enough to exercise the block walker and the drive.
    fn synth_side() -> Vec<u8> {
        let mut s = vec![0u8; SIDE_LEN];
        s[0] = 0x01;
        s[1..15].copy_from_slice(b"*NINTENDO-HVC*");
        let mut pos = 56;
        s[pos] = 0x02;
        s[pos + 1] = 1; // one file
        pos += 2;
        s[pos] = 0x03; // file header
        s[pos + 13] = 4; // size = 4 (LE u16)
        s[pos + 14] = 0;
        pos += 16;
        s[pos] = 0x04; // file body: type + 4 data bytes
        s[pos + 1] = 0xaa;
        s[pos + 2] = 0xbb;
        s[pos + 3] = 0xcc;
        s[pos + 4] = 0xdd;
        s
    }

    #[test]
    fn detects_headerless_and_fwnes() {
        let raw = synth_side();
        assert!(FdsDisk::is_fds(&raw));
        let mut wrapped = b"FDS\x1a".to_vec();
        wrapped.push(1);
        wrapped.extend(std::iter::repeat(0).take(11));
        wrapped.extend_from_slice(&raw);
        assert!(FdsDisk::is_fds(&wrapped));
        assert_eq!(FdsDisk::parse(&wrapped).unwrap().sides.len(), 1);
        assert!(!FdsDisk::is_fds(b"NES\x1a\x01\x01"));
    }

    #[test]
    fn medium_reconstruction_has_lead_in_gap_then_blocks() {
        let side = MediumSide::build(&synth_side());
        // Lead-in gap of zeros, then block 1's start mark + type byte.
        assert!(side.image[..LEAD_IN_GAP].iter().all(|&b| b == 0));
        assert_eq!(side.image[LEAD_IN_GAP], START_MARK);
        assert_eq!(side.image[LEAD_IN_GAP + 1], 0x01);
        // block1 = mark(1) + data(56) + CRC(2) + gap(1); block 2 begins with its
        // own mark, then the type byte.
        let block2 = LEAD_IN_GAP + 1 + 56 + CRC_FOOTER + BLOCK_GAP;
        assert_eq!(side.image[block2], START_MARK);
        assert_eq!(side.image[block2 + 1], 0x02);
    }

    #[test]
    fn drive_scans_gap_then_streams_the_block() {
        let disk = FdsDisk::parse(&synth_side()).unwrap();
        let mut fds = Fds::new(disk, vec![0u8; BIOS_LEN]);
        // Enable I/O; start the motor (bit1 low), read mode (bit2), not yet armed.
        fds.write_disk_reg(0x4023, 0x01);
        fds.write_disk_reg(0x4025, 0x01 | 0x04); // motor start, read, unarmed
        // Spin up.
        for _ in 0..(SPINUP_CYCLES + BYTE_CYCLES + 4) {
            fds.clock();
        }
        // Arm the transfer (bit6): scan the lead-in gap then deliver the block.
        fds.write_disk_reg(0x4025, 0x01 | 0x04 | 0x40); // + transfer arm
        // Enough byte periods to cross the whole lead-in gap and reach block 1.
        for _ in 0..((LEAD_IN_GAP as u32 + 2) * BYTE_CYCLES) {
            fds.clock();
            if fds.byte_ready {
                break;
            }
        }
        assert!(fds.byte_ready);
        assert_eq!(fds.read_reg, 0x01); // first non-gap byte = block 1's type
        assert!(fds.in_block);
    }

    #[test]
    fn side_switch_reads_the_selected_side() {
        // Two sides whose block-1 game-name byte (offset 16) differs, so we can
        // tell which side the drive is streaming.
        let mut a = synth_side();
        a[16] = 0xAA;
        let mut b = synth_side();
        b[16] = 0xBB;
        let disk = FdsDisk::parse(&[a, b].concat()).unwrap();
        let mut fds = Fds::new(disk, vec![0u8; BIOS_LEN]);
        assert_eq!(fds.side_count(), 2);

        // Read the 17th byte (offset 16) of whichever side is inserted.
        let read_gamename = |fds: &mut Fds| -> u8 {
            // Let any post-swap "disk absent" settle window elapse first.
            for _ in 0..(REINSERT_CYCLES + 16) {
                fds.clock();
            }
            fds.write_disk_reg(0x4023, 0x01); // enable I/O
            fds.write_disk_reg(0x4025, 0x01 | 0x04); // motor start, read
            for _ in 0..(SPINUP_CYCLES + BYTE_CYCLES + 4) {
                fds.clock();
            }
            fds.write_disk_reg(0x4025, 0x01 | 0x04 | 0x40); // arm transfer
            // Consume bytes until we've read 17 (offset 0..=16), acking each.
            let mut last = 0u8;
            for _ in 0..17 {
                for _ in 0..(LEAD_IN_GAP as u32 + 4) * BYTE_CYCLES {
                    fds.clock();
                    if fds.byte_ready {
                        break;
                    }
                }
                last = fds.read_disk_reg(0x4031);
            }
            last
        };

        // Side 0 is inserted at power-on.
        assert_eq!(read_gamename(&mut fds), 0xAA);
        // Flip to side 1 and the drive now streams side-1 data.
        fds.insert_side(1);
        assert_eq!(fds.current_side(), 1);
        assert_eq!(read_gamename(&mut fds), 0xBB);
    }

    #[test]
    fn timer_irq_counts_down_and_repeats() {
        let disk = FdsDisk::parse(&synth_side()).unwrap();
        let mut fds = Fds::new(disk, vec![0u8; BIOS_LEN]);
        fds.write_disk_reg(0x4023, 0x01); // enable I/O
        fds.write_disk_reg(0x4020, 3); // reload low
        fds.write_disk_reg(0x4021, 0); // reload high
        fds.write_disk_reg(0x4022, 0x03); // repeat + enable
        // counter starts at 3; fires when it reaches 0.
        for _ in 0..4 {
            fds.clock();
        }
        assert!(fds.irq_line());
        // Reading $4030 acknowledges the timer IRQ.
        let s = fds.read_disk_reg(0x4030);
        assert_eq!(s & 0x01, 0x01);
        assert!(!fds.timer_irq_flag);
    }
}
