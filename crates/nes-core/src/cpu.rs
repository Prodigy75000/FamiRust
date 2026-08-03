// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Ricoh 2A03 CPU — an NMOS 6502 with decimal mode fused off.
//!
//! Cycle-stepped: the CPU talks to memory only through [`CpuBus`], and **every
//! `read`/`write` call is exactly one CPU cycle**. Because the 6502 has no idle
//! bus cycles (even "internal" cycles do a dummy read), performing each access
//! in the exact order the hardware does automatically reproduces the correct
//! cycle count *and* the per-cycle bus trace — which is what the TomHarte
//! `nes6502` single-step vectors verify. Nothing here counts cycles by table;
//! timing is an emergent property of doing the right accesses in the right order.
//!
//! Implemented from `docs/notes/cpu-2a03-6502.md` (clean-room notes distilled
//! from the NESdev 6502 cycle reference), never from other emulator source.

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
use flag::*;

/// The CPU's view of the system: one access == one cycle. The real machine's
/// bus implements this and ticks the PPU/APU per access; the CPU test harness
/// implements it over flat 64 KiB memory with a bus-access log.
pub trait CpuBus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, val: u8);
    /// Current NMI line level (PPU vblank-NMI). The CPU edge-detects it. Default
    /// low so the conformance harness sees no interrupts.
    fn nmi(&self) -> bool {
        false
    }
    /// Current IRQ line level (APU/mapper, level-sensitive, gated by the I flag).
    fn irq(&self) -> bool {
        false
    }
}

