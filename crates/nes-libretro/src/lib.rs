//! libretro C ABI front for the NES core.
//!
//! Placeholder: the `retro_*` entry points (video/audio/input, plus
//! `retro_serialize`/`retro_unserialize` wired straight to
//! [`nes_core::Nes::save_state`] / [`nes_core::Nes::load_state`] so the
//! byte-identical states are exposed to the frontend) land once the core runs a
//! frame. Built as a `cdylib` -> `libnescore_libretro.so`, renamed to the
//! Trophy Hub libretro convention on deploy.

// Reference the core so the dependency edge is real and the crate builds as a
// cdylib target from the start.
pub use nes_core;
