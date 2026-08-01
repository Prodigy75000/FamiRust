//! 2A03 APU — two pulse channels, triangle, noise, and the DMC sample channel.
//!
//! Stub: the frame counter and channel register storage are staged here with a
//! save-state contract; the sequencer timing, length/envelope/sweep units, the
//! non-linear mixer, and the DMC's DMA (which steals CPU cycles and matters for
//! timing accuracy) are built once the CPU can clock it.

use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

pub struct Apu {
    /// Raw last-written values of $4000-$4013, $4015, $4017. Real per-unit state
    /// replaces this once the channels exist; kept as a flat block for now so
    /// register writes have somewhere to live and save states stay stable.
    pub regs: [u8; 0x18],
    pub frame_counter: u16,
    pub frame_mode_5step: bool,
    pub irq_inhibit: bool,
    pub irq_pending: bool,
}

impl Default for Apu {
    fn default() -> Self {
        Apu {
            regs: [0; 0x18],
            frame_counter: 0,
            frame_mode_5step: false,
            irq_inhibit: false,
            irq_pending: false,
        }
    }
}

impl Apu {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SaveState for Apu {
    fn save(&self, w: &mut WriteCursor) {
        w.bytes(&self.regs);
        w.u16(self.frame_counter);
        w.bool(self.frame_mode_5step);
        w.bool(self.irq_inhibit);
        w.bool(self.irq_pending);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        r.bytes(&mut self.regs)?;
        self.frame_counter = r.u16()?;
        self.frame_mode_5step = r.bool()?;
        self.irq_inhibit = r.bool()?;
        self.irq_pending = r.bool()?;
        Ok(())
    }
}
