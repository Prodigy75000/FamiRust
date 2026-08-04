// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! FamiRust — a clean-room NES / Famicom emulator core in Rust.
//!
//! Design priorities, in order:
//!   1. **Correctness / accuracy.** A cycle-stepped 2A03 CPU interleaved with a
//!      dot-accurate 2C02 PPU and the 2A03 APU. Conformance-first: the CPU is
//!      ground to green against the TomHarte single-step vectors before it is
//!      trusted to drive anything else.
//!   2. **Byte-identical, deterministic save states** (see [`save`]). This is a
//!      hard requirement, not a feature — cross-engine netplay and rollback are
//!      only sound if two machines in the same logical state always serialize to
//!      the same bytes. Everything mutable participates in one fixed-order
//!      serialization; nothing platform- or pointer-dependent enters it.
//!   3. **No third-party emulator source.** Clean-room from hardware
//!      documentation only; reference emulators are off-limits.
//!
//! The public surface is intentionally small for now: build a [`Nes`] from a
//! ROM image, and snapshot / restore its state. The stepping/run loop and the
//! frontend-facing frame + audio + input API arrive with the CPU and PPU.

pub mod apu;
pub mod bus;
pub mod cart;
pub mod controller;
pub mod cpu;
pub mod fds;
pub mod header_db;
pub mod ppu;
pub mod save;

pub use apu::SAMPLE_RATE;
pub use cpu::{Cpu, CpuBus};
pub use save::{LoadError, ReadCursor, SaveState, WriteCursor};

use bus::Bus;

/// Leading magic on every save state. Identifies the producing core so a
/// cross-engine transfer can reject a foreign or corrupt buffer up front rather
/// than misinterpret it (contract rule 4/5 in `docs/SAVESTATE.md`).
pub const STATE_MAGIC: &[u8; 8] = b"FAMIRST1";

/// Save-state layout version. Bump on ANY change to the serialized field set or
/// order; older builds refuse a newer version cleanly (never panic, never guess).
/// v2: added the PPU vbl_just_cleared field (sub-cycle timing rework).
/// v3: sub-cycle timing rework.
/// v4: FDS RAM Adapter mapper (new serialized field set for disk games; carts
/// on the older mappers are unaffected and their v3 states still load).
pub const STATE_VERSION: u16 = 4;

/// A whole NES: CPU plus the bus that owns every other device.
pub struct Nes {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl Nes {
    /// Build a machine from a raw `.nes` (iNES / NES 2.0) image, then run the
    /// 7-cycle power-on reset so PC is loaded from the reset vector.
    pub fn from_rom(rom: &[u8]) -> Result<Self, cart::CartError> {
        let cart = cart::Cartridge::from_ines(rom)?;
        let mapper = cart::make_mapper(cart)?;
        let mut nes = Nes {
            cpu: Cpu::new(),
            bus: Bus::new(mapper),
        };
        nes.cpu.reset(&mut nes.bus);
        Ok(nes)
    }

    /// Build a machine from an FDS disk image (`.fds`, headered or raw) plus the
    /// 8 KiB FDS BIOS (`disksys.rom`, user-supplied). The RAM Adapter is the
    /// mapper; the BIOS drives boot from its reset vector. The disk is not a
    /// cartridge, so it does not go through [`cart::from_ines`].
    pub fn from_fds(disk_image: &[u8], bios: &[u8]) -> Result<Self, fds::FdsError> {
        let disk = fds::FdsDisk::parse(disk_image)?;
        let mapper = fds::Fds::new(disk, bios.to_vec());
        let mut nes = Nes {
            cpu: Cpu::new(),
            bus: Bus::new(Box::new(mapper)),
        };
        nes.cpu.reset(&mut nes.bus);
        Ok(nes)
    }

    /// True if `data` looks like an FDS disk image (so a frontend knows to call
    /// [`Nes::from_fds`] with a BIOS instead of [`Nes::from_rom`]).
    pub fn is_fds(data: &[u8]) -> bool {
        fds::FdsDisk::is_fds(data)
    }

    // ---- FDS disk-control (side/disk swap) ----

    /// Number of FDS disk sides (0 if this is not an FDS machine).
    pub fn fds_side_count(&self) -> usize {
        self.bus.mapper.fds_side_count()
    }

    /// Insert FDS `side` (0-based), which also re-inserts the disk after an eject.
    pub fn fds_insert_side(&mut self, side: usize) {
        self.bus.mapper.fds_insert_side(side);
    }

