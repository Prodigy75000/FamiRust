//! 2A03 APU — two pulse channels, triangle, noise, and the DMC sample channel,
//! plus the frame sequencer and the non-linear mixer.
//!
//! Implemented from `docs/notes/apu-2a03.md` (clean-room notes distilled from the
//! NESdev/Blargg APU references), never from other emulator source. Ticked once
//! per CPU cycle; the frame sequencer uses exact CPU-cycle thresholds. Output is
//! mixed through the non-linear DAC curves and decimated to a host sample rate.

use crate::cart::Mapper;
use crate::save::{LoadError, ReadCursor, SaveState, WriteCursor};

/// NTSC CPU clock (Hz), used for host-rate decimation.
const CPU_HZ: f64 = 1_789_773.0;
/// Host output sample rate.
pub const SAMPLE_RATE: u32 = 44_100;

const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];
const NOISE_PERIOD: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];
const DMC_RATE: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];
const DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];
const TRIANGLE_SEQ: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

// ---------------- shared sub-units ----------------

#[derive(Default)]
struct Envelope {
    start: bool,
    divider: u8,
    decay: u8,
    /// $400x low nibble: period AND constant-volume value.
    period: u8,
    constant: bool,
    loop_flag: bool,
}
impl Envelope {
    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.period;
        } else if self.divider == 0 {
            self.divider = self.period;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.loop_flag {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }
    fn volume(&self) -> u8 {
        if self.constant {
            self.period
        } else {
            self.decay
        }
    }
    fn save(&self, w: &mut WriteCursor) {
        w.bool(self.start);
        w.u8(self.divider);
        w.u8(self.decay);
        w.u8(self.period);
        w.bool(self.constant);
        w.bool(self.loop_flag);
    }
    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.start = r.bool()?;
        self.divider = r.u8()?;
        self.decay = r.u8()?;
        self.period = r.u8()?;
        self.constant = r.bool()?;
        self.loop_flag = r.bool()?;
        Ok(())
    }
}

#[derive(Default)]
struct LengthCounter {
    value: u8,
    halt: bool,
    enabled: bool,
}
impl LengthCounter {
    fn clock(&mut self) {
        if self.value > 0 && !self.halt {
            self.value -= 1;
        }
    }
    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.value = 0;
        }
    }
    fn load(&mut self, index: u8) {
        if self.enabled {
            self.value = LENGTH_TABLE[index as usize & 0x1f];
        }
    }
    fn save(&self, w: &mut WriteCursor) {
        w.u8(self.value);
        w.bool(self.halt);
        w.bool(self.enabled);
    }
    fn read(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.value = r.u8()?;
        self.halt = r.bool()?;
        self.enabled = r.bool()?;
        Ok(())
    }
}

// ---------------- pulse ----------------

#[derive(Default)]
struct Pulse {
    /// True for pulse 2 (affects the sweep negate behavior).
    is_pulse2: bool,
    env: Envelope,
    len: LengthCounter,
    duty: u8,
    seq_phase: u8,
    timer_period: u16,
    timer: u16,
    // sweep
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_divider: u8,
    sweep_reload: bool,
}
impl Pulse {
    fn write(&mut self, reg: u16, val: u8) {
        match reg {
            0 => {
                self.duty = val >> 6;
                self.len.halt = val & 0x20 != 0;
                self.env.loop_flag = val & 0x20 != 0;
                self.env.constant = val & 0x10 != 0;
                self.env.period = val & 0x0f;
            }
            1 => {
                self.sweep_enabled = val & 0x80 != 0;
                self.sweep_period = (val >> 4) & 0x07;
                self.sweep_negate = val & 0x08 != 0;
                self.sweep_shift = val & 0x07;
                self.sweep_reload = true;
            }
            2 => self.timer_period = (self.timer_period & 0x700) | val as u16,
            3 => {
                self.timer_period = (self.timer_period & 0x0ff) | (((val as u16) & 0x07) << 8);
                self.len.load(val >> 3);
                self.seq_phase = 0;
                self.env.start = true;
            }
            _ => {}
        }
    }

