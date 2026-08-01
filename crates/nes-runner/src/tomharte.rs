//! TomHarte 6502 single-step conformance harness.
//!
//! This is how the 2A03 CPU is ground to green: each vector sets up registers +
//! flat 64 KiB memory, runs one instruction, and asserts the final registers,
//! memory, and per-cycle bus trace. Wired once the CPU exposes a single-step
//! entry point; for now it reports that the corpus isn't hooked up yet so the
//! workspace builds cleanly.
//!
//! Vectors are vendored (gitignored) under `FamiRust/tests/vendor/`; point this
//! at that directory once the CPU lands.

fn main() {
    eprintln!(
        "tomharte: 2A03 CPU core not yet implemented — no vectors run.\n\
         Build the cycle-stepped 6502 in nes-core/src/cpu.rs, then wire this \
         harness to tests/vendor/ (the SingleStepTests 6502 corpus)."
    );
}
