; SPDX-License-Identifier: CC0-1.0
;
; NOISY NEIGHBORS -- the rooms.
;
; A room is typed out. That is the whole format: fourteen characters of name
; followed by thirteen rows of sixteen characters, and the .str directive turns
; each character into the byte the loader looks up in `charmap`. Nothing is
; compiled, packed or indexed by hand, so a room can be edited by editing the
; picture of it, which is the only way anyone was ever going to author enough
; of these to find out which ones are actually any good.
;
;   .  air                   =  platform
;   #  wall                   %  backdrop brick
;   ^  spikes                 v  ceiling spikes
;   c  cracked floor          o  rubble
;   D  a way out (stand in it and press UP)
;   W  a wardrobe (stand in it and you are hidden; no button, no timer)
;   *  torch                  |  chain
;   1  where the big one starts
;   2  where the small one starts
;
; Hiding is generous on purpose: any corner of your box overlapping a wardrobe
; counts. You have two seconds from hearing him, and a hiding place you have to
; line up exactly is not one.
;
; Two of them, and two ways out, and a room does not end until BOTH of them are
; standing in one. One of you reaching the door is worth nothing, which is the
; entire cartridge stated as a rule: see room_verdict in main.s.
;
; The rule that costs a playtest, inherited from the same engine: platforms go
; TWO rows apart, never three. Standing on a block puts you in the row above
; it and the jump clears 36 pixels, so row 8 to row 6 works and row 8 to row 5
; does not. Four of six rooms in the first draft of the previous cartridge
; broke this and every one of them looked fine.
;
; The asserts under each room are the safety net a picture format needs: a row
; typed one character short would otherwise slide every later row left by one
; and produce a room that looks plausible and is not the one that was drawn.

ROOM_COUNT = 1

room_lo:
  .byte <room_1
room_hi:
  .byte >room_1

; ---------------------------------------------------------------------------
; 1. The landing. A shakedown room and not yet a puzzle: two people, two ways
;    out, and a stairwell between them that neither of them should walk into.
;
;    It exists to be driven by the harness rather than to be played. It proves
;    the things that have to be true before any actual room is worth drawing:
;    that the pair take their own pads and move independently, that each of
;    them can reach their own door, that neither door opens on its own, and
;    that one of them stepping into the spikes takes the room away from both.
;
;    The two wardrobes are here for the same reason and are placed for the
;    harness rather than for a player: one within a step of each spawn, so the
;    tests can put somebody in one and somebody else not in one without any of
;    it depending on exactly how far a walk carries.
; ---------------------------------------------------------------------------
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
.assert room_end - room_1 == NAME_LEN + ROOM_BYTES, "room 1 is not the right size"
room_end:
