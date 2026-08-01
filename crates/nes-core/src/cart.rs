//! Cartridge image parsing (iNES / NES 2.0) and the mapper boundary.
//!
//! This decodes the 16-byte header that fronts every `.nes` file into the
//! facts the bus needs: PRG/CHR sizes, mirroring, mapper number, and whether
//! the cart has battery-backed save RAM. Actual bank-switching behaviour lives
//! behind the [`Mapper`] trait; only NROM (mapper 0) is wired initially, enough
//! to boot the simplest carts and the CPU test harnesses.

use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

/// Nametable mirroring. Header carts declare Horizontal/Vertical/FourScreen;
/// mapper-controlled mirroring (MMC1 etc.) can also select a single-screen bank
/// at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    /// Cart supplies 4 KiB of its own VRAM; both logical tables are distinct.
    FourScreen,
    /// All four nametables map to CIRAM bank A.
    SingleScreenA,
    /// All four nametables map to CIRAM bank B.
    SingleScreenB,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartError {
    BadMagic,
    Truncated,
    UnsupportedMapper(u16),
}

/// A parsed cartridge: decoded header plus the raw PRG/CHR banks.
pub struct Cartridge {
    pub mapper: u16,
    pub mirroring: Mirroring,
    pub has_battery: bool,
    pub prg_rom: Vec<u8>,
    /// CHR ROM; empty when the cart uses CHR RAM instead.
    pub chr_rom: Vec<u8>,
    pub chr_is_ram: bool,
    /// 8 KiB of battery/work RAM at $6000-$7FFF for carts that have it.
    pub prg_ram: Vec<u8>,
}

const HEADER_LEN: usize = 16;
const PRG_BANK: usize = 16 * 1024;
const CHR_BANK: usize = 8 * 1024;
const TRAINER_LEN: usize = 512;

impl Cartridge {
    /// Parse a full `.nes` image. Supports iNES; NES 2.0 headers are accepted
    /// and their extended size fields honoured (the format is a superset).
    pub fn from_ines(rom: &[u8]) -> Result<Self, CartError> {
        if rom.len() < HEADER_LEN || &rom[0..4] != b"NES\x1a" {
            return Err(CartError::BadMagic);
        }
        let flags6 = rom[6];
        let flags7 = rom[7];
        let is_nes2 = (flags7 & 0x0c) == 0x08;

        let mut prg_banks = rom[4] as usize;
        let mut chr_banks = rom[5] as usize;
        let mut mapper = ((flags7 & 0xf0) | (flags6 >> 4)) as u16;
        if is_nes2 {
            // NES 2.0 widens mapper to 12 bits and PRG/CHR sizes to 12 bits.
            mapper |= ((rom[8] as u16) & 0x0f) << 8;
            prg_banks |= ((rom[9] as usize) & 0x0f) << 8;
            chr_banks |= ((rom[9] as usize) & 0xf0) << 4;
        }

        let mirroring = if flags6 & 0x08 != 0 {
            Mirroring::FourScreen
        } else if flags6 & 0x01 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
        let has_battery = flags6 & 0x02 != 0;
        let has_trainer = flags6 & 0x04 != 0;

        let mut off = HEADER_LEN;
        if has_trainer {
            off += TRAINER_LEN;
        }

        let prg_len = prg_banks * PRG_BANK;
        let prg_end = off + prg_len;
        if prg_end > rom.len() {
            return Err(CartError::Truncated);
        }
        let prg_rom = rom[off..prg_end].to_vec();

        let chr_len = chr_banks * CHR_BANK;
        let chr_end = prg_end + chr_len;
        if chr_end > rom.len() {
            return Err(CartError::Truncated);
        }
        let (chr_rom, chr_is_ram) = if chr_banks == 0 {
            // No CHR ROM -> the cart wires 8 KiB of CHR RAM instead.
            (vec![0u8; CHR_BANK], true)
        } else {
            (rom[prg_end..chr_end].to_vec(), false)
        };

        Ok(Cartridge {
            mapper,
            mirroring,
            has_battery,
            prg_rom,
            chr_rom,
            chr_is_ram,
            prg_ram: vec![0u8; 8 * 1024],
        })
    }
}

/// The mapper boundary. Every cartridge chip family (NROM, MMC1, MMC3, ...)
/// implements this so the bus stays mapper-agnostic. CPU-visible reads/writes
/// in $4020-$FFFF and PPU-visible reads/writes in $0000-$1FFF route here.
///
/// Mappers carry mutable bank/latch state, so they participate in save states.
pub trait Mapper: SaveState {
    fn cpu_read(&mut self, addr: u16) -> u8;
    fn cpu_write(&mut self, addr: u16, val: u8);
    fn ppu_read(&mut self, addr: u16) -> u8;
    fn ppu_write(&mut self, addr: u16, val: u8);
    fn mirroring(&self) -> Mirroring;
    /// Level of the mapper's IRQ line (MMC3 scanline counter etc.). Default low.
    fn irq(&self) -> bool {
        false
    }
    /// Called once per CPU cycle, for mappers with a CPU-cycle IRQ counter
    /// (Irem H3001, Sunsoft FME-7, ...). Default no-op.
    fn tick_cpu(&mut self) {}
}