    fn sweep_target(&self) -> u16 {
        let change = self.timer_period >> self.sweep_shift;
        if self.sweep_negate {
            if self.is_pulse2 {
                self.timer_period.wrapping_sub(change)
            } else {
                self.timer_period.wrapping_sub(change).wrapping_sub(1)
            }
        } else {
            self.timer_period + change
        }
    }
    fn muted(&self) -> bool {
        self.timer_period < 8 || self.sweep_target() > 0x7ff
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.timer_period;
            self.seq_phase = (self.seq_phase + 1) & 7;
        } else {
            self.timer -= 1;
        }
    }

    fn clock_sweep(&mut self) {
        if self.sweep_divider == 0 && self.sweep_enabled && self.sweep_shift != 0 && !self.muted() {
            self.timer_period = self.sweep_target();
        }
        if self.sweep_divider == 0 || self.sweep_reload {
            self.sweep_divider = self.sweep_period;
            self.sweep_reload = false;
        } else {
            self.sweep_divider -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.len.value == 0 || self.muted() || DUTY[self.duty as usize][self.seq_phase as usize] == 0
        {
            0
        } else {
            self.env.volume()
        }
    }

    fn save(&self, w: &mut WriteCursor) {
        self.env.save(w);
        self.len.save(w);
        w.u8(self.duty);
        w.u8(self.seq_phase);
        w.u16(self.timer_period);
        w.u16(self.timer);
        w.bool(self.sweep_enabled);
        w.u8(self.sweep_period);
        w.bool(self.sweep_negate);
        w.u8(self.sweep_shift);
        w.u8(self.sweep_divider);
        w.bool(self.sweep_reload);
    }
    fn read(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.env.load(r)?;
        self.len.read(r)?;
        self.duty = r.u8()?;
        self.seq_phase = r.u8()?;
        self.timer_period = r.u16()?;
        self.timer = r.u16()?;
        self.sweep_enabled = r.bool()?;
        self.sweep_period = r.u8()?;
        self.sweep_negate = r.bool()?;
        self.sweep_shift = r.u8()?;
        self.sweep_divider = r.u8()?;
        self.sweep_reload = r.bool()?;
        Ok(())
    }
}

// ---------------- triangle ----------------

#[derive(Default)]
struct Triangle {
    len: LengthCounter,
    timer_period: u16,
    timer: u16,
    seq_phase: u8,
    linear_counter: u8,
    linear_reload_value: u8,
    linear_reload: bool,
    control: bool, // shared with length halt
}
impl Triangle {
    fn write(&mut self, reg: u16, val: u8) {
        match reg {
            0 => {
                self.control = val & 0x80 != 0;
                self.len.halt = val & 0x80 != 0;
                self.linear_reload_value = val & 0x7f;
            }
            2 => self.timer_period = (self.timer_period & 0x700) | val as u16,
            3 => {
                self.timer_period = (self.timer_period & 0x0ff) | (((val as u16) & 0x07) << 8);
                self.len.load(val >> 3);
                self.linear_reload = true;
            }
            _ => {}
        }
    }
    fn clock_timer(&mut self) {
        // Clocked every CPU cycle. Advance only while both counters are non-zero;
        // freeze (don't advance) at ultrasonic periods to keep buzz out of the mix.
        if self.timer == 0 {
            self.timer = self.timer_period;
            if self.linear_counter > 0 && self.len.value > 0 && self.timer_period >= 2 {
                self.seq_phase = (self.seq_phase + 1) & 31;
            }
        } else {
            self.timer -= 1;
        }
    }
    fn clock_linear(&mut self) {
        if self.linear_reload {
            self.linear_counter = self.linear_reload_value;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }
        if !self.control {
            self.linear_reload = false;
        }
    }
    fn output(&self) -> u8 {
        TRIANGLE_SEQ[self.seq_phase as usize]
    }
    fn save(&self, w: &mut WriteCursor) {
        self.len.save(w);
        w.u16(self.timer_period);
        w.u16(self.timer);
        w.u8(self.seq_phase);
        w.u8(self.linear_counter);
        w.u8(self.linear_reload_value);
        w.bool(self.linear_reload);
        w.bool(self.control);
    }
    fn read(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.len.read(r)?;
        self.timer_period = r.u16()?;
        self.timer = r.u16()?;
        self.seq_phase = r.u8()?;
        self.linear_counter = r.u8()?;
        self.linear_reload_value = r.u8()?;
        self.linear_reload = r.bool()?;
        self.control = r.bool()?;
        Ok(())
    }
}

