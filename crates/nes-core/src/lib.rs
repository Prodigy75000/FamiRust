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

pub use save::{LoadError, ReadCursor, SaveState, WriteCursor};

use bus::Bus;
use cpu::Cpu;

/// A whole NES: CPU plus the bus that owns every other device.
pub struct Nes {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl Nes {
    /// Build a machine from a raw `.nes` (iNES / NES 2.0) image.
    pub fn from_rom(rom: &[u8]) -> Result<Self, cart::CartError> {
        let cart = cart::Cartridge::from_ines(rom)?;
        let mapper = cart::make_mapper(cart)?;
        Ok(Nes {
            cpu: Cpu::new(),
            bus: Bus::new(mapper),
        })
    }

    /// Serialize the entire machine to a byte-identical snapshot. Two machines
    /// in the same logical state produce equal `Vec<u8>` on any platform.
    pub fn save_state(&self) -> Vec<u8> {
        let mut w = WriteCursor::new();
        self.cpu.save(&mut w);
        self.bus.save(&mut w);
        w.into_bytes()
    }

    /// Restore a snapshot produced by [`Nes::save_state`] on a machine built
    /// from the *same ROM*. Refuses truncated, malformed, or over-long buffers
    /// (a cross-engine transfer must reject a mismatched state, never guess).
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        let mut r = ReadCursor::new(bytes);
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

    #[test]
    fn load_rejects_truncated_state() {
        let mut nes = Nes::from_rom(&synth_rom()).unwrap();
        let mut snap = nes.save_state();
        snap.truncate(snap.len() - 1);
        assert_eq!(nes.load_state(&snap), Err(LoadError::UnexpectedEof));
    }
}