/// Mapper 0: fixed PRG (16 or 32 KiB), fixed CHR. The bring-up mapper.
pub struct Nrom {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_is_ram: bool,
    mirroring: Mirroring,
}

impl Nrom {
    pub fn new(cart: Cartridge) -> Self {
        Nrom {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            chr_is_ram: cart.chr_is_ram,
            mirroring: cart.mirroring,
        }
    }
}

impl Mapper for Nrom {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xffff => {
                // 16 KiB carts mirror the single bank into both halves.
                let idx = (addr as usize - 0x8000) & (self.prg.len() - 1);
                self.prg[idx]
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, val: u8) {
        if let 0x6000..=0x7fff = addr {
            let n = self.prg_ram.len();
            self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr[(addr as usize) & (self.chr.len() - 1)]
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let n = self.chr.len();
            self.chr[(addr as usize) & (n - 1)] = val;
        }
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}

impl SaveState for Nrom {
    fn save(&self, w: &mut WriteCursor) {
        // Only the mutable state travels: PRG-RAM and (if present) CHR-RAM.
        // ROM banks are reconstructed from the cart image on load, never saved.
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        Ok(())
    }
}

/// Mapper 1: MMC1 (SxROM). A 5-bit serial shift register loaded one bit per
/// write drives four internal registers (control, CHR bank 0/1, PRG bank) that
/// select PRG/CHR banks and runtime nametable mirroring.
pub struct Mmc1 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_is_ram: bool,
    prg_banks: usize, // count of 16 KiB PRG banks

    shift: u8,
    control: u8, // bits0-1 mirroring, 2-3 PRG mode, 4 CHR mode
    chr0: u8,
    chr1: u8,
    prg_bank: u8,
}

impl Mmc1 {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks = (cart.prg_rom.len() / PRG_BANK).max(1);
        Mmc1 {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            chr_is_ram: cart.chr_is_ram,
            prg_banks,
            shift: 0x10,       // sentinel bit marks the 5th write
            control: 0x0c,     // power-on: PRG mode 3 (fix last bank at $C000)
            chr0: 0,
            chr1: 0,
            prg_bank: 0,
        }
    }

    /// Map a CPU address in $8000-$FFFF to a byte offset in PRG ROM.
    fn prg_offset(&self, addr: u16) -> usize {
        let last = self.prg_banks - 1;
        let bank16 = |n: usize| (n % self.prg_banks) * PRG_BANK;
        let off = (addr as usize) & 0x3fff;
        match (self.control >> 2) & 0x3 {
            0 | 1 => {
                // 32 KiB switch (ignore low bit of the bank number).
                let base = ((self.prg_bank as usize & 0x0e)) * PRG_BANK;
                (base + (addr as usize - 0x8000)) % (self.prg_banks * PRG_BANK)
            }
            2 => {
                // Fix first bank at $8000, switch 16 KiB at $C000.
                if addr < 0xc000 {
                    bank16(0) + off
                } else {
                    bank16(self.prg_bank as usize & 0x0f) + off
                }
            }
            _ => {
                // Fix last bank at $C000, switch 16 KiB at $8000.
                if addr < 0xc000 {
                    bank16(self.prg_bank as usize & 0x0f) + off
                } else {
                    bank16(last) + off
                }
            }
        }
    }

    /// Map a PPU address in $0000-$1FFF to a byte offset in CHR.
    fn chr_offset(&self, addr: u16) -> usize {
        let a = addr as usize & 0x1fff;
        let n = self.chr.len().max(1);
        if self.control & 0x10 == 0 {
            // 8 KiB switch (ignore low bit of chr0).
            ((self.chr0 as usize & 0x1e) * 0x1000 + a) % n
        } else {
            // Two 4 KiB banks.
            if a < 0x1000 {
                ((self.chr0 as usize) * 0x1000 + a) % n
            } else {
                ((self.chr1 as usize) * 0x1000 + (a - 0x1000)) % n
            }
        }
    }

    fn write_serial(&mut self, addr: u16, val: u8) {
        if val & 0x80 != 0 {
            // Reset: clear the shift register and force PRG mode 3.
            self.shift = 0x10;
            self.control |= 0x0c;
            return;
        }
        let complete = self.shift & 1 != 0; // sentinel reached bit 0 -> 5th write
        self.shift = (self.shift >> 1) | ((val & 1) << 4);
        if complete {
            let data = self.shift & 0x1f;
            match (addr >> 13) & 3 {
                0 => self.control = data,
                1 => self.chr0 = data,
                2 => self.chr1 = data,
                _ => self.prg_bank = data,
            }
            self.shift = 0x10;
        }
    }
}

