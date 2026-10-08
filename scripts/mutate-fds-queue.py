#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
"""Break $E8D2 and $E8E1 on purpose, one way at a time, and check a test notices.

    python scripts/mutate-fds-queue.py      # about fifteen minutes


A green suite is only evidence if it could have been red. Each mutation below
is byte-neutral (the `.assert` on the routines' shared size would otherwise
refuse it before any test ran) or fixes up that assert itself, and each is
checked twice: the anchor has to match exactly once, and the assembled image
has to actually change. A mutation that silently failed to apply looks exactly
like a test suite that caught it.

## One mutation was dropped because it is a provable no-op

An earlier sweep swapped `dec $06` and `sta BUF,x` inside the copy loop and no
test noticed. That is not a gap: the two instructions commute. The store goes
to $0302+x and can never be $0006, the store sets no flags, so the `bne` reads
the `dec`'s Z either way, and the cycle count is the same. The bytes move and
nothing observable does. It is listed here rather than in the table so the next
person does not spend an afternoon writing a test for it.
"""
import hashlib
import io
import os
import subprocess
import sys

SRC = "firmware/fds-hle/src/main.s"
BIN = "firmware/fds-hle/fds-hle.bin"

MUTANTS = [
    # --- the shared tail --------------------------------------------------
    (
        "the header goes in as low-then-high",
        "  lda $03                   ; 3\n  sta BUF,x                 ; 5\n  inx",
        "  lda $02                   ; 3\n  sta BUF,x                 ; 5\n  inx",
    ),
    (
        "the write index is taken as zero rather than read",
        "  ldx BUF-1                 ; 4   the write index",
        "  ldx #$00\n  nop                       ; 4   the write index",
    ),
    (
        "the limit is read from the index instead of the size",
        "@data:\n  inx                       ; 2\n  cpx BUF-2                 ; 4",
        "@data:\n  inx                       ; 2\n  cpx BUF-1                 ; 4",
    ),
    (
        "no room is kept for the terminator",
        "  cpx BUF-2                 ; 4\n  bcs @full                 ; 2\n  stx BUF-1",
        "  nop\n  nop\n  nop\n  nop\n  nop                       ; 2\n  stx BUF-1",
    ),
    (
        "the address moves on by 31 instead of 32",
        "  adc #$20                  ; 2   on to the next row",
        "  adc #$1f                  ; 2   on to the next row",
    ),
    (
        "the block counter is not decremented",
        "  dec $05                   ; 5\n  bne queue_block",
        "  dec $04                   ; 5\n  bne queue_block",
    ),
    (
        "an overflow truncates instead of rolling back",
        "@full:\n  ldx BUF-1                 ; 4   roll back",
        "@full:\n  nop\n  nop\n  nop                       ; 4   roll back",
    ),
    (
        "an overflow is reported as a success",
        "  lda #$01                  ; 2   the error code",
        "  lda #$FF                  ; 2   the error code",
    ),
    (
        "the copy costs two cycles a byte more",
        (
            "  dec $06                   ; 5\n  bne @data                 ; 3 taken",
            ".assert (vram_queue_end - vram_queue) == 169",
        ),
        (
            "  dec $06                   ; 5\n  nop\n  bne @data                 ; 3 taken",
            ".assert (vram_queue_end - vram_queue) == 170",
        ),
    ),
    (
        "the caller is stepped over one operand byte instead of two",
        "  adc #$02                  ; 2   through",
        "  adc #$01                  ; 2   through",
    ),
    # --- $E8D2 ------------------------------------------------------------
    (
        "$E8D2 starts its copy index one byte late",
        "  ldy #$FF                  ; 2   the copy loop",
        "  ldy #$00                  ; 2   the copy loop",
    ),
    # --- $E8E1 ------------------------------------------------------------
    (
        "$E8E1 reads the packed nibbles the other way round",
        (
            "  and #$0F                  ; 2   cheaper than keeping it: bytes per block\n  sta $04",
            "  lsr a                     ; 2   and blocks\n  sta $05",
        ),
        (
            "  and #$0F                  ; 2   cheaper than keeping it: bytes per block\n  sta $05",
            "  lsr a                     ; 2   and blocks\n  sta $04",
        ),
    ),
    (
        "$E8E1 takes the whole shape byte as the count",
        "  and #$0F                  ; 2   cheaper than keeping it",
        "  and #$FF                  ; 2   cheaper than keeping it",
    ),
    (
        "$E8E1 copies the shape byte as data",
        "  ldy #$00                  ; 2\n  lda ($00),y               ; 5   the shape byte",
        "  ldy #$FF                  ; 2\n  lda ($00),y               ; 5   the shape byte",
    ),
]


def assemble():
    subprocess.run(
        ["cargo", "run", "-q", "-p", "nes-asm", "--", SRC, "-o", BIN, "-s"],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    return hashlib.md5(open(BIN, "rb").read()).hexdigest()


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

    caught, missed = [], []
    try:
        for name, anchor, repl in MUTANTS:
            anchors = anchor if isinstance(anchor, tuple) else (anchor,)
            repls = repl if isinstance(repl, tuple) else (repl,)
            s = original
            for a, r in zip(anchors, repls):
                n = s.count(a)
                if n != 1:
                    print("ANCHOR MISS (%d matches) for %r" % (n, name))
                    sys.exit(2)
                s = s.replace(a, r)
            io.open(SRC, "w", encoding="utf-8", newline="\n").write(s)
            md5 = assemble()
            if md5 == clean:
                print("IMAGE UNCHANGED for %r, so this proves nothing" % name)
                sys.exit(2)
            r = subprocess.run(
                ["cargo", "test", "-p", "nes-core", "--test", "fds_hle_routines"],
                capture_output=True,
                text=True,
            )
            if r.returncode == 0:
                missed.append(name)
                print("MISSED   %s" % name)
            else:
                hits = failed_tests(r.stdout + r.stderr)
                caught.append((name, hits))
                print("caught   %-55s by %d test(s): %s" % (name, len(hits), hits[0] if hits else "?"))
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
