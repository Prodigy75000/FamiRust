// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! 2C02 Picture Processing Unit — dot-accurate.
//!
//! Implemented from `docs/notes/ppu-2c02.md` (clean-room notes distilled from the
//! NESdev PPU reference pages), never from other emulator source.
//!
//! One `tick()` == one PPU dot == one pixel; the system clocks three dots per CPU
//! cycle. The PPU owns its 2 KiB CIRAM (nametables), 32-byte palette, and 256-byte
//! OAM; pattern-table (CHR) accesses and nametable mirroring go through the
//! cartridge [`Mapper`]. This revision implements the register interface, the
//! loopy v/t/x/w scroll registers, the background rendering pipeline, frame
//! timing, and vblank/NMI. Sprite rendering + the exact sprite-0/overflow and
//! $2002-race timing land next.

use crate::cart::{Mapper, Mirroring};
use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

pub const FRAME_W: usize = 256;
pub const FRAME_H: usize = 240;
/// Debug tile-sheet dimensions (both pattern tables, 16 tiles wide).
pub const DBG_TILES_W: usize = 128;
pub const DBG_TILES_H: usize = 256;

/// 64-entry NTSC master palette as ARGB8888. This is the FirebrandX
/// "unsaturated-v6" decode (measured 2C02 composite colors), the de-facto
/// reference palette shipped as the default in Mesen and others — reference
/// color data, not emulator logic. Chosen so hues match the mainstream
/// emulators our users compare against (purples read as purple, not magenta).
#[rustfmt::skip]
const MASTER_PALETTE: [u32; 64] = [
    0x666666, 0x002a88, 0x1412a7, 0x3b00a4, 0x5c007e, 0x6e0040, 0x6c0600, 0x561d00,
    0x333500, 0x0b4800, 0x005200, 0x004f08, 0x00404d, 0x000000, 0x000000, 0x000000,
    0xadadad, 0x155fd9, 0x4240ff, 0x7527fe, 0xa01acc, 0xb71e7b, 0xb53120, 0x994e00,
    0x6b6d00, 0x388700, 0x0c9300, 0x008f32, 0x007c8d, 0x000000, 0x000000, 0x000000,
    0xfffeff, 0x64b0ff, 0x9290ff, 0xc676ff, 0xf36aff, 0xfe6ecc, 0xfe8170, 0xea9e22,
    0xbcbe00, 0x88d800, 0x5ce430, 0x45e082, 0x48cdde, 0x4f4f4f, 0x000000, 0x000000,
    0xfffeff, 0xc0dfff, 0xd3d2ff, 0xe8c8ff, 0xfbc2ff, 0xfec4ea, 0xfeccc5, 0xf7d8a5,
    0xe4e594, 0xcfef96, 0xbdf4ab, 0xb3f3cc, 0xb5ebf2, 0xb8b8b8, 0x000000, 0x000000,
];

pub struct Ppu {
    // ---- memory ----
    /// 2 KiB console nametable RAM (CIRAM); mirroring maps the 4 logical tables.
    pub ciram: [u8; 0x800],
    /// 32-byte palette RAM ($3F00-$3F1F).
    pub palette: [u8; 0x20],
    /// 256-byte primary OAM (64 sprites x 4 bytes).
    pub oam: [u8; 0x100],
    /// 32-byte secondary OAM (the up-to-8 sprites selected for the next line).
    pub secondary_oam: [u8; 0x20],

    // ---- external register file ($2000-$2007) ----
    pub ctrl: u8,   // PPUCTRL
    pub mask: u8,   // PPUMASK
    pub status: u8, // PPUSTATUS (bits 7,6,5 meaningful)
    pub oam_addr: u8,

    // ---- internal "loopy" registers ----
    pub v: u16,       // current VRAM address (15 bits)
    pub t: u16,       // temporary VRAM address (15 bits)
    pub x_fine: u8,   // fine X (3 bits)
    pub w: bool,      // write toggle (shared $2005/$2006)
    pub read_buffer: u8, // $2007 buffered-read latch
    pub io_bus: u8,   // open-bus / decay latch for the PPU I/O bus

    // ---- timing ----
    pub scanline: u16, // 0..=261 (261 = pre-render)
    pub dot: u16,      // 0..=340
    pub frame_odd: bool,
    pub frame: u64,
    /// Set true at (241,1); the host polls it to grab a completed frame.
    pub frame_complete: bool,
    /// True during the CPU cycle in which the vblank flag was set (at 241,1);
    /// a coincident $2002 read consumes it to suppress the flag + NMI.
    vbl_just_set: bool,
    /// Symmetric: true on the dot the vblank flag is cleared (261,1); a coincident
    /// $2002 read still returns the flag as set (the pre-clear value).
    vbl_just_cleared: bool,
    /// One-tick-delayed NMI line level (see [`Ppu::nmi_line`]).
    nmi_delayed: bool,
    /// Debug: scanline at which sprite-0 hit was set this frame (-1 = none yet).
    pub dbg_s0_scanline: i32,
    /// Debug: dot at which sprite-0 hit was set this frame (-1 = none yet).
    pub dbg_s0_dot: i32,