impl Mapper for Mmc1 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xffff => {
                let off = self.prg_offset(addr);
                self.prg[off]
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x6000..=0x7fff => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
            }
            0x8000..=0xffff => self.write_serial(addr, val),
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        let off = self.chr_offset(addr);
        self.chr[off]
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let off = self.chr_offset(addr);
            self.chr[off] = val;
        }
    }

    fn mirroring(&self) -> Mirroring {
        match self.control & 0x03 {
            0 => Mirroring::SingleScreenA,
            1 => Mirroring::SingleScreenB,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
        }
    }
}

impl SaveState for Mmc1 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.u8(self.shift);
        w.u8(self.control);
        w.u8(self.chr0);
        w.u8(self.chr1);
        w.u8(self.prg_bank);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        self.shift = r.u8()?;
        self.control = r.u8()?;
        self.chr0 = r.u8()?;
        self.chr1 = r.u8()?;
        self.prg_bank = r.u8()?;
        Ok(())
    }
}

/// Mapper 2: UxROM. One switchable 16 KiB PRG bank at $8000, the last bank fixed
/// at $C000; 8 KiB CHR RAM; fixed header mirroring.
pub struct Uxrom {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_banks: usize,
    bank: u8,
    mirroring: Mirroring,
}
impl Uxrom {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks = (cart.prg_rom.len() / PRG_BANK).max(1);
        Uxrom {
            prg: cart.prg_rom,
            chr: cart.chr_rom, // CHR RAM (8 KiB) for mapper 2
            prg_banks,
            bank: 0,
            mirroring: cart.mirroring,
        }
    }
}
impl Mapper for Uxrom {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xbfff => self.prg[(self.bank as usize % self.prg_banks) * PRG_BANK + (addr as usize - 0x8000)],
            0xc000..=0xffff => self.prg[(self.prg_banks - 1) * PRG_BANK + (addr as usize - 0xc000)],
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        if addr >= 0x8000 {
            self.bank = val & 0x0f;
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr[addr as usize & (self.chr.len() - 1)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        let n = self.chr.len();
        self.chr[addr as usize & (n - 1)] = val;
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Uxrom {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.chr);
        w.u8(self.bank);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.chr)?;
        self.bank = r.u8()?;
        Ok(())
    }
}

/// Mapper 3: CNROM. Fixed PRG (NROM-style), one switchable 8 KiB CHR bank.
pub struct Cnrom {
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_bank: u8,
    mirroring: Mirroring,
}
impl Cnrom {
    pub fn new(cart: Cartridge) -> Self {
        Cnrom {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            chr_bank: 0,
            mirroring: cart.mirroring,
        }
    }
}
impl Mapper for Cnrom {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xffff => self.prg[(addr as usize - 0x8000) & (self.prg.len() - 1)],
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        if addr >= 0x8000 {
            self.chr_bank = val;
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let base = (self.chr_bank as usize) * CHR_BANK;
        self.chr[(base + addr as usize) % self.chr.len()]
    }
    fn ppu_write(&mut self, _addr: u16, _val: u8) {}
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Cnrom {
    fn save(&self, w: &mut WriteCursor) {
        w.u8(self.chr_bank);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.chr_bank = r.u8()?;
        Ok(())
    }
}

/// Mapper 7: AxROM. One switchable 32 KiB PRG bank; 8 KiB CHR RAM; single-screen
/// mirroring selected by the bank register's bit 4.
pub struct Axrom {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_banks32: usize,
    bank: u8,
    mirroring: Mirroring,
}
impl Axrom {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks32 = (cart.prg_rom.len() / (32 * 1024)).max(1);
        Axrom {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_banks32,
            bank: 0,
            mirroring: Mirroring::SingleScreenA,
        }
    }
}
impl Mapper for Axrom {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xffff => {
                let base = (self.bank as usize % self.prg_banks32) * 32 * 1024;
                self.prg[base + (addr as usize - 0x8000)]
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        if addr >= 0x8000 {
            self.bank = val & 0x07;
            self.mirroring = if val & 0x10 != 0 {
                Mirroring::SingleScreenB
            } else {
                Mirroring::SingleScreenA
            };
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr[addr as usize & (self.chr.len() - 1)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        let n = self.chr.len();
        self.chr[addr as usize & (n - 1)] = val;
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Axrom {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.chr);
        w.u8(self.bank);
        w.u8(match self.mirroring {
            Mirroring::SingleScreenB => 1,
            _ => 0,
        });
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.chr)?;
        self.bank = r.u8()?;
        self.mirroring = if r.u8()? != 0 {
            Mirroring::SingleScreenB
        } else {
            Mirroring::SingleScreenA
        };
        Ok(())
    }
}

/// Mapper 4: MMC3 (TxROM). Two switchable 8 KiB PRG banks + two fixed; CHR as
/// 2x2 KiB + 4x1 KiB banks; runtime H/V mirroring; and the scanline IRQ counter
/// clocked by PPU A12 rising edges (SMB3 etc. split their HUD with it).
pub struct Mmc3 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_is_ram: bool,
    prg_banks8: usize, // count of 8 KiB PRG banks
    chr_banks1: usize, // count of 1 KiB CHR banks

