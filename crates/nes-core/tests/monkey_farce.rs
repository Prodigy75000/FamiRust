// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Core-level tests driven by MONKEY FARCE, the platformer that ships in this
//! tree. Like the demo cartridge tests beside them these need no ROM the user
//! has to supply, so they run on a fresh clone.
//!
//! Most of what is checked here is really a claim about the cartridge rather
//! than about the emulator, and that is on purpose. The game's whole premise is
//! that two kinds of block are impossible to tell apart by looking, which is
//! exactly the sort of claim that rots quietly the first time somebody adjusts
//! the art. It is checked below against the pixels the PPU actually produced.

use std::path::PathBuf;

const BTN_A: u8 = 0x01;
const BTN_UP: u8 = 0x10;
const BTN_START: u8 = 0x08;
const BTN_RIGHT: u8 = 0x80;

/// The monkey's face colour: entry $36 of the 2C02 master palette, which only
/// his sprite palette contains. No background palette in this cartridge holds
/// it, so a pixel this colour is him and nothing else.
const SKIN: u32 = 0x00fe_ccc5;

// Room 1's bridge, in screen pixels. Two honest platform blocks, then two
// liars, then two more honest ones, all on the same row.
const BRIDGE_Y: usize = 160;
const HONEST_X: usize = 80;
const LIAR_X: usize = 112;

fn rom() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("roms/monkey-farce/monkey-farce.nes");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn boot() -> nes_core::Nes {
    let mut nes = nes_core::Nes::from_rom(&rom()).expect("cartridge should load");
    for _ in 0..40 {
        nes.step_frame();
    }
    nes
}

/// Boot and press START, leaving the machine on the first frame of room 1.
fn in_room_1() -> nes_core::Nes {
    let mut nes = boot();
    for _ in 0..8 {
        nes.set_buttons(0, BTN_START);
        nes.step_frame();
    }
    nes.set_buttons(0, 0);
    for _ in 0..12 {
        nes.step_frame();
    }
    nes
}

fn run(nes: &mut nes_core::Nes, frames: u32, buttons: u8) -> Vec<u32> {
    let mut fb = Vec::new();
    for _ in 0..frames {
        nes.set_buttons(0, buttons);
        fb = nes.step_frame().to_vec();
    }
    fb
}

/// Where the hero is, as the top-left of the block of skin-coloured pixels.
/// None while he is off-screen, which is what a death animation ends as.
fn find_hero(fb: &[u32]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for y in 0..240 {
        for x in 0..256 {
            if fb[y * 256 + x] & 0xFF_FF_FF == SKIN {
                best = Some(match best {
                    None => (x, y),
                    Some((bx, by)) => (bx.min(x), by),
                });
            }
        }
        if best.is_some() {
            break; // the first row of skin is the top of his head
        }
    }
    best
}

fn region(fb: &[u32], ox: usize, oy: usize, w: usize, h: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(w * h);
    for y in oy..oy + h {
        for x in ox..ox + w {
            out.push(fb[y * 256 + x] & 0xFF_FF_FF);
        }
    }
    out
}

fn tile16(fb: &[u32], ox: usize, oy: usize) -> Vec<u32> {
    region(fb, ox, oy, 16, 16)
}

/// Walk right off the left ledge, jump onto the bridge, and stop. Leaves the
/// hero standing on the second of the two honest platform blocks.
fn onto_the_bridge(nes: &mut nes_core::Nes) {
    run(nes, 32, BTN_RIGHT);
    run(nes, 12, BTN_RIGHT | BTN_A);
    run(nes, 22, BTN_RIGHT);
    run(nes, 10, 0);
}

#[test]
fn the_liar_is_pixel_identical_to_the_honest_platform() {
    // The claim the whole cartridge rests on, checked where it counts: not in
    // the tile tables, which is where it is arranged, but in the frame the PPU
    // handed back, which is all the player ever gets to see.
    //
    // Give the liar its own tile, its own palette, or a single different pixel,
    // and this fails. There is deliberately no tolerance: "almost identical" is
    // a tell, and a tell is the one thing this game cannot have.
    let mut nes = in_room_1();
    let fb = run(&mut nes, 4, 0);

    let honest = tile16(&fb, HONEST_X, BRIDGE_Y);
    let liar = tile16(&fb, LIAR_X, BRIDGE_Y);
    assert_eq!(
        honest, liar,
        "the block that holds you up and the block that does not no longer look \
         the same; there is now something to spot, and the game is about nothing"
    );

    // And the comparison has to be worth something: if that patch of screen were
    // blank, two blanks would match and prove nothing.
    let mut distinct = honest.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() >= 3,
        "the bridge is only {} colours; this test would pass on an empty screen",
        distinct.len()
    );
}

#[test]
fn the_honest_platform_holds_him_up() {
    let mut nes = in_room_1();
    onto_the_bridge(&mut nes);

    let landed = find_hero(&run(&mut nes, 1, 0)).expect("monkey should be on screen");
    let later = find_hero(&run(&mut nes, 30, 0)).expect("monkey should still be on screen");
    assert_eq!(
        landed.1, later.1,
        "he sank {} pixels while stood still on a solid block",
        later.1 as i32 - landed.1 as i32
    );
    assert!(
        landed.1 < BRIDGE_Y,
        "he should be standing on top of the bridge, not inside it (y={})",
        landed.1
    );
}