    // ---- background fetch pipeline ----
    bg_next_id: u8,
    bg_next_attr: u8,
    bg_next_lo: u8,
    bg_next_hi: u8,
    bg_shift_lo: u16,
    bg_shift_hi: u16,
    bg_attr_lo: u16,
    bg_attr_hi: u16,

    // ---- sprite output units for the line currently being drawn ----
    sprite_count: u8,
    sprite_pat_lo: [u8; 8],
    sprite_pat_hi: [u8; 8],
    sprite_attr: [u8; 8],
    sprite_x: [u8; 8],
    /// True when OAM sprite 0 is among this line's sprites (in slot 0).
    sprite_zero_present: bool,

    /// ARGB8888 framebuffer, handed to the frontend each completed frame.
    pub framebuffer: Vec<u32>,

    // --- debug harness (not serialized; runtime-only) ---
    /// Which layers to composite into the visible framebuffer: bit0=BG, bit1=OBJ.
    /// Default 0b11 (both) => identical to normal rendering. Sprite-0 hit still
    /// uses the real pixels, so toggling never changes game logic.
    pub dbg_layer_mask: u8,
    /// When set, each rendered pixel is also written to the per-layer buffers.
    pub dbg_capture: bool,
    /// BG-only frame (backdrop where transparent) and OBJ-only frame (magenta
    /// where transparent), captured during rendering when `dbg_capture`.
    pub bg_layer: Vec<u32>,
    pub obj_layer: Vec<u32>,
    /// Dot at which the second $2006 write last set `v` (−1 = none this scanline).
    /// Used to make a $2006 write coincident with the dot-256 vertical increment
    /// "win" over the increment, as on hardware (fixes mid-frame split jitter).
    v_write_dot: i32,
    /// Scanline of that write, so the guard only applies on the same line.
    v_write_line: u16,
}