    bank_select: u8, // $8000
    regs: [u8; 8],   // R0..R7 bank values
    mirroring: Mirroring,

    // scanline IRQ
    irq_latch: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enable: bool,
    irq_flag: bool,
    prev_a12: bool,
}
impl Mmc3 {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks8 = (cart.prg_rom.len() / (8 * 1024)).max(1);
        let chr_banks1 = (cart.chr_rom.len() / 1024).max(1);
        Mmc3 {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            chr_is_ram: cart.chr_is_ram,
            prg_banks8,
            chr_banks1,
            bank_select: 0,
            regs: [0; 8],
            mirroring: cart.mirroring,
            irq_latch: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enable: false,
            irq_flag: false,
            prev_a12: false,
        }
    }

    fn prg_offset(&self, addr: u16) -> usize {
        let last = self.prg_banks8 - 1;
        let mode = self.bank_select & 0x40 != 0; // PRG mode
        let bank = |n: usize| (n % self.prg_banks8) * 0x2000;
        let region = (addr as usize - 0x8000) / 0x2000; // 0..3 (8 KiB windows)
        let off = addr as usize & 0x1fff;
        let b = match (region, mode) {
            (0, false) => self.regs[6] as usize, // $8000
            (0, true) => last - 1,               // fixed second-to-last
            (1, _) => self.regs[7] as usize,     // $A000 = R7
            (2, false) => last - 1,              // $C000 fixed second-to-last
            (2, true) => self.regs[6] as usize,  // $C000 = R6 in mode 1
            _ => last,                           // $E000 fixed last
        };
        bank(b) + off
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let a = addr as usize & 0x1fff;
        let inv = self.bank_select & 0x80 != 0; // CHR A12 inversion
        let region = a / 0x400; // 0..7 (1 KiB windows)
        let r = if inv { region ^ 4 } else { region };
        // R0/R1 are 2 KiB banks (low bit ignored); R2..R5 are 1 KiB.
        let bank1 = match r {
            0 => (self.regs[0] & 0xfe) as usize,
            1 => (self.regs[0] & 0xfe) as usize + 1,
            2 => (self.regs[1] & 0xfe) as usize,
            3 => (self.regs[1] & 0xfe) as usize + 1,
            4 => self.regs[2] as usize,
            5 => self.regs[3] as usize,
            6 => self.regs[4] as usize,
            _ => self.regs[5] as usize,
        };
        (bank1 % self.chr_banks1) * 0x400 + (a & 0x3ff)
    }

    /// Clock the IRQ counter on a filtered A12 rising edge.
    fn clock_irq(&mut self) {
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_latch;
            self.irq_reload = false;
        } else {
            self.irq_counter -= 1;
        }
        if self.irq_counter == 0 && self.irq_enable {
            self.irq_flag = true;
        }
    }
}
impl Mapper for Mmc3 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xffff => {
                let off = self.prg_offset(addr);
                self.prg[off]
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x6000..=0x7fff => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
            }
            0x8000..=0x9fff => {
                if addr & 1 == 0 {
                    self.bank_select = val;
                } else {
                    self.regs[(self.bank_select & 0x07) as usize] = val;
                }
            }
            0xa000..=0xbfff => {
                if addr & 1 == 0 {
                    self.mirroring = if val & 1 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    };
                }
                // odd: PRG-RAM protect (ignored)
            }
            0xc000..=0xdfff => {
                if addr & 1 == 0 {
                    self.irq_latch = val;
                } else {
                    self.irq_reload = true;
                    self.irq_counter = 0;
                }
            }
            0xe000..=0xffff => {
                if addr & 1 == 0 {
                    self.irq_enable = false;
                    self.irq_flag = false;
                } else {
                    self.irq_enable = true;
                }
            }
            _ => {}
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        // Detect A12 rising edge (filtered by tracking the previous level) to
        // clock the scanline IRQ counter.
        let a12 = addr & 0x1000 != 0;
        if a12 && !self.prev_a12 {
            self.clock_irq();
        }
        self.prev_a12 = a12;
        let off = self.chr_offset(addr);
        self.chr[off % self.chr.len()]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        let a12 = addr & 0x1000 != 0;
        if a12 && !self.prev_a12 {
            self.clock_irq();
        }
        self.prev_a12 = a12;
        if self.chr_is_ram {
            let off = self.chr_offset(addr) % self.chr.len();
            self.chr[off] = val;
        }
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    fn irq(&self) -> bool {
        self.irq_flag
    }
}
impl SaveState for Mmc3 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.u8(self.bank_select);
        w.bytes(&self.regs);
        w.u8(match self.mirroring {
            Mirroring::Horizontal => 0,
            _ => 1,
        });
        w.u8(self.irq_latch);
        w.u8(self.irq_counter);
        w.bool(self.irq_reload);
        w.bool(self.irq_enable);
        w.bool(self.irq_flag);
        w.bool(self.prev_a12);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        self.bank_select = r.u8()?;
        r.bytes(&mut self.regs)?;
        self.mirroring = if r.u8()? == 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        self.irq_latch = r.u8()?;
        self.irq_counter = r.u8()?;
        self.irq_reload = r.bool()?;
        self.irq_enable = r.bool()?;
        self.irq_flag = r.bool()?;
        self.prev_a12 = r.bool()?;
        Ok(())
    }
}

