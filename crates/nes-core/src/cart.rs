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

/// Serialize a [`Mirroring`] to a stable byte (for mappers with runtime-switchable
/// mirroring across all four modes, e.g. FME-7).
fn mirroring_code(m: Mirroring) -> u8 {
    match m {
        Mirroring::Horizontal => 0,
        Mirroring::Vertical => 1,
        Mirroring::FourScreen => 2,
        Mirroring::SingleScreenA => 3,
        Mirroring::SingleScreenB => 4,
    }
}
fn mirroring_from_code(c: u8) -> Result<Mirroring, LoadError> {
    Ok(match c {
        0 => Mirroring::Horizontal,
        1 => Mirroring::Vertical,
        2 => Mirroring::FourScreen,
        3 => Mirroring::SingleScreenA,
        4 => Mirroring::SingleScreenB,
        _ => return Err(LoadError::BadValue("mirroring")),
    })
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
        // Byte 7's upper nibble is the mapper's high 4 bits ONLY in a well-formed
        // header. "Archaic" iNES dumps (byte 7's reserved bits 2-3 nonzero) had
        // byte 7 clobbered -- most infamously by the "DiskDude!" ripper, whose 'D'
        // (0x44) lands in byte 7 and fakes a "4" mapper nibble. Whole ROM sets of
        // MMC1 games (Blaster Master, Willow, Iron Tank, ...) get misread as mapper
        // 65 otherwise. When byte 7 is unreliable, the mapper is byte 6's nibble only.
        let flags7_reliable = is_nes2 || (flags7 & 0x0c) == 0;

        let mut prg_banks = rom[4] as usize;
        let mut chr_banks = rom[5] as usize;
        let mut mapper = if flags7_reliable {
            ((flags7 & 0xf0) | (flags6 >> 4)) as u16
        } else {
            (flags6 >> 4) as u16
        };
        if is_nes2 {
            // NES 2.0 widens mapper to 12 bits and PRG/CHR sizes to 12 bits.
            mapper |= ((rom[8] as u16) & 0x0f) << 8;
            prg_banks |= ((rom[9] as usize) & 0x0f) << 8;
            chr_banks |= ((rom[9] as usize) & 0xf0) << 4;
        }

        // Header-correction DB: old dumps get the mapper (and sometimes mirroring,
        // RAM sizing, or battery) wrong. Look the whole cart up by ROM-data
        // checksum (see `header_db`) and apply each provided field at its site.
        // NES 2.0 headers are modern/curated, so we only correct plain iNES.
        let fix = if is_nes2 {
            None
        } else {
            crate::header_db::correction(crate::header_db::rom_data_crc32(rom))
        };
        if let Some(m) = fix.and_then(|f| f.mapper) {
            mapper = m;
        }

        let mut mirroring = if flags6 & 0x08 != 0 {
            Mirroring::FourScreen
        } else if flags6 & 0x01 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
        if let Some(mir) = fix.and_then(|f| f.mirroring) {
            mirroring = mir;
        }
        let has_trainer = flags6 & 0x04 != 0;

        // RAM sizing. Plain iNES cannot express work-RAM / CHR-RAM sizes, so we
        // keep the historical 8 KiB defaults. NES 2.0 gives them precisely in
        // bytes 10 (PRG) and 11 (CHR): each nibble is a shift count, size =
        // 64 << shift bytes (0 => none), split into a volatile half (low nibble)
        // and a battery/NVRAM half (high nibble). We keep the buffer a power of
        // two and at least the 8 KiB the mappers assume, so their `& (len-1)`
        // mirroring stays valid and no cart gets a zero-length (un-maskable)
        // buffer -- getting the SIZE right is what keeps save-state bytes matched
        // across clients whose dumps disagree (SXROM's 32 KiB, MMC5, ...).
        let ram_bytes = |nibble: u8| -> usize { if nibble == 0 { 0 } else { 64usize << nibble } };
        let (mut prg_ram_size, mut chr_ram_size, prg_nvram) = if is_nes2 {
            let prg = ram_bytes(rom[10] & 0x0f) + ram_bytes(rom[10] >> 4);
            let chr = ram_bytes(rom[11] & 0x0f) + ram_bytes(rom[11] >> 4);
            (
                prg.max(8 * 1024).next_power_of_two(),
                chr.max(CHR_BANK).next_power_of_two(),
                rom[10] >> 4 != 0,
            )
        } else {
            (8 * 1024, CHR_BANK, false)
        };
        // DB may override the RAM sizes for iNES dumps the header can't describe
        // (an iNES-headered SXROM needs 32 KiB work RAM, not the 8 KiB default).
        // Keep the same power-of-two / minimum invariant the mappers rely on.
        if let Some(sz) = fix.and_then(|f| f.prg_ram) {
            prg_ram_size = sz.max(8 * 1024).next_power_of_two();
        }
        if let Some(sz) = fix.and_then(|f| f.chr_ram) {
            chr_ram_size = sz.max(CHR_BANK).next_power_of_two();
        }
        // Battery-backed if the iNES flag says so, NES 2.0 declares PRG-NVRAM, or
        // the DB corrects a dump whose battery bit is wrong.
        let mut has_battery = (flags6 & 0x02 != 0) || prg_nvram;
        if let Some(b) = fix.and_then(|f| f.battery) {
            has_battery = b;
        }

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
            // No CHR ROM -> the cart wires CHR RAM instead (size from NES 2.0).
            (vec![0u8; chr_ram_size], true)
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
            prg_ram: vec![0u8; prg_ram_size],
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

    /// CHR read for a SPRITE pattern fetch (as opposed to a background tile).
    /// Defaults to a plain [`Mapper::ppu_read`]; MMC5 overrides it because it
    /// banks sprite CHR separately from background CHR in 8x16-sprite mode.
    fn ppu_read_sprite(&mut self, addr: u16) -> u8 {
        self.ppu_read(addr)
    }

    /// PPUCTRL ($2000) was written. MMC5 needs the 8x16-sprite bit to pick which
    /// CHR bank set a background fetch uses. Default no-op.
    fn ppu_ctrl(&mut self, _ctrl: u8) {}

    /// Start of PPU scanline `scanline` (0..=261); `rendering` = BG or sprites
    /// enabled. MMC5 clocks its in-frame scanline IRQ counter here. Default no-op.
    fn ppu_scanline(&mut self, _scanline: u16, _rendering: bool) {}

    /// MMC5 extended-attribute mode: per-tile background palette (2 bits) read from
    /// ExRAM, indexed by the tile's position within the nametable (`v & 0x3FF`).
    /// `None` = use the normal attribute-table byte. Default `None`.
    fn ppu_ext_attr(&mut self, _nt_tile: u16) -> Option<u8> {
        None
    }

    /// MMC5 extended-attribute mode: one background pattern byte for the tile at
    /// nametable position `nt_tile`, fetched from that tile's ExRAM-selected 4 KiB
    /// CHR bank. `tile_id` is the nametable byte, `fine_y` 0..7, `hi` selects the
    /// upper bit-plane. `None` = use the normal pattern-table fetch. Default `None`.
    fn ppu_ext_pattern(&mut self, _nt_tile: u16, _tile_id: u8, _fine_y: u16, _hi: bool) -> Option<u8> {
        None
    }

    /// Human-readable dump of mapper-internal registers, for headless diagnosis.
    /// Default empty.
    fn debug_dump(&self) -> String {
        String::new()
    }

    /// Test hook: force MMC5 extended-attribute mode on (to validate a captured
    /// state whose format predates the `$5104` field). Default no-op.
    fn dbg_force_ext_attr(&mut self) {}
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
        // SUROM/SXROM (512 KiB PRG): the CHR bank register's bit 4 supplies PRG
        // A18, selecting which 256 KiB half the 4-bit 16 KiB bank register indexes
        // into. Only carts bigger than 256 KiB (>16 banks) use it; on everything
        // else CHR bit 4 is a real CHR bank bit and must not touch PRG. Dragon
        // Warrior 3/4 are the classic 512 KiB cases.
        let a18 = if self.prg_banks > 16 { self.chr0 as usize & 0x10 } else { 0 };
        let bank16 = |n: usize| ((n | a18) % self.prg_banks) * PRG_BANK;
        let total = self.prg_banks * PRG_BANK;
        let off = (addr as usize) & 0x3fff;
        match (self.control >> 2) & 0x3 {
            0 | 1 => {
                // 32 KiB switch (ignore low bit of the bank number).
                let base = bank16(self.prg_bank as usize & 0x0e);
                (base + (addr as usize - 0x8000)) % total
            }
            2 => {
                // Fix first bank (of the current 256 KiB half) at $8000, switch $C000.
                if addr < 0xc000 {
                    bank16(0) + off
                } else {
                    bank16(self.prg_bank as usize & 0x0f) + off
                }
            }
            _ => {
                // Switch 16 KiB at $8000, fix the LAST bank of the current 256 KiB
                // half at $C000 (0x0f | A18, so SUROM fixes bank 15 or 31).
                if addr < 0xc000 {
                    bank16(self.prg_bank as usize & 0x0f) + off
                } else {
                    bank16(0x0f) + off
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

    // TQROM (mapper 119): CHR ROM + a separate 8 KiB CHR-RAM; each 1 KiB bank's
    // bit 6 selects RAM (empty + not `tqrom` for plain TxROM, so identical there).
    tqrom: bool,
    chr_ram: Vec<u8>,

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
        Self::with_kind(cart, false)
    }
    /// TQROM (mapper 119): MMC3 with 64 KiB CHR ROM + 8 KiB CHR RAM.
    pub fn new_tqrom(cart: Cartridge) -> Self {
        Self::with_kind(cart, true)
    }
    fn with_kind(cart: Cartridge, tqrom: bool) -> Self {
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
            tqrom,
            chr_ram: if tqrom { vec![0u8; 8 * 1024] } else { Vec::new() },
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

    /// Map a PPU pattern address to `(is_ram, index)`. `is_ram` is always false
    /// unless this is a TQROM cart whose selected bank has bit 6 set.
    fn chr_map(&self, addr: u16) -> (bool, usize) {
        let a = addr as usize & 0x1fff;
        let inv = self.bank_select & 0x80 != 0; // CHR A12 inversion
        let region = a / 0x400; // 0..7 (1 KiB windows)
        let r = if inv { region ^ 4 } else { region };
        // The 7-bit bank value the MMC3 drives for this 1 KiB slot. R0/R1 are
        // 2 KiB banks (low bit forced by the sub-slot); R2..R5 are 1 KiB.
        let v = match r {
            0 => self.regs[0] & 0xfe,
            1 => (self.regs[0] & 0xfe) | 1,
            2 => self.regs[1] & 0xfe,
            3 => (self.regs[1] & 0xfe) | 1,
            4 => self.regs[2],
            5 => self.regs[3],
            6 => self.regs[4],
            _ => self.regs[5],
        };
        let off = a & 0x3ff;
        if self.tqrom && v & 0x40 != 0 {
            (true, (v as usize & 0x07) * 0x400 + off) // 8 KiB CHR-RAM
        } else {
            let bank = if self.tqrom { (v & 0x3f) as usize } else { v as usize };
            (false, (bank % self.chr_banks1) * 0x400 + off)
        }
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
        match self.chr_map(addr) {
            (true, off) => self.chr_ram[off % self.chr_ram.len()],
            (false, off) => self.chr[off % self.chr.len()],
        }
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        let a12 = addr & 0x1000 != 0;
        if a12 && !self.prev_a12 {
            self.clock_irq();
        }
        self.prev_a12 = a12;
        match self.chr_map(addr) {
            (true, off) => {
                let n = self.chr_ram.len();
                self.chr_ram[off % n] = val;
            }
            (false, off) if self.chr_is_ram => {
                let n = self.chr.len();
                self.chr[off % n] = val;
            }
            _ => {}
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
        if self.tqrom {
            w.bytes(&self.chr_ram);
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
        if self.tqrom {
            r.bytes(&mut self.chr_ram)?;
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

/// Mapper 13: CPROM (Videomation). 32 KiB fixed PRG and 16 KiB of CHR-RAM in
/// four 4 KiB banks: the low pattern table ($0000-$0FFF) is fixed to bank 0, the
/// high one ($1000-$1FFF) is switched by the low 2 bits written to $8000-$FFFF.
pub struct Cprom {
    prg: Vec<u8>,
    chr: Vec<u8>, // 16 KiB CHR-RAM
    chr_bank: usize,
    mirroring: Mirroring,
}
impl Cprom {
    pub fn new(cart: Cartridge) -> Self {
        Cprom {
            prg: cart.prg_rom,
            chr: vec![0u8; 16 * 1024],
            chr_bank: 0,
            mirroring: cart.mirroring,
        }
    }
    #[inline]
    fn chr_index(&self, addr: u16) -> usize {
        // Low 4 KiB fixed to bank 0; high 4 KiB uses the selected bank.
        let bank = if addr & 0x1000 != 0 { self.chr_bank & 3 } else { 0 };
        bank * 0x1000 + (addr as usize & 0x0fff)
    }
}
impl Mapper for Cprom {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xffff => self.prg[(addr as usize - 0x8000) % self.prg.len()],
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        if addr >= 0x8000 {
            self.chr_bank = (val & 0x03) as usize;
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr[self.chr_index(addr)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        let i = self.chr_index(addr);
        self.chr[i] = val;
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}
impl SaveState for Cprom {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.chr);
        w.u8(self.chr_bank as u8);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.chr)?;
        self.chr_bank = r.u8()? as usize;
        Ok(())
    }
}

/// Mapper 113: HES / NINA-03-06. Like NINA-03 (79) but a single register at
/// $4100-$5FFF also carries a 4th CHR bank bit and a mirroring bit:
///   `MCPP PCCC` -- M=mirroring(1=vert), bit6=CHR high bit, PPP=32 KiB PRG bank,
///   CCC=low 3 CHR bits. Used by AVE/HES carts (Deathbots, Rad Racket).
pub struct Nina113 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_banks32: usize,
    chr_banks8: usize,
    prg_bank: usize,
    chr_bank: usize,
    mirroring: Mirroring,
}
impl Nina113 {
    pub fn new(cart: Cartridge) -> Self {
        Nina113 {
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
impl Mapper for Nina113 {
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
            self.prg_bank = ((val >> 3) & 0x07) as usize;
            self.chr_bank = ((val & 0x07) | ((val >> 3) & 0x08)) as usize; // bit6 -> CHR bit3
            self.mirroring = if val & 0x80 != 0 {
                Mirroring::Vertical
            } else {
                Mirroring::Horizontal
            };
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
impl SaveState for Nina113 {
    fn save(&self, w: &mut WriteCursor) {
        w.u32(self.prg_bank as u32);
        w.u32(self.chr_bank as u32);
        w.u8(mirroring_code(self.mirroring));
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.prg_bank = r.u32()? as usize;
        self.chr_bank = r.u32()? as usize;
        self.mirroring = mirroring_from_code(r.u8()?)?;
        Ok(())
    }
}

/// Mapper 232: Camerica/Codemasters BF9096 (the Quattro multicarts). Two-level
/// PRG banking: a write to $8000-$BFFF sets the 64 KiB block (bits 4-3), a write
/// to $C000-$FFFF sets the inner 16 KiB bank (bits 1-0). $8000 shows block*4+inner;
/// $C000 shows the block's last 16 KiB (block*4+3). 8 KiB CHR-RAM.
pub struct Bf9096 {
    prg: Vec<u8>,
    chr: Vec<u8>, // 8 KiB CHR-RAM
    prg_banks16: usize,
    block: u8, // outer 64 KiB block
    inner: u8, // inner 16 KiB bank
    mirroring: Mirroring,
}
impl Bf9096 {
    pub fn new(cart: Cartridge) -> Self {
        Bf9096 {
            prg_banks16: (cart.prg_rom.len() / PRG_BANK).max(1),
            prg: cart.prg_rom,
            chr: if cart.chr_rom.is_empty() { vec![0u8; CHR_BANK] } else { cart.chr_rom },
            block: 0,
            inner: 0,
            mirroring: cart.mirroring,
        }
    }
    #[inline]
    fn bank16(&self, low: bool) -> usize {
        let b = ((self.block as usize) << 2) | if low { self.inner as usize } else { 3 };
        b % self.prg_banks16
    }
}
impl Mapper for Bf9096 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xbfff => self.prg[self.bank16(true) * PRG_BANK + (addr as usize - 0x8000)],
            0xc000..=0xffff => self.prg[self.bank16(false) * PRG_BANK + (addr as usize - 0xc000)],
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x8000..=0xbfff => self.block = (val >> 3) & 3,
            0xc000..=0xffff => self.inner = val & 3,
            _ => {}
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
impl SaveState for Bf9096 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.chr);
        w.u8(self.block);
        w.u8(self.inner);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.chr)?;
        self.block = r.u8()?;
        self.inner = r.u8()?;
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

/// Mapper 65 (Irem H3001): two switchable 8 KiB PRG banks with a layout bit,
/// eight 1 KiB CHR banks, and a 16-bit down-counter IRQ that ticks every CPU
/// cycle. Per the NESdev wiki register map:
///   $8000       PRG Reg 0 -> 8k window that is either $8000 (layout 0) or $C000 (layout 1)
///   $A000       PRG Reg 1 -> 8k @ $A000 (always)
///   $9000 b7    PRG layout: 0 => $C000 fixed to bank $3E; 1 => $8000 fixed to bank $3E
///   $E000       always fixed to bank $3F (holds the reset/IRQ vectors)
///   $9001 b7-6  mirroring: %00=Vert %10=Horz %01/%11=1scA
///   $9003 b7    IRQ enable (write also acks)
///   $9004       reload counter from latch (write also acks)
///   $9005/$9006 high/low byte of the 16-bit reload latch
///   $B000-$B007 eight 1 KiB CHR banks
/// The fixed banks $3E/$3F are masked against ROM size, i.e. second-to-last and
/// last 8 KiB banks. Getting $E000 wrong reads a garbage reset vector -> CPU JAM,
/// which is what an earlier three-register guess did to Blaster Master.
#[allow(dead_code)]
pub struct H3001 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_is_ram: bool,
    prg_banks8: usize,
    chr_banks1: usize,
    prg_regs: [u8; 2], // $8000 -> Reg0, $A000 -> Reg1
    prg_mode: bool,    // $9000 bit 7: false => Reg0 @ $8000, true => Reg0 @ $C000
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
            prg_regs: [0, 1], // power-on: $8000=$00, $A000=$01
            prg_mode: false,
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
                    0 => {
                        if self.prg_mode {
                            0x3e
                        } else {
                            self.prg_regs[0] as usize
                        }
                    } // $8000
                    1 => self.prg_regs[1] as usize, // $A000
                    2 => {
                        if self.prg_mode {
                            self.prg_regs[0] as usize
                        } else {
                            0x3e
                        }
                    } // $C000
                    _ => 0x3f,                       // $E000 fixed last (reset vectors)
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
            0x9000 => self.prg_mode = val & 0x80 != 0,
            0x9001 => {
                self.mirroring = match val >> 6 {
                    0b00 => Mirroring::Vertical,
                    0b10 => Mirroring::Horizontal,
                    _ => Mirroring::SingleScreenA, // %01, %11
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
            0xa000..=0xafff => self.prg_regs[1] = val,
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
        w.bool(self.prg_mode);
        w.bytes(&self.chr_regs);
        w.u8(mirroring_code(self.mirroring));
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
        self.prg_mode = r.bool()?;
        r.bytes(&mut self.chr_regs)?;
        self.mirroring = mirroring_from_code(r.u8()?)?;
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

/// Mapper 73 (Konami VRC3): 16 KiB switchable PRG at $8000 + fixed last, 8 KiB
/// CHR RAM, and a 16-bit (or 8-bit) CPU-cycle IRQ counter that *increments*.
/// Used by the non-Mike-Tyson "Punch-Out!!" and Salamander.
#[allow(dead_code)]
pub struct Vrc3 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_ram: Vec<u8>,
    prg_banks: usize,
    prg_bank: usize,

    irq_latch: u16,
    irq_counter: u16,
    irq_enable: bool,
    irq_ack_enable: bool,
    irq_mode_8bit: bool,
    irq_flag: bool,
}
impl Vrc3 {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks = (cart.prg_rom.len() / PRG_BANK).max(1);
        Vrc3 {
            prg_banks,
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            prg_ram: cart.prg_ram,
            prg_bank: 0, // power-up bank is undefined on VRC3; game sets it
            irq_latch: 0,
            irq_counter: 0,
            irq_enable: false,
            irq_ack_enable: false,
            irq_mode_8bit: false,
            irq_flag: false,
        }
    }
}
impl Mapper for Vrc3 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => self.prg_ram[(addr as usize - 0x6000) & (self.prg_ram.len() - 1)],
            0x8000..=0xbfff => self.prg[(self.prg_bank % self.prg_banks) * PRG_BANK + (addr as usize - 0x8000)],
            0xc000..=0xffff => self.prg[(self.prg_banks - 1) * PRG_BANK + (addr as usize - 0xc000)],
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr & 0xf000 {
            0x6000 | 0x7000 => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
            }
            0x8000 => self.irq_latch = (self.irq_latch & 0xfff0) | (val as u16 & 0x0f),
            0x9000 => self.irq_latch = (self.irq_latch & 0xff0f) | ((val as u16 & 0x0f) << 4),
            0xa000 => self.irq_latch = (self.irq_latch & 0xf0ff) | ((val as u16 & 0x0f) << 8),
            0xb000 => self.irq_latch = (self.irq_latch & 0x0fff) | ((val as u16 & 0x0f) << 12),
            0xc000 => {
                self.irq_ack_enable = val & 0x01 != 0;
                self.irq_enable = val & 0x02 != 0;
                self.irq_mode_8bit = val & 0x04 != 0;
                self.irq_flag = false;
                if self.irq_enable {
                    self.irq_counter = self.irq_latch;
                }
            }
            0xd000 => {
                self.irq_flag = false;
                self.irq_enable = self.irq_ack_enable;
            }
            0xf000 => self.prg_bank = (val & 0x07) as usize,
            _ => {}
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
        Mirroring::Vertical // VRC3 has fixed (header) mirroring; typ. vertical
    }
    fn irq(&self) -> bool {
        self.irq_flag
    }
    fn tick_cpu(&mut self) {
        if !self.irq_enable {
            return;
        }
        if self.irq_mode_8bit {
            let lo = self.irq_counter & 0x00ff;
            if lo == 0x00ff {
                self.irq_flag = true;
                self.irq_counter = (self.irq_counter & 0xff00) | (self.irq_latch & 0x00ff);
            } else {
                self.irq_counter = (self.irq_counter & 0xff00) | (lo + 1);
            }
        } else if self.irq_counter == 0xffff {
            self.irq_flag = true;
            self.irq_counter = self.irq_latch;
        } else {
            self.irq_counter += 1;
        }
    }
}
impl SaveState for Vrc3 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        w.bytes(&self.chr);
        w.u32(self.prg_bank as u32);
        w.u16(self.irq_latch);
        w.u16(self.irq_counter);
        w.bool(self.irq_enable);
        w.bool(self.irq_ack_enable);
        w.bool(self.irq_mode_8bit);
        w.bool(self.irq_flag);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        let mut ram = vec![0u8; self.prg_ram.len()];
        r.bytes(&mut ram)?;
        self.prg_ram = ram;
        let mut chr = vec![0u8; self.chr.len()];
        r.bytes(&mut chr)?;
        self.chr = chr;
        self.prg_bank = r.u32()? as usize;
        self.irq_latch = r.u16()?;
        self.irq_counter = r.u16()?;
        self.irq_enable = r.bool()?;
        self.irq_ack_enable = r.bool()?;
        self.irq_mode_8bit = r.bool()?;
        self.irq_flag = r.bool()?;
        Ok(())
    }
}

/// Mapper 5: MMC5 / ExROM — Nintendo's most complex mapper. This is a pragmatic
/// subset that covers what the library actually renders: flexible PRG banking
/// (4 modes, per-bank ROM/RAM select) and PRG-RAM banking, CHR banking (4 modes)
/// with the 8x16-sprite BG/sprite CHR split, nametable control ($5105, mapped to
/// our mirroring set), the hardware multiplier, and the in-frame scanline IRQ.
/// Deferred until a game needs them: the two extra audio channels + PCM, the
/// extended-attribute nametable mode, and the vertical split screen.
const MMC5_PRG_RAM: usize = 64 * 1024;
pub struct Mmc5 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_ram: Vec<u8>,
    exram: [u8; 0x400],
    prg_banks8: usize,
    chr_banks1: usize,
    prg_ram_banks8: usize,

    prg_mode: u8,
    chr_mode: u8,
    prg_regs: [u8; 5], // $5113 (RAM bank) .. $5117
    chr_spr: [u8; 8],  // $5120-$5127 (sprite CHR)
    chr_bg: [u8; 4],   // $5128-$512B (background CHR in 8x16 mode)
    chr_upper: u8,     // $5130 high bank bits
    nt_map: u8,        // $5105
    exram_mode: u8,    // $5104: 0/1 PPU (extra-NT / extended-attr), 2/3 CPU RAM

    mult_a: u8, // $5205
    mult_b: u8, // $5206

    irq_scanline: u8, // $5203
    irq_enable: bool, // $5204 bit7
    irq_pending: bool,
    in_frame: bool,
    scan_counter: u16,

    tall_sprites: bool, // PPUCTRL bit5
}
impl Mmc5 {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks8 = (cart.prg_rom.len() / 0x2000).max(1);
        let chr_banks1 = (cart.chr_rom.len() / 0x400).max(1);
        Mmc5 {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            chr_is_ram: cart.chr_is_ram,
            prg_ram: vec![0u8; MMC5_PRG_RAM],
            exram: [0u8; 0x400],
            prg_banks8,
            chr_banks1,
            prg_ram_banks8: MMC5_PRG_RAM / 0x2000,
            prg_mode: 3, // power-on: 8 KiB banks (matches most reset code)
            chr_mode: 0,
            prg_regs: [0, 0, 0, 0, 0xff], // $5117 -> last ROM bank at reset
            chr_spr: [0; 8],
            chr_bg: [0; 4],
            chr_upper: 0,
            nt_map: 0,
            exram_mode: 0,
            mult_a: 0,
            mult_b: 0,
            irq_scanline: 0,
            irq_enable: false,
            irq_pending: false,
            in_frame: false,
            scan_counter: 0,
            tall_sprites: false,
        }
    }

    /// (8 KiB bank number, is_rom) for a CPU $8000-$FFFF slot 0..3.
    fn prg_slot(&self, slot: usize) -> (usize, bool) {
        let r = &self.prg_regs;
        let bank = |reg: u8| (reg & 0x7f) as usize;
        let rom = |reg: u8| reg & 0x80 != 0;
        match self.prg_mode {
            0 => ((bank(r[4]) & !3) + slot, true), // 32 KiB from $5117
            1 => {
                if slot < 2 {
                    ((bank(r[2]) & !1) + (slot & 1), rom(r[2])) // $5115 16K
                } else {
                    ((bank(r[4]) & !1) + (slot & 1), true) // $5117 16K
                }
            }
            2 => match slot {
                0 | 1 => ((bank(r[2]) & !1) + (slot & 1), rom(r[2])), // $5115 16K
                2 => (bank(r[3]), rom(r[3])),                        // $5116 8K
                _ => (bank(r[4]), true),                             // $5117 8K
            },
            _ => match slot {
                0 => (bank(r[1]), rom(r[1])), // $5114
                1 => (bank(r[2]), rom(r[2])), // $5115
                2 => (bank(r[3]), rom(r[3])), // $5116
                _ => (bank(r[4]), true),      // $5117 (always ROM)
            },
        }
    }

    fn prg_read(&self, addr: u16) -> u8 {
        if addr < 0x8000 {
            // $6000-$7FFF: PRG-RAM bank from $5113.
            let bank = self.prg_regs[0] as usize & (self.prg_ram_banks8 - 1);
            return self.prg_ram[bank * 0x2000 + (addr as usize - 0x6000)];
        }
        let slot = (addr as usize - 0x8000) >> 13;
        let (bank, is_rom) = self.prg_slot(slot);
        let off = addr as usize & 0x1fff;
        if is_rom {
            self.prg[(bank % self.prg_banks8) * 0x2000 + off]
        } else {
            self.prg_ram[(bank % self.prg_ram_banks8) * 0x2000 + off]
        }
    }

    fn prg_write(&mut self, addr: u16, val: u8) {
        if addr < 0x8000 {
            let bank = self.prg_regs[0] as usize & (self.prg_ram_banks8 - 1);
            self.prg_ram[bank * 0x2000 + (addr as usize - 0x6000)] = val;
            return;
        }
        let slot = (addr as usize - 0x8000) >> 13;
        let (bank, is_rom) = self.prg_slot(slot);
        if !is_rom {
            let off = addr as usize & 0x1fff;
            self.prg_ram[(bank % self.prg_ram_banks8) * 0x2000 + off] = val;
        }
    }

    /// 1 KiB CHR bank for a 1 KiB slot 0..7 given a register set and the CHR mode.
    fn chr_1k(&self, slot: usize, regs: &[u8]) -> usize {
        let mask = regs.len() - 1;
        let (reg, sub, sz) = match self.chr_mode {
            0 => (7 & mask, slot, 8usize),           // 8 KiB
            1 => ((if slot < 4 { 3 } else { 7 }) & mask, slot & 3, 4), // 4 KiB
            2 => ((slot | 1) & mask, slot & 1, 2),   // 2 KiB
            _ => (slot & mask, 0, 1),                // 1 KiB
        };
        let bank = ((self.chr_upper as usize) << 8) | regs[reg] as usize;
        bank * sz + sub
    }

    fn chr_byte(&self, bank1k: usize, addr: u16) -> u8 {
        let idx = (bank1k % self.chr_banks1) * 0x400 + (addr as usize & 0x3ff);
        if self.chr_is_ram {
            self.chr[idx % self.chr.len()]
        } else {
            self.chr[idx]
        }
    }
}
impl Mapper for Mmc5 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x5204 => {
                let s = (self.irq_pending as u8) << 7 | (self.in_frame as u8) << 6;
                self.irq_pending = false;
                s
            }
            0x5205 => (self.mult_a as u16 * self.mult_b as u16) as u8,
            0x5206 => ((self.mult_a as u16 * self.mult_b as u16) >> 8) as u8,
            0x5c00..=0x5fff => self.exram[addr as usize - 0x5c00],
            0x6000..=0xffff => self.prg_read(addr),
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x5100 => self.prg_mode = val & 3,
            0x5101 => self.chr_mode = val & 3,
            0x5104 => self.exram_mode = val & 3,
            0x5105 => self.nt_map = val,
            0x5113 => self.prg_regs[0] = val,
            0x5114..=0x5117 => self.prg_regs[(addr - 0x5113) as usize] = val,
            0x5120..=0x5127 => self.chr_spr[(addr - 0x5120) as usize] = val,
            0x5128..=0x512b => self.chr_bg[(addr - 0x5128) as usize] = val,
            0x5130 => self.chr_upper = val & 3,
            0x5203 => self.irq_scanline = val,
            0x5204 => self.irq_enable = val & 0x80 != 0,
            0x5205 => self.mult_a = val,
            0x5206 => self.mult_b = val,
            0x5c00..=0x5fff => self.exram[addr as usize - 0x5c00] = val,
            0x6000..=0xffff => self.prg_write(addr, val),
            _ => {} // $5102/3 RAM protect, $5106/7 fill-mode, audio -> ignored
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        // Background fetch: 8x16 mode uses the BG CHR set, else the sprite set.
        let slot = (addr >> 10) as usize & 7;
        let bank = if self.tall_sprites {
            self.chr_1k(slot, &self.chr_bg)
        } else {
            self.chr_1k(slot, &self.chr_spr)
        };
        self.chr_byte(bank, addr)
    }

    fn ppu_read_sprite(&mut self, addr: u16) -> u8 {
        let slot = (addr >> 10) as usize & 7;
        let bank = self.chr_1k(slot, &self.chr_spr);
        self.chr_byte(bank, addr)
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let slot = (addr >> 10) as usize & 7;
            let bank = self.chr_1k(slot, &self.chr_spr);
            let n = self.chr.len();
            let idx = ((bank % self.chr_banks1) * 0x400 + (addr as usize & 0x3ff)) % n;
            self.chr[idx] = val;
        }
    }

    fn ppu_ctrl(&mut self, ctrl: u8) {
        self.tall_sprites = ctrl & 0x20 != 0;
    }

    fn ppu_scanline(&mut self, scanline: u16, rendering: bool) {
        if !rendering || scanline >= 240 {
            self.in_frame = false;
            return;
        }
        if !self.in_frame {
            self.in_frame = true;
            self.scan_counter = 0;
        } else {
            self.scan_counter += 1;
        }
        if self.scan_counter as u8 == self.irq_scanline && self.irq_scanline != 0 {
            self.irq_pending = true;
        }
    }

    fn irq(&self) -> bool {
        self.irq_pending && self.irq_enable
    }

    fn ppu_ext_attr(&mut self, nt_tile: u16) -> Option<u8> {
        // Mode 1 = extended attribute: each ExRAM byte's top 2 bits are the tile's
        // palette. (Modes 0/2/3 use the normal attribute table.)
        if self.exram_mode != 1 {
            return None;
        }
        Some((self.exram[nt_tile as usize & 0x3ff] >> 6) & 3)
    }

    fn ppu_ext_pattern(&mut self, nt_tile: u16, tile_id: u8, fine_y: u16, hi: bool) -> Option<u8> {
        // Mode 1: the low 6 bits of the tile's ExRAM byte (plus $5130's 2 high bits)
        // select a 4 KiB CHR bank for THAT background tile, so a screen can use far
        // more than 256 distinct tiles -- exactly how Koei's dense menus are drawn.
        if self.exram_mode != 1 {
            return None;
        }
        let ex = self.exram[nt_tile as usize & 0x3ff] as usize;
        let bank4k = ((self.chr_upper as usize) << 6) | (ex & 0x3f);
        let off = tile_id as usize * 16 + fine_y as usize + if hi { 8 } else { 0 };
        let idx = (bank4k * 0x1000 + off) % self.chr.len().max(1);
        Some(self.chr[idx])
    }

    fn dbg_force_ext_attr(&mut self) {
        self.exram_mode = 1;
    }

    fn mirroring(&self) -> Mirroring {
        let m = self.nt_map;
        let s = [m & 3, (m >> 2) & 3, (m >> 4) & 3, (m >> 6) & 3];
        match s {
            [0, 0, 1, 1] => Mirroring::Horizontal,
            [0, 1, 0, 1] => Mirroring::Vertical,
            [0, 0, 0, 0] => Mirroring::SingleScreenA,
            [1, 1, 1, 1] => Mirroring::SingleScreenB,
            _ => Mirroring::Horizontal, // ExRAM / fill-mode NTs approximated
        }
    }

    fn debug_dump(&self) -> String {
        let m = self.nt_map;
        let nt = [m & 3, (m >> 2) & 3, (m >> 4) & 3, (m >> 6) & 3];
        let ex_nz = self.exram.iter().filter(|&&b| b != 0).count();
        format!(
            "MMC5: prg_mode={} chr_mode={} tall_sprites={} nt_map=${:02x} quads={:?} \
             chr_spr={:02x?} chr_bg={:02x?} chr_upper={} prg_regs={:02x?} \
             exram_nonzero={}/1024 exram[0..16]={:02x?}",
            self.prg_mode,
            self.chr_mode,
            self.tall_sprites,
            m,
            nt,
            self.chr_spr,
            self.chr_bg,
            self.chr_upper,
            self.prg_regs,
            ex_nz,
            &self.exram[..16],
        )
    }
}
impl SaveState for Mmc5 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.bytes(&self.exram);
        w.u8(self.prg_mode);
        w.u8(self.chr_mode);
        for &b in &self.prg_regs {
            w.u8(b);
        }
        for &b in &self.chr_spr {
            w.u8(b);
        }
        for &b in &self.chr_bg {
            w.u8(b);
        }
        w.u8(self.chr_upper);
        w.u8(self.nt_map);
        w.u8(self.exram_mode);
        w.u8(self.mult_a);
        w.u8(self.mult_b);
        w.u8(self.irq_scanline);
        w.bool(self.irq_enable);
        w.bool(self.irq_pending);
        w.bool(self.in_frame);
        w.u16(self.scan_counter);
        w.bool(self.tall_sprites);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.prg_ram)?;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        r.bytes(&mut self.exram)?;
        self.prg_mode = r.u8()?;
        self.chr_mode = r.u8()?;
        for b in &mut self.prg_regs {
            *b = r.u8()?;
        }
        for b in &mut self.chr_spr {
            *b = r.u8()?;
        }
        for b in &mut self.chr_bg {
            *b = r.u8()?;
        }
        self.chr_upper = r.u8()?;
        self.nt_map = r.u8()?;
        self.exram_mode = r.u8()?;
        self.mult_a = r.u8()?;
        self.mult_b = r.u8()?;
        self.irq_scanline = r.u8()?;
        self.irq_enable = r.bool()?;
        self.irq_pending = r.bool()?;
        self.in_frame = r.bool()?;
        self.scan_counter = r.u16()?;
        self.tall_sprites = r.bool()?;
        Ok(())
    }
}

