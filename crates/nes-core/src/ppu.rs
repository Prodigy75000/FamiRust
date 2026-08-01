//! 2C02 Picture Processing Unit.
//!
//! Stub: register file, palette/OAM/VRAM storage, and the save-state contract
//! are staged here so the bus can be wired end-to-end. The background/sprite
//! rendering pipeline and the exact 341-dot x 262-scanline timing (including the
//! odd-frame short scanline and the sprite-0 hit / overflow quirks) land after
//! the CPU passes conformance and can drive it.

use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

pub const FRAME_W: usize = 256;
pub const FRAME_H: usize = 240;

pub struct Ppu {
    /// 2 KiB of on-board nametable VRAM (mirroring is applied at access time).
    pub vram: [u8; 2048],
    /// 256 bytes of primary OAM (64 sprites x 4 bytes).
    pub oam: [u8; 256],
    /// 32-entry palette RAM at $3F00-$3F1F.
    pub palette: [u8; 32],
    /// Registers/latches that must survive a save (scroll/addr toggle, etc.).
    pub ctrl: u8,
    pub mask: u8,
    pub status: u8,
    pub oam_addr: u8,
    pub v: u16, // current VRAM address (15-bit "loopy v")
    pub t: u16, // temporary VRAM address ("loopy t")
    pub x_fine: u8,
    pub write_toggle: bool,
    pub read_buffer: u8,
    pub scanline: i16,
    pub dot: u16,
    pub frame: u64,
    /// ARGB8888 framebuffer handed to the frontend each completed frame.
    pub framebuffer: Vec<u32>,
}

impl Default for Ppu {
    fn default() -> Self {
        Ppu {
            vram: [0; 2048],
            oam: [0; 256],
            palette: [0; 32],
            ctrl: 0,
            mask: 0,
            status: 0,
            oam_addr: 0,
            v: 0,
            t: 0,
            x_fine: 0,
            write_toggle: false,
            read_buffer: 0,
            scanline: 0,
            dot: 0,
            frame: 0,
            framebuffer: vec![0; FRAME_W * FRAME_H],
        }
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SaveState for Ppu {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.vram);
        w.bytes(&self.oam);
        w.bytes(&self.palette);
        w.u8(self.ctrl);
        w.u8(self.mask);
        w.u8(self.status);
        w.u8(self.oam_addr);
        w.u16(self.v);
        w.u16(self.t);
        w.u8(self.x_fine);
        w.bool(self.write_toggle);
        w.u8(self.read_buffer);
        w.i16(self.scanline);
        w.u16(self.dot);
        w.u64(self.frame);
        // The framebuffer is pure output, reconstructed by rendering — never
        // part of the machine state that has to be byte-identical.
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.vram)?;
        r.bytes(&mut self.oam)?;
        r.bytes(&mut self.palette)?;
        self.ctrl = r.u8()?;
        self.mask = r.u8()?;
        self.status = r.u8()?;
        self.oam_addr = r.u8()?;
        self.v = r.u16()?;
        self.t = r.u16()?;
        self.x_fine = r.u8()?;
        self.write_toggle = r.bool()?;
        self.read_buffer = r.u8()?;
        self.scanline = r.i16()?;
        self.dot = r.u16()?;
        self.frame = r.u64()?;
        Ok(())
    }
}
