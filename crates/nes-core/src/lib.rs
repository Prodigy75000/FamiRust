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
pub mod ppu;
pub mod save;

pub use cpu::{Cpu, CpuBus};
pub use save::{LoadError, ReadCursor, SaveState, WriteCursor};

use bus::Bus;

/// Leading magic on every save state. Identifies the producing core so a
/// cross-engine transfer can reject a foreign or corrupt buffer up front rather
/// than misinterpret it (contract rule 4/5 in `docs/SAVESTATE.md`).
pub const STATE_MAGIC: &[u8; 8] = b"FAMIRST1";

/// Save-state layout version. Bump on ANY change to the serialized field set or
/// order; older builds refuse a newer version cleanly (never panic, never guess).
pub const STATE_VERSION: u16 = 1;

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
        assert_eq!(&snap[8..10], &[0x01, 0x00]); // format_version = 1, LE
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