#[test]
fn the_liar_does_not() {
    // Same room, same bridge, same two frames of walking. The only difference
    // is which block he ends up over.
    let mut nes = in_room_1();
    onto_the_bridge(&mut nes);
    let landed = find_hero(&run(&mut nes, 1, 0)).expect("monkey should be on screen");

    // Keep walking right, onto the pair that is not there.
    run(&mut nes, 20, BTN_RIGHT);
    let after = find_hero(&run(&mut nes, 1, BTN_RIGHT));

    match after {
        None => {} // already fallen off the bottom, which is the same verdict
        Some(after) => assert!(
            after.1 > landed.1 + 8,
            "he walked onto the liar and stayed up: was y={}, now y={}",
            landed.1,
            after.1
        ),
    }
}

#[test]
fn falling_into_the_spikes_costs_a_life_and_puts_him_back() {
    let mut nes = in_room_1();
    let start = find_hero(&run(&mut nes, 1, 0)).expect("monkey should be on screen");

    // The HUD's life counter: both digits, on screen tile row 1 from column 24.
    // Reading only the tens would pass while the count fell from 10 to 1.
    let digits = |fb: &Vec<u32>| region(fb, 24 * 8, 8, 16, 8);
    let before = digits(&run(&mut nes, 1, 0));

    // Walk straight off the ledge into the spike pit and wait out the death.
    run(&mut nes, 60, BTN_RIGHT);
    let fb = run(&mut nes, 60, 0);

    assert_ne!(
        before,
        digits(&fb),
        "he died in the spikes and the life counter did not move"
    );
    let back = find_hero(&fb).expect("monkey should have respawned");
    assert_eq!(
        back, start,
        "he did not come back to where the room starts him"
    );
}

/// Walk the whole of room 1 and end up standing in the doorway.
fn to_the_door(nes: &mut nes_core::Nes) {
    onto_the_bridge(nes);
    run(nes, 20, 0);
    run(nes, 12, BTN_RIGHT | BTN_A); // over the two liars
    run(nes, 30, BTN_RIGHT);
    run(nes, 20, 0);
    run(nes, 12, BTN_RIGHT | BTN_A); // off the far end, onto the ledge
    run(nes, 60, BTN_RIGHT); // and along it to the door
}

fn room_name(nes: &mut nes_core::Nes) -> Vec<u32> {
    region(&run(nes, 1, 0), 8, 8, 14 * 8, 8)
}

#[test]
fn a_door_you_have_not_asked_to_open_stays_shut() {
    // The door is not a tripwire, which is the whole reason it takes a button.
    // Standing in it, and standing in it for a while, has to be safe: room 6
    // runs its floor straight through a doorway and the player is meant to be
    // able to walk past.
    let mut nes = in_room_1();
    let before = room_name(&mut nes);
    to_the_door(&mut nes);
    run(&mut nes, 120, 0);
    assert_eq!(
        before,
        room_name(&mut nes),
        "the room changed while he was only standing in the doorway"
    );
}

#[test]
fn pressing_up_in_a_doorway_opens_it() {
    // Room 1 is crossable. Proving that is worth more than it looks: a game
    // that lies about the floor is one bad measurement away from being one
    // nobody can finish, and the room names are how the screen says which it is.
    let mut nes = in_room_1();
    let room1 = room_name(&mut nes);

    to_the_door(&mut nes);
    run(&mut nes, 4, BTN_UP);
    run(&mut nes, 90, 0); // the pause, then the next room

    assert_ne!(
        room1,
        room_name(&mut nes),
        "he asked the door to open and the room did not change"
    );
}

#[test]
fn two_instances_play_identically() {
    // Determinism is the premise the save-state contract rests on, and a game
    // exercises far more of the machine than a menu does.
    let mut a = in_room_1();
    let mut b = in_room_1();
    onto_the_bridge(&mut a);
    onto_the_bridge(&mut b);
    assert!(
        run(&mut a, 30, BTN_RIGHT) == run(&mut b, 30, BTN_RIGHT),
        "two identical runs diverged"
    );
    assert_eq!(a.save_state(), b.save_state(), "and their states disagree");
}

#[test]
fn a_state_saved_mid_jump_replays_the_same_jump() {
    let mut nes = in_room_1();
    run(&mut nes, 32, BTN_RIGHT);
    run(&mut nes, 6, BTN_RIGHT | BTN_A); // caught in the air, still rising

    let saved = nes.save_state();
    assert_eq!(saved.len(), nes.state_size(), "state_size disagrees with save_state");

    let first = run(&mut nes, 40, BTN_RIGHT);
    nes.load_state(&saved).expect("state should reload");
    let second = run(&mut nes, 40, BTN_RIGHT);
    assert!(first == second, "the same jump replayed differently");
}
