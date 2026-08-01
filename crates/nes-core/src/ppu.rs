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

/// 64-entry NTSC master palette as ARGB8888. A widely-used decoded 2C02 palette
/// (the wiki ships only downloadable `.pal` files); swappable for closer accuracy.
#[rustfmt::skip]
const MASTER_PALETTE: [u32; 64] = [
    0x7c7c7c, 0x0000fc, 0x0000bc, 0x4428bc, 0x940084, 0xa80020, 0xa81000, 0x881400,
    0x503000, 0x007800, 0x006800, 0x005800, 0x004058, 0x000000, 0x000000, 0x000000,
    0xbcbcbc, 0x0078f8, 0x0058f8, 0x6844fc, 0xd800cc, 0xe40058, 0xf83800, 0xe45c10,
    0xac7c00, 0x00b800, 0x00a800, 0x00a844, 0x008888, 0x000000, 0x000000, 0x000000,
    0xf8f8f8, 0x3cbcfc, 0x6888fc, 0x9878f8, 0xf878f8, 0xf85898, 0xf87858, 0xfca044,
    0xf8b800, 0xb8f818, 0x58d854, 0x58f898, 0x00e8d8, 0x787878, 0x000000, 0x000000,
    0xfcfcfc, 0xa4e4fc, 0xb8b8f8, 0xd8b8f8, 0xf8b8f8, 0xf8a4c0, 0xf0d0b0, 0xfce0a8,
    0xf8d878, 0xd8f878, 0xb8f8b8, 0xb8f8d8, 0x00fcfc, 0xf8d8f8, 0x000000, 0x000000,
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
    /// Set when $2002 is read in the race window just before/at the vblank set
    /// point; suppresses the flag + NMI for that frame.
    suppress_vbl: bool,
    /// One-tick-delayed NMI line level (see [`Ppu::nmi_line`]).
    nmi_delayed: bool,
    /// Debug: scanline at which sprite-0 hit was set this frame (-1 = none yet).
    pub dbg_s0_scanline: i32,

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
            suppress_vbl: false,
            nmi_delayed: false,
            dbg_s0_scanline: -1,
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

    // ---------------- PPU-internal memory access ----------------

    /// Map a nametable address ($2000-$3EFF) to a CIRAM byte index per mirroring.
    #[inline]
    fn ciram_index(&self, addr: u16, mirroring: Mirroring) -> usize {
        let addr = addr & 0x2fff; // fold $3000-$3EFF onto $2000-$2EFF
        let slot = (addr >> 10) & 0x3; // which logical nametable (0..3)
        let bank = match mirroring {
            Mirroring::Horizontal => (slot >> 1) & 1, // 0,1->A ; 2,3->B
            Mirroring::Vertical => slot & 1,          // 0,2->A ; 1,3->B
            Mirroring::SingleScreenA => 0,
            Mirroring::SingleScreenB => 1,
            // Four-screen would need cartridge VRAM; approximate with vertical.
            Mirroring::FourScreen => slot & 1,
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
            0x2000..=0x3eff => self.ciram[self.ciram_index(addr, mapper.mirroring())],
            0x3f00..=0x3fff => self.palette[Self::palette_index(addr)],
            _ => unreachable!(),
        }
    }

    fn mem_write(&mut self, addr: u16, val: u8, mapper: &mut dyn Mapper) {
        match addr & 0x3fff {
            0x0000..=0x1fff => mapper.ppu_write(addr & 0x1fff, val),
            0x2000..=0x3eff => {
                let i = self.ciram_index(addr, mapper.mirroring());
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
                // VBlank read race (this read samples at the cycle-start dot;
                // the set happens at (241,1) during this cycle's later dots).
                // Reading on the dot the flag would be set reads 0 and suppresses
                // the flag + NMI for this frame.
                if self.scanline == 241 && self.dot == 1 {
                    status &= 0x7f;
                    self.suppress_vbl = true;
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
                let addr = 0x23c0
                    | (self.v & 0x0c00)
                    | ((self.v >> 4) & 0x38)
                    | ((self.v >> 2) & 0x07);
                let attr = self.mem_read(addr, mapper);
                let shift = ((self.v >> 4) & 4) | (self.v & 2);
                self.bg_next_attr = (attr >> shift) & 0x3;
            }
            4 => {
                let base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0 };
                let fine_y = (self.v >> 12) & 7;
                let addr = base + (self.bg_next_id as u16) * 16 + fine_y;
                self.bg_next_lo = self.mem_read(addr, mapper);
            }
            6 => {
                let base = if self.ctrl & 0x10 != 0 { 0x1000 } else { 0 };
                let fine_y = (self.v >> 12) & 7;
                let addr = base + (self.bg_next_id as u16) * 16 + fine_y + 8;
                self.bg_next_hi = self.mem_read(addr, mapper);
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

                let mut lo = self.mem_read(addr, mapper);
                let mut hi = self.mem_read(addr + 8, mapper);
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
                self.mem_read(addr, mapper);
                self.mem_read(addr + 8, mapper);
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

        // Multiplex background and sprite (see notes §11).
        let pal_addr = match (bg_pixel != 0, sp_pixel != 0) {
            (false, false) => 0x3f00, // backdrop
            (false, true) => 0x3f10 | ((sp_palette as u16) << 2) | sp_pixel as u16,
            (true, false) => 0x3f00 | ((bg_palette as u16) << 2) | bg_pixel as u16,
            (true, true) => {
                // Sprite-0 hit: both opaque, from sprite 0, not at x=255.
                if is_sprite0 && x != 255 {
                    if self.status & 0x40 == 0 {
                        self.dbg_s0_scanline = self.scanline as i32;
                    }
                    self.status |= 0x40;
                }
                if sp_behind {
                    0x3f00 | ((bg_palette as u16) << 2) | bg_pixel as u16
                } else {
                    0x3f10 | ((sp_palette as u16) << 2) | sp_pixel as u16
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
    pub fn tick(&mut self, mapper: &mut dyn Mapper) {
        let rendering = self.rendering_enabled();
        let visible = self.scanline < 240;
        let prerender = self.scanline == 261;

        if (visible || prerender) && rendering {
            // Background fetch + shift on the fetch dots.
            if (self.dot >= 1 && self.dot <= 256) || (self.dot >= 321 && self.dot <= 336) {
                self.shift_bg();
                self.fetch_bg(mapper);
            }
            if self.dot == 256 {
                self.increment_y();
            }
            if self.dot == 257 {
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
            if !self.suppress_vbl {
                self.status |= 0x80; // vblank
            }
            self.suppress_vbl = false;
            self.frame_complete = true;
        }
        if prerender && self.dot == 1 {
            self.status &= !0xe0; // clear vblank, sprite-0, overflow
            self.dbg_s0_scanline = -1;
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
        w.bool(self.suppress_vbl);
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
        self.suppress_vbl = r.bool()?;
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
