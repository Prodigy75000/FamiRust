//! Ricoh 2A03 CPU — an NMOS 6502 with decimal mode fused off.
//!
//! Only the architectural register file and its save-state contract exist so
//! far. The instruction core is built next, cycle-by-cycle, and ground to green
//! against the TomHarte `6502` single-step vectors via the `tomharte` harness —
//! the same conformance-first approach used for the GBA ARM7 and SNES 65C816.
//!
//! Accuracy intent: this will be a *cycle-stepped* 6502 (every read/write tick
//! visible to the bus), because the PPU/APU and DMA timing that games depend on
//! only fall out correctly when the CPU's individual memory cycles interleave
//! with them. A whole-instruction interpreter cannot reproduce the sub-
//! instruction bus behaviour netplay determinism ultimately rests on.

use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

/// Processor status flags (P register) bit positions.
pub mod flag {
    pub const CARRY: u8 = 1 << 0;
    pub const ZERO: u8 = 1 << 1;
    pub const IRQ_DISABLE: u8 = 1 << 2;
    pub const DECIMAL: u8 = 1 << 3; // present in P but the 2A03 ALU ignores it
    pub const BREAK: u8 = 1 << 4;
    pub const UNUSED: u8 = 1 << 5; // physically always reads 1
    pub const OVERFLOW: u8 = 1 << 6;
    pub const NEGATIVE: u8 = 1 << 7;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,
    /// Total CPU cycles elapsed — part of saved state so timing is continuous
    /// across a save/load boundary.
    pub cycles: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        // Post-reset architectural state. The reset vector fetch and real PC
        // load happen in `reset()` once a bus is attached.
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xfd,
            pc: 0,
            p: flag::IRQ_DISABLE | flag::UNUSED,
            cycles: 0,
        }
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SaveState for Cpu {
    fn save(&self, w: &mut WriteCursor) {
        w.u8(self.a);
        w.u8(self.x);
        w.u8(self.y);
        w.u8(self.sp);
        w.u16(self.pc);
        w.u8(self.p);
        w.u64(self.cycles);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.a = r.u8()?;
        self.x = r.u8()?;
        self.y = r.u8()?;
        self.sp = r.u8()?;
        self.pc = r.u16()?;
        self.p = r.u8()?;
        self.cycles = r.u64()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_file_round_trips_byte_identically() {
        let cpu = Cpu {
            a: 0x42,
            x: 0x13,
            y: 0x37,
            sp: 0xf0,
            pc: 0xc123,
            p: flag::CARRY | flag::NEGATIVE | flag::UNUSED,
            cycles: 1_234_567,
        };
        let mut w = WriteCursor::new();
        cpu.save(&mut w);
        let bytes = w.into_bytes();

        let mut back = Cpu::default();
        let mut r = ReadCursor::new(&bytes);
        back.load(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(cpu, back);

        // Re-serializing the loaded state yields identical bytes.
        let mut w2 = WriteCursor::new();
        back.save(&mut w2);
        assert_eq!(bytes, w2.into_bytes());
    }
}
