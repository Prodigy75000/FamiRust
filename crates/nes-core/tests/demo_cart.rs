// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Core-level tests driven by the FamiRust demo cartridge.
//!
//! Every other test in this repository either uses a synthetic ROM or needs a
//! commercial dump the user has to supply. This one runs against a cartridge
//! that ships in the tree, so the checks below are reproducible by anyone who
//! clones the repository, including on a machine with no game ROMs on it.

use std::path::PathBuf;

// Controller bits as `set_buttons` takes them: bit 0 is the first button the
// hardware shifts out.
const BTN_START: u8 = 0x08;
const BTN_DOWN: u8 = 0x20;
const BTN_B: u8 = 0x02;

fn demo_rom() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("roms/famirust-demo/famirust-demo.nes");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn boot() -> nes_core::Nes {
    let mut nes = nes_core::Nes::from_rom(&demo_rom()).expect("demo cartridge should load");
    // Let reset, the RAM clear and the first screen build finish.
    for _ in 0..40 {
        nes.step_frame();
    }
    nes
}

/// Press a button long enough for the cart's edge detector to see it, then let
/// go long enough for the next press to be a fresh edge.
fn tap(nes: &mut nes_core::Nes, mask: u8) {
    for _ in 0..6 {
        nes.set_buttons(0, mask);
        nes.step_frame();
    }
    for _ in 0..6 {
        nes.set_buttons(0, 0);
        nes.step_frame();
    }
}

fn rgb(px: u32) -> (u8, u8, u8) {
    (((px >> 16) & 0xFF) as u8, ((px >> 8) & 0xFF) as u8, (px & 0xFF) as u8)
}

fn row(fb: &[u32], y: usize) -> &[u32] {
    &fb[y * 256..(y + 1) * 256]
}

#[test]
fn title_screen_draws_something() {
    let mut nes = boot();
    let fb = nes.step_frame().to_vec();
    assert_eq!(fb.len(), 256 * 240);

    let lit = fb.iter().filter(|&&p| p & 0xFF_FF_FF != fb[0] & 0xFF_FF_FF).count();
    assert!(
        lit > 1500,
        "title screen has only {lit} non-backdrop pixels; it should be full of text"
    );

    let mut colours: Vec<u32> = fb.iter().map(|p| p & 0xFF_FF_FF).collect();
    colours.sort_unstable();
    colours.dedup();
    assert!(
        colours.len() >= 4,
        "title screen uses {} colours; the palette and attribute tables should give it more",
        colours.len()
    );
}

#[test]
fn two_instances_run_identically() {
    // Determinism is the premise the save-state contract rests on. If two
    // fresh machines fed the same inputs diverge, nothing downstream is sound.
    let mut a = boot();
    let mut b = boot();
    for _ in 0..3 {
        tap(&mut a, BTN_DOWN);
        tap(&mut b, BTN_DOWN);
    }
    let fa = a.step_frame().to_vec();
    let fb = b.step_frame().to_vec();
    assert!(fa == fb, "two identical runs produced different frames");
    assert_eq!(a.save_state(), b.save_state(), "and different save states");
}

#[test]
fn save_state_round_trips_and_replays_identically() {
    let mut nes = boot();
    tap(&mut nes, BTN_DOWN);
    tap(&mut nes, BTN_START); // into the scrolling scene, so the state is not idle

    let saved = nes.save_state();
    assert_eq!(saved.len(), nes.state_size(), "state_size disagrees with save_state");

    // Reloading immediately must reproduce the same bytes.
    nes.load_state(&saved).expect("state should reload");
    assert_eq!(nes.save_state(), saved, "reload changed the state");

    // And running on from the state must reproduce the same future.
    let mut first = Vec::new();
    for _ in 0..30 {
        first = nes.step_frame().to_vec();
    }
    nes.load_state(&saved).expect("state should reload");
    let mut second = Vec::new();
    for _ in 0..30 {
        second = nes.step_frame().to_vec();
    }
    assert!(first == second, "replay from a restored state diverged");
}

#[test]
fn truncated_state_is_refused() {
    let mut nes = boot();
    let saved = nes.save_state();
    assert!(
        nes.load_state(&saved[..saved.len() - 1]).is_err(),
        "a truncated state must be rejected, not partially applied"
    );
    let mut too_long = saved.clone();
    too_long.push(0);
    assert!(
        nes.load_state(&too_long).is_err(),
        "an over-long state must be rejected"
    );
}

