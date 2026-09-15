// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Core-level tests driven by NOISY NEIGHBORS, the two-player escape that
//! ships in this tree. Like the other cartridge tests beside them these need no
//! ROM the user has to supply, so they run on a fresh clone.
//!
//! What is checked here is the cartridge's one rule rather than the emulator:
//! a room does not end because one of them reached a way out, it ends because
//! both of them did. That is a sentence in a README until something boots the
//! ROM, walks two players to two different doors, and watches what the machine
//! does when only one of them asks to leave.
//!
//! It doubles as the only two-controller test in this repository, so a
//! regression that silently stopped delivering port 2 would fail here.

use std::path::PathBuf;

const BTN_UP: u8 = 0x10;
const BTN_START: u8 = 0x08;
const BTN_LEFT: u8 = 0x40;
const BTN_RIGHT: u8 = 0x80;

const P1: usize = 0;
const P2: usize = 1;

// Where the cartridge parks each player, and what it parks about them. Taken
// from the symbols the assembler emits (p1_xh, p2_xh, p1_state, p2_state):
// each player's block is sixteen bytes inside psave at $0500.
const P1_XH: usize = 0x0501;
const P2_XH: usize = 0x0511;
const P1_STATE: usize = 0x050C;
const P2_STATE: usize = 0x051C;
const MODE: usize = 0x001B;
const ROOM_TIMER: usize = 0x0037;
const LIVES: usize = 0x004A;
const NOISE: usize = 0x004B;
const NB_STATE: usize = 0x004F;

const HS_ALIVE: u8 = 0;
const HS_WIN: u8 = 2;
const M_PLAY: u8 = 1;
const M_END: u8 = 2;

const NB_IDLE: u8 = 0;
const NB_SWEEP: u8 = 2;
const NOISE_MAX: u8 = 192;
const NB_WARN: usize = 120;
const NB_SWEEP_FRAMES: usize = 150;

// Where the two of them start in room 1, in whole pixels: columns 3 and 12 of
// a sixteen-pixel grid.
const SPAWN_1: u8 = 48;
const SPAWN_2: u8 = 192;

fn rom() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("roms/noisy-neighbors/noisy-neighbors.nes");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Boot, press START, and stop on the first frame of room 1 with both pads
/// released.
fn in_room_1() -> nes_core::Nes {
    let mut nes = nes_core::Nes::from_rom(&rom()).expect("cartridge should load");
    for _ in 0..40 {
        nes.step_frame();
    }
    for _ in 0..8 {
        nes.set_buttons(P1, BTN_START);
        nes.step_frame();
    }
    nes.set_buttons(P1, 0);
    for _ in 0..12 {
        nes.step_frame();
    }
    assert_eq!(nes.system_ram()[MODE], M_PLAY, "START should have begun room 1");
    nes
}

/// Hold `a` on pad 1 and `b` on pad 2 for `frames` frames.
fn hold(nes: &mut nes_core::Nes, a: u8, b: u8, frames: usize) {
    for _ in 0..frames {
        nes.set_buttons(P1, a);
        nes.set_buttons(P2, b);
        nes.step_frame();
    }
}

/// Walk them apart, each to the door on their own side of the stairwell.
fn to_their_doors() -> nes_core::Nes {
    let mut nes = in_room_1();
    hold(&mut nes, BTN_LEFT, BTN_RIGHT, 100);
    hold(&mut nes, 0, 0, 10);
    nes
}

#[test]
fn the_two_pads_move_two_different_players() {
    let mut nes = in_room_1();
    assert_eq!(nes.system_ram()[P1_XH], SPAWN_1, "player 1 starts on his own spawn");
    assert_eq!(nes.system_ram()[P2_XH], SPAWN_2, "player 2 starts on hers");

    // Only pad 2 is touched. If port 2 were not reaching the cartridge, or if
    // both pads drove one player, one of these two would give it away.
    hold(&mut nes, 0, BTN_LEFT, 30);
    assert_eq!(
        nes.system_ram()[P1_XH], SPAWN_1,
        "player 1 moved while only pad 2 was held"
    );
    assert!(
        nes.system_ram()[P2_XH] < SPAWN_2,
        "pad 2 held LEFT and player 2 did not move"
    );
}

/// The cartridge's whole premise, on the machine rather than in a comment.
#[test]
fn one_of_them_in_a_door_does_not_end_the_room() {
    let mut nes = to_their_doors();

    // Player 1 alone stands in his door and asks to leave.
    hold(&mut nes, BTN_UP, 0, 30);

    assert_eq!(nes.system_ram()[P1_STATE], HS_WIN, "player 1 should be in his door");
    assert_eq!(
        nes.system_ram()[P2_STATE], HS_ALIVE,
        "player 2 did nothing and should still be playing"
    );
    assert_eq!(
        nes.system_ram()[ROOM_TIMER], 0,
        "the room ended on one player reaching a door, which is the one thing \
         this cartridge must never do"
    );
    assert_eq!(nes.system_ram()[MODE], M_PLAY, "still in the room");
}