    /// Eject the FDS disk (the drive reports "no disk" until a side is inserted).
    pub fn fds_eject(&mut self) {
        self.bus.mapper.fds_eject();
    }

    /// Currently inserted FDS side (255 = ejected; 0 for non-FDS machines).
    pub fn fds_current_side(&self) -> usize {
        self.bus.mapper.fds_current_side()
    }

    /// Run one instruction. Returns the CPU cycles it consumed.
    pub fn step(&mut self) -> u64 {
        self.cpu.step(&mut self.bus)
    }

    /// Re-run the power-on reset sequence (for test ROMs that request a reset).
    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.bus);
    }

    /// Read a CPU-space byte without advancing the machine (no PPU tick). For
    /// test harnesses probing PRG-RAM result ports; not a cycle-accurate read.
    pub fn peek(&mut self, addr: u16) -> u8 {
        self.bus.peek(addr)
    }

    /// Run until the PPU completes a frame (reaches vblank), then return the
    /// ARGB8888 framebuffer (256x240).
    pub fn step_frame(&mut self) -> &[u32] {
        self.bus.ppu.frame_complete = false;
        while !self.bus.ppu.frame_complete {
            self.cpu.step(&mut self.bus);
        }
        &self.bus.ppu.framebuffer
    }

    /// Set the button bitmask for controller `port` (0 or 1). See
    /// [`controller::button`] for bit positions.
    pub fn set_buttons(&mut self, port: usize, buttons: u8) {
        self.bus.controllers[port].buttons = buttons;
    }

    /// Drain accumulated host-rate audio samples (f32, ~44.1 kHz mono).
    pub fn take_audio(&mut self) -> Vec<f32> {
        self.bus.apu.take_samples()
    }

    /// Debug: the scanline at which sprite-0 hit was set this frame (-1 = none).
    pub fn dbg_sprite0_scanline(&self) -> i32 {
        self.bus.ppu.dbg_s0_scanline
    }

    /// Debug: the dot (0..340) at which sprite-0 hit was set this frame (-1 = none).
    pub fn dbg_sprite0_dot(&self) -> i32 {
        self.bus.ppu.dbg_s0_dot
    }

    /// Debug: absolute PPU frame counter.
    pub fn dbg_frame(&self) -> u64 {
        self.bus.ppu.frame
    }

    /// Debug: current PPUMASK (bit1 = show BG left-8, bit2 = show sprites left-8).
    pub fn dbg_ppu_mask(&self) -> u8 {
        self.bus.ppu.mask
    }

    /// Debug: current CPU program counter (to spot a hang/stuck loop).
    pub fn dbg_pc(&self) -> u16 {
        self.cpu.pc
    }

    pub fn dbg_halted(&self) -> bool {
        self.cpu.halted
    }

    pub fn dbg_mapper(&self) -> String {
        self.bus.mapper.debug_dump()
    }

    /// The 2 KiB internal RAM ($0000-$07FF). This is RetroAchievements'
    /// `RETRO_MEMORY_SYSTEM_RAM` for the NES -- the region almost every
    /// achievement set reads.
    pub fn system_ram(&mut self) -> &mut [u8] {
        &mut self.bus.ram
    }

    /// Cartridge work/save RAM ($6000-$7FFF) if the mapper provides it, else
    /// `None`. This is RA's `RETRO_MEMORY_SAVE_RAM` and the battery save.
    pub fn save_ram(&mut self) -> Option<&mut [u8]> {
        self.bus.mapper.save_ram()
    }

    // ---- debug harness (see docs/DEBUG_HARNESS.md) ----

    /// Enable/disable per-layer capture. Off by default (zero cost in normal
    /// play); the frontend turns it on while the Core Debug overlay is open.
    pub fn dbg_set_capture(&mut self, on: bool) {
        self.bus.ppu.dbg_capture = on;
    }
    /// Which layers composite into the visible frame: bit0=BG, bit1=OBJ.
    pub fn dbg_set_layer_mask(&mut self, mask: u8) {
        self.bus.ppu.dbg_layer_mask = mask;
    }
    /// The BG-only frame from the last rendered frame (needs capture on).
    pub fn dbg_bg_layer(&self) -> &[u32] {
        &self.bus.ppu.bg_layer
    }
    /// The OBJ-only frame (magenta = transparent) from the last rendered frame.
    pub fn dbg_obj_layer(&self) -> &[u32] {
        &self.bus.ppu.obj_layer
    }
    /// OAM as a JSON array of 64 sprites, for the on-device inspector.
    pub fn dbg_oam_json(&self) -> String {
        self.bus.ppu.dbg_oam_json()
    }
    /// The 32 palette entries as ARGB8888.
    pub fn dbg_palette_argb(&self) -> [u32; 32] {
        self.bus.ppu.dbg_palette_argb()
    }
    /// The pattern-table tile sheet (`DBG_TILES_W` x `DBG_TILES_H` ARGB8888).
    pub fn dbg_tiles_argb(&mut self) -> Vec<u32> {
        self.bus.ppu.dbg_tiles_argb(&mut *self.bus.mapper)
    }

    /// Test hook: force MMC5 extended-attribute mode (to validate a captured state
    /// whose format predates the `$5104` field).
    pub fn dbg_force_ext_attr(&mut self) {
        self.bus.mapper.dbg_force_ext_attr();
    }

    /// PPUCTRL byte (for diagnosing which nametable/pattern table is selected).
    pub fn dbg_ppu_ctrl(&self) -> u8 {
        self.bus.ppu.ctrl
    }

    /// Scroll/mask registers + a coarse map of which columns of nametable 0 hold
    /// non-zero tiles (helps tell a rendering bug from a partly-written nametable).
    pub fn dbg_ppu_scroll(&self) -> String {
        let p = &self.bus.ppu;
        // Per-column count of non-zero tile bytes in physical CIRAM bank A (0x000..0x3c0).
        let mut cols = [0u16; 32];
        for row in 0..30usize {
            for col in 0..32usize {
                if p.ciram[row * 32 + col] != 0 {
                    cols[col] += 1;
                }
            }
        }
        let colmap: String = cols
            .iter()
            .map(|&c| if c == 0 { '.' } else { std::char::from_digit((c as u32).min(35), 36).unwrap() })
            .collect();
        // Tile indices of one mid-screen row (row 18) to see if cols 12..31 are a
        // distinct "blank" tile or real varied indices.
        let row: String = (0..32).map(|c| format!("{:02x} ", p.ciram[18 * 32 + c])).collect();
        // Attribute row covering tile-row 18 (attr row 4 = CIRAM 0x3c0 + 4*8) + palette.
        let attr: String = (0..8).map(|c| format!("{:02x} ", p.ciram[0x3c0 + 4 * 8 + c])).collect();
        let pal: String = (0..32).map(|i| format!("{:02x} ", p.palette[i])).collect();
        format!(
            "v=${:04x} t=${:04x} fineX={} mask=${:02x} ctrl=${:02x}  coarseX={} coarseY={} ntbase={}  NT0 col-fill[{}]\n  NT0 row18 tiles: {}",
            p.v, p.t, p.x_fine, p.mask, p.ctrl,
            p.v & 0x1f, (p.v >> 5) & 0x1f, (p.v >> 10) & 3,
            colmap, row,
        ) + &format!("\n  NT0 row18 attr(8): {}\n  palette: {}", attr, pal)
    }

    /// Serialize the entire machine to a byte-identical snapshot. The layout is
    /// `MAGIC(8) || format_version(u16 LE) || cpu || bus`. Two machines in the
    /// same logical state produce an equal `Vec<u8>` on any target triple.
    pub fn save_state(&self) -> Vec<u8> {
        let mut w = WriteCursor::new();
        w.bytes(STATE_MAGIC);
        w.u16(STATE_VERSION);
        self.cpu.save(&mut w);
        self.bus.save(&mut w);
        w.into_bytes()
    }

    /// Byte length of a snapshot for this machine. Stable for a given ROM/mapper
    /// and equal across platforms per `format_version` — the netplay handshake
    /// keys on it (contract rule 7). Cheap enough to compute by serializing.
    pub fn state_size(&self) -> usize {
        self.save_state().len()
    }

    /// Restore a snapshot produced by [`Nes::save_state`] on a machine built
    /// from the *same ROM*. Verifies the magic and refuses a newer
    /// `format_version`, then refuses truncated, malformed, or over-long buffers
    /// (a cross-engine transfer must reject a mismatched state, never guess).
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        let mut r = ReadCursor::new(bytes);
        let mut magic = [0u8; 8];
        r.bytes(&mut magic)?;
        if &magic != STATE_MAGIC {
            return Err(LoadError::BadMagic);
        }
        let version = r.u16()?;
        if version > STATE_VERSION {
            return Err(LoadError::UnsupportedVersion(version));
        }
        self.cpu.load(&mut r)?;
        self.bus.load(&mut r)?;
        r.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_rom() -> Vec<u8> {
        let mut v = vec![0u8; 16];
        v[0..4].copy_from_slice(b"NES\x1a");
        v[4] = 1; // 16 KiB PRG
        v[5] = 1; // 8 KiB CHR
        v.extend(std::iter::repeat(0u8).take(16 * 1024));
        v.extend(std::iter::repeat(0u8).take(8 * 1024));
        v
    }

    #[test]
    fn exposes_system_ram_for_retroachievements() {
        // SYSTEM_RAM is exactly the 2 KiB internal RAM and aliases bus.ram, so RA
        // reads the live emulated memory (not a copy).
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        assert_eq!(nes.system_ram().len(), 2048);
        nes.system_ram()[0x321] = 0xab;
        assert_eq!(nes.bus.ram[0x321], 0xab);
        // NROM has no battery work RAM, so SAVE_RAM is absent. (MMC1/MMC3/MMC5
        // expose their 8 KiB+ PRG-RAM here -- verified against Zelda/SMB3 dumps.)
        assert!(nes.save_ram().is_none());
    }

    /// A minimal one-side FDS image (block 1 + file-count + one small file) and a
    /// dummy BIOS, enough to build an FDS machine for the save-state round-trip.
    fn synth_fds() -> Vec<u8> {
        let mut s = vec![0u8; 65500];
        s[0] = 0x01;
        s[1..15].copy_from_slice(b"*NINTENDO-HVC*");
        s[56] = 0x02; // file-amount block
        s[57] = 1;
        s[58] = 0x03; // file header, size 2
        s[71] = 2;
        s[74] = 0x04; // file body
        s
    }

    #[test]
    fn fds_machine_state_round_trips_byte_identically() {
        let disk = synth_fds();
        let bios = vec![0u8; fds::BIOS_LEN];
        let mut nes = Nes::from_fds(&disk, &bios).unwrap();
        assert_eq!(nes.fds_side_count(), 1);
        // Drive the disk registers so the RAM Adapter carries non-trivial state.
        nes.bus.write(0x4023, 0x01); // enable disk I/O
        nes.bus.write(0x4025, 0x05); // motor start, read
        for _ in 0..64 {
            nes.step();
        }
        let snap = nes.save_state();
        assert_eq!(&snap[8..10], &[0x04, 0x00]); // format_version = 4 (FDS)

        let mut other = Nes::from_fds(&disk, &bios).unwrap();
        other.load_state(&snap).unwrap();
        assert_eq!(snap, other.save_state());
    }

    /// A cross-side transfer must be refused: a state whose side count does not
    /// match this machine's disk is rejected, never guessed.
    #[test]
    fn fds_load_rejects_mismatched_disk() {
        let one_side = synth_fds();
        let bios = vec![0u8; fds::BIOS_LEN];
        let nes = Nes::from_fds(&one_side, &bios).unwrap();
        let snap = nes.save_state();
        // A two-side disk has a different serialized shape; loading the one-side
        // state into it must fail rather than corrupt the drive.
        let two_side = [one_side.clone(), one_side].concat();
        let mut other = Nes::from_fds(&two_side, &bios).unwrap();
        assert!(other.load_state(&snap).is_err());
    }

    #[test]
    fn machine_state_round_trips_byte_identically() {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        // Dirty some state so the snapshot is non-trivial.
        nes.cpu.a = 0x99;
        nes.cpu.pc = 0x8000;
        nes.bus.ram[0x123] = 0x45;
        nes.bus.controllers[0].buttons = controller::button::START;

        let snap = nes.save_state();

        let mut other = Nes::from_rom(&synth_rom()).unwrap();
        other.load_state(&snap).unwrap();
        // The restored machine re-serializes to the exact same bytes.
        assert_eq!(snap, other.save_state());
    }

    /// Golden header: the exact leading bytes are part of the cross-platform
    /// contract. A zeroed machine's first 10 bytes are magic + version, and the
    /// serialized size is fixed for this ROM/mapper.
    #[test]
    fn state_header_is_golden_and_size_is_stable() {
        let nes = Nes::from_rom(&synth_rom()).unwrap();
        let snap = nes.save_state();
        assert_eq!(&snap[0..8], b"FAMIRST1");
        assert_eq!(&snap[8..10], &[0x04, 0x00]); // format_version = 4, LE
        // NROM 16K PRG + CHR ROM (no CHR RAM): size is deterministic.
        // header(10) + cpu(15) + ram(2048) + ppu + apu + 2 pads + mapper(prg_ram
        // 8192) + open_bus(1). Pinned to a literal -- comparing against
        // `state_size()` would be vacuous, since that is itself `save_state().len()`.
        assert_eq!(snap.len(), GOLDEN_RESET_LEN);
        assert_eq!(nes.state_size(), GOLDEN_RESET_LEN);
    }

    /// FNV-1a (64-bit). Defined inline so the golden test depends on nothing but
    /// the serializer itself: integer-only, fixed-width, no allocation, and no
    /// behavior of its own that could vary by target.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Serialized length of a freshly-reset NROM machine built from `synth_rom`.
    const GOLDEN_RESET_LEN: usize = 12_825;
    /// Serialized length of the `golden_fixture` machine (same ROM, dirtied +
    /// stepped). Equal to `GOLDEN_RESET_LEN` unless a field became
    /// content-dependent, which contract rule 7 forbids.
    const GOLDEN_FIXTURE_LEN: usize = 12_825;
    /// FNV-1a 64 over the entire `golden_fixture` snapshot.
    const GOLDEN_FIXTURE_FNV: u64 = 0xd6ec_b126_a505_efc3;

    /// A fixed, deliberately non-trivial machine. Every value and every step
    /// count below is part of the golden fixture -- changing one changes the
    /// expected checksum, on purpose.
    fn golden_fixture() -> Nes {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        nes.cpu.a = 0x99;
        nes.cpu.x = 0x3c;
        nes.cpu.y = 0xf1;
        nes.cpu.pc = 0x8000;
        nes.bus.ram[0x123] = 0x45;
        nes.bus.ram[0x7ff] = 0xa7;
        nes.bus.controllers[0].buttons = controller::button::START;
        for _ in 0..512 {
            nes.step();
        }
        nes
    }

    /// **Golden bytes — the contract's "whole ballgame"**
    /// (`TrophyHubResources/specs/play/IN_HOUSE_CORE_SAVESTATE_SPEC.md`).
    ///
    /// A fixed machine state must serialize to one exact byte string on every
    /// target triple. Because the format is target-independent *by construction*
    /// (LE, fixed-width, fixed field order, no `usize`, no map iteration), these
    /// golden values are identical everywhere -- so this test passing on the dev
    /// host is what makes iOS/Android/Desktop state agreement a *tested* property
    /// rather than a hoped-for one. A full hex literal of a ~10 KiB NES state is
    /// unusable in review, so the byte string is pinned as (exact length +
    /// checksum over the whole buffer): any added, removed, reordered, or
    /// re-widened field turns this red.
    #[test]
    fn state_is_golden_byte_for_byte() {
        let snap = golden_fixture().save_state();
        assert_eq!(&snap[0..8], b"FAMIRST1");
        assert_eq!(&snap[8..10], &[0x04, 0x00]);
        assert_eq!(snap.len(), GOLDEN_FIXTURE_LEN);
        assert_eq!(fnv1a64(&snap), GOLDEN_FIXTURE_FNV);
    }

    /// Two independently-built machines driven through the identical input
    /// sequence must serialize identically -- determinism across instances, not
    /// just round-trip fidelity within one (contract rule 6). This is the
    /// property netplay lockstep actually leans on.
    #[test]
    fn independent_instances_serialize_identically() {
        assert_eq!(golden_fixture().save_state(), golden_fixture().save_state());
    }

    #[test]
    fn load_rejects_truncated_state() {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        let mut snap = nes.save_state();
        snap.truncate(snap.len() - 1);
        assert_eq!(nes.load_state(&snap), Err(LoadError::UnexpectedEof));
    }

    #[test]
    fn load_rejects_wrong_magic() {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        let mut snap = nes.save_state();
        snap[0] ^= 0xff; // corrupt the magic
        assert_eq!(nes.load_state(&snap), Err(LoadError::BadMagic));
    }

    #[test]
    fn load_rejects_newer_version() {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        let mut snap = nes.save_state();
        // Stamp format_version = 0x00FF (LE at offset 8), far newer than we know.
        snap[8] = 0xff;
        snap[9] = 0x00;
        assert_eq!(nes.load_state(&snap), Err(LoadError::UnsupportedVersion(0x00ff)));
    }

    #[test]
    fn load_rejects_oversized_state() {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        let mut snap = nes.save_state();
        snap.push(0x00); // one trailing byte too many
        assert_eq!(nes.load_state(&snap), Err(LoadError::TrailingBytes));
    }
}