// ---------------- noise ----------------

struct Noise {
    env: Envelope,
    len: LengthCounter,
    timer_period: u16,
    timer: u16,
    lfsr: u16,
    mode: bool,
}
impl Default for Noise {
    fn default() -> Self {
        Noise {
            env: Envelope::default(),
            len: LengthCounter::default(),
            timer_period: 0,
            timer: 0,
            lfsr: 1, // must be non-zero
            mode: false,
        }
    }
}
impl Noise {
    fn write(&mut self, reg: u16, val: u8) {
        match reg {
            0 => {
                self.len.halt = val & 0x20 != 0;
                self.env.loop_flag = val & 0x20 != 0;
                self.env.constant = val & 0x10 != 0;
                self.env.period = val & 0x0f;
            }
            2 => {
                self.mode = val & 0x80 != 0;
                self.timer_period = NOISE_PERIOD[val as usize & 0x0f];
            }
            3 => {
                self.len.load(val >> 3);
                self.env.start = true;
            }
            _ => {}
        }
    }
    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.timer_period;
            let tap = if self.mode { 6 } else { 1 };
            let feedback = (self.lfsr & 1) ^ ((self.lfsr >> tap) & 1);
            self.lfsr >>= 1;
            self.lfsr |= feedback << 14;
        } else {
            self.timer -= 1;
        }
    }
    fn output(&self) -> u8 {
        if self.len.value == 0 || self.lfsr & 1 == 1 {
            0
        } else {
            self.env.volume()
        }
    }
    fn save(&self, w: &mut WriteCursor) {
        self.env.save(w);
        self.len.save(w);
        w.u16(self.timer_period);
        w.u16(self.timer);
        w.u16(self.lfsr);
        w.bool(self.mode);
    }
    fn read(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.env.load(r)?;
        self.len.read(r)?;
        self.timer_period = r.u16()?;
        self.timer = r.u16()?;
        self.lfsr = r.u16()?;
        self.mode = r.bool()?;
        Ok(())
    }
}

// ---------------- DMC ----------------

#[derive(Default)]
struct Dmc {
    irq_enable: bool,
    loop_flag: bool,
    rate: u16,
    timer: u16,
    level: u8, // 7-bit output (0..127)
    sample_addr: u16,
    sample_len: u16,
    cur_addr: u16,
    bytes_remaining: u16,
    sample_buffer: Option<u8>,
    shift: u8,
    bits_remaining: u8,
    silence: bool,
    irq_flag: bool,
    /// CPU cycles the DMC DMA should stall the CPU (consumed by the bus).
    pub stall: u32,
}
impl Dmc {
    fn write(&mut self, reg: u16, val: u8) {
        match reg {
            0 => {
                self.irq_enable = val & 0x80 != 0;
                self.loop_flag = val & 0x40 != 0;
                self.rate = DMC_RATE[val as usize & 0x0f];
                if !self.irq_enable {
                    self.irq_flag = false;
                }
            }
            1 => self.level = val & 0x7f,
            2 => self.sample_addr = 0xc000 + ((val as u16) << 6),
            3 => self.sample_len = ((val as u16) << 4) + 1,
            _ => {}
        }
    }

    fn restart(&mut self) {
        self.cur_addr = self.sample_addr;
        self.bytes_remaining = self.sample_len;
    }

    /// Fetch a sample byte if the buffer is empty and bytes remain. Reads CPU
    /// space via the mapper (DMC samples live in PRG at $C000+).
    fn maybe_fetch(&mut self, mapper: &mut dyn Mapper) {
        if self.sample_buffer.is_none() && self.bytes_remaining > 0 {
            self.sample_buffer = Some(mapper.cpu_read(self.cur_addr));
            self.stall += 4; // approximate DMA stall
            self.cur_addr = if self.cur_addr == 0xffff {
                0x8000
            } else {
                self.cur_addr + 1
            };
            self.bytes_remaining -= 1;
            if self.bytes_remaining == 0 {
                if self.loop_flag {
                    self.restart();
                } else if self.irq_enable {
                    self.irq_flag = true;
                }
            }
        }
    }

