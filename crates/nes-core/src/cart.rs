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

/// Construct the mapper implementation for a parsed cart.
pub fn make_mapper(cart: Cartridge) -> Result<Box<dyn Mapper>, CartError> {
    match cart.mapper {
        0 => Ok(Box::new(Nrom::new(cart))),
        1 => Ok(Box::new(Mmc1::new(cart))),
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