#[test]
fn sprite_zero_splits_the_scrolling_scene_at_scanline_32() {
    // The cart puts a one-scanline sprite on the last line of its status bar
    // (scanline 31) and changes the horizontal scroll the moment the hit
    // registers. So scanline 31 must still be the solid white bar and scanline
    // 32 must already be the playfield sky. If the split leaked, one of those
    // two rows stops being a single flat colour.
    let mut nes = boot();
    tap(&mut nes, BTN_DOWN); // menu: SPRITES -> SCROLL SPLIT
    tap(&mut nes, BTN_START);
    for _ in 0..20 {
        nes.step_frame();
    }
    let fb = nes.step_frame().to_vec();

    let bar = row(&fb, 31);
    let sky = row(&fb, 32);

    let bar0 = bar[0] & 0xFF_FF_FF;
    let sky0 = sky[0] & 0xFF_FF_FF;
    assert!(
        bar.iter().all(|p| p & 0xFF_FF_FF == bar0),
        "scanline 31 is not one flat colour: the split started too early"
    );
    assert!(
        sky.iter().all(|p| p & 0xFF_FF_FF == sky0),
        "scanline 32 is not one flat colour"
    );
    assert_ne!(bar0, sky0, "scanlines 31 and 32 should not be the same colour");

    let (br, bg, bb) = rgb(bar0);
    assert!(
        br > 200 && bg > 200 && bb > 200,
        "scanline 31 should be the white status bar, got {:?}",
        rgb(bar0)
    );
    let (sr, sg, sb) = rgb(sky0);
    assert!(
        sb > sr && sb > sg,
        "scanline 32 should be the blue sky, got {:?}",
        rgb(sky0)
    );

    assert_eq!(
        nes.dbg_sprite0_scanline(),
        31,
        "sprite 0 should hit on the last scanline of the status bar"
    );
}

#[test]
fn the_scene_actually_scrolls() {
    // A split that never moves would pass the test above while being useless.
    let mut nes = boot();
    tap(&mut nes, BTN_DOWN);
    tap(&mut nes, BTN_START);
    for _ in 0..20 {
        nes.step_frame();
    }
    let before = row(&nes.step_frame().to_vec(), 100).to_vec();
    for _ in 0..8 {
        nes.step_frame();
    }
    let after = row(&nes.step_frame().to_vec(), 100).to_vec();
    assert!(before != after, "the playfield below the split never moved");

    // The status bar above the split must NOT have moved with it.
    let bar_before = row(&nes.step_frame().to_vec(), 12).to_vec();
    for _ in 0..8 {
        nes.step_frame();
    }
    let bar_after = row(&nes.step_frame().to_vec(), 12).to_vec();
    assert!(
        bar_before == bar_after,
        "the status bar scrolled; the split is not holding it still"
    );
}

#[test]
fn leaving_a_scene_does_not_leave_its_text_on_the_menu() {
    // Scenes that update text push single-tile writes into a queue that the NMI
    // drains during vertical blank. Pressing B still runs the scene's tick, so
    // the queue is full at the moment the menu is redrawn, and the next NMI used
    // to paint those stale writes on top of the fresh menu.
    //
    // The menu's only moving parts are four sprites bobbing in a fixed band, so
    // everything outside that band must come back exactly as it was.
    const ORB_BAND: std::ops::Range<usize> = 56..72;

    let mut nes = boot();
    for _ in 0..4 {
        tap(&mut nes, BTN_DOWN); // land on INPUT, the busiest text screen
    }
    for _ in 0..10 {
        nes.step_frame();
    }
    let pristine = nes.step_frame().to_vec();

    tap(&mut nes, BTN_START);
    for _ in 0..20 {
        nes.step_frame();
    }
    tap(&mut nes, BTN_B);
    for _ in 0..10 {
        nes.step_frame();
    }
    let returned = nes.step_frame().to_vec();

    let mut dirty: Vec<usize> = Vec::new();
    for y in (0..240).filter(|y| !ORB_BAND.contains(y)) {
        if row(&pristine, y) != row(&returned, y) {
            dirty.push(y);
        }
    }
    assert!(
        dirty.is_empty(),
        "the menu came back changed on {} scanline(s) (first few: {:?}); \
         a scene left queued writes behind when it exited",
        dirty.len(),
        &dirty[..dirty.len().min(8)]
    );
}