    fn clock_timer(&mut self, mapper: &mut dyn Mapper) {
        self.maybe_fetch(mapper);
        if self.timer == 0 {
            // Reload = rate-1 so the output-change period is exactly `rate` CPU
            // cycles (clocked every CPU cycle).
            self.timer = self.rate.saturating_sub(1);
            self.clock_output();
        } else {
            self.timer -= 1;
        }
    }

    fn clock_output(&mut self) {
        if !self.silence {
            if self.shift & 1 != 0 {
                if self.level <= 125 {
                    self.level += 2;
                }
            } else if self.level >= 2 {
                self.level -= 2;
            }
        }
        self.shift >>= 1;
        if self.bits_remaining > 0 {
            self.bits_remaining -= 1;
        }
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
            if let Some(b) = self.sample_buffer.take() {
                self.silence = false;
                self.shift = b;
            } else {
                self.silence = true;
            }
        }
    }

    fn output(&self) -> u8 {
        self.level
    }
    fn active(&self) -> bool {
        self.bytes_remaining > 0
    }

    fn save(&self, w: &mut WriteCursor) {
        w.bool(self.irq_enable);
        w.bool(self.loop_flag);
        w.u16(self.rate);
        w.u16(self.timer);
        w.u8(self.level);
        w.u16(self.sample_addr);
        w.u16(self.sample_len);
        w.u16(self.cur_addr);
        w.u16(self.bytes_remaining);
        w.u8(self.sample_buffer.unwrap_or(0));
        w.bool(self.sample_buffer.is_some());
        w.u8(self.shift);
        w.u8(self.bits_remaining);
        w.bool(self.silence);
        w.bool(self.irq_flag);
        w.u32(self.stall);
    }
    fn read(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.irq_enable = r.bool()?;
        self.loop_flag = r.bool()?;
        self.rate = r.u16()?;
        self.timer = r.u16()?;
        self.level = r.u8()?;
        self.sample_addr = r.u16()?;
        self.sample_len = r.u16()?;
        self.cur_addr = r.u16()?;
        self.bytes_remaining = r.u16()?;
        let byte = r.u8()?;
        let has = r.bool()?;
        self.sample_buffer = if has { Some(byte) } else { None };
        self.shift = r.u8()?;
        self.bits_remaining = r.u8()?;
        self.silence = r.bool()?;
        self.irq_flag = r.bool()?;
        self.stall = r.u32()?;
        Ok(())
    }
}

// ---------------- the APU ----------------

pub struct Apu {
    pulse1: Pulse,
    pulse2: Pulse,
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,

    // frame sequencer
    frame_cycle: u32,
    frame_mode_5step: bool,
    frame_irq_inhibit: bool,
    frame_irq_flag: bool,
    frame_reset_delay: u8,
    frame_reset_pending: bool,
    cycle_parity: bool, // toggles each CPU cycle (APU units step on one phase)

    // host-rate output
    pulse_table: [f32; 31],
    tnd_table: [f32; 203],
    sample_accum: f64,
    samples: Vec<f32>,
    hp_prev_in: f32,
    hp_prev_out: f32,
}

impl Default for Apu {
    fn default() -> Self {
        let mut pulse2 = Pulse::default();
        pulse2.is_pulse2 = true;
        let mut dmc = Dmc::default();
        dmc.rate = DMC_RATE[0]; // avoid a zero reload before $4010 is written
        let mut pulse_table = [0f32; 31];
        for (i, v) in pulse_table.iter_mut().enumerate() {
            *v = if i == 0 {
                0.0
            } else {
                95.88 / (8128.0 / i as f32 + 100.0)
            };
        }
        let mut tnd_table = [0f32; 203];
        for (i, v) in tnd_table.iter_mut().enumerate() {
            *v = if i == 0 {
                0.0
            } else {
                163.67 / (24329.0 / i as f32 + 100.0)
            };
        }
        Apu {
            pulse1: Pulse::default(),
            pulse2,
            triangle: Triangle::default(),
            noise: Noise::default(),
            dmc,
            frame_cycle: 0,
            frame_mode_5step: false,
            frame_irq_inhibit: false,
            frame_irq_flag: false,
            frame_reset_delay: 0,
            frame_reset_pending: false,
            cycle_parity: false,
            pulse_table,
            tnd_table,
            sample_accum: 0.0,
            samples: Vec::new(),
            hp_prev_in: 0.0,
            hp_prev_out: 0.0,
        }
    }
}