#[test]
fn both_of_them_in_their_doors_ends_it() {
    let mut nes = to_their_doors();
    hold(&mut nes, BTN_UP, 0, 30);
    assert_eq!(nes.system_ram()[MODE], M_PLAY, "one door is not the way out");

    // Now she asks too.
    hold(&mut nes, BTN_UP, BTN_UP, 10);
    assert_eq!(nes.system_ram()[P2_STATE], HS_WIN, "player 2 should be in her door");
    assert_ne!(
        nes.system_ram()[ROOM_TIMER], 0,
        "both of them are in a door and the room did not end"
    );

    // Room 1 is the only room, so running it out lands on the ending screen.
    hold(&mut nes, 0, 0, 90);
    assert_eq!(nes.system_ram()[MODE], M_END, "the cartridge should have finished");
}

// ---------------------------------------------------------------------------
// Noise, and the man it fetches
// ---------------------------------------------------------------------------

const BTN_A: u8 = 0x01;

/// One jump and its landing, on one pad, from a standing start. Jumping does
/// not move you sideways, so this is the one way to make a mess without going
/// anywhere, which is what lets the tests below fill the bar and then still be
/// standing next to a wardrobe.
fn one_jump(nes: &mut nes_core::Nes, port: usize) {
    let other = 1 - port;
    nes.set_buttons(other, 0);
    for _ in 0..4 {
        nes.set_buttons(port, BTN_A);
        nes.step_frame();
    }
    for _ in 0..45 {
        nes.set_buttons(port, 0);
        nes.step_frame();
    }
}

/// Hold `a` and `b` until `done` says so, for at most `cap` frames. Returns
/// whether it happened.
///
/// The tests below watch for what they are waiting on rather than counting
/// frames to it. Counting looked fine and was wrong twice: `until_he_comes`
/// returns at the end of the jump that summoned him, which can be most of a
/// second after he was actually summoned, so every deadline measured from
/// there was already part spent.
fn wait_for(
    nes: &mut nes_core::Nes,
    a: u8,
    b: u8,
    cap: usize,
    done: impl Fn(&mut nes_core::Nes) -> bool,
) -> bool {
    for _ in 0..cap {
        nes.set_buttons(P1, a);
        nes.set_buttons(P2, b);
        nes.step_frame();
        if done(nes) {
            return true;
        }
    }
    false
}

#[test]
fn a_room_starts_silent() {
    let nes = &mut in_room_1();
    hold(nes, 0, 0, 60);
    assert_eq!(
        nes.system_ram()[NOISE],
        0,
        "the room opened by charging them for a noise neither of them made"
    );
    assert_eq!(nes.system_ram()[NB_STATE], NB_IDLE);
}

/// The asymmetry the whole cartridge hangs off: the one who can do the heavy
/// work is the one who gets you both caught.
#[test]
fn the_big_one_is_louder_than_the_small_one_at_the_same_thing() {
    let mut nes = in_room_1();
    one_jump(&mut nes, P1);
    let big = nes.system_ram()[NOISE];

    let mut nes = in_room_1();
    one_jump(&mut nes, P2);
    let small = nes.system_ram()[NOISE];

    assert!(big > 0, "the big one jumped and landed in silence");
    assert!(small > 0, "the small one should not be completely silent");
    assert!(
        big >= small * 3,
        "the big one cost {big} and the small one {small}, which is not enough \
         of a difference for a player to feel that it matters who moves"
    );
}

/// Standing still has to be a real move, or the only strategy is to hurry.
#[test]
fn staying_still_quietens_the_room_but_not_at_once() {
    let mut nes = in_room_1();
    one_jump(&mut nes, P1);
    let after = nes.system_ram()[NOISE];

    // Inside the grace period nothing is forgiven yet.
    hold(&mut nes, 0, 0, 30);
    assert_eq!(
        nes.system_ram()[NOISE],
        after,
        "the building started forgetting immediately, so being careless costs \
         nothing as long as you pause between mistakes"
    );

    // Past it, the room drains.
    hold(&mut nes, 0, 0, 180);
    assert!(
        nes.system_ram()[NOISE] < after,
        "a long silence did not quieten the room at all"
    );
}