/// A "bank-swap" mapper covering several simple discrete-logic families that
/// differ only in which register bits pick the 32 KiB PRG bank and 8 KiB CHR
/// bank: GxROM (66), Color Dreams (11), BNROM (34). One register at $8000-$FFFF.
pub struct BankSwap {
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_banks32: usize,
    chr_banks8: usize,
    prg_bank: usize,
    chr_bank: usize,
    mirroring: Mirroring,
    /// How to decode a $8000-$FFFF write into (prg32, chr8).
    kind: BankSwapKind,
}
#[derive(Clone, Copy)]
pub enum BankSwapKind {
    Gxrom,       // PRG=(v>>4)&3, CHR=v&3
    ColorDreams, // PRG=v&3, CHR=(v>>4)&0xF
    Bnrom,       // PRG=v (32K), CHR RAM
}
impl BankSwap {
    pub fn new(cart: Cartridge, kind: BankSwapKind) -> Self {
        BankSwap {
            prg_banks32: (cart.prg_rom.len() / (32 * 1024)).max(1),
            chr_banks8: (cart.chr_rom.len() / CHR_BANK).max(1),
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            chr_is_ram: cart.chr_is_ram,
            prg_bank: 0,
            chr_bank: 0,
            mirroring: cart.mirroring,
            kind,
        }
    }
}
impl Mapper for BankSwap {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xffff => {
                let base = (self.prg_bank % self.prg_banks32) * 32 * 1024;
                self.prg[base + (addr as usize - 0x8000)]
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        if addr >= 0x8000 {
            let (p, c) = match self.kind {
                BankSwapKind::Gxrom => (((val >> 4) & 3) as usize, (val & 3) as usize),
                BankSwapKind::ColorDreams => ((val & 3) as usize, ((val >> 4) & 0x0f) as usize),
                BankSwapKind::Bnrom => (val as usize, 0),
            };
            self.prg_bank = p;
            self.chr_bank = c;
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let base = (self.chr_bank % self.chr_banks8) * CHR_BANK;
        self.chr[(base + addr as usize) % self.chr.len()]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let n = self.chr.len();
            self.chr[addr as usize & (n - 1)] = val;
        }
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for BankSwap {
    fn save(&self, w: &mut WriteCursor) {
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.u32(self.prg_bank as u32);
        w.u32(self.chr_bank as u32);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        self.prg_bank = r.u32()? as usize;
        self.chr_bank = r.u32()? as usize;
        Ok(())
    }
}

/// Mapper 71 (Camerica/Codemasters): UxROM-like — a switchable 16 KiB PRG bank
/// at $8000 selected by writes to $C000-$FFFF, fixed last bank at $C000, CHR RAM.
pub struct Camerica {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_banks: usize,
    bank: u8,
    mirroring: Mirroring,
}
impl Camerica {
    pub fn new(cart: Cartridge) -> Self {
        Camerica {
            prg_banks: (cart.prg_rom.len() / PRG_BANK).max(1),
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            bank: 0,
            mirroring: cart.mirroring,
        }
    }
}
impl Mapper for Camerica {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xbfff => self.prg[(self.bank as usize % self.prg_banks) * PRG_BANK + (addr as usize - 0x8000)],
            0xc000..=0xffff => self.prg[(self.prg_banks - 1) * PRG_BANK + (addr as usize - 0xc000)],
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        // BF9093/BF9097: the 16 KiB PRG bank register is at $C000-$FFFF.
        if addr >= 0xc000 {
            self.bank = val & 0x0f;
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr[addr as usize & (self.chr.len() - 1)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        let n = self.chr.len();
        self.chr[addr as usize & (n - 1)] = val;
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Camerica {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.chr);
        w.u8(self.bank);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.chr)?;
        self.bank = r.u8()?;
        Ok(())
    }
}

/// Mapper 79 (NINA-03/06, AVE): fixed 32 KiB PRG bank + 8 KiB CHR bank, selected
/// by a write to $4100-$5FFF (bit3 = PRG 32K, bits0-2 = CHR 8K).
pub struct Nina03 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_banks32: usize,
    chr_banks8: usize,
    prg_bank: usize,
    chr_bank: usize,
    mirroring: Mirroring,
}
impl Nina03 {
    pub fn new(cart: Cartridge) -> Self {
        Nina03 {
            prg_banks32: (cart.prg_rom.len() / (32 * 1024)).max(1),
            chr_banks8: (cart.chr_rom.len() / CHR_BANK).max(1),
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_bank: 0,
            chr_bank: 0,
            mirroring: cart.mirroring,
        }
    }
}
impl Mapper for Nina03 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xffff => {
                let base = (self.prg_bank % self.prg_banks32) * 32 * 1024;
                self.prg[base + (addr as usize - 0x8000)]
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        if (0x4100..=0x5fff).contains(&addr) {
            self.prg_bank = ((val >> 3) & 1) as usize;
            self.chr_bank = (val & 0x07) as usize;
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let base = (self.chr_bank % self.chr_banks8) * CHR_BANK;
        self.chr[(base + addr as usize) % self.chr.len()]
    }
    fn ppu_write(&mut self, _addr: u16, _val: u8) {}
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Nina03 {
    fn save(&self, w: &mut WriteCursor) {
        w.u32(self.prg_bank as u32);
        w.u32(self.chr_bank as u32);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.prg_bank = r.u32()? as usize;
        self.chr_bank = r.u32()? as usize;
        Ok(())
    }
}

/// Mappers 9 (MMC2, Punch-Out) and 10 (MMC4, Fire Emblem). Both use a pair of
/// CHR "latches" that flip when the PPU fetches tile $FD vs $FE, selecting which
/// 4 KiB CHR bank shows. MMC2 switches 8 KiB PRG at $8000 (three fixed banks
/// after); MMC4 switches 16 KiB PRG at $8000 (last 16 KiB fixed).
pub struct Mmc2 {
    is_mmc4: bool,
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    prg_banks: usize, // 8K (mmc2) or 16K (mmc4) window count is derived on read
    prg_bank: u8,
    chr_banks: [u8; 4], // [$0000/FD, $0000/FE, $1000/FD, $1000/FE] (4 KiB each)
    latch0: bool,       // false=FD, true=FE for $0000
    latch1: bool,       // for $1000
    mirroring: Mirroring,
}
impl Mmc2 {
    pub fn new(cart: Cartridge, is_mmc4: bool) -> Self {
        let unit = if is_mmc4 { 16 * 1024 } else { 8 * 1024 };
        Mmc2 {
            is_mmc4,
            prg_banks: (cart.prg_rom.len() / unit).max(1),
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            prg_bank: 0,
            chr_banks: [0; 4],
            latch0: true,
            latch1: true,
            mirroring: cart.mirroring,
        }
    }
    fn chr4(&self, sel: usize, off: usize) -> u8 {
        let bank = self.chr_banks[sel] as usize;
        self.chr[(bank * 0x1000 + off) % self.chr.len().max(1)]
    }
}
impl Mapper for Mmc2 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xffff => {
                if self.is_mmc4 {
                    // 16K switchable at $8000, fixed last 16K at $C000.
                    if addr < 0xc000 {
                        self.prg[(self.prg_bank as usize % self.prg_banks) * 0x4000 + (addr as usize - 0x8000)]
                    } else {
                        self.prg[(self.prg_banks - 1) * 0x4000 + (addr as usize - 0xc000)]
                    }
                } else {
                    // 8K switchable at $8000, last three 8K fixed.
                    let region = (addr as usize - 0x8000) / 0x2000;
                    let off = addr as usize & 0x1fff;
                    // $8000 switchable; $A000/$C000/$E000 = last three 8K banks.
                    let bank = match region {
                        0 => self.prg_bank as usize & 0x0f,
                        1 => self.prg_banks - 3,
                        2 => self.prg_banks - 2,
                        _ => self.prg_banks - 1,
                    };
                    self.prg[(bank % self.prg_banks) * 0x2000 + off]
                }
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x6000..=0x7fff => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
            }
            0xa000..=0xafff => self.prg_bank = val & 0x0f,
            0xb000..=0xbfff => self.chr_banks[0] = val & 0x1f, // $0000 FD
            0xc000..=0xcfff => self.chr_banks[1] = val & 0x1f, // $0000 FE
            0xd000..=0xdfff => self.chr_banks[2] = val & 0x1f, // $1000 FD
            0xe000..=0xefff => self.chr_banks[3] = val & 0x1f, // $1000 FE
            0xf000..=0xffff => {
                self.mirroring = if val & 1 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            _ => {}
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let a = addr as usize & 0x1fff;
        let val = if a < 0x1000 {
            self.chr4(if self.latch0 { 1 } else { 0 }, a)
        } else {
            self.chr4(if self.latch1 { 3 } else { 2 }, a - 0x1000)
        };
        // Update the latches AFTER the fetch (tile $FD -> FD latch, $FE -> FE).
        match addr & 0x1ff8 {
            0x0fd8 => self.latch0 = false,
            0x0fe8 => self.latch0 = true,
            0x1fd8 => self.latch1 = false,
            0x1fe8 => self.latch1 = true,
            _ => {}
        }
        val
    }
    fn ppu_write(&mut self, _addr: u16, _val: u8) {}
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Mmc2 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        w.u8(self.prg_bank);
        w.bytes(&self.chr_banks);
        w.bool(self.latch0);
        w.bool(self.latch1);
        w.u8(match self.mirroring {
            Mirroring::Horizontal => 0,
            _ => 1,
        });
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        self.prg_bank = r.u8()?;
        r.bytes(&mut self.chr_banks)?;
        self.latch0 = r.bool()?;
        self.latch1 = r.bool()?;
        self.mirroring = if r.u8()? == 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        Ok(())
    }
}