impl Default for Ppu {
    fn default() -> Self {
        Ppu {
            ciram: [0; 0x800],
            palette: [0; 0x20],
            oam: [0; 0x100],
            secondary_oam: [0xff; 0x20],
            ctrl: 0,
            mask: 0,
            status: 0,
            oam_addr: 0,
            v: 0,
            t: 0,
            x_fine: 0,
            w: false,
            read_buffer: 0,
            io_bus: 0,
            scanline: 261, // start on the pre-render line
            dot: 0,
            frame_odd: false,
            frame: 0,
            frame_complete: false,
            vbl_just_set: false,
            vbl_just_cleared: false,
            nmi_delayed: false,
            dbg_s0_scanline: -1,
            dbg_s0_dot: -1,
            bg_next_id: 0,
            bg_next_attr: 0,
            bg_next_lo: 0,
            bg_next_hi: 0,
            bg_shift_lo: 0,
            bg_shift_hi: 0,
            bg_attr_lo: 0,
            bg_attr_hi: 0,
            sprite_count: 0,
            sprite_pat_lo: [0; 8],
            sprite_pat_hi: [0; 8],
            sprite_attr: [0; 8],
            sprite_x: [0; 8],
            sprite_zero_present: false,
            framebuffer: vec![0; FRAME_W * FRAME_H],
            dbg_layer_mask: 0b11,
            dbg_capture: false,
            bg_layer: vec![0; FRAME_W * FRAME_H],
            obj_layer: vec![0; FRAME_W * FRAME_H],
            v_write_dot: -1,
            v_write_line: 0,
        }
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn rendering_enabled(&self) -> bool {
        self.mask & 0x18 != 0 // background or sprite enable
    }

    /// NMI line to the CPU. The 2C02 asserts /NMI one PPU cycle after the vblank
    /// flag rises, so we expose a one-tick-delayed level (`nmi_delayed`).
    #[inline]
    pub fn nmi_line(&self) -> bool {
        self.nmi_delayed
    }

    #[inline]
    fn nmi_condition(&self) -> bool {
        (self.ctrl & 0x80 != 0) && (self.status & 0x80 != 0)
    }

    /// Called by the bus at the start of each CPU cycle, before that cycle's PPU
    /// dots are clocked, so `vbl_just_set` reflects only this cycle's dots.
    #[inline]
    pub fn begin_cpu_cycle(&mut self) {
        self.vbl_just_set = false;
    }

    // ---------------- PPU-internal memory access ----------------

    /// Map a nametable address ($2000-$3EFF) to a CIRAM byte index. The mapper may
    /// override the CIRAM bank per nametable (TxSROM); otherwise it's fixed by the
    /// mirroring mode.
    #[inline]
    fn ciram_index(&self, addr: u16, mapper: &dyn Mapper) -> usize {
        let addr = addr & 0x2fff; // fold $3000-$3EFF onto $2000-$2EFF
        let slot = (addr >> 10) & 0x3; // which logical nametable (0..3)
        let bank: u16 = if let Some(b) = mapper.nt_ciram_bank(slot as usize) {
            (b & 1) as u16
        } else {
            match mapper.mirroring() {
                Mirroring::Horizontal => (slot >> 1) & 1, // 0,1->A ; 2,3->B
                Mirroring::Vertical => slot & 1,          // 0,2->A ; 1,3->B
                Mirroring::SingleScreenA => 0,
                Mirroring::SingleScreenB => 1,
                // Four-screen would need cartridge VRAM; approximate with vertical.
                Mirroring::FourScreen => slot & 1,
            }
        };
        (bank as usize) * 0x400 + (addr as usize & 0x3ff)
    }

    /// Palette RAM index with the $3F10/$14/$18/$1C -> $3F00/$04/$08/$0C aliasing.
    #[inline]
    fn palette_index(addr: u16) -> usize {
        let mut i = (addr & 0x1f) as usize;
        if i >= 0x10 && (i & 0x03) == 0 {
            i -= 0x10;
        }
        i
    }

    fn mem_read(&self, addr: u16, mapper: &mut dyn Mapper) -> u8 {
        match addr & 0x3fff {
            0x0000..=0x1fff => mapper.ppu_read(addr & 0x1fff),
            0x2000..=0x3eff => self.ciram[self.ciram_index(addr, &*mapper)],
            0x3f00..=0x3fff => self.palette[Self::palette_index(addr)],
            _ => unreachable!(),
        }
    }

    fn mem_write(&mut self, addr: u16, val: u8, mapper: &mut dyn Mapper) {
        match addr & 0x3fff {
            0x0000..=0x1fff => mapper.ppu_write(addr & 0x1fff, val),
            0x2000..=0x3eff => {
                let i = self.ciram_index(addr, &*mapper);
                self.ciram[i] = val;
            }
            0x3f00..=0x3fff => self.palette[Self::palette_index(addr)] = val & 0x3f,
            _ => unreachable!(),
        }
    }

    // ---------------- CPU-facing register interface ----------------

    /// CPU read of $2000-$2007 (already reduced to the low 3 bits by the bus).
    pub fn read_register(&mut self, reg: u16, mapper: &mut dyn Mapper) -> u8 {
        match reg & 7 {
            2 => {
                // PPUSTATUS: top 3 bits are flags, low 5 are open bus. Reading
                // clears vblank and resets the write toggle.
                let mut status = self.status;
                // VBlank read race: this read samples at the END of the CPU cycle
                // (dots already clocked). If the flag was set during THIS cycle's
                // dots (`vbl_just_set`), the read is coincident: it returns 0 and
                // the flag + NMI are suppressed for this frame.
                if self.vbl_just_set {
                    status &= 0x7f;
                    self.nmi_delayed = false;
                }
                // Coincident with the clear: the read still sees the flag set.
                if self.vbl_just_cleared {
                    status |= 0x80;
                }
                let v = (status & 0xe0) | (self.io_bus & 0x1f);
                self.status &= 0x7f; // clear vblank
                self.w = false;
                self.io_bus = v;
                v
            }
            4 => {
                // OAMDATA: attribute byte bits 2-4 read back as 0.
                let mut v = self.oam[self.oam_addr as usize];
                if self.oam_addr & 3 == 2 {
                    v &= 0xe3;
                }
                self.io_bus = v;
                v
            }
            7 => {
                // PPUDATA buffered read.
                let addr = self.v & 0x3fff;
                let result = if addr >= 0x3f00 {
                    // Palette: return immediately (with open-bus top 2 bits), and
                    // load the buffer with the nametable byte "underneath".
                    self.read_buffer = self.mem_read(addr - 0x1000, mapper);
                    (self.palette[Self::palette_index(addr)] & 0x3f) | (self.io_bus & 0xc0)
                } else {
                    let buffered = self.read_buffer;
                    self.read_buffer = self.mem_read(addr, mapper);
                    buffered
                };
                self.increment_v();
                self.io_bus = result;
                result
            }
            _ => self.io_bus, // write-only registers read as open bus
        }
    }

    /// CPU write of $2000-$2007.
    pub fn write_register(&mut self, reg: u16, val: u8, mapper: &mut dyn Mapper) {
        self.io_bus = val;
        match reg & 7 {
            0 => {
                // PPUCTRL: nametable select -> t bits 10-11.
                self.ctrl = val;
                self.t = (self.t & 0xf3ff) | (((val as u16) & 0x03) << 10);
                mapper.ppu_ctrl(val); // MMC5 tracks the 8x16-sprite bit
            }
            1 => self.mask = val,
            3 => self.oam_addr = val,
            4 => {
                // OAMDATA write: store and post-increment OAMADDR.
                self.oam[self.oam_addr as usize] = val;
                self.oam_addr = self.oam_addr.wrapping_add(1);
            }
            5 => {
                // PPUSCROLL (x2).
                if !self.w {
                    self.t = (self.t & 0xffe0) | ((val as u16) >> 3);
                    self.x_fine = val & 0x07;
                    self.w = true;
                } else {
                    self.t = (self.t & 0x8c1f)
                        | (((val as u16) & 0x07) << 12)
                        | (((val as u16) & 0xf8) << 2);
                    self.w = false;
                }
            }
            6 => {
                // PPUADDR (x2).
                if !self.w {
                    self.t = (self.t & 0x00ff) | (((val as u16) & 0x3f) << 8);
                    self.w = true;
                } else {
                    self.t = (self.t & 0xff00) | val as u16;
                    self.v = self.t;
                    self.w = false;
                    // Remember this write so a coincident dot-256 Y-increment does
                    // not clobber it (hardware: the write wins). `self.dot` here is
                    // the dot AFTER this CPU cycle's three PPU ticks.
                    self.v_write_dot = self.dot as i32;
                    self.v_write_line = self.scanline;
                }
            }
            7 => {
                // PPUDATA write.
                let addr = self.v & 0x3fff;
                self.mem_write(addr, val, mapper);
                self.increment_v();
            }
            _ => {}
        }
    }

    /// Direct OAM write used by $4014 OAM DMA (bus-driven).
    #[inline]
    pub fn oam_dma_write(&mut self, val: u8) {
        self.oam[self.oam_addr as usize] = val;
        self.oam_addr = self.oam_addr.wrapping_add(1);
    }

    #[inline]
    fn increment_v(&mut self) {
        let step = if self.ctrl & 0x04 != 0 { 32 } else { 1 };
        self.v = self.v.wrapping_add(step) & 0x7fff;
    }

    // ---------------- scroll increment / copy routines ----------------

    fn increment_coarse_x(&mut self) {
        if self.v & 0x001f == 0x001f {
            self.v &= !0x001f;
            self.v ^= 0x0400; // flip horizontal nametable
        } else {
            self.v += 1;
        }
    }

    fn increment_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v += 0x1000; // fine Y++
        } else {
            self.v &= !0x7000; // fine Y = 0
            let mut y = (self.v & 0x03e0) >> 5;
            if y == 29 {
                y = 0;
                self.v ^= 0x0800; // flip vertical nametable
            } else if y == 31 {
                y = 0;
            } else {
                y += 1;
            }
            self.v = (self.v & !0x03e0) | (y << 5);
        }
    }

    #[inline]
    fn copy_horizontal(&mut self) {
        self.v = (self.v & !0x041f) | (self.t & 0x041f);
    }
    #[inline]
    fn copy_vertical(&mut self) {
        self.v = (self.v & !0x7be0) | (self.t & 0x7be0);
    }

    // ---------------- background pipeline ----------------

    #[inline]
    fn load_shifters(&mut self) {
        self.bg_shift_lo = (self.bg_shift_lo & 0xff00) | self.bg_next_lo as u16;
        self.bg_shift_hi = (self.bg_shift_hi & 0xff00) | self.bg_next_hi as u16;
        // Attribute bits are constant across the tile's 8 pixels; expand to 8x1.
        self.bg_attr_lo = (self.bg_attr_lo & 0xff00) | if self.bg_next_attr & 1 != 0 { 0xff } else { 0 };
        self.bg_attr_hi = (self.bg_attr_hi & 0xff00) | if self.bg_next_attr & 2 != 0 { 0xff } else { 0 };
    }

    #[inline]
    fn shift_bg(&mut self) {
        self.bg_shift_lo <<= 1;
        self.bg_shift_hi <<= 1;
        self.bg_attr_lo <<= 1;
        self.bg_attr_hi <<= 1;
    }

    fn fetch_bg(&mut self, mapper: &mut dyn Mapper) {
        match (self.dot - 1) & 7 {
            0 => {
                self.load_shifters();
                self.bg_next_id = self.mem_read(0x2000 | (self.v & 0x0fff), mapper);
            }
            2 => {
                // MMC5 extended-attribute mode overrides the palette per tile.
                let nt_tile = self.v & 0x03ff;
                self.bg_next_attr = match mapper.ppu_ext_attr(nt_tile) {
                    Some(pal) => pal,
                    None => {
                        let addr = 0x23c0
                            | (self.v & 0x0c00)
                            | ((self.v >> 4) & 0x38)
                            | ((self.v >> 2) & 0x07);
                        let attr = self.mem_read(addr, mapper);
                        let shift = ((self.v >> 4) & 4) | (self.v & 2);
                        (attr >> shift) & 0x3
                    }
                };
            }
            4 => {
                let fine_y = (self.v >> 12) & 7;
                let nt_tile = self.v & 0x03ff;
                self.bg_next_lo = match mapper.ppu_ext_pattern(nt_tile, self.bg_next_id, fine_y, false) {
                    Some(b) => b,
                    None => {
                        let base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0 };
                        self.mem_read(base + (self.bg_next_id as u16) * 16 + fine_y, mapper)
                    }
                };
            }
            6 => {
                let fine_y = (self.v >> 12) & 7;
                let nt_tile = self.v & 0x03ff;
                self.bg_next_hi = match mapper.ppu_ext_pattern(nt_tile, self.bg_next_id, fine_y, true) {
                    Some(b) => b,
                    None => {
                        let base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0 };
                        self.mem_read(base + (self.bg_next_id as u16) * 16 + fine_y + 8, mapper)
                    }
                };
            }
            7 => self.increment_coarse_x(),
            _ => {}
        }
    }

    // ---------------- sprite pipeline ----------------

    /// Evaluate primary OAM for the NEXT scanline into secondary OAM (up to 8),
    /// then fetch each selected sprite's pattern bytes into the output units.
    /// Performed at dot 257 (the hardware evaluates on line L for line L+1).
    fn evaluate_sprites(&mut self, mapper: &mut dyn Mapper) {
        self.secondary_oam = [0xff; 0x20];
        self.sprite_count = 0;
        self.sprite_zero_present = false;
        let height: i16 = if self.ctrl & 0x20 != 0 { 16 } else { 8 };
        // Evaluate against the CURRENT scanline (pre-render = -1); the selected
        // sprites are drawn on the NEXT line. So a sprite at OAM Y=y first
        // appears on line y+1 (row = eval_line - y).
        let eval_line: i16 = if self.scanline == 261 { -1 } else { self.scanline as i16 };

        for n in 0..64 {
            let y = self.oam[n * 4] as i16;
            let diff = eval_line - y;
            if diff >= 0 && diff < height {
                if self.sprite_count < 8 {
                    let s = self.sprite_count as usize;
                    self.secondary_oam[s * 4..s * 4 + 4]
                        .copy_from_slice(&self.oam[n * 4..n * 4 + 4]);
                    if n == 0 {
                        self.sprite_zero_present = true;
                    }
                    self.sprite_count += 1;
                } else {
                    // 9th in-range sprite -> overflow. (The hardware's buggy
                    // diagonal scan is modeled in stage 4; this is the basic set.)
                    self.status |= 0x20;
                    break;
                }
            }
        }

        // Fetch pattern bytes for ALL 8 sprite slots every scanline. Slots beyond
        // sprite_count are dummy fetches (tile $FF) that render nothing but keep
        // PPU A12 toggling every scanline — the MMC3 scanline IRQ counts on that.
        for i in 0..8 {
            if i < self.sprite_count as usize {
                let y = self.secondary_oam[i * 4] as i16;
                let tile = self.secondary_oam[i * 4 + 1];
                let attr = self.secondary_oam[i * 4 + 2];
                let flip_v = attr & 0x80 != 0;
                let mut row = (eval_line - y) as u16;

                let addr = if height == 8 {
                    let base = if self.ctrl & 0x08 != 0 { 0x1000 } else { 0 };
                    if flip_v {
                        row = 7 - row;
                    }
                    base + (tile as u16) * 16 + row
                } else {
                    // 8x16: tile bit0 selects table; halves are tile&0xFE / |1.
                    let base = ((tile & 1) as u16) * 0x1000;
                    let mut t = (tile & 0xfe) as u16;
                    if flip_v {
                        row = 15 - row;
                    }
                    if row >= 8 {
                        t += 1;
                        row -= 8;
                    }
                    base + t * 16 + row
                };

                let mut lo = mapper.ppu_read_sprite(addr & 0x1fff);
                let mut hi = mapper.ppu_read_sprite((addr + 8) & 0x1fff);
                if attr & 0x40 != 0 {
                    lo = lo.reverse_bits();
                    hi = hi.reverse_bits();
                }
                self.sprite_pat_lo[i] = lo;
                self.sprite_pat_hi[i] = hi;
                self.sprite_attr[i] = attr;
                self.sprite_x[i] = self.secondary_oam[i * 4 + 3];
            } else {
                // Empty slot: dummy fetch of tile $FF from the sprite pattern
                // table (result discarded; only the bus access matters for A12).
                let base = if height == 16 || self.ctrl & 0x08 != 0 { 0x1000 } else { 0x0000 };
                let addr = base | 0x0ff0;
                mapper.ppu_read_sprite(addr & 0x1fff);
                mapper.ppu_read_sprite((addr + 8) & 0x1fff);
            }
        }
    }

    /// The sprite candidate for screen x: returns (pattern, palette, behind_bg,
    /// is_sprite0). The first (lowest-index) opaque sprite wins.
    fn sprite_pixel(&self, x: usize) -> (u8, u8, bool, bool) {
        if self.mask & 0x10 == 0 || (x < 8 && self.mask & 0x04 == 0) {
            return (0, 0, false, false);
        }
        for i in 0..self.sprite_count as usize {
            let dx = x as i16 - self.sprite_x[i] as i16;
            if dx < 0 || dx >= 8 {
                continue;
            }
            let bit = 7 - dx as u8;
            let p0 = (self.sprite_pat_lo[i] >> bit) & 1;
            let p1 = (self.sprite_pat_hi[i] >> bit) & 1;
            let pattern = (p1 << 1) | p0;
            if pattern != 0 {
                let attr = self.sprite_attr[i];
                return (pattern, attr & 0x03, attr & 0x20 != 0, self.sprite_zero_present && i == 0);
            }
        }
        (0, 0, false, false)
    }

    /// Compose and store the pixel for the current visible dot (x = dot-1).
    fn render_pixel(&mut self, mapper: &mut dyn Mapper) {
        let x = (self.dot - 1) as usize;
        let y = self.scanline as usize;

        let mut bg_pixel = 0u8;
        let mut bg_palette = 0u8;
        if self.mask & 0x08 != 0 && !(x < 8 && self.mask & 0x02 == 0) {
            let bit = 0x8000u16 >> self.x_fine;
            let p0 = ((self.bg_shift_lo & bit) != 0) as u8;
            let p1 = ((self.bg_shift_hi & bit) != 0) as u8;
            bg_pixel = (p1 << 1) | p0;
            let a0 = ((self.bg_attr_lo & bit) != 0) as u8;
            let a1 = ((self.bg_attr_hi & bit) != 0) as u8;
            bg_palette = (a1 << 1) | a0;
        }

        let (sp_pixel, sp_palette, sp_behind, is_sprite0) = self.sprite_pixel(x);

        // Sprite-0 hit is hardware behavior on the REAL pixels -- independent of
        // any debug layer toggle, so game logic never changes when a layer is off.
        if bg_pixel != 0 && sp_pixel != 0 && is_sprite0 && x != 255 {
            if self.status & 0x40 == 0 {
                self.dbg_s0_scanline = self.scanline as i32;
                self.dbg_s0_dot = self.dot as i32;
            }
            self.status |= 0x40;
        }

        // Debug per-layer capture: the raw BG-only and OBJ-only images (ignores
        // the layer mask, so each layer is viewable independently).
        if self.dbg_capture {
            let i = y * FRAME_W + x;
            let bg_idx = self.mem_read(0x3f00 | ((bg_palette as u16) << 2) | bg_pixel as u16, mapper) & 0x3f;
            self.bg_layer[i] = 0xff00_0000 | MASTER_PALETTE[bg_idx as usize];
            self.obj_layer[i] = if sp_pixel != 0 {
                let si = self.mem_read(0x3f10 | ((sp_palette as u16) << 2) | sp_pixel as u16, mapper) & 0x3f;
                0xff00_0000 | MASTER_PALETTE[si as usize]
            } else {
                0xffff_00ff // magenta = transparent
            };
        }

        // Composite honoring the debug layer mask (default 0b11 => normal).
        let bg_show = if self.dbg_layer_mask & 1 != 0 { bg_pixel } else { 0 };
        let sp_show = if self.dbg_layer_mask & 2 != 0 { sp_pixel } else { 0 };
        let pal_addr = match (bg_show != 0, sp_show != 0) {
            (false, false) => 0x3f00, // backdrop
            (false, true) => 0x3f10 | ((sp_palette as u16) << 2) | sp_show as u16,
            (true, false) => 0x3f00 | ((bg_palette as u16) << 2) | bg_show as u16,
            (true, true) => {
                if sp_behind {
                    0x3f00 | ((bg_palette as u16) << 2) | bg_show as u16
                } else {
                    0x3f10 | ((sp_palette as u16) << 2) | sp_show as u16
                }
            }
        };
        let mut index = self.mem_read(pal_addr, mapper) & 0x3f;
        if self.mask & 0x01 != 0 {
            index &= 0x30; // greyscale
        }
        self.framebuffer[y * FRAME_W + x] = 0xff00_0000 | MASTER_PALETTE[index as usize];
    }

    // ---------------- the per-dot clock ----------------

    /// Advance one PPU dot. `mapper` supplies CHR + mirroring.
    // ---- debug harness emitters (see docs/DEBUG_HARNESS.md) ----

    /// OAM as a JSON array of the 64 sprites, for the on-device inspector.
    pub fn dbg_oam_json(&self) -> String {
        let tall = self.ctrl & 0x20 != 0;
        let mut s = String::with_capacity(64 * 96);
        s.push('[');
        for i in 0..64 {
            let b = i * 4;
            let (y, tile, attr, x) = (self.oam[b], self.oam[b + 1], self.oam[b + 2], self.oam[b + 3]);
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"index\":{i},\"x\":{x},\"y\":{y},\"tile\":{tile},\"palette\":{},\"priority\":{},\"flipH\":{},\"flipV\":{},\"size\":\"{}\"}}",
                attr & 3,
                (attr >> 5) & 1,
                (attr >> 6) & 1,
                (attr >> 7) & 1,
                if tall { "8x16" } else { "8x8" },
            ));
        }
        s.push(']');
        s
    }

    /// The 32 palette entries ($3F00-$3F1F) as ARGB8888 (16 BG + 16 sprite).
    pub fn dbg_palette_argb(&self) -> [u32; 32] {
        let mut out = [0u32; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = 0xff00_0000 | MASTER_PALETTE[(self.palette[i] & 0x3f) as usize];
        }
        out
    }

    /// Both pattern tables (512 tiles) as a 128x256 ARGB sheet, 16 tiles wide,
    /// coloured with BG palette 0. `DBG_TILES_W`/`_H` describe the dimensions.
    pub fn dbg_tiles_argb(&self, mapper: &mut dyn Mapper) -> Vec<u32> {
        const COLS: usize = 16;
        let w = COLS * 8; // 128
        let h = (512 / COLS) * 8; // 256
        let mut out = vec![0xff00_0000u32; w * h];
        let pal = [self.palette[0], self.palette[1], self.palette[2], self.palette[3]];
        for t in 0..512usize {
            let base = (t as u16) * 16;
            let (tx, ty) = ((t % COLS) * 8, (t / COLS) * 8);
            for row in 0..8u16 {
                let lo = mapper.ppu_read(base + row);
                let hi = mapper.ppu_read(base + row + 8);
                for col in 0..8usize {
                    let bit = 7 - col;
                    let p = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
                    let ci = (pal[p as usize] & 0x3f) as usize;
                    out[(ty + row as usize) * w + tx + col] = 0xff00_0000 | MASTER_PALETTE[ci];
                }
            }
        }
        out
    }

    pub fn tick(&mut self, mapper: &mut dyn Mapper) {
        // `vbl_just_set` is a one-dot flag: it is true only immediately after the
        // dot that set the vblank flag, so a $2002 read (which samples after this
        // cycle's last dot) is "coincident" only when the set landed on that dot.
        self.vbl_just_set = false;
        self.vbl_just_cleared = false;
        let rendering = self.rendering_enabled();
        let visible = self.scanline < 240;
        let prerender = self.scanline == 261;

        // Notify the mapper at the start of each scanline (MMC5 scanline IRQ).
        if self.dot == 0 {
            mapper.ppu_scanline(self.scanline, rendering);
        }

        if (visible || prerender) && rendering {
            // Background fetch + shift on the fetch dots.
            if (self.dot >= 1 && self.dot <= 256) || (self.dot >= 321 && self.dot <= 336) {
                self.shift_bg();
                self.fetch_bg(mapper);
            }
            if self.dot == 256 {
                // A $2006 write that landed on this exact dot (this scanline) has
                // just set `v` for the split; the coincident increment must not
                // re-bump it (matches hardware — fixes Bart/Micro Machines status-
                // bar jitter). Otherwise the vertical position advances normally.
                let coincident_write =
                    self.v_write_line == self.scanline && self.v_write_dot == 256;
                if !coincident_write {
                    self.increment_y();
                }
            }
            if self.dot == 257 {
                self.v_write_dot = -1; // consumed; do not carry to the next line
                self.load_shifters();
                self.copy_horizontal();
                // Evaluate + fetch sprites for the next scanline.
                self.evaluate_sprites(mapper);
            }
            // Dummy nametable fetches at 337 & 339 (feed mapper A12 watchers).
            if self.dot == 338 || self.dot == 340 {
                self.bg_next_id = self.mem_read(0x2000 | (self.v & 0x0fff), mapper);
            }
            if prerender && self.dot >= 280 && self.dot <= 304 {
                self.copy_vertical();
            }
        }

        if visible && self.dot >= 1 && self.dot <= 256 {
            self.render_pixel(mapper);
        }

        // VBlank set / clear.
        if self.scanline == 241 && self.dot == 1 {
            self.status |= 0x80; // vblank
            self.vbl_just_set = true; // consumed by a coincident $2002 read
            self.frame_complete = true;
        }
        if prerender && self.dot == 1 {
            self.status &= !0xe0; // clear vblank, sprite-0, overflow
            self.vbl_just_cleared = true;
            self.dbg_s0_scanline = -1;
            self.dbg_s0_dot = -1;
        }

        // Propagate the NMI line with a one-tick delay.
        self.nmi_delayed = self.nmi_condition();

        self.advance(rendering);
    }

    fn advance(&mut self, rendering: bool) {
        // Odd-frame dot skip: on the pre-render line of odd frames with rendering
        // on, (261,339) is followed directly by (0,0) of the next frame.
        if self.scanline == 261 && self.dot == 339 && self.frame_odd && rendering {
            self.dot = 0;
            self.scanline = 0;
            self.frame_odd = !self.frame_odd;
            self.frame = self.frame.wrapping_add(1);
            return;
        }
        self.dot += 1;
        if self.dot > 340 {
            self.dot = 0;
            self.scanline += 1;
            if self.scanline > 261 {
                self.scanline = 0;
                self.frame_odd = !self.frame_odd;
                self.frame = self.frame.wrapping_add(1);
            }
        }
    }
}