/// Fill the bar by jumping on the spot, and stop as soon as he has heard it.
fn until_he_comes() -> nes_core::Nes {
    let mut nes = in_room_1();
    for _ in 0..40 {
        if nes.system_ram()[NB_STATE] != NB_IDLE {
            return nes;
        }
        one_jump(&mut nes, P1);
    }
    panic!(
        "forty jumps did not bring him out; the bar reached {} of {NOISE_MAX}",
        nes.system_ram()[NOISE]
    );
}

#[test]
fn filling_the_bar_brings_him_out() {
    let mut nes = until_he_comes();
    assert_eq!(
        nes.system_ram()[NOISE], NOISE_MAX,
        "he came out before the bar was full, so the bar is lying about when"
    );
}

#[test]
fn he_catches_whoever_is_not_in_a_wardrobe() {
    let mut nes = until_he_comes();
    let lives = nes.system_ram()[LIVES];

    // Neither of them moves, so the warning is spent standing in the open.
    assert!(
        wait_for(&mut nes, 0, 0, NB_WARN + 30, |n| n.system_ram()[NB_STATE] == NB_SWEEP),
        "he never came into the room"
    );
    assert!(
        wait_for(&mut nes, 0, 0, NB_SWEEP_FRAMES, |n| n.system_ram()[LIVES] < lives),
        "he swept a room with two people standing in the middle of it and          nothing happened"
    );
}

#[test]
fn a_wardrobe_is_the_answer_and_costs_no_button() {
    let mut nes = until_he_comes();
    let lives = nes.system_ram()[LIVES];

    // The warning is for this: one step each, into the wardrobe beside them.
    // Note that neither of them presses anything to hide. Walking in is all
    // of it, which is what lets a parked character stay hidden.
    hold(&mut nes, BTN_LEFT, BTN_RIGHT, 12);
    assert!(
        wait_for(&mut nes, 0, 0, NB_WARN + 30, |n| n.system_ram()[NB_STATE] == NB_SWEEP),
        "he never came into the room"
    );
    assert!(
        wait_for(&mut nes, 0, 0, NB_SWEEP_FRAMES + 120, |n| {
            n.system_ram()[NB_STATE] == NB_IDLE
        }),
        "he never finished sweeping and left"
    );

    assert_eq!(
        nes.system_ram()[LIVES], lives,
        "they were both in a wardrobe and he found them anyway"
    );
    assert_eq!(
        nes.system_ram()[NB_STATE], NB_IDLE,
        "he should have given up and gone back inside"
    );
    assert_eq!(
        nes.system_ram()[NOISE], 0,
        "the building should be quiet again once he has gone"
    );
}

/// One of them hiding is not enough, for the same reason one of them reaching
/// a door is not enough.
#[test]
fn one_of_them_hiding_is_not_enough() {
    let mut nes = until_he_comes();
    let lives = nes.system_ram()[LIVES];

    // Only the big one takes cover.
    hold(&mut nes, BTN_LEFT, 0, 12);
    assert!(
        wait_for(&mut nes, 0, 0, NB_WARN + NB_SWEEP_FRAMES + 30, |n| {
            n.system_ram()[LIVES] < lives
        }),
        "she was standing in the open and he walked straight past her"
    );
}

/// Four frames of lockstep netplay delay, applied the way `InputDelayBuffer`
/// applies it: both pads see the input late and the first frames see nothing.
/// The run has to reach the same ending, or something in the room needs input
/// to land on an exact frame and cannot be played over a link.
#[test]
fn it_still_finishes_under_four_frames_of_input_delay() {
    const DELAY: usize = 4;
    let mut nes = nes_core::Nes::from_rom(&rom()).expect("cartridge should load");
    let mut script: Vec<(u8, u8)> = Vec::new();
    script.extend(std::iter::repeat((0, 0)).take(40));
    script.extend(std::iter::repeat((BTN_START, 0)).take(8));
    script.extend(std::iter::repeat((0, 0)).take(12));
    script.extend(std::iter::repeat((BTN_LEFT, BTN_RIGHT)).take(100));
    script.extend(std::iter::repeat((0, 0)).take(10));
    script.extend(std::iter::repeat((BTN_UP, 0)).take(30));
    script.extend(std::iter::repeat((BTN_UP, BTN_UP)).take(10));
    script.extend(std::iter::repeat((0, 0)).take(90));

    for f in 0..script.len() + DELAY {
        let (a, b) = if f >= DELAY { script[f - DELAY] } else { (0, 0) };
        nes.set_buttons(P1, a);
        nes.set_buttons(P2, b);
        nes.step_frame();
    }
    assert_eq!(
        nes.system_ram()[MODE],
        M_END,
        "the same run finishes locally and not over a lockstep link, so \
         something in it is timed to the frame"
    );
}