/// Mapper 65 (Irem H3001): three switchable 8 KiB PRG banks + fixed last, eight
/// 1 KiB CHR banks, and a 16-bit CPU-cycle IRQ counter.
#[allow(dead_code)]
pub struct H3001 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_is_ram: bool,
    prg_banks8: usize,
    chr_banks1: usize,
    prg_regs: [u8; 3], // banks at $8000, $A000, $C000
    chr_regs: [u8; 8],
    mirroring: Mirroring,
    irq_counter: u16,
    irq_latch: u16,
    irq_enable: bool,
    irq_flag: bool,
}
impl H3001 {
    pub fn new(cart: Cartridge) -> Self {
        H3001 {
            prg_banks8: (cart.prg_rom.len() / (8 * 1024)).max(1),
            chr_banks1: (cart.chr_rom.len() / 1024).max(1),
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            chr_is_ram: cart.chr_is_ram,
            prg_regs: [0; 3],
            chr_regs: [0; 8],
            mirroring: cart.mirroring,
            irq_counter: 0,
            irq_latch: 0,
            irq_enable: false,
            irq_flag: false,
        }
    }
}
impl Mapper for H3001 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xffff => {
                let region = (addr as usize - 0x8000) / 0x2000; // 0..3
                let bank = match region {
                    0 => self.prg_regs[0] as usize,
                    1 => self.prg_regs[1] as usize,
                    2 => self.prg_regs[2] as usize,
                    _ => self.prg_banks8 - 1, // fixed last
                };
                self.prg[(bank % self.prg_banks8) * 0x2000 + (addr as usize & 0x1fff)]
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x6000..=0x7fff => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
            }
            0x8000..=0x8fff => self.prg_regs[0] = val,
            0xa000..=0xafff => self.prg_regs[1] = val,
            0xc000..=0xcfff => self.prg_regs[2] = val,
            0x9001 => {
                self.mirroring = if val & 0x80 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            0x9003 => {
                self.irq_enable = val & 0x80 != 0;
                self.irq_flag = false;
            }
            0x9004 => {
                self.irq_counter = self.irq_latch;
                self.irq_flag = false;
            }
            0x9005 => self.irq_latch = (self.irq_latch & 0x00ff) | ((val as u16) << 8),
            0x9006 => self.irq_latch = (self.irq_latch & 0xff00) | val as u16,
            0xb000..=0xb007 => self.chr_regs[(addr & 0x0007) as usize] = val,
            _ => {}
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let slot = (addr as usize & 0x1fff) / 0x400; // 0..7
        let bank = self.chr_regs[slot] as usize;
        self.chr[(bank % self.chr_banks1) * 0x400 + (addr as usize & 0x3ff)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let slot = (addr as usize & 0x1fff) / 0x400;
            let bank = self.chr_regs[slot] as usize;
            let i = ((bank % self.chr_banks1) * 0x400 + (addr as usize & 0x3ff)) % self.chr.len();
            self.chr[i] = val;
        }
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    fn irq(&self) -> bool {
        self.irq_flag
    }
    fn tick_cpu(&mut self) {
        if self.irq_enable && self.irq_counter > 0 {
            self.irq_counter -= 1;
            if self.irq_counter == 0 {
                self.irq_flag = true;
            }
        }
    }
}
impl SaveState for H3001 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.bytes(&self.prg_regs);
        w.bytes(&self.chr_regs);
        w.u8(match self.mirroring {
            Mirroring::Horizontal => 0,
            _ => 1,
        });
        w.u16(self.irq_counter);
        w.u16(self.irq_latch);
        w.bool(self.irq_enable);
        w.bool(self.irq_flag);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        r.bytes(&mut self.prg_regs)?;
        r.bytes(&mut self.chr_regs)?;
        self.mirroring = if r.u8()? == 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        self.irq_counter = r.u16()?;
        self.irq_latch = r.u16()?;
        self.irq_enable = r.bool()?;
        self.irq_flag = r.bool()?;
        Ok(())
    }
}