/// The interrupt selected by a cycle's poll. NMI (edge-latched) outranks IRQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    None,
    Nmi,
    Irq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    /// Status register. Internally bit 5 (unused) is held 1 and bit 4 (B) is
    /// held 0; the pushed byte synthesizes B per cause (see [`Cpu::status_byte`]).
    pub p: u8,
    /// Total CPU cycles elapsed — saved so timing is continuous across a state
    /// load. Incremented once per bus access in [`Cpu::rb`] / [`Cpu::wb`].
    pub cycles: u64,
    /// Set by a JAM/KIL opcode: the CPU is locked until reset.
    pub halted: bool,
    /// NMI edge latch (edge-triggered; consumed when the NMI is serviced).
    pub nmi_flag: bool,
    /// Last sampled NMI line level, for edge detection.
    pub nmi_line_prev: bool,
    /// Interrupt poll result for the cycle just executed, and the one before it.
    /// The 6502 acts on the poll from the instruction's *second-to-last* cycle,
    /// so a line change on the final cycle is deferred one instruction.
    pub poll: Pending,
    pub poll_prev: Pending,
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xfd,
            pc: 0,
            p: IRQ_DISABLE | UNUSED,
            cycles: 0,
            halted: false,
            nmi_flag: false,
            nmi_line_prev: false,
            poll: Pending::None,
            poll_prev: Pending::None,
        }
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- one-cycle bus primitives (the only places that count cycles) ----

    #[inline]
    fn rb<B: CpuBus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        self.cycles += 1;
        let v = bus.read(addr);
        self.poll_interrupts(bus);
        v
    }

    #[inline]
    fn wb<B: CpuBus>(&mut self, bus: &mut B, addr: u16, val: u8) {
        self.cycles += 1;
        bus.write(addr, val);
        self.poll_interrupts(bus);
    }

    /// Sample the interrupt lines once per cycle. NMI is edge-latched; IRQ is
    /// level-sensitive and gated by the I flag. `poll_prev` lags `poll` by one
    /// cycle so the instruction boundary acts on the second-to-last cycle's poll.
    #[inline]
    fn poll_interrupts<B: CpuBus>(&mut self, bus: &B) {
        self.poll_prev = self.poll;
        let nmi = bus.nmi();
        if nmi && !self.nmi_line_prev {
            self.nmi_flag = true; // latch the rising edge
        }
        self.nmi_line_prev = nmi;
        self.poll = if self.nmi_flag {
            Pending::Nmi
        } else if bus.irq() && !self.get_flag(IRQ_DISABLE) {
            Pending::Irq
        } else {
            Pending::None
        };
    }

    /// Read at PC then advance PC (an opcode/operand fetch).
    #[inline]
    fn fetch<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        let v = self.rb(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    #[inline]
    fn push<B: CpuBus>(&mut self, bus: &mut B, val: u8) {
        self.wb(bus, 0x0100 | self.sp as u16, val);
        self.sp = self.sp.wrapping_sub(1);
    }

    #[inline]
    fn pull<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        self.rb(bus, 0x0100 | self.sp as u16)
    }

    // ---- flag helpers ----

    #[inline]
    fn set_flag(&mut self, bit: u8, cond: bool) {
        if cond {
            self.p |= bit;
        } else {
            self.p &= !bit;
        }
    }
    #[inline]
    fn get_flag(&self, bit: u8) -> bool {
        self.p & bit != 0
    }
    #[inline]
    fn set_zn(&mut self, v: u8) {
        self.set_flag(ZERO, v == 0);
        self.set_flag(NEGATIVE, v & 0x80 != 0);
    }

    /// The P byte as pushed to the stack. `software` (PHP/BRK) pushes B=1;
    /// hardware IRQ/NMI push B=0. Bit 5 is always 1.
    #[inline]
    fn status_byte(&self, software: bool) -> u8 {
        (self.p & 0xcf) | UNUSED | if software { BREAK } else { 0 }
    }
    /// Store a pulled P byte, discarding bits 4/5 (they have no register).
    #[inline]
    fn set_status(&mut self, byte: u8) {
        self.p = (byte & 0xcf) | UNUSED;
    }

    // ---- addressing modes (each performs its own dummy reads => cycle count) ----

    /// Zero page: operand address is the fetched byte (high byte 0).
    #[inline]
    fn addr_zp<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        self.fetch(bus) as u16
    }

    /// Zero page,X / ,Y: fetch base, dummy read at base, add index (wraps in ZP).
    #[inline]
    fn addr_zp_indexed<B: CpuBus>(&mut self, bus: &mut B, index: u8) -> u16 {
        let base = self.fetch(bus);
        self.rb(bus, base as u16); // dummy read at un-indexed ZP address
        base.wrapping_add(index) as u16
    }

    /// Absolute.
    #[inline]
    fn addr_abs<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        (hi << 8) | lo
    }

    /// Absolute,X / ,Y. `always_dummy` = true for writes and RMW (they always
    /// spend the un-fixed dummy read); false for reads (dummy only on page cross).
    #[inline]
    fn addr_abs_indexed<B: CpuBus>(&mut self, bus: &mut B, index: u8, always_dummy: bool) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        let base = ((hi as u16) << 8) | lo as u16;
        let addr = base.wrapping_add(index as u16);
        let crossed = (addr & 0xff00) != (base & 0xff00);
        if crossed || always_dummy {
            let unfixed = (base & 0xff00) | (addr & 0x00ff);
            self.rb(bus, unfixed); // dummy read at un-fixed high byte
        }
        addr
    }

    /// Indexed indirect (zp,X): dummy read at base, pointer = (base+X)&0xFF, read
    /// 16-bit effective address from the zero page (pointer read wraps in ZP).
    #[inline]
    fn addr_izx<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let base = self.fetch(bus);
        self.rb(bus, base as u16); // dummy read
        let ptr = base.wrapping_add(self.x);
        let lo = self.rb(bus, ptr as u16) as u16;
        let hi = self.rb(bus, ptr.wrapping_add(1) as u16) as u16;
        (hi << 8) | lo
    }

    /// Indirect indexed (zp),Y. `always_dummy` as in [`Cpu::addr_abs_indexed`].
    #[inline]
    fn addr_izy<B: CpuBus>(&mut self, bus: &mut B, always_dummy: bool) -> u16 {
        let ptr = self.fetch(bus);
        let lo = self.rb(bus, ptr as u16) as u16;
        let hi = self.rb(bus, ptr.wrapping_add(1) as u16) as u16; // ZP wrap
        let base = (hi << 8) | lo;
        let addr = base.wrapping_add(self.y as u16);
        let crossed = (addr & 0xff00) != (base & 0xff00);
        if crossed || always_dummy {
            let unfixed = (base & 0xff00) | (addr & 0x00ff);
            self.rb(bus, unfixed);
        }
        addr
    }

    // ---- ALU / operation helpers ----

    fn adc(&mut self, m: u8) {
        let c = self.get_flag(CARRY) as u16;
        let sum = self.a as u16 + m as u16 + c;
        let result = sum as u8;
        self.set_flag(CARRY, sum > 0xff);
        // Signed overflow: A and M same sign, result differs.
        self.set_flag(OVERFLOW, (!(self.a ^ m) & (self.a ^ result) & 0x80) != 0);
        self.a = result;
        self.set_zn(result);
    }
    #[inline]
    fn sbc(&mut self, m: u8) {
        self.adc(m ^ 0xff); // A - M - (1-C) == A + ~M + C
    }
    fn compare(&mut self, reg: u8, m: u8) {
        let t = reg.wrapping_sub(m);
        self.set_flag(CARRY, reg >= m);
        self.set_flag(ZERO, reg == m);
        self.set_flag(NEGATIVE, t & 0x80 != 0);
    }
    fn and(&mut self, m: u8) {
        self.a &= m;
        self.set_zn(self.a);
    }
    fn ora(&mut self, m: u8) {
        self.a |= m;
        self.set_zn(self.a);
    }
    fn eor(&mut self, m: u8) {
        self.a ^= m;
        self.set_zn(self.a);
    }
    fn bit(&mut self, m: u8) {
        self.set_flag(ZERO, (self.a & m) == 0);
        self.set_flag(NEGATIVE, m & 0x80 != 0);
        self.set_flag(OVERFLOW, m & 0x40 != 0);
    }
    fn asl(&mut self, v: u8) -> u8 {
        self.set_flag(CARRY, v & 0x80 != 0);
        let r = v << 1;
        self.set_zn(r);
        r
    }
    fn lsr(&mut self, v: u8) -> u8 {
        self.set_flag(CARRY, v & 0x01 != 0);
        let r = v >> 1;
        self.set_zn(r);
        r
    }
    fn rol(&mut self, v: u8) -> u8 {
        let c = self.get_flag(CARRY) as u8;
        self.set_flag(CARRY, v & 0x80 != 0);
        let r = (v << 1) | c;
        self.set_zn(r);
        r
    }
    fn ror(&mut self, v: u8) -> u8 {
        let c = self.get_flag(CARRY) as u8;
        self.set_flag(CARRY, v & 0x01 != 0);
        let r = (v >> 1) | (c << 7);
        self.set_zn(r);
        r
    }
    fn inc(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.set_zn(r);
        r
    }
    fn dec(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.set_zn(r);
        r
    }

    /// Read-modify-write scaffold: read, write the original back (dummy write),
    /// then write the modified value. `f` computes the modified value and sets
    /// flags. This reproduces the observable RMW double-write.
    #[inline]
    fn rmw<B: CpuBus, F: FnOnce(&mut Self, u8) -> u8>(&mut self, bus: &mut B, addr: u16, f: F) {
        let v = self.rb(bus, addr);
        self.wb(bus, addr, v); // dummy write of original value
        let r = f(self, v);
        self.wb(bus, addr, r);
    }

    /// Conditional relative branch. Offset fetch is cycle 2; a taken branch adds
    /// a dummy read at PC, and a page cross adds a second dummy read.
    #[inline]
    fn branch<B: CpuBus>(&mut self, bus: &mut B, taken: bool) {
        let offset = self.fetch(bus) as i8 as i16;
        if taken {
            let old_pc = self.pc;
            self.rb(bus, old_pc); // dummy read at PC
            let target = (old_pc as i16).wrapping_add(offset) as u16;
            if (target & 0xff00) != (old_pc & 0xff00) {
                let unfixed = (old_pc & 0xff00) | (target & 0x00ff);
                self.rb(bus, unfixed); // page-cross dummy read
            }
            self.pc = target;
        }
    }

    // ---- reset / interrupts ----

    /// 7-cycle reset: writes suppressed, S -= 3, I set, PC = [$FFFC].
    pub fn reset<B: CpuBus>(&mut self, bus: &mut B) {
        self.rb(bus, self.pc);
        self.rb(bus, self.pc);
        // Three "pushes" that only decrement S (writes suppressed on reset).
        self.rb(bus, 0x0100 | self.sp as u16);
        self.sp = self.sp.wrapping_sub(1);
        self.rb(bus, 0x0100 | self.sp as u16);
        self.sp = self.sp.wrapping_sub(1);
        self.rb(bus, 0x0100 | self.sp as u16);
        self.sp = self.sp.wrapping_sub(1);
        let lo = self.rb(bus, 0xfffc) as u16;
        let hi = self.rb(bus, 0xfffd) as u16;
        self.pc = (hi << 8) | lo;
        self.set_flag(IRQ_DISABLE, true);
        self.halted = false;
    }

    /// Shared IRQ/NMI hardware interrupt entry (B=0 pushed).
    fn service_interrupt<B: CpuBus>(&mut self, bus: &mut B, nmi: bool) {
        self.rb(bus, self.pc); // dummy
        self.rb(bus, self.pc); // dummy
        self.push(bus, (self.pc >> 8) as u8);
        self.push(bus, self.pc as u8);
        self.push(bus, self.status_byte(false));
        self.set_flag(IRQ_DISABLE, true);
        let vec: u16 = if nmi { 0xfffa } else { 0xfffe };
        let lo = self.rb(bus, vec) as u16;
        let hi = self.rb(bus, vec + 1) as u16;
        self.pc = (hi << 8) | lo;
    }

    /// Execute one instruction (or service a pending interrupt). Returns the
    /// number of cycles it consumed.
    pub fn step<B: CpuBus>(&mut self, bus: &mut B) -> u64 {
        let start = self.cycles;

        if self.halted {
            self.rb(bus, self.pc); // JAM keeps the bus alive but never advances
            return self.cycles - start;
        }

        // Act on the interrupt poll from the previous instruction's
        // second-to-last cycle (see `poll_interrupts`).
        match self.poll_prev {
            Pending::Nmi => {
                self.nmi_flag = false; // edge consumed
                self.service_interrupt(bus, true);
                return self.cycles - start;
            }
            Pending::Irq => {
                self.service_interrupt(bus, false);
                return self.cycles - start;
            }
            Pending::None => {}
        }

        let op = self.fetch(bus);
        self.execute(bus, op);
        self.cycles - start
    }

    fn execute<B: CpuBus>(&mut self, bus: &mut B, op: u8) {
        match op {
            // ---------------- Loads ----------------
            0xa9 => { let v = self.fetch(bus); self.a = v; self.set_zn(v); }
            0xa5 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }
            0xb5 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }
            0xad => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }
            0xbd => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }
            0xb9 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }
            0xa1 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }
            0xb1 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.a = v; self.set_zn(v); }

            0xa2 => { let v = self.fetch(bus); self.x = v; self.set_zn(v); }
            0xa6 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.x = v; self.set_zn(v); }
            0xb6 => { let a = self.addr_zp_indexed(bus, self.y); let v = self.rb(bus, a); self.x = v; self.set_zn(v); }
            0xae => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.x = v; self.set_zn(v); }
            0xbe => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.x = v; self.set_zn(v); }

            0xa0 => { let v = self.fetch(bus); self.y = v; self.set_zn(v); }
            0xa4 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.y = v; self.set_zn(v); }
            0xb4 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.y = v; self.set_zn(v); }
            0xac => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.y = v; self.set_zn(v); }
            0xbc => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.y = v; self.set_zn(v); }

            // ---------------- Stores ----------------
            0x85 => { let a = self.addr_zp(bus); self.wb(bus, a, self.a); }
            0x95 => { let a = self.addr_zp_indexed(bus, self.x); self.wb(bus, a, self.a); }
            0x8d => { let a = self.addr_abs(bus); self.wb(bus, a, self.a); }
            0x9d => { let a = self.addr_abs_indexed(bus, self.x, true); self.wb(bus, a, self.a); }
            0x99 => { let a = self.addr_abs_indexed(bus, self.y, true); self.wb(bus, a, self.a); }
            0x81 => { let a = self.addr_izx(bus); self.wb(bus, a, self.a); }
            0x91 => { let a = self.addr_izy(bus, true); self.wb(bus, a, self.a); }

            0x86 => { let a = self.addr_zp(bus); self.wb(bus, a, self.x); }
            0x96 => { let a = self.addr_zp_indexed(bus, self.y); self.wb(bus, a, self.x); }
            0x8e => { let a = self.addr_abs(bus); self.wb(bus, a, self.x); }

            0x84 => { let a = self.addr_zp(bus); self.wb(bus, a, self.y); }
            0x94 => { let a = self.addr_zp_indexed(bus, self.x); self.wb(bus, a, self.y); }
            0x8c => { let a = self.addr_abs(bus); self.wb(bus, a, self.y); }

            // ---------------- Transfers ----------------
            0xaa => { self.rb(bus, self.pc); self.x = self.a; self.set_zn(self.x); } // TAX
            0xa8 => { self.rb(bus, self.pc); self.y = self.a; self.set_zn(self.y); } // TAY
            0x8a => { self.rb(bus, self.pc); self.a = self.x; self.set_zn(self.a); } // TXA
            0x98 => { self.rb(bus, self.pc); self.a = self.y; self.set_zn(self.a); } // TYA
            0xba => { self.rb(bus, self.pc); self.x = self.sp; self.set_zn(self.x); } // TSX
            0x9a => { self.rb(bus, self.pc); self.sp = self.x; } // TXS (no flags)

            // ---------------- Stack ----------------
            0x48 => { self.rb(bus, self.pc); self.push(bus, self.a); } // PHA
            0x08 => { self.rb(bus, self.pc); let b = self.status_byte(true); self.push(bus, b); } // PHP
            0x68 => { self.rb(bus, self.pc); self.rb(bus, 0x0100 | self.sp as u16); let v = self.pull(bus); self.a = v; self.set_zn(v); } // PLA
            0x28 => { self.rb(bus, self.pc); self.rb(bus, 0x0100 | self.sp as u16); let v = self.pull(bus); self.set_status(v); } // PLP

            // ---------------- Logical ----------------
            0x29 => { let v = self.fetch(bus); self.and(v); }
            0x25 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.and(v); }
            0x35 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.and(v); }
            0x2d => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.and(v); }
            0x3d => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.and(v); }
            0x39 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.and(v); }
            0x21 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.and(v); }
            0x31 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.and(v); }

            0x09 => { let v = self.fetch(bus); self.ora(v); }
            0x05 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.ora(v); }
            0x15 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.ora(v); }
            0x0d => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.ora(v); }
            0x1d => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.ora(v); }
            0x19 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.ora(v); }
            0x01 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.ora(v); }
            0x11 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.ora(v); }

            0x49 => { let v = self.fetch(bus); self.eor(v); }
            0x45 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.eor(v); }
            0x55 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.eor(v); }
            0x4d => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.eor(v); }
            0x5d => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.eor(v); }
            0x59 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.eor(v); }
            0x41 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.eor(v); }
            0x51 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.eor(v); }

            0x24 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.bit(v); }
            0x2c => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.bit(v); }

            // ---------------- Arithmetic ----------------
            0x69 => { let v = self.fetch(bus); self.adc(v); }
            0x65 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.adc(v); }
            0x75 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.adc(v); }
            0x6d => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.adc(v); }
            0x7d => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.adc(v); }
            0x79 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.adc(v); }
            0x61 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.adc(v); }
            0x71 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.adc(v); }

            0xe9 | 0xeb => { let v = self.fetch(bus); self.sbc(v); } // SBC + illegal alias $EB
            0xe5 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.sbc(v); }
            0xf5 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.sbc(v); }
            0xed => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.sbc(v); }
            0xfd => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.sbc(v); }
            0xf9 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.sbc(v); }
            0xe1 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.sbc(v); }
            0xf1 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.sbc(v); }

            0xc9 => { let v = self.fetch(bus); self.compare(self.a, v); }
            0xc5 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.compare(self.a, v); }
            0xd5 => { let a = self.addr_zp_indexed(bus, self.x); let v = self.rb(bus, a); self.compare(self.a, v); }
            0xcd => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.compare(self.a, v); }
            0xdd => { let a = self.addr_abs_indexed(bus, self.x, false); let v = self.rb(bus, a); self.compare(self.a, v); }
            0xd9 => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.compare(self.a, v); }
            0xc1 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.compare(self.a, v); }
            0xd1 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.compare(self.a, v); }

            0xe0 => { let v = self.fetch(bus); self.compare(self.x, v); }
            0xe4 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.compare(self.x, v); }
            0xec => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.compare(self.x, v); }

            0xc0 => { let v = self.fetch(bus); self.compare(self.y, v); }
            0xc4 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.compare(self.y, v); }
            0xcc => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.compare(self.y, v); }

            // ---------------- Inc / Dec (register) ----------------
            0xe8 => { self.rb(bus, self.pc); self.x = self.x.wrapping_add(1); self.set_zn(self.x); }
            0xca => { self.rb(bus, self.pc); self.x = self.x.wrapping_sub(1); self.set_zn(self.x); }
            0xc8 => { self.rb(bus, self.pc); self.y = self.y.wrapping_add(1); self.set_zn(self.y); }
            0x88 => { self.rb(bus, self.pc); self.y = self.y.wrapping_sub(1); self.set_zn(self.y); }

            // ---------------- Inc / Dec (memory, RMW) ----------------
            0xe6 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| c.inc(v)); }
            0xf6 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| c.inc(v)); }
            0xee => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| c.inc(v)); }
            0xfe => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| c.inc(v)); }
            0xc6 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| c.dec(v)); }
            0xd6 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| c.dec(v)); }
            0xce => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| c.dec(v)); }
            0xde => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| c.dec(v)); }

            // ---------------- Shifts / rotates ----------------
            0x0a => { self.rb(bus, self.pc); let r = self.asl(self.a); self.a = r; }
            0x06 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| c.asl(v)); }
            0x16 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| c.asl(v)); }
            0x0e => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| c.asl(v)); }
            0x1e => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| c.asl(v)); }

            0x4a => { self.rb(bus, self.pc); let r = self.lsr(self.a); self.a = r; }
            0x46 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| c.lsr(v)); }
            0x56 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| c.lsr(v)); }
            0x4e => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| c.lsr(v)); }
            0x5e => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| c.lsr(v)); }

            0x2a => { self.rb(bus, self.pc); let r = self.rol(self.a); self.a = r; }
            0x26 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| c.rol(v)); }
            0x36 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| c.rol(v)); }
            0x2e => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| c.rol(v)); }
            0x3e => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| c.rol(v)); }

            0x6a => { self.rb(bus, self.pc); let r = self.ror(self.a); self.a = r; }
            0x66 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| c.ror(v)); }
            0x76 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| c.ror(v)); }
            0x6e => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| c.ror(v)); }
            0x7e => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| c.ror(v)); }

            // ---------------- Flag ops ----------------
            0x18 => { self.rb(bus, self.pc); self.set_flag(CARRY, false); }
            0x38 => { self.rb(bus, self.pc); self.set_flag(CARRY, true); }
            0x58 => { self.rb(bus, self.pc); self.set_flag(IRQ_DISABLE, false); }
            0x78 => { self.rb(bus, self.pc); self.set_flag(IRQ_DISABLE, true); }
            0xb8 => { self.rb(bus, self.pc); self.set_flag(OVERFLOW, false); }
            0xd8 => { self.rb(bus, self.pc); self.set_flag(DECIMAL, false); }
            0xf8 => { self.rb(bus, self.pc); self.set_flag(DECIMAL, true); }

            // ---------------- Branches ----------------
            0x10 => { let t = !self.get_flag(NEGATIVE); self.branch(bus, t); } // BPL
            0x30 => { let t = self.get_flag(NEGATIVE); self.branch(bus, t); } // BMI
            0x50 => { let t = !self.get_flag(OVERFLOW); self.branch(bus, t); } // BVC
            0x70 => { let t = self.get_flag(OVERFLOW); self.branch(bus, t); } // BVS
            0x90 => { let t = !self.get_flag(CARRY); self.branch(bus, t); } // BCC
            0xb0 => { let t = self.get_flag(CARRY); self.branch(bus, t); } // BCS
            0xd0 => { let t = !self.get_flag(ZERO); self.branch(bus, t); } // BNE
            0xf0 => { let t = self.get_flag(ZERO); self.branch(bus, t); } // BEQ

            // ---------------- Jumps / calls ----------------
            0x4c => { let a = self.addr_abs(bus); self.pc = a; } // JMP abs
            0x6c => {
                // JMP indirect with the NMOS page-boundary wrap bug.
                let ptr = self.addr_abs(bus);
                let lo = self.rb(bus, ptr) as u16;
                let hi = self.rb(bus, (ptr & 0xff00) | ((ptr + 1) & 0x00ff)) as u16;
                self.pc = (hi << 8) | lo;
            }
            0x20 => {
                // JSR: push return address (points at last JSR byte) then fetch ADH.
                let adl = self.fetch(bus);
                self.rb(bus, 0x0100 | self.sp as u16); // internal stack read
                self.push(bus, (self.pc >> 8) as u8);
                self.push(bus, self.pc as u8);
                let adh = self.fetch(bus);
                self.pc = ((adh as u16) << 8) | adl as u16;
            }
            0x60 => {
                // RTS
                self.rb(bus, self.pc); // dummy
                self.rb(bus, 0x0100 | self.sp as u16); // dummy
                let lo = self.pull(bus) as u16;
                let hi = self.pull(bus) as u16;
                self.pc = (hi << 8) | lo;
                self.rb(bus, self.pc); // dummy read at PC
                self.pc = self.pc.wrapping_add(1);
            }
            0x40 => {
                // RTI
                self.rb(bus, self.pc); // dummy
                self.rb(bus, 0x0100 | self.sp as u16); // dummy
                let p = self.pull(bus);
                self.set_status(p);
                let lo = self.pull(bus) as u16;
                let hi = self.pull(bus) as u16;
                self.pc = (hi << 8) | lo;
            }
            0x00 => {
                // BRK (software interrupt, B=1 pushed)
                self.fetch(bus); // read+ignore signature byte
                self.push(bus, (self.pc >> 8) as u8);
                self.push(bus, self.pc as u8);
                let b = self.status_byte(true);
                self.push(bus, b);
                self.set_flag(IRQ_DISABLE, true);
                let lo = self.rb(bus, 0xfffe) as u16;
                let hi = self.rb(bus, 0xffff) as u16;
                self.pc = (hi << 8) | lo;
            }

            // ---------------- NOPs (official + illegal) ----------------
            0xea | 0x1a | 0x3a | 0x5a | 0x7a | 0xda | 0xfa => { self.rb(bus, self.pc); } // implied NOP
            0x80 | 0x82 | 0x89 | 0xc2 | 0xe2 => { self.fetch(bus); } // immediate NOP
            0x04 | 0x44 | 0x64 => { let a = self.addr_zp(bus); self.rb(bus, a); } // zp NOP
            0x14 | 0x34 | 0x54 | 0x74 | 0xd4 | 0xf4 => { let a = self.addr_zp_indexed(bus, self.x); self.rb(bus, a); } // zpx NOP
            0x0c => { let a = self.addr_abs(bus); self.rb(bus, a); } // abs NOP
            0x1c | 0x3c | 0x5c | 0x7c | 0xdc | 0xfc => { let a = self.addr_abs_indexed(bus, self.x, false); self.rb(bus, a); } // abx NOP (+page-cross dummy)

            // ---------------- Illegal RMW+ALU (stable) ----------------
            // SLO = ASL M; ORA A,M
            0x07 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            0x17 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            0x0f => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            0x1f => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            0x1b => { let a = self.addr_abs_indexed(bus, self.y, true); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            0x03 => { let a = self.addr_izx(bus); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            0x13 => { let a = self.addr_izy(bus, true); self.rmw(bus, a, |c, v| { let r = c.asl(v); c.ora(r); r }); }
            // RLA = ROL M; AND A,M
            0x27 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            0x37 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            0x2f => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            0x3f => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            0x3b => { let a = self.addr_abs_indexed(bus, self.y, true); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            0x23 => { let a = self.addr_izx(bus); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            0x33 => { let a = self.addr_izy(bus, true); self.rmw(bus, a, |c, v| { let r = c.rol(v); c.and(r); r }); }
            // SRE = LSR M; EOR A,M
            0x47 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            0x57 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            0x4f => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            0x5f => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            0x5b => { let a = self.addr_abs_indexed(bus, self.y, true); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            0x43 => { let a = self.addr_izx(bus); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            0x53 => { let a = self.addr_izy(bus, true); self.rmw(bus, a, |c, v| { let r = c.lsr(v); c.eor(r); r }); }
            // RRA = ROR M; ADC A,M
            0x67 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            0x77 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            0x6f => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            0x7f => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            0x7b => { let a = self.addr_abs_indexed(bus, self.y, true); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            0x63 => { let a = self.addr_izx(bus); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            0x73 => { let a = self.addr_izy(bus, true); self.rmw(bus, a, |c, v| { let r = c.ror(v); c.adc(r); r }); }
            // DCP = DEC M; CMP A,M
            0xc7 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            0xd7 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            0xcf => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            0xdf => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            0xdb => { let a = self.addr_abs_indexed(bus, self.y, true); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            0xc3 => { let a = self.addr_izx(bus); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            0xd3 => { let a = self.addr_izy(bus, true); self.rmw(bus, a, |c, v| { let r = c.dec(v); let acc = c.a; c.compare(acc, r); r }); }
            // ISC = INC M; SBC A,M
            0xe7 => { let a = self.addr_zp(bus); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }
            0xf7 => { let a = self.addr_zp_indexed(bus, self.x); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }
            0xef => { let a = self.addr_abs(bus); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }
            0xff => { let a = self.addr_abs_indexed(bus, self.x, true); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }
            0xfb => { let a = self.addr_abs_indexed(bus, self.y, true); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }
            0xe3 => { let a = self.addr_izx(bus); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }
            0xf3 => { let a = self.addr_izy(bus, true); self.rmw(bus, a, |c, v| { let r = c.inc(v); c.sbc(r); r }); }

            // ---------------- Illegal: SAX / LAX ----------------
            0x87 => { let a = self.addr_zp(bus); self.wb(bus, a, self.a & self.x); }
            0x97 => { let a = self.addr_zp_indexed(bus, self.y); self.wb(bus, a, self.a & self.x); }
            0x8f => { let a = self.addr_abs(bus); self.wb(bus, a, self.a & self.x); }
            0x83 => { let a = self.addr_izx(bus); self.wb(bus, a, self.a & self.x); }

            0xa7 => { let a = self.addr_zp(bus); let v = self.rb(bus, a); self.a = v; self.x = v; self.set_zn(v); }
            0xb7 => { let a = self.addr_zp_indexed(bus, self.y); let v = self.rb(bus, a); self.a = v; self.x = v; self.set_zn(v); }
            0xaf => { let a = self.addr_abs(bus); let v = self.rb(bus, a); self.a = v; self.x = v; self.set_zn(v); }
            0xbf => { let a = self.addr_abs_indexed(bus, self.y, false); let v = self.rb(bus, a); self.a = v; self.x = v; self.set_zn(v); }
            0xa3 => { let a = self.addr_izx(bus); let v = self.rb(bus, a); self.a = v; self.x = v; self.set_zn(v); }
            0xb3 => { let a = self.addr_izy(bus, false); let v = self.rb(bus, a); self.a = v; self.x = v; self.set_zn(v); }

            // ---------------- Illegal immediate ALU ----------------
            0x0b | 0x2b => { let v = self.fetch(bus); self.and(v); self.set_flag(CARRY, self.a & 0x80 != 0); } // ANC
            0x4b => { let v = self.fetch(bus); self.a &= v; let r = self.lsr(self.a); self.a = r; } // ALR
            0x6b => {
                // ARR: A = (A & M) then a ROR with quirky C/V (binary-mode 2A03 form).
                let v = self.fetch(bus);
                self.a &= v;
                let c = self.get_flag(CARRY) as u8;
                let r = (self.a >> 1) | (c << 7);
                self.a = r;
                self.set_zn(r);
                self.set_flag(CARRY, r & 0x40 != 0);
                self.set_flag(OVERFLOW, ((r >> 6) ^ (r >> 5)) & 1 != 0);
            }
            0xcb => {
                // AXS/SBX: X = (A & X) - M, binary, C = borrow.
                let v = self.fetch(bus);
                let t = self.a & self.x;
                self.set_flag(CARRY, t >= v);
                let r = t.wrapping_sub(v);
                self.x = r;
                self.set_zn(r);
            }
            0x8b => {
                // XAA/ANE (unstable): A = (A | magic) & X & M. Magic per TomHarte model.
                let v = self.fetch(bus);
                self.a = (self.a | 0xee) & self.x & v;
                self.set_zn(self.a);
            }
            0xab => {
                // LAX #imm / LXA (unstable): A = X = (A | magic) & M.
                let v = self.fetch(bus);
                let r = (self.a | 0xee) & v;
                self.a = r;
                self.x = r;
                self.set_zn(r);
            }

            // ---------------- Illegal unstable stores ----------------
            0x9c => { self.store_high_and_val(bus, self.x, self.y); } // SHY abx: index=X, store Y & (H+1)
            0x9e => { self.store_high_and_val(bus, self.y, self.x); } // SHX aby: index=Y, store X & (H+1)
            0x9f => { self.store_sha(bus, self.y); } // SHA aby
            0x93 => { self.store_sha_izy(bus); } // SHA izy
            0x9b => {
                // TAS/SHS aby: S = A & X; store S & (H+1)
                self.sp = self.a & self.x;
                let s = self.sp;
                self.store_high_and_val(bus, self.y, s);
            }
            0xbb => {
                // LAS aby: A = X = S = M & S
                let a = self.addr_abs_indexed(bus, self.y, false);
                let v = self.rb(bus, a);
                let r = v & self.sp;
                self.a = r;
                self.x = r;
                self.sp = r;
                self.set_zn(r);
            }

            // ---------------- JAM / KIL ----------------
            // The CPU locks up. The reference model emits a fixed 11-cycle bus
            // pattern (opcode fetch already done): a dummy operand read, then a
            // jammed read sequence over the vector addresses, then halt. PC is
            // left at opcode+1.
            0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92 | 0xb2 | 0xd2 | 0xf2 => {
                self.rb(bus, self.pc); // cycle 2: dummy operand read (no PC++)
                self.rb(bus, 0xffff);
                self.rb(bus, 0xfffe);
                self.rb(bus, 0xfffe);
                self.rb(bus, 0xffff);
                self.rb(bus, 0xffff);
                self.rb(bus, 0xffff);
                self.rb(bus, 0xffff);
                self.rb(bus, 0xffff);
                self.rb(bus, 0xffff); // cycles 3..11
                self.halted = true;
            }
        }
    }

    /// Unstable store core (SHY/SHX/SHA/TAS): value = `reg & (H+1)` where H is
    /// the base address high byte; the operand is indexed by `index`. On a page
    /// cross the target's high byte is corrupted to the value — the accurate
    /// model the TomHarte vectors encode.
    fn store_high_and_val<B: CpuBus>(&mut self, bus: &mut B, index: u8, reg: u8) {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        let base = ((hi as u16) << 8) | lo as u16;
        let addr = base.wrapping_add(index as u16);
        let unfixed = (base & 0xff00) | (addr & 0x00ff);
        self.rb(bus, unfixed); // always-dummy read
        let value = reg & hi.wrapping_add(1);
        let target = if (addr & 0xff00) != (base & 0xff00) {
            (value as u16) << 8 | (addr & 0x00ff)
        } else {
            addr
        };
        self.wb(bus, target, value);
    }

    fn store_sha<B: CpuBus>(&mut self, bus: &mut B, index: u8) {
        let val = self.a & self.x;
        self.store_high_and_val(bus, index, val);
    }

    fn store_sha_izy<B: CpuBus>(&mut self, bus: &mut B) {
        let ptr = self.fetch(bus);
        let lo = self.rb(bus, ptr as u16) as u16;
        let hi = self.rb(bus, ptr.wrapping_add(1) as u16);
        let base = ((hi as u16) << 8) | lo;
        let addr = base.wrapping_add(self.y as u16);
        let unfixed = (base & 0xff00) | (addr & 0x00ff);
        self.rb(bus, unfixed);
        let value = self.a & self.x & hi.wrapping_add(1);
        let target = if (addr & 0xff00) != (base & 0xff00) {
            (value as u16) << 8 | (addr & 0x00ff)
        } else {
            addr
        };
        self.wb(bus, target, value);
    }
}

fn decode_pending(v: u8) -> Result<Pending, LoadError> {
    match v {
        0 => Ok(Pending::None),
        1 => Ok(Pending::Nmi),
        2 => Ok(Pending::Irq),
        _ => Err(LoadError::BadValue("Pending")),
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
        w.bool(self.halted);
        w.bool(self.nmi_flag);
        w.bool(self.nmi_line_prev);
        w.u8(self.poll as u8);
        w.u8(self.poll_prev as u8);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.a = r.u8()?;
        self.x = r.u8()?;
        self.y = r.u8()?;
        self.sp = r.u8()?;
        self.pc = r.u16()?;
        self.p = r.u8()?;
        self.cycles = r.u64()?;
        self.halted = r.bool()?;
        self.nmi_flag = r.bool()?;
        self.nmi_line_prev = r.bool()?;
        self.poll = decode_pending(r.u8()?)?;
        self.poll_prev = decode_pending(r.u8()?)?;
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
            p: CARRY | NEGATIVE | UNUSED,
            cycles: 1_234_567,
            halted: false,
            nmi_flag: true,
            nmi_line_prev: false,
            poll: Pending::Nmi,
            poll_prev: Pending::None,
        };
        let mut w = WriteCursor::new();
        cpu.save(&mut w);
        let bytes = w.into_bytes();

        let mut back = Cpu::default();
        let mut r = ReadCursor::new(&bytes);
        back.load(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(cpu, back);

        let mut w2 = WriteCursor::new();
        back.save(&mut w2);
        assert_eq!(bytes, w2.into_bytes());
    }

    /// Flat 64 KiB test bus (one access = one cycle, no logging needed here).
    struct FlatBus {
        mem: [u8; 0x10000],
    }
    impl CpuBus for FlatBus {
        fn read(&mut self, addr: u16) -> u8 {
            self.mem[addr as usize]
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.mem[addr as usize] = val;
        }
    }

    fn run_one(prog: &[u8], setup: impl FnOnce(&mut Cpu)) -> (Cpu, FlatBus, u64) {
        let mut bus = FlatBus { mem: [0; 0x10000] };
        for (i, b) in prog.iter().enumerate() {
            bus.mem[0x0200 + i] = *b;
        }
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        setup(&mut cpu);
        let cycles = cpu.step(&mut bus);
        (cpu, bus, cycles)
    }

    #[test]
    fn lda_immediate_sets_flags_and_takes_two_cycles() {
        let (cpu, _, cyc) = run_one(&[0xa9, 0x00], |_| {}); // LDA #$00
        assert_eq!(cpu.a, 0x00);
        assert!(cpu.get_flag(ZERO));
        assert!(!cpu.get_flag(NEGATIVE));
        assert_eq!(cyc, 2);
    }

    #[test]
    fn adc_overflow_and_carry() {
        // 0x50 + 0x50 = 0xA0: signed overflow set, carry clear, negative set.
        let (cpu, _, _) = run_one(&[0x69, 0x50], |c| {
            c.a = 0x50;
            c.set_flag(CARRY, false);
        });
        assert_eq!(cpu.a, 0xa0);
        assert!(cpu.get_flag(OVERFLOW));
        assert!(!cpu.get_flag(CARRY));
        assert!(cpu.get_flag(NEGATIVE));
    }

    #[test]
    fn absolute_x_read_page_cross_costs_extra_cycle() {
        // LDA $12F0,X with X=0x20 crosses into $1310 -> 5 cycles.
        let (cpu, _, cyc) = run_one(&[0xbd, 0xf0, 0x12], |c| c.x = 0x20);
        let _ = cpu;
        assert_eq!(cyc, 5);
        // No cross: X=0x05 -> 4 cycles.
        let (_, _, cyc2) = run_one(&[0xbd, 0xf0, 0x12], |c| c.x = 0x05);
        assert_eq!(cyc2, 4);
    }

    #[test]
    fn inc_zeropage_is_rmw_five_cycles_with_double_write() {
        let (_, bus, cyc) = run_one(&[0xe6, 0x40], |_| {});
        assert_eq!(cyc, 5);
        assert_eq!(bus.mem[0x40], 0x01);
    }

    #[test]
    fn jmp_indirect_page_boundary_bug() {
        // JMP ($02FF): PCL from $02FF, PCH from $0200 (NOT $0300).
        let mut bus = FlatBus { mem: [0; 0x10000] };
        bus.mem[0x0200] = 0x6c;
        bus.mem[0x0201] = 0xff;
        bus.mem[0x0202] = 0x02;
        bus.mem[0x02ff] = 0x34; // low byte of target
        bus.mem[0x0200_usize] = 0x6c; // (keep opcode; $0200 also holds PCH source)
        // Put the PCH-source at $0200 = 0x12 would collide with opcode; use $0300
        // to prove the bug by making $0200 the buggy source:
        bus.mem[0x0300] = 0x99; // the *correct* (non-buggy) high source
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        cpu.step(&mut bus);
        // High byte came from $0200 (0x6c), not $0300 (0x99).
        assert_eq!(cpu.pc, ((bus.mem[0x0200] as u16) << 8) | 0x34);
    }

    #[test]
    fn brk_pushes_b_set_and_jumps_through_vector() {
        let mut bus = FlatBus { mem: [0; 0x10000] };
        bus.mem[0x0200] = 0x00; // BRK
        bus.mem[0xfffe] = 0x00;
        bus.mem[0xffff] = 0x90; // IRQ/BRK vector -> $9000
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        cpu.sp = 0xfd;
        let cyc = cpu.step(&mut bus);
        assert_eq!(cyc, 7);
        assert_eq!(cpu.pc, 0x9000);
        // Pushed P (at $0100+0xFB) has B (bit4) and unused (bit5) set.
        let pushed = bus.mem[0x01fb];
        assert_eq!(pushed & 0x30, 0x30);
        assert!(cpu.get_flag(IRQ_DISABLE));
    }
}