/// Mapper 69: Sunsoft FME-7 (a.k.a. Sunsoft-5). A command/parameter register pair
/// ($8000 = command 0-F, $A000 = parameter): 8x 1 KiB CHR banks, three switchable
/// 8 KiB PRG banks ($8000/$A000/$C000) + a fixed last bank at $E000 + a RAM/ROM
/// bank at $6000, mirroring control, and a 16-bit down-counting CPU-cycle IRQ.
/// (The optional Sunsoft-5B audio channels are deferred.)
pub struct Fme7 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_ram: Vec<u8>,
    prg_banks8: usize,
    chr_banks1: usize,
    command: u8,
    chr_bank: [u8; 8],
    prg6000: u8, // bit7 = map at $6000, bit6 = RAM(1)/ROM(0), bits5-0 = bank
    prg8000: u8,
    prg_a000: u8,
    prg_c000: u8,
    mirroring: Mirroring,
    irq_counter: u16,
    counter_enable: bool,
    irq_enable: bool,
    irq_flag: bool,
}
impl Fme7 {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks8 = (cart.prg_rom.len() / 0x2000).max(1);
        let chr_banks1 = (cart.chr_rom.len() / 0x400).max(1);
        Fme7 {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            chr_is_ram: cart.chr_is_ram,
            prg_ram: cart.prg_ram,
            prg_banks8,
            chr_banks1,
            command: 0,
            chr_bank: [0; 8],
            prg6000: 0,
            prg8000: 0,
            prg_a000: 0,
            prg_c000: 0,
            mirroring: cart.mirroring,
            irq_counter: 0,
            counter_enable: false,
            irq_enable: false,
            irq_flag: false,
        }
    }
    fn rom8(&self, bank: usize, addr: u16) -> u8 {
        self.prg[(bank % self.prg_banks8) * 0x2000 + (addr as usize & 0x1fff)]
    }
}
impl Mapper for Fme7 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => {
                if self.prg6000 & 0x80 == 0 {
                    0 // not mapped -> open bus
                } else if self.prg6000 & 0x40 != 0 {
                    let n = self.prg_ram.len();
                    self.prg_ram[(addr as usize - 0x6000) & (n - 1)]
                } else {
                    self.rom8((self.prg6000 & 0x3f) as usize, addr)
                }
            }
            0x8000..=0x9fff => self.rom8(self.prg8000 as usize & 0x3f, addr),
            0xa000..=0xbfff => self.rom8(self.prg_a000 as usize & 0x3f, addr),
            0xc000..=0xdfff => self.rom8(self.prg_c000 as usize & 0x3f, addr),
            0xe000..=0xffff => self.rom8(self.prg_banks8 - 1, addr),
            _ => 0,
        }
    }
    fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x6000..=0x7fff => {
                if self.prg6000 & 0xc0 == 0xc0 {
                    let n = self.prg_ram.len();
                    self.prg_ram[(addr as usize - 0x6000) & (n - 1)] = val;
                }
            }
            0x8000..=0x9fff => self.command = val & 0x0f,
            0xa000..=0xbfff => match self.command {
                0..=7 => self.chr_bank[self.command as usize] = val,
                8 => self.prg6000 = val,
                9 => self.prg8000 = val,
                0xa => self.prg_a000 = val,
                0xb => self.prg_c000 = val,
                0xc => {
                    self.mirroring = match val & 3 {
                        0 => Mirroring::Vertical,
                        1 => Mirroring::Horizontal,
                        2 => Mirroring::SingleScreenA,
                        _ => Mirroring::SingleScreenB,
                    };
                }
                0xd => {
                    self.counter_enable = val & 0x80 != 0;
                    self.irq_enable = val & 0x01 != 0;
                    self.irq_flag = false; // writing IRQ control acknowledges
                }
                0xe => self.irq_counter = (self.irq_counter & 0xff00) | val as u16,
                _ => self.irq_counter = (self.irq_counter & 0x00ff) | ((val as u16) << 8),
            },
            _ => {}
        }
    }
    fn ppu_read(&mut self, addr: u16) -> u8 {
        let bank = self.chr_bank[(addr >> 10) as usize & 7] as usize;
        self.chr[(bank % self.chr_banks1) * 0x400 + (addr as usize & 0x3ff)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let bank = self.chr_bank[(addr >> 10) as usize & 7] as usize;
            let n = self.chr.len();
            self.chr[((bank % self.chr_banks1) * 0x400 + (addr as usize & 0x3ff)) % n] = val;
        }
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    fn irq(&self) -> bool {
        self.irq_flag
    }
    fn tick_cpu(&mut self) {
        if self.counter_enable {
            let (n, under) = self.irq_counter.overflowing_sub(1);
            self.irq_counter = n;
            if under && self.irq_enable {
                self.irq_flag = true;
            }
        }
    }
}
impl SaveState for Fme7 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.u8(self.command);
        for &b in &self.chr_bank {
            w.u8(b);
        }
        w.u8(self.prg6000);
        w.u8(self.prg8000);
        w.u8(self.prg_a000);
        w.u8(self.prg_c000);
        w.u8(mirroring_code(self.mirroring));
        w.u16(self.irq_counter);
        w.bool(self.counter_enable);
        w.bool(self.irq_enable);
        w.bool(self.irq_flag);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.prg_ram)?;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        self.command = r.u8()?;
        for b in &mut self.chr_bank {
            *b = r.u8()?;
        }
        self.prg6000 = r.u8()?;
        self.prg8000 = r.u8()?;
        self.prg_a000 = r.u8()?;
        self.prg_c000 = r.u8()?;
        self.mirroring = mirroring_from_code(r.u8()?)?;
        self.irq_counter = r.u16()?;
        self.counter_enable = r.bool()?;
        self.irq_enable = r.bool()?;
        self.irq_flag = r.bool()?;
        Ok(())
    }
}