/// Mapper 34 variant NINA-001 (the CHR-ROM form of mapper 34): registers live in
/// PRG-RAM space — $7FFD picks a 32 KiB PRG bank, $7FFE/$7FFF pick two 4 KiB CHR
/// banks. (The CHR-RAM form of mapper 34 is BNROM, handled by BankSwap.)
pub struct Nina001 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    prg_banks32: usize,
    chr_banks4: usize,
    prg_bank: usize,
    chr_bank0: usize,
    chr_bank1: usize,
    mirroring: Mirroring,
}
impl Nina001 {
    pub fn new(cart: Cartridge) -> Self {
        Nina001 {
            prg_banks32: (cart.prg_rom.len() / (32 * 1024)).max(1),
            chr_banks4: (cart.chr_rom.len() / 0x1000).max(1),
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            prg_bank: 0,
            chr_bank0: 0,
            chr_bank1: 1,
            mirroring: cart.mirroring,
        }
    }
}
impl Mapper for Nina001 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xffff => {
                let base = (self.prg_bank % self.prg_banks32) * 32 * 1024;
                self.prg[base + (addr as usize - 0x8000)]
            }
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x7ffd => self.prg_bank = (val & 1) as usize,
            0x7ffe => self.chr_bank0 = (val & 0x0f) as usize,
            0x7fff => self.chr_bank1 = (val & 0x0f) as usize,
            0x6000..=0x7fff => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
            }
            _ => {}
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let (bank, off) = if addr < 0x1000 {
            (self.chr_bank0, addr as usize)
        } else {
            (self.chr_bank1, addr as usize - 0x1000)
        };
        self.chr[((bank % self.chr_banks4) * 0x1000 + off) % self.chr.len()]
    }
    fn ppu_write(&mut self, _addr: u16, _val: u8) {}
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Nina001 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        w.u32(self.prg_bank as u32);
        w.u32(self.chr_bank0 as u32);
        w.u32(self.chr_bank1 as u32);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        self.prg_bank = r.u32()? as usize;
        self.chr_bank0 = r.u32()? as usize;
        self.chr_bank1 = r.u32()? as usize;
        Ok(())
    }
}

