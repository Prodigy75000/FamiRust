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
pub const STATE_VERSION: u16 = 3;

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
        assert_eq!(&snap[8..10], &[0x03, 0x00]); // format_version = 3, LE
        // NROM 16K PRG + CHR ROM (no CHR RAM): size is deterministic.
        // header(10) + cpu(15) + ram(2048) + ppu + apu + 2 pads + mapper(prg_ram
        // 8192) + open_bus(1). Assert it is fixed and matches state_size().
        assert_eq!(snap.len(), nes.state_size());
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