/// Mapper 64: Tengen RAMBO-1 (800032). An MMC3 relative: the same $8000/$8001
/// select/data pair, but 16 registers, a 1 KiB *or* 2 KiB CHR mode, THREE
/// switchable 8 KiB PRG banks (R6/R7/R15) plus the fixed last bank, and a
/// dual-mode IRQ counter — either clocked by PPU A12 like MMC3 (scanline) or by
/// a CPU-cycle prescaler. Klax, Shinobi, Skull & Crossbones, Gyruss, ...
pub struct Rambo1 {
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_is_ram: bool,
    prg_ram: Vec<u8>,
    prg_banks8: usize,
    chr_banks1: usize,
    bank_select: u8, // $8000
    regs: [u8; 16],
    mirroring: Mirroring,
    irq_latch: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enable: bool,
    irq_flag: bool,
    irq_cycle_mode: bool,
    cycle_prescaler: u8,
    prev_a12: bool,
}
impl Rambo1 {
    pub fn new(cart: Cartridge) -> Self {
        let prg_banks8 = (cart.prg_rom.len() / 0x2000).max(1);
        let chr_banks1 = (cart.chr_rom.len() / 0x400).max(1);
        Rambo1 {
            prg: cart.prg_rom,
            chr: cart.chr_rom,
            chr_is_ram: cart.chr_is_ram,
            prg_ram: cart.prg_ram,
            prg_banks8,
            chr_banks1,
            bank_select: 0,
            // Power-on with a LINEAR PRG and CHR map, like NROM: PRG $8000=bank0,
            // $A000=1, $C000=2, $E000=last (R6=0/R7=1/R15=2); CHR slots 0..7 = banks
            // 0..7 (R0=0/R1=2/R2=4/R3=5/R4=6/R5=7, since R0/R1 are 2 KiB). Games
            // that program their own banks overwrite this at once; ones that don't
            // (Mystery Quest is NROM-on-RAMBO-1) rely on the sequential default --
            // all-zero registers stack bank 0/1 across each window, so PRG jumps
            // into the wrong bank and JAMs and CHR shows the wrong, repeated tiles.
            regs: {
                let mut r = [0u8; 16];
                r[1] = 2; r[2] = 4; r[3] = 5; r[4] = 6; r[5] = 7;
                r[7] = 1; r[15] = 2;
                r
            },
            mirroring: cart.mirroring,
            irq_latch: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enable: false,
            irq_flag: false,
            irq_cycle_mode: false,
            cycle_prescaler: 0,
            prev_a12: false,
        }
    }
    fn prg_bank(&self, region: usize) -> usize {
        let p = self.bank_select & 0x40 != 0;
        let r = &self.regs;
        (match (region, p) {
            (0, false) => r[6],
            (0, true) => r[15],
            (1, false) => r[7],
            (1, true) => r[6],
            (2, false) => r[15],
            (2, true) => r[7],
            _ => return self.prg_banks8 - 1, // $E000 fixed to last
        }) as usize
    }
    fn chr_bank(&self, slot: usize) -> usize {
        let r = &self.regs;
        // CHR mode is bank_select bit 5 (K), NOT bit 7. Klax sets K=1 for its
        // all-1 KiB playfield; reading the wrong bit garbled the bottom of screen.
        (if self.bank_select & 0x20 == 0 {
            // 2 KiB + 1 KiB layout.
            match slot {
                0 => r[0] & 0xfe,
                1 => (r[0] & 0xfe) + 1,
                2 => r[1] & 0xfe,
                3 => (r[1] & 0xfe) + 1,
                4 => r[2],
                5 => r[3],
                6 => r[4],
                _ => r[5],
            }
        } else {
            // All 1 KiB (R8/R9 fill in for the split 2 KiB slots).
            match slot {
                0 => r[0],
                1 => r[8],
                2 => r[1],
                3 => r[9],
                4 => r[2],
                5 => r[3],
                6 => r[4],
                _ => r[5],
            }
        }) as usize
    }
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
impl Mapper for Rambo1 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7fff => {
                let n = self.prg_ram.len();
                self.prg_ram[(addr as usize - 0x6000) & (n - 1)]
            }
            0x8000..=0xffff => {
                let region = (addr as usize - 0x8000) / 0x2000;
                let bank = self.prg_bank(region) % self.prg_banks8;
                self.prg[bank * 0x2000 + (addr as usize & 0x1fff)]
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
                    self.regs[(self.bank_select & 0x0f) as usize] = val;
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
            }
            0xc000..=0xdfff => {
                if addr & 1 == 0 {
                    self.irq_latch = val;
                } else {
                    self.irq_cycle_mode = val & 1 != 0;
                    self.irq_reload = true;
                    self.cycle_prescaler = 0;
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
        let a12 = addr & 0x1000 != 0;
        if !self.irq_cycle_mode && a12 && !self.prev_a12 {
            self.clock_irq();
        }
        self.prev_a12 = a12;
        let bank = self.chr_bank((addr >> 10) as usize & 7) % self.chr_banks1;
        self.chr[bank * 0x400 + (addr as usize & 0x3ff)]
    }
    fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_is_ram {
            let bank = self.chr_bank((addr >> 10) as usize & 7) % self.chr_banks1;
            let n = self.chr.len();
            self.chr[(bank * 0x400 + (addr as usize & 0x3ff)) % n] = val;
        }
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    fn irq(&self) -> bool {
        self.irq_flag
    }
    fn tick_cpu(&mut self) {
        if self.irq_cycle_mode {
            self.cycle_prescaler += 1;
            if self.cycle_prescaler >= 4 {
                self.cycle_prescaler = 0;
                self.clock_irq();
            }
        }
    }
}
impl SaveState for Rambo1 {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.prg_ram);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
        w.u8(self.bank_select);
        for &b in &self.regs {
            w.u8(b);
        }
        w.u8(mirroring_code(self.mirroring));
        w.u8(self.irq_latch);
        w.u8(self.irq_counter);
        w.bool(self.irq_reload);
        w.bool(self.irq_enable);
        w.bool(self.irq_flag);
        w.bool(self.irq_cycle_mode);
        w.u8(self.cycle_prescaler);
        w.bool(self.prev_a12);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.prg_ram)?;
        if self.chr_is_ram {
            let mut chr = vec![0u8; self.chr.len()];
            r.bytes(&mut chr)?;
            self.chr = chr;
        }
        self.bank_select = r.u8()?;
        for b in &mut self.regs {
            *b = r.u8()?;
        }
        self.mirroring = mirroring_from_code(r.u8()?)?;
        self.irq_latch = r.u8()?;
        self.irq_counter = r.u8()?;
        self.irq_reload = r.bool()?;
        self.irq_enable = r.bool()?;
        self.irq_flag = r.bool()?;
        self.irq_cycle_mode = r.bool()?;
        self.cycle_prescaler = r.u8()?;
        self.prev_a12 = r.bool()?;
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
        5 => Ok(Box::new(Mmc5::new(cart))),
        7 => Ok(Box::new(Axrom::new(cart))),
        64 => Ok(Box::new(Rambo1::new(cart))),
        69 => Ok(Box::new(Fme7::new(cart))),
        9 => Ok(Box::new(Mmc2::new(cart, false))),
        11 => Ok(Box::new(BankSwap::new(cart, BankSwapKind::ColorDreams))),
        13 => Ok(Box::new(Cprom::new(cart))),
        119 => Ok(Box::new(Mmc3::new_tqrom(cart))),
        113 => Ok(Box::new(Nina113::new(cart))),
        232 => Ok(Box::new(Bf9096::new(cart))),
        // Mapper 34: NINA-001 (CHR ROM) vs BNROM (CHR RAM).
        34 if !cart.chr_is_ram => Ok(Box::new(Nina001::new(cart))),
        34 => Ok(Box::new(BankSwap::new(cart, BankSwapKind::Bnrom))),
        65 => Ok(Box::new(H3001::new(cart))),
        66 => Ok(Box::new(BankSwap::new(cart, BankSwapKind::Gxrom))),
        71 => Ok(Box::new(Camerica::new(cart))),
        79 => Ok(Box::new(Nina03::new(cart))),
        // 73 (Konami VRC3) has a complete impl below but hangs its boot -- a
        // CPU-cycle-counted IRQ subtlety we have not cracked. Left unwired
        // (report unsupported) rather than shipping a frozen black screen.
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
    fn ines_ram_defaults_to_8k() {
        // Plain iNES can't express RAM size -> the historical 8 KiB defaults.
        let cart = Cartridge::from_ines(&synth_ines(0x00)).unwrap();
        assert_eq!(cart.prg_ram.len(), 8 * 1024);
        assert_eq!(cart.chr_rom.len(), CHR_BANK); // CHR ROM present here
    }

    #[test]
    fn nes2_ram_sizes_from_header() {
        // NES 2.0 image: PRG-NVRAM 32 KiB (byte10 hi nibble = 9 -> 64<<9), CHR-RAM
        // 16 KiB (byte11 lo nibble = 8 -> 64<<8), CHR ROM absent (byte5 = 0).
        let mut v = vec![0u8; HEADER_LEN];
        v[0..4].copy_from_slice(b"NES\x1a");
        v[4] = 1; // 16 KiB PRG
        v[5] = 0; // no CHR ROM -> CHR RAM
        v[7] = 0x08; // NES 2.0 marker (bits 3:2 == 10)
        v[10] = 0x90; // PRG: volatile 0, NVRAM shift 9 => 32 KiB battery RAM
        v[11] = 0x08; // CHR: volatile shift 8 => 16 KiB CHR RAM
        v.extend(std::iter::repeat(0xa5).take(PRG_BANK));
        let cart = Cartridge::from_ines(&v).unwrap();
        assert_eq!(cart.prg_ram.len(), 32 * 1024, "PRG-RAM sized from NES 2.0");
        assert!(cart.chr_is_ram);
        assert_eq!(cart.chr_rom.len(), 16 * 1024, "CHR-RAM sized from NES 2.0");
        assert!(cart.has_battery, "PRG-NVRAM implies battery-backed");
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