/// Construct the mapper implementation for a parsed cart.
pub fn make_mapper(cart: Cartridge) -> Result<Box<dyn Mapper>, CartError> {
    match cart.mapper {
        0 => Ok(Box::new(Nrom::new(cart))),
        1 => Ok(Box::new(Mmc1::new(cart))),
        2 => Ok(Box::new(Uxrom::new(cart))),
        3 => Ok(Box::new(Cnrom::new(cart))),
        4 => Ok(Box::new(Mmc3::new(cart))),
        7 => Ok(Box::new(Axrom::new(cart))),
        9 => Ok(Box::new(Mmc2::new(cart, false))),
        11 => Ok(Box::new(BankSwap::new(cart, BankSwapKind::ColorDreams))),
        // Mapper 34: NINA-001 (CHR ROM) vs BNROM (CHR RAM).
        34 if !cart.chr_is_ram => Ok(Box::new(Nina001::new(cart))),
        34 => Ok(Box::new(BankSwap::new(cart, BankSwapKind::Bnrom))),
        66 => Ok(Box::new(BankSwap::new(cart, BankSwapKind::Gxrom))),
        71 => Ok(Box::new(Camerica::new(cart))),
        79 => Ok(Box::new(Nina03::new(cart))),
        // 65 (Irem H3001) hangs (IRQ) -- deferred to the IRQ-mapper pass.
        other => Err(CartError::UnsupportedMapper(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid iNES image: 1 PRG bank, 1 CHR bank, mapper 0.
    fn synth_ines(flags6: u8) -> Vec<u8> {
        let mut v = vec![0u8; HEADER_LEN];
        v[0..4].copy_from_slice(b"NES\x1a");
        v[4] = 1; // 16 KiB PRG
        v[5] = 1; // 8 KiB CHR
        v[6] = flags6;
        v.extend(std::iter::repeat(0xa5).take(PRG_BANK));
        v.extend(std::iter::repeat(0x5a).take(CHR_BANK));
        v
    }

    #[test]
    fn parses_nrom_header_and_banks() {
        let img = synth_ines(0x01); // vertical mirroring, battery off
        let cart = Cartridge::from_ines(&img).unwrap();
        assert_eq!(cart.mapper, 0);
        assert_eq!(cart.mirroring, Mirroring::Vertical);
        assert!(!cart.has_battery);
        assert_eq!(cart.prg_rom.len(), PRG_BANK);
        assert_eq!(cart.chr_rom.len(), CHR_BANK);
        assert_eq!(cart.prg_rom[0], 0xa5);
        assert_eq!(cart.chr_rom[0], 0x5a);
    }

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(Cartridge::from_ines(b"not a rom really").err(), Some(CartError::BadMagic));
    }

    #[test]
    fn sixteen_k_prg_mirrors_into_both_halves() {
        let img = synth_ines(0x00);
        let cart = Cartridge::from_ines(&img).unwrap();
        let mut m = Nrom::new(cart);
        // $8000 and $C000 alias the same 16 KiB bank.
        assert_eq!(m.cpu_read(0x8000), m.cpu_read(0xc000));
    }
}
