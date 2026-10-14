#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
"""Break $EC22 on purpose, one way at a time, and check a test notices.

    python scripts/mutate-fds-metasprite.py      # about ten minutes

The same discipline as mutate-fds-queue.py: each mutation has to match its
anchor exactly once and has to change the assembled image, because a mutation
that silently failed to apply looks exactly like a suite that caught it. Each
one breaks a single measured behaviour of the metasprite blitter, so a miss
names the behaviour no test is holding.

Only the $EC22 tests are run, by name, which is what keeps this to minutes.
"""
import hashlib
import io
import os
import subprocess
import sys

SRC = "firmware/fds-hle/src/main.s"
BIN = "firmware/fds-hle/fds-hle.bin"

# The tests that exercise $EC22. Kept as a list rather than a filter string so
# a renamed test fails loudly below instead of quietly dropping out.
TESTS = [
    "metasprite_at_ec22_draws_a_grid_and_matches_the_real_bios",
    "druids_call_draws_literal_tiles_from_a_frame",
    "the_mode_is_picked_after_the_frame_moves_the_source",
    "the_control_byte_hides_on_bit_7_and_otherwise_draws_nothing",
    "a_zero_in_either_nibble_draws_nothing_and_leaves_x_alone",
    "flips_walk_the_tiles_backwards_and_never_move_a_sprite",
    "the_attribute_keeps_palette_and_priority_and_takes_flips_from_08",
    "a_row_at_f4_stays_there_rather_than_wrapping_back_on_screen",
    "a_grid_past_the_page_end_spills_y_attr_and_x_but_wraps_its_tiles",
    "the_exit_flags_come_from_the_last_sum_each_walk_does",
    "a_sweep_of_forced_blocks_matches_the_real_bios",
    "the_metasprite_is_not_slower_than_the_real_bios",
]

MUTANTS = [
    (
        "the attribute keeps bit 2",
        "  and #$23\n  ora flip_bits,x",
        "  and #$27\n  ora flip_bits,x",
    ),
    (
        "the flip bits are swapped",
        ("  .byte $00, $80, $00, $00", "  .byte $40, $C0\n"),
        ("  .byte $00, $40, $00, $00", "  .byte $80, $C0\n"),
    ),
    (
        "a row at $F4 wraps like any other",
        "  cmp #$F4                  ; $F4 stays $F4",
        "  cmp #$F5                  ; $F4 stays $F4",
    ),
    (
        "the hide bit parks rows at $F0",
        "  ldx #$F4                  ; bit 7",
        "  ldx #$F0                  ; bit 7",
    ),
    (
        "a nonzero control byte draws instead of returning",
        "  beq @show\n  bmi @hide",
        "  beq @show\n  bpl @show",
    ),
    (
        "the early exits clobber x",
        "  sta $02                   ; the OAM pointer, $0200 + offset",
        "  sta $02                   ; the OAM pointer, $0200 + offset\n  ldx #$00",
    ),
    (
        "strips step 16 apart",
        "  adc #$08\n  sta $0A\n  jmp @strip",
        "  adc #$10\n  sta $0A\n  jmp @strip",
    ),
    (
        "the frame is ignored",
        "  beq @moved                ; frame 0",
        "  jmp @moved                ; frame 0",
    ),
    (
        "the frame drops its carry into the high byte",
        "  bcc @nc\n  inc $08",
        "  bcc @nc\n  nop\n  nop",
    ),
    (
        "the mode is picked before the frame is added",
        "@moved:\n  ldy #$00\n  ldx #$00\n  lda $08\n  bne @ptrmode",
        "@moved:\n  ldy #$06\n  lda ($00),y\n  php\n  ldy #$00\n  ldx #$00\n  plp\n  bne @ptrmode",
    ),
    (
        "literal tiles are read through the pointer",
        "  tya                       ; literal: the tile is its own number",
        "  lda ($07),y               ; literal: the tile is its own number",
    ),
    (
        "both flips end one tile short",
        "  dey                       ; base + n - 1",
        "  nop                       ; base + n - 1",
    ),
    (
        "the linear walks leave the flips in $0C",
        "@linear:\n  lda $04\n  sta $0C",
        "@linear:\n  lda $04\n  nop\n  nop",
    ),
    (
        "horizontal leaves $0C at zero rather than -rows",
        "  lda $02\n  sta $0C                   ; -rows on the way out",
        "  lda $02\n  nop\n  nop                       ; -rows on the way out",
    ),
    (
        "vertical clears overflow on the way out",
        "  lda $06                   ; base + n\n  tay",
        "  lda $06                   ; base + n\n  clv\n  tay",
    ),
]


def assemble():
    subprocess.run(
        ["cargo", "run", "-q", "-p", "nes-asm", "--", SRC, "-o", BIN, "-s"],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    return hashlib.md5(open(BIN, "rb").read()).hexdigest()


def run_tests():
    cmd = ["cargo", "test", "--release", "-p", "nes-core", "--test", "fds_hle_routines", "--"]
    cmd += TESTS + ["--exact"]
    return subprocess.run(cmd, capture_output=True, text=True)


def failed_tests(out):
    """The test names libtest listed under its `failures:` summary."""
    names, collecting = [], False
    for line in out.splitlines():
        if line.startswith("failures:"):
            collecting = True
            continue
        if collecting:
            if line.startswith("    ") and line.strip():
                names.append(line.strip())
            elif line.strip():
                collecting = False
    return sorted(set(n for n in names if n.replace("_", "").isalnum()))


def main():
    original = io.open(SRC, encoding="utf-8").read()
    clean = assemble()
    print("clean image %s" % clean)
    r = run_tests()
    ran = r.stdout.count(" ... ok")
    if r.returncode != 0 or ran != len(TESTS):
        print("the clean image must pass all %d tests first (%d passed)" % (len(TESTS), ran))
        print(r.stdout[-2000:])
        sys.exit(2)

    caught, missed = [], []
    try:
        for name, anchor, repl in MUTANTS:
            anchors = anchor if isinstance(anchor, tuple) else (anchor,)
            repls = repl if isinstance(repl, tuple) else (repl,)
            s = original
            for a, b in zip(anchors, repls):
                n = s.count(a)
                if n != 1:
                    print("ANCHOR MISS (%d matches) for %r" % (n, name))
                    sys.exit(2)
                s = s.replace(a, b)
            io.open(SRC, "w", encoding="utf-8", newline="\n").write(s)
            md5 = assemble()
            if md5 == clean:
                print("IMAGE UNCHANGED for %r, so this proves nothing" % name)
                sys.exit(2)
            r = run_tests()
            if r.returncode == 0:
                missed.append(name)
                print("MISSED   %s" % name)
            else:
                hits = failed_tests(r.stdout + r.stderr)
                caught.append((name, hits))
                print("caught   %-50s by %d test(s): %s" % (name, len(hits), hits[0] if hits else "?"))
    finally:
        io.open(SRC, "w", encoding="utf-8", newline="\n").write(original)
        back = assemble()
        assert back == clean, "failed to restore: %s vs %s" % (back, clean)
        print("restored %s" % back)

    print("\n%d caught, %d missed" % (len(caught), len(missed)))
    for m in missed:
        print("  MISSED: %s" % m)
    sys.exit(1 if missed else 0)


if __name__ == "__main__":
    os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    main()