impl Apu {
    pub fn new() -> Self {
        Self::default()
    }

    fn quarter_clock(&mut self) {
        self.pulse1.env.clock();
        self.pulse2.env.clock();
        self.noise.env.clock();
        self.triangle.clock_linear();
    }
    fn half_clock(&mut self) {
        self.pulse1.len.clock();
        self.pulse2.len.clock();
        self.triangle.len.clock();
        self.noise.len.clock();
        self.pulse1.clock_sweep();
        self.pulse2.clock_sweep();
    }

    fn clock_frame_sequencer(&mut self) {
        // Apply a pending $4017 reset after its 3/4-cycle delay.
        if self.frame_reset_pending {
            self.frame_reset_delay -= 1;
            if self.frame_reset_delay == 0 {
                self.frame_reset_pending = false;
                self.frame_cycle = 0;
            }
        }

        self.frame_cycle += 1;
        let c = self.frame_cycle;
        if self.frame_mode_5step {
            match c {
                7457 => self.quarter_clock(),
                14913 => {
                    self.quarter_clock();
                    self.half_clock();
                }
                22371 => self.quarter_clock(),
                37281 => {
                    self.quarter_clock();
                    self.half_clock();
                }
                37282 => self.frame_cycle = 0,
                _ => {}
            }
        } else {
            match c {
                7457 => self.quarter_clock(),
                14913 => {
                    self.quarter_clock();
                    self.half_clock();
                }
                22371 => self.quarter_clock(),
                29828 => {
                    if !self.frame_irq_inhibit {
                        self.frame_irq_flag = true;
                    }
                }
                29829 => {
                    self.quarter_clock();
                    self.half_clock();
                    if !self.frame_irq_inhibit {
                        self.frame_irq_flag = true;
                    }
                }
                29830 => {
                    if !self.frame_irq_inhibit {
                        self.frame_irq_flag = true;
                    }
                    self.frame_cycle = 0;
                }
                _ => {}
            }
        }
    }

    /// Advance one CPU cycle.
    pub fn tick(&mut self, mapper: &mut dyn Mapper) {
        self.clock_frame_sequencer();

        // Triangle + DMC timers run every CPU cycle (the DMC rate table is in
        // CPU cycles); pulse/noise timers run on every other (APU) cycle.
        self.triangle.clock_timer();
        self.dmc.clock_timer(mapper);
        if self.cycle_parity {
            self.pulse1.clock_timer();
            self.pulse2.clock_timer();
            self.noise.clock_timer();
        }
        self.cycle_parity = !self.cycle_parity;

        // Decimate to the host sample rate.
        self.sample_accum += 1.0;
        let cycles_per_sample = CPU_HZ / SAMPLE_RATE as f64;
        if self.sample_accum >= cycles_per_sample {
            self.sample_accum -= cycles_per_sample;
            let s = self.mix();
            self.samples.push(s);
        }
    }

    fn mix(&mut self) -> f32 {
        let p = (self.pulse1.output() + self.pulse2.output()) as usize;
        let t = (3 * self.triangle.output() + 2 * self.noise.output() + self.dmc.output()) as usize;
        let raw = self.pulse_table[p] + self.tnd_table[t];
        // First-order DC-blocking high-pass for pleasant output.
        let out = raw - self.hp_prev_in + 0.9995 * self.hp_prev_out;
        self.hp_prev_in = raw;
        self.hp_prev_out = out;
        out
    }

    // ---------------- register interface ----------------