impl SaveState for Ppu {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.ciram);
        w.bytes(&self.palette);
        w.bytes(&self.oam);
        w.bytes(&self.secondary_oam);
        w.u8(self.ctrl);
        w.u8(self.mask);
        w.u8(self.status);
        w.u8(self.oam_addr);
        w.u16(self.v);
        w.u16(self.t);
        w.u8(self.x_fine);
        w.bool(self.w);
        w.u8(self.read_buffer);
        w.u8(self.io_bus);
        w.u16(self.scanline);
        w.u16(self.dot);
        w.bool(self.frame_odd);
        w.u64(self.frame);
        w.bool(self.frame_complete);
        w.bool(self.vbl_just_set);
        w.bool(self.vbl_just_cleared);
        w.bool(self.nmi_delayed);
        w.u8(self.bg_next_id);
        w.u8(self.bg_next_attr);
        w.u8(self.bg_next_lo);
        w.u8(self.bg_next_hi);
        w.u16(self.bg_shift_lo);
        w.u16(self.bg_shift_hi);
        w.u16(self.bg_attr_lo);
        w.u16(self.bg_attr_hi);
        w.u8(self.sprite_count);
        w.bytes(&self.sprite_pat_lo);
        w.bytes(&self.sprite_pat_hi);
        w.bytes(&self.sprite_attr);
        w.bytes(&self.sprite_x);
        w.bool(self.sprite_zero_present);
        // Framebuffer is pure output, reconstructed by rendering — not saved.
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.ciram)?;
        r.bytes(&mut self.palette)?;
        r.bytes(&mut self.oam)?;
        r.bytes(&mut self.secondary_oam)?;
        self.ctrl = r.u8()?;
        self.mask = r.u8()?;
        self.status = r.u8()?;
        self.oam_addr = r.u8()?;
        self.v = r.u16()?;
        self.t = r.u16()?;
        self.x_fine = r.u8()?;
        self.w = r.bool()?;
        self.read_buffer = r.u8()?;
        self.io_bus = r.u8()?;
        self.scanline = r.u16()?;
        self.dot = r.u16()?;
        self.frame_odd = r.bool()?;
        self.frame = r.u64()?;
        self.frame_complete = r.bool()?;
        self.vbl_just_set = r.bool()?;
        self.vbl_just_cleared = r.bool()?;
        self.nmi_delayed = r.bool()?;
        self.bg_next_id = r.u8()?;
        self.bg_next_attr = r.u8()?;
        self.bg_next_lo = r.u8()?;
        self.bg_next_hi = r.u8()?;
        self.bg_shift_lo = r.u16()?;
        self.bg_shift_hi = r.u16()?;
        self.bg_attr_lo = r.u16()?;
        self.bg_attr_hi = r.u16()?;
        self.sprite_count = r.u8()?;
        r.bytes(&mut self.sprite_pat_lo)?;
        r.bytes(&mut self.sprite_pat_hi)?;
        r.bytes(&mut self.sprite_attr)?;
        r.bytes(&mut self.sprite_x)?;
        self.sprite_zero_present = r.bool()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart;

    /// A minimal NROM cart, just to satisfy `tick`'s CHR/mirroring needs.
    fn nrom_mapper() -> Box<dyn cart::Mapper> {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1; // 16 KiB PRG
        rom[5] = 1; // 8 KiB CHR
        rom.extend(std::iter::repeat(0u8).take(16 * 1024 + 8 * 1024));
        cart::make_mapper(cart::Cartridge::from_ines(&rom).unwrap()).unwrap()
    }

    /// Positions the PPU at (scanline 175, dot 256) with rendering on and fine Y 0,
    /// then performs the second $2006 write (v <- t) exactly as the bus would after
    /// that CPU cycle's three ticks, and ticks the coincident dot-256 once.
    fn run_split_write(coincident: bool) -> u16 {
        let mut ppu = Ppu::new();
        let mut mapper = nrom_mapper();
        ppu.mask = 0x08; // show background => rendering enabled
        ppu.scanline = 175;
        ppu.dot = 256;
        ppu.v = 0x72a6; // mid-render address, fine Y = 7
        ppu.t = 0x02c0; // target: coarse Y 22, fine Y 0 (the status-bar scroll)
        ppu.w = true; // next $2006 write is the second byte
        ppu.write_register(6, 0xc0, &mut *mapper); // v <- t = 0x02c0, records dot 256
        if !coincident {
            // Simulate the write having landed a couple dots earlier, so the dot-256
            // increment is *not* coincident and must apply normally.
            ppu.v_write_dot = -1;
        }
        ppu.tick(&mut *mapper); // processes dot 256 (the Y increment)
        (ppu.v >> 12) & 0x7 // resulting fine Y
    }

    /// Regression: a second $2006 write coincident with the dot-256 vertical
    /// increment must win — fine Y stays 0 (the value the game wrote). This is the
    /// Bart vs. the Space Mutants / Micro Machines status-bar jitter fix.
    #[test]
    fn dot256_coincident_2006_write_survives_y_increment() {
        assert_eq!(run_split_write(true), 0, "coincident write must not be re-incremented");
        // And with no coincident write, the dot-256 increment applies as usual.
        assert_eq!(run_split_write(false), 1, "non-coincident increment must still fire");
    }
}
