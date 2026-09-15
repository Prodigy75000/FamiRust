<!--
SPDX-License-Identifier: CC0-1.0
NOISY NEIGHBORS. Dedicated to the public domain; see LICENSE.
-->

# NOISY NEIGHBORS

A two-player escape for the NES. You are both locked in the same building on
the same bad night, and a room is not finished until **both** of you are
standing in a way out.

![Both of them at their own doors](screenshots/3-both-doors.png)

**[`noisy-neighbors.nes`](noisy-neighbors.nes)**: 40,976 bytes, mapper 0
(NROM), 32 KB PRG, 8 KB CHR, vertical mirroring. Runs on real hardware and on
any emulator.

## Status: v0.1, a foundation and not yet a game

This is honest rather than modest. What exists is the engine underneath the
game and one shakedown room to prove it, and the list below is what is missing.
Do not read the rest of this file as a description of something you can play.

**Done.** Two players on two pads, each with their own physics and their own
spawn. Two ways out, and a room that only ends when both of them are in one.
One of them dying taking the room away from both. The shared noise bar, the
neighbor it fetches, and the wardrobes you hide in. All of it proven on the
machine rather than asserted here, and proven again under four frames of
lockstep netplay delay.

**Not done.** The two of them being different from each other in what they can
*do*: the only asymmetry that exists so far is how loud they are. Carryable
crates, vents, held plates. Real rooms, of which there are none; there is one
room and it is a shakedown rig rather than a puzzle. Its own art and its own
tune, both still borrowed from MONKEY FARCE next door, and the wardrobe
currently reads too much like the door for something you have to pick out in
two seconds. `tools/reach2.py`, the two-body solver. The one-player mode.

## The one rule

One of you reaching a door is worth nothing. The room ends when both of you are
standing in one and both of you have asked to leave.

That sentence is the cartridge, and it is not left as a sentence. `room_verdict`
in `main.s` is where it lives, and
`crates/nes-core/tests/noisy_neighbors.rs` boots the ROM, walks two players to
two different doors, has one of them ask to leave, and fails if anything at all
happens. Making the room end on a single player flips that test, which is how
we know the test is worth having.

## Two things it will not do

**Nothing asks the two of you to act on the same frame.** Every gate that wants
both of you will open wide and stay open. This is meant to be played over a
lockstep netplay link, which delays every input by four frames on both seats,
and a window measured in single frames is exactly what cannot survive that. A
window of twenty frames survives it with room to spare, because human
coordination error on "press it together" is already several times larger than
the delay.

This is a claim, so it is measured. The scripted harness runs any script under
the real delay:

```sh
cargo run -p nes-runner --bin play -- roms/noisy-neighbors/noisy-neighbors.nes \
    "30=start;40=/;61=left/right;160=/;171=up/;201=up/up;300?mode=M_END" out --delay 4
```

A run that finishes at `--delay 0` and still finishes at `--delay 4` contains
no input that had to land on an exact frame. The same check runs as a test.

**Nothing is decided by who moves first.** A room one of you can lose for the
other by being early is a room two strangers cannot play.

## Noise

One bar, for the pair. That is the point of it: his mistakes fill your meter,
and there is no way to be individually careful.

The big one is five times as loud as the small one doing the same thing. A jump
and its landing cost him fifteen out of a hundred and ninety-two; they cost her
three. He leaves a trickle behind him when he walks and she leaves none. So the
one who can do the heavy work is the one who keeps getting you both caught,
which is the game and the joke at the same time.

Walking off a ledge is free. It is the one way down that costs nothing, and
rooms are allowed to be built around that.

Nothing drains for a full second after the last sound, and then it drains
quickly. That grace is the whole mechanism and the first version did not have
it. A flat drain has to be slower than the noise going in or nothing ever
accumulates, and faster than it or standing still is not a real move, and those
turned out to be the same number: one jump and its landing put in fifteen over
about fifty frames, and a flat drain of one per four frames took out almost
exactly fifteen over the same fifty. The bar sat at zero however carelessly it
was played.

## The neighbor

When the bar fills you hear him at his door and the tune stops. That is two
seconds of warning and it is all the warning there is. Then he comes in and
sweeps, and anyone he finds standing costs you the room.

Hiding is standing in a wardrobe. No button, no timer, no animation to be
caught halfway through, and any corner of your box overlapping one counts. A
hiding place whose angles have to be worked out is not a hiding place when the
working out has to happen in two seconds, and it would be worse again over a
link with input delay on it.

That rule is also what makes a one-player mode possible, which is worth writing
down before any room is built that quietly assumes otherwise:

> A gate may require **both bodies**. It may never require **both hands**.

Standing somewhere is shared state that survives you looking away. Pressing
something is not. Hold that line and one person swapping between the two
characters can solve every room the pair can, with no room redrawn and no
puzzle weakened, because a character you parked in a wardrobe is still in the
wardrobe while you are busy being the other one.

## Two of them, one set of variables

The hero physics came over from MONKEY FARCE unchanged, which was the point of
forking rather than rewriting: those routines had already been playtested into
shape. So there is one working hero in zero page and not two, and the player
being updated is swapped into it and back out again around his update.

The alternative was to index every hero variable by player, which means holding
X as a player number all the way through the physics and the collision code,
and those routines already use X and Y as scratch. Sixteen bytes in and sixteen
out, twice a frame, is sixty-four byte moves a frame. That is nothing, set
against rewriting the one part of the cartridge that was known to be right.

Everything from `hero_input` down to `ground_probe` therefore has no idea there
is another player at all. `tick_play` loops, `pick_pad` hands the working set
whichever pad belongs to it, and only `room_verdict` is ever told there are two.

## A room is a picture

Same format as next door, and the same reason: authoring has to be fast enough
that you can afford to throw rooms away.

```
room_1:
  .str "THE LANDING   "
  .str "################"
  .str "#..............#"
  .str "#.*..........*.#"
  .str "#..............#"
  .str "#....======....#"
  .str "#..............#"
  .str "#..............#"
  .str "#..............#"
  .str "#....======....#"
  .str "#..............#"
  .str "#DW1........2WD#"
  .str "#####^^^^^^#####"
  .str "################"
```

Fourteen characters of name, then thirteen rows of sixteen. `1` is where the
big one starts, `2` is where the small one starts, and `W` is a wardrobe. A
mistyped row fails the build.

## Building it

```sh
scripts/build-noisy-neighbors.sh
```

which regenerates the art and the tune, assembles, rewrites `SHA256SUMS`, and
runs both test suites. The committed `.nes` is checked against the committed
source on every `cargo test`.

## Licence

CC0 1.0 Universal. See [`LICENSE`](LICENSE). Use it for anything.

"Nintendo Entertainment System" and "Famicom" are trademarks of Nintendo.
FamiRust is an independent, clean-room reimplementation and is not affiliated
with or endorsed by Nintendo.