    pub fn write_register(&mut self, addr: u16, val: u8) {
        match addr {
            0x4000..=0x4003 => self.pulse1.write(addr - 0x4000, val),
            0x4004..=0x4007 => self.pulse2.write(addr - 0x4004, val),
            0x4008..=0x400b => self.triangle.write(addr - 0x4008, val),
            0x400c..=0x400f => self.noise.write(addr - 0x400c, val),
            0x4010..=0x4013 => self.dmc.write(addr - 0x4010, val),
            0x4015 => {
                self.pulse1.len.set_enabled(val & 0x01 != 0);
                self.pulse2.len.set_enabled(val & 0x02 != 0);
                self.triangle.len.set_enabled(val & 0x04 != 0);
                self.noise.len.set_enabled(val & 0x08 != 0);
                if val & 0x10 != 0 {
                    if self.dmc.bytes_remaining == 0 {
                        self.dmc.restart();
                    }
                } else {
                    self.dmc.bytes_remaining = 0;
                }
                self.dmc.irq_flag = false; // any $4015 write clears the DMC IRQ
            }
            0x4017 => {
                self.frame_mode_5step = val & 0x80 != 0;
                self.frame_irq_inhibit = val & 0x40 != 0;
                if self.frame_irq_inhibit {
                    self.frame_irq_flag = false;
                }
                // Reset the sequencer after a 3/4-cycle delay (parity-dependent).
                self.frame_reset_delay = if self.cycle_parity { 3 } else { 4 };
                self.frame_reset_pending = true;
                // 5-step mode issues an immediate half+quarter clock.
                if self.frame_mode_5step {
                    self.quarter_clock();
                    self.half_clock();
                }
            }
            _ => {}
        }
    }

    /// $4015 read: status + IRQ flags. Reading clears the frame IRQ flag.
    pub fn read_status(&mut self) -> u8 {
        let mut v = 0u8;
        if self.pulse1.len.value > 0 {
            v |= 0x01;
        }
        if self.pulse2.len.value > 0 {
            v |= 0x02;
        }
        if self.triangle.len.value > 0 {
            v |= 0x04;
        }
        if self.noise.len.value > 0 {
            v |= 0x08;
        }
        if self.dmc.active() {
            v |= 0x10;
        }
        if self.frame_irq_flag {
            v |= 0x40;
        }
        if self.dmc.irq_flag {
            v |= 0x80;
        }
        self.frame_irq_flag = false;
        v
    }

    /// Level of the CPU IRQ line contributed by the APU.
    pub fn irq_asserted(&self) -> bool {
        self.frame_irq_flag || self.dmc.irq_flag
    }

    /// Consume any pending DMC DMA CPU-stall cycles (the bus applies them).
    pub fn take_dma_stall(&mut self) -> u32 {
        std::mem::take(&mut self.dmc.stall)
    }

    /// Drain the accumulated host-rate samples (f32, roughly -1..1).
    pub fn take_samples(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }
}

impl SaveState for Apu {
    fn save(&self, w: &mut WriteCursor) {
        self.pulse1.save(w);
        self.pulse2.save(w);
        self.triangle.save(w);
        self.noise.save(w);
        self.dmc.save(w);
        w.u32(self.frame_cycle);
        w.bool(self.frame_mode_5step);
        w.bool(self.frame_irq_inhibit);
        w.bool(self.frame_irq_flag);
        w.u8(self.frame_reset_delay);
        w.bool(self.frame_reset_pending);
        w.bool(self.cycle_parity);
        // Output-path state (sample accumulator, filters, buffer) is not part of
        // the deterministic machine state and is intentionally not serialized.
    }

    fn load(&mut self, r: &mut ReadCursor) -> Result<(), LoadError> {
        self.pulse1.read(r)?;
        self.pulse2.read(r)?;
        self.triangle.read(r)?;
        self.noise.read(r)?;
        self.dmc.read(r)?;
        self.frame_cycle = r.u32()?;
        self.frame_mode_5step = r.bool()?;
        self.frame_irq_inhibit = r.bool()?;
        self.frame_irq_flag = r.bool()?;
        self.frame_reset_delay = r.u8()?;
        self.frame_reset_pending = r.bool()?;
        self.cycle_parity = r.bool()?;
        Ok(())
    }
}
