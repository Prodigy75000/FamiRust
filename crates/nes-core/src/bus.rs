//! CPU address bus and the system memory map.
//!
//! Routes the 6502's 16-bit address space to the right device:
//!   $0000-$1FFF  2 KiB internal RAM, mirrored four times
//!   $2000-$3FFF  PPU registers ($2000-$2007), mirrored every 8 bytes
//!   $4000-$4017  APU and I/O registers (incl. $4016/$4017 controllers, OAM DMA)
//!   $4018-$401F  normally-disabled test registers (open bus)
//!   $4020-$FFFF  cartridge space (PRG ROM/RAM, mapper registers)
//!
//! Only internal RAM and cartridge routing are live; PPU/APU register decode is
//! stubbed to open bus until those units are built. Kept deliberately thin so
//! the CPU bring-up has a real bus to talk to without waiting on the PPU.

use crate::apu::Apu;
use crate::cart::Mapper;
use crate::controller::Controller;
use crate::cpu::CpuBus;
use crate::ppu::Ppu;
use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

pub struct Bus {
    pub ram: [u8; 2048],
    pub ppu: Ppu,
    pub apu: Apu,
    pub controllers: [Controller; 2],
    pub mapper: Box<dyn Mapper>,
    /// Last value on the data bus, returned for open-bus reads.
    open_bus: u8,
}

impl Bus {
    pub fn new(mapper: Box<dyn Mapper>) -> Self {
        Bus {
            ram: [0; 2048],
            ppu: Ppu::new(),
            apu: Apu::new(),
            controllers: [Controller::new(), Controller::new()],
            mapper,
            open_bus: 0,
        }
    }

}

impl Bus {
    /// Advance the rest of the system for one CPU cycle: three PPU dots (and,
    /// later, one APU tick). Every CPU bus access is one cycle and calls this.
    #[inline]
    fn tick(&mut self) {
        let m = &mut *self.mapper;
        self.ppu.tick(m);
        self.ppu.tick(m);
        self.ppu.tick(m);
    }

    /// $4014 OAM DMA: copy 256 bytes from CPU page `hi` into OAM at the current
    /// OAMADDR, one read + one write cycle per byte (each ticks the PPU).
    fn oam_dma(&mut self, hi: u8) {
        let base = (hi as u16) << 8;
        for i in 0..256u16 {
            let b = self.read(base + i); // read cycle (ticks)
            self.tick(); // write cycle
            self.ppu.oam_dma_write(b);
        }
    }
}

impl CpuBus for Bus {
    fn read(&mut self, addr: u16) -> u8 {
        self.tick();
        let val = match addr {
            0x0000..=0x1fff => self.ram[(addr & 0x07ff) as usize],
            0x2000..=0x3fff => self.ppu.read_register(addr & 7, &mut *self.mapper),
            0x4016 => self.controllers[0].read() | (self.open_bus & 0xe0),
            0x4017 => self.controllers[1].read() | (self.open_bus & 0xe0),
            0x4000..=0x4015 | 0x4018..=0x401f => self.open_bus, // APU/test: TODO
            0x4020..=0xffff => self.mapper.cpu_read(addr),
        };
        self.open_bus = val;
        val
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.tick();
        self.open_bus = val;
        match addr {
            0x0000..=0x1fff => self.ram[(addr & 0x07ff) as usize] = val,
            0x2000..=0x3fff => self.ppu.write_register(addr & 7, val, &mut *self.mapper),
            0x4014 => self.oam_dma(val),
            0x4016 => {
                let strobe = val & 1 != 0;
                self.controllers[0].set_strobe(strobe);
                self.controllers[1].set_strobe(strobe);
            }
            0x4000..=0x4013 | 0x4015 | 0x4017..=0x401f => {} // APU/test: TODO
            0x4020..=0xffff => self.mapper.cpu_write(addr, val),
        }
    }

    fn nmi(&self) -> bool {
        self.ppu.nmi_line()
    }

    fn irq(&self) -> bool {
        false // APU frame IRQ / DMC / mapper IRQ: TODO
    }
}

impl SaveState for Bus {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.ram);
        self.ppu.save(w);
        self.apu.save(w);
        self.controllers[0].save(w);
        self.controllers[1].save(w);
        self.mapper.save(w);
        w.u8(self.open_bus);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.ram)?;
        self.ppu.load(r)?;
        self.apu.load(r)?;
        self.controllers[0].load(r)?;
        self.controllers[1].load(r)?;
        self.mapper.load(r)?;
        self.open_bus = r.u8()?;
        Ok(())
    }
}
