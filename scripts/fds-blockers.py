#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
"""Which unwritten BIOS routine is blocking the most titles, right now.

The census in `docs/notes/FDS-CENSUS-2026-09-29.md` ranks entry points by how
many titles CALL them, measured under the real BIOS. That is the right order to
measure in and the wrong order to implement in, because a title that calls five
routines is stopped by whichever one it reaches FIRST, and the other four buy
nothing until that one exists.

So this runs every corpus disk on OUR BIOS and asks a different question: which
stub did it hit, and which stub would unblock the most titles if it were
written next. A routine 30 titles call is worth less than one 8 titles are
stuck on.

    cargo build --release -p nes-runner --bin fdstrace
    python scripts/fds-blockers.py

Needs the corpus extracted under out/corpus (see scripts/fds-routine.py for how)
and a built HLE image at firmware/fds-hle/fds-hle.bin.
"""

import argparse
import collections
import glob
import os
import re
import subprocess
import sys

SRC = "firmware/fds-hle/src/main.s"
CORPUS = "out/corpus"
HLE = "firmware/fds-hle/fds-hle.bin"

# `.org $XXXX` on one line and `jmp unimplemented` on the next is a stub: an
# entry point we owe. Anything else at an .org is a routine we have written.
#
# This UNDERCOUNTS by one, knowingly. $E237 is also unwritten, but it sits two
# bytes before $E239 and so has no room for a `jmp` of its own; it carries a
# single `lda #0` and falls through into $E239's stub. A game reaching it still
# gets NO ROUTINE and `fdstrace` still names it, but it will not appear in the
# owed list below, and a title stopped there would be reported as having
# reached no stub at all. No corpus title does, as of 2026-10-07.
STUB = re.compile(r"^\.org \$([0-9A-Fa-f]{4})\s*$")


def stubs(path):
    out = set()
    with open(path, encoding="utf-8") as f:
        lines = [l.rstrip("\n") for l in f]
    for i, line in enumerate(lines[:-1]):
        m = STUB.match(line.strip())
        if m and lines[i + 1].strip() == "jmp unimplemented":
            out.add(int(m.group(1), 16))
    return out


def find_bin():
    for c in ("target/release/fdstrace.exe", "target/release/fdstrace"):
        if os.path.exists(c):
            return os.path.abspath(c)
    sys.exit("no fdstrace (cargo build --release -p nes-runner --bin fdstrace)")


ENTRY = re.compile(r"^    \$([0-9a-f]{4})\s+(\d+) x", re.M)
VERDICT = re.compile(r"-> (\w[\w-]*)\s*$", re.M)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--frames", type=int, default=1800)
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    owed = stubs(SRC)
    if not owed:
        print("No stubs left in %s. Every entry point the census found is written." % SRC)
        return
    print("%d entry points still owed\n" % len(owed))

    binary = find_bin()
    bios = os.path.abspath(HLE)
    if not os.path.exists(bios):
        sys.exit("no HLE image at %s (scripts/build-fds-bios.sh)" % HLE)
    disks = sorted(glob.glob(os.path.join(CORPUS, "*.fds")))
    if not disks:
        sys.exit("no disks under %s" % CORPUS)

    blocked_by = collections.defaultdict(list)
    clean = []
    env = dict(os.environ, FDS_BIOS=bios)
    for i, disk in enumerate(disks):
        if args.limit and i >= args.limit:
            break
        title = os.path.splitext(os.path.basename(disk))[0]
        try:
            p = subprocess.run([binary, disk, "--frames", str(args.frames), "--mash"],
                               capture_output=True, text=True, timeout=600, env=env)
        except subprocess.TimeoutExpired:
            print("%3d %-46s TIMEOUT" % (i + 1, title[:46]), flush=True)
            continue
        hit = [int(a, 16) for a, _ in ENTRY.findall(p.stdout)]
        # The stub it reached. A title can only ever be stopped by one at a
        # time, because our stub stops the machine.
        stuck = [a for a in hit if a in owed]
        if stuck:
            # The one it reached last is the one it is sitting on.
            blocked_by[stuck[-1]].append(title)
            print("%3d %-46s blocked on $%04X" % (i + 1, title[:46], stuck[-1]), flush=True)
        else:
            v = VERDICT.search(p.stdout)
            clean.append((title, v.group(1) if v else "?"))
            print("%3d %-46s %s" % (i + 1, title[:46], v.group(1) if v else "?"), flush=True)

    print("\n=== what to write next ===\n")
    ranked = sorted(blocked_by.items(), key=lambda kv: -len(kv[1]))
    for addr, titles in ranked:
        print("  $%04X  %3d title(s) stuck here" % (addr, len(titles)))
        for t in titles[:6]:
            print("           %s" % t[:60])
        if len(titles) > 6:
            print("           ... %d more" % (len(titles) - 6))
    total = sum(len(t) for t in blocked_by.values())
    print("\n  %d of %d titles are stopped by a missing routine." % (total, len(disks)))
    byv = collections.Counter(v for _, v in clean)
    print("  the other %d reached no stub at all: %s"
          % (len(clean), ", ".join("%s=%d" % kv for kv in sorted(byv.items()))))


if __name__ == "__main__":
    main()
