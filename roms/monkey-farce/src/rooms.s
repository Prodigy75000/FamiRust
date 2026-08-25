; SPDX-License-Identifier: CC0-1.0
;
; MONKEY FARCE -- the rooms.
;
; A room is typed out. That is the whole format: fourteen characters of name
; followed by thirteen rows of sixteen characters, and the .str directive turns
; each character into the byte the loader looks up in `charmap`. Nothing is
; compiled, packed or indexed by hand, so a room can be edited by editing the
; picture of it, which is the only way anyone was ever going to author enough
; of these to find out which ones are actually any good.
;
;   .  air                   =  platform, holds you up
;   #  wall                  ~  platform, does not
;   c  cracked floor (goes)   o  rubble (never held you up, and says so)
;   ^  spikes                 v  ceiling spikes
;   L  lava                   D  the door out (press UP in it)
;   *  torch                  |  chain            %  backdrop brick
;   S  where you start        B  a banana
;   W  saw                    C  crusher
;   >  wall that shoots right <  wall that shoots left
;
; Two rules that are not obvious from the picture and cost a playtest each:
;
;   Platforms go TWO rows apart, never three. Standing on a block puts you in
;   the row above it, and the jump clears 36 pixels, so row 8 to row 6 works
;   and row 8 to row 5 does not. Four of the six rooms in the first draft broke
;   this and every one of them looked fine.
;
;   A shooter goes in the row the player OCCUPIES, not the row of the floor
;   they are standing on. Its dart leaves at its own height, so a `>` drawn one
;   row too high sails over everybody's head, and the room becomes a corridor
;   with decorative gunfire in it.
;
; Neither is a matter of opinion, so neither is left to the eye:
; `tools/reach.py` simulates real jumps from every block you can stand on and
; reports any room where the door, or a banana, is out of reach.
;
; The asserts under each room are the other safety net: a row typed one
; character short would otherwise slide every later row left by one and produce
; a room that looks plausible and is not the one that was drawn.

ROOM_COUNT = 6

room_lo:
  .byte <room_1, <room_2, <room_3, <room_4, <room_5, <room_6
room_hi:
  .byte >room_1, >room_2, >room_3, >room_4, >room_5, >room_6

; ---------------------------------------------------------------------------
; 1. The teaching room. Two platforms in the middle of the bridge are not
;    platforms. There is no tell, because they are drawn with the same tiles
;    out of the same palette as the four that are: the only difference between
;    them lives in a flags table the player cannot see.
;
;    The bait hangs directly over the first liar, so the natural greedy line
;    (jump up, take it, come down) lands on exactly the block that is not
;    there. That is the entire lesson of the cartridge, taught once, cheaply.
; ---------------------------------------------------------------------------
room_1:
  .str "THE FIRST LIE "
  .str "################"
  .str "#..............#"
  .str "#.*..........*.#"
  .str "#..............#"
  .str "#....%%%%%%....#"
  .str "#..............#"
  .str "#......B.......#"
  .str "#..............#"
  .str "#....==~~==....#"
  .str "#S............D#"
  .str "#====^^^^^^^===#"
  .str "################"
  .str "################"
.assert room_2 - room_1 == NAME_LEN + ROOM_BYTES, "room 1 is not the right size"

; ---------------------------------------------------------------------------
; 2. Patience. Everything here is honest and it still kills you, because the
;    cracked floor gives you about a third of a second and the crusher only
;    commits once you are underneath it. The room is a rhythm test wearing a
;    trap's clothes.
; ---------------------------------------------------------------------------
room_2:
  .str "PATIENCE      "
  .str "################"
  .str "#vvvvvvvvvvvvvv#"
  .str "#..............#"
  .str "#...C...C......#"
  .str "#..............#"
  .str "#*............*#"
  .str "#..............#"
  .str "#..............#"
  .str "#...cc..cc..cc.#"
  .str "#S...........BD#"
  .str "#===LLLLLLLL===#"
  .str "################"
  .str "################"
.assert room_3 - room_2 == NAME_LEN + ROOM_BYTES, "room 2 is not the right size"

; ---------------------------------------------------------------------------
; 3. The gallery. The walls shoot, and which wall block is a shooter is not
;    marked. Each one takes its firing phase from where it sits in the grid, so
;    they never line up into one safe rhythm.
;
;    The wall the player spawns against does NOT shoot. It used to, and a dart
;    left it on the same frame the room appeared, which is not a hard opening
;    but a coin flip taken out of the player's hands. The gun on the far wall
;    takes about a second and three quarters to cross the room, which is the
;    difference between being shot at and being executed.
; ---------------------------------------------------------------------------
room_3:
  .str "THE GALLERY   "
  .str "################"
  .str "#..............#"
  .str "#..............#"
  .str "#..............#"
  .str "#..............#"
  .str "#....B...B.....#"
  .str "#...========...#"
  .str ">..B...........<"
  .str "#..=~==..====..#"
  .str "#S............D<"
  .str "#==============#"
  .str "################"
  .str "################"
.assert room_4 - room_3 == NAME_LEN + ROOM_BYTES, "room 3 is not the right size"

; ---------------------------------------------------------------------------
; 4. The mill. A saw takes its patrol from the room rather than from a
;    parameter: it looks left and right along its own row until it finds
;    something solid, and paddles between the two. Draw it a longer corridor
;    and it patrols a longer corridor. Nothing to tune, nothing to forget.
; ---------------------------------------------------------------------------
room_4:
  .str "THE MILL      "
  .str "################"
  .str "#..............#"
  .str "#.*..........*.#"
  .str "#..............#"
  .str "#...W......W...#"
  .str "#.....B..B.....#"
  .str "#....======....#"
  .str "#......W.......#"
  .str "#.===.===.~==..#"
  .str "#S...........BD#"
  .str "#====^^^^^^^===#"
  .str "################"
  .str "################"
.assert room_5 - room_4 == NAME_LEN + ROOM_BYTES, "room 4 is not the right size"

; ---------------------------------------------------------------------------
; 5. Free bananas. Seven of them up four tiers, laid out like a reward for
;    exploring. Most are over something. The room is honest in the only way
;    that matters: every one of them can be reached from somewhere you can
;    stand, which is checked rather than believed.
; ---------------------------------------------------------------------------
room_5:
  .str "FREE BANANAS  "
  .str "################"
  .str "#..............#"
  .str "#..............#"
  .str "#..B.......B...#"
  .str "#..=~.....~=...#"
  .str "#.....B..B.....#"
  .str "#....==..cc....#"
  .str "#.B.........B..#"
  .str "#.==~~...~~==..#"
  .str "#S...........BD#"
  .str "#===^^^^^^^^===#"
  .str "################"
  .str "################"
.assert room_6 - room_5 == NAME_LEN + ROOM_BYTES, "room 5 is not the right size"

; ---------------------------------------------------------------------------
; 6. The last lie. The door is visible from the start and the floor runs
;    all the way to it. It is not that simple, and the reason it is not that
;    simple is the one block in this room that has nothing wrong with it.
; ---------------------------------------------------------------------------
room_6:
  .str "THE LAST LIE  "
  .str "################"
  .str "#vvvvvvvvvvvvvv#"
  .str "#..............#"
  .str ">....C....C....<"
  .str "#..............#"
  .str "#..W.......W...#"
  .str "#..===..===....#"
  .str "#..............#"
  .str "#.==..cc..==.=.#"
  .str "#S...........BD#"
  .str "#==~~~^^^^~~~==#"
  .str "################"
  .str "################"
.assert rooms_end - room_6 == NAME_LEN + ROOM_BYTES, "room 6 is not the right size"

rooms_end:
