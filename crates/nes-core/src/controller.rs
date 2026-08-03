// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Standard NES controller (the shift-register joypad at $4016/$4017).
//!
//! Button state is latched on a write to $4016 with the strobe bit set, then
//! shifted out one bit per read. Both the latched shift value and the live
//! button state are saved so input timing survives a state load — essential for
//! deterministic netplay rollback.

use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

/// Button bit order as shifted out of the pad: A, B, Select, Start, Up, Down,
/// Left, Right (LSB first).
pub mod button {
    pub const A: u8 = 1 << 0;
    pub const B: u8 = 1 << 1;
    pub const SELECT: u8 = 1 << 2;
    pub const START: u8 = 1 << 3;
    pub const UP: u8 = 1 << 4;
    pub const DOWN: u8 = 1 << 5;
    pub const LEFT: u8 = 1 << 6;
    pub const RIGHT: u8 = 1 << 7;
}

#[derive(Default)]
pub struct Controller {
    /// Live button bitmask, updated by the frontend before each frame.
    pub buttons: u8,
    /// Value latched into the shift register at the last strobe.
    shift: u8,
    /// Strobe bit ($4016 bit 0). While high, the register reloads continuously.
    strobe: bool,
}

impl Controller {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_strobe(&mut self, on: bool) {
        self.strobe = on;
        if on {
            self.shift = self.buttons;
        }
    }

    /// A single read of $4016/$4017: return bit 0 of the shift register, then
    /// shift (unless the strobe is holding it loaded).
    pub fn read(&mut self) -> u8 {
        if self.strobe {
            self.shift = self.buttons;
        }
        let bit = self.shift & 1;
        self.shift = (self.shift >> 1) | 0x80; // open-bus-ish: reads 1s after 8
        bit
    }
}

impl SaveState for Controller {
    fn save(&self, w: &mut WriteCursor) {
        w.u8(self.buttons);
        w.u8(self.shift);
        w.bool(self.strobe);
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.buttons = r.u8()?;
        self.shift = r.u8()?;
        self.strobe = r.bool()?;
        Ok(())
    }
}
