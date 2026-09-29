#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
"""Profile one FDS BIOS routine across every disk in the corpus that calls it.

Step four of replacing `disksys.rom` (see docs/notes/FDS-HLE.md) is writing the
37 entry points the census found games using. Each one needs its interface
worked out first, and this is the plumbing for that: it looks up which titles
call the routine, runs `fdsprof` on each, and prints the profiles together so
they can be compared.

Comparing across titles is the whole point. One game's call tells you what that
game passes; six games' calls tell you which registers are arguments and which
are incidental, and which addresses are the routine's own state rather than that
game's data.

    python scripts/fds-routine.py EAEA              # every caller, up to --disks
    python scripts/fds-routine.py EAEA --trace      # plus the first call in order
    python scripts/fds-routine.py --list            # what is called by how many

## Setup

    cargo build --release -p nes-runner --bin fdsprof
    python scripts/fds-census.py            # writes out/fds-census/rows/, needed
                                            # to know who calls what
    # and the disks themselves, extracted from the corpus archive:
    python - <<'EOF'
    import zipfile, io, os
    Z = r"G:\\My Drive\\FamiRust\\Dumps\\Cylum's Famicom Disk System ROM Collection (02-16-2021).zip"
    safe = lambda t: "".join(c if c.isalnum() or c in " -_." else "_" for c in t)
    z = zipfile.ZipFile(Z)
    os.makedirs("out/corpus", exist_ok=True)
    for name in sorted(x for x in z.namelist() if x.lower().endswith(".zip")):
        iz = zipfile.ZipFile(io.BytesIO(z.read(name)))
        fds = [x for x in iz.namelist() if x.lower().endswith(".fds")]
        if len(fds) == 1:
            t = safe(os.path.splitext(os.path.basename(name))[0])
            open("out/corpus/%s.fds" % t, "wb").write(iz.read(fds[0]))
    EOF
"""

import argparse
import collections
import glob
import json
import os
import subprocess
import sys

ROWS = "out/fds-census/rows"
CORPUS = "out/corpus"
BIOS = "dumps/fds/disksys.rom"


def census():
    """title -> set of entry addresses that title was seen entering."""
    out = {}
    for p in sorted(glob.glob(os.path.join(ROWS, "*.json"))):
        with open(p, encoding="utf-8") as f:
            row = json.load(f)
        title = os.path.splitext(os.path.basename(p))[0]
        out[title] = {t["to"] for t in row.get("targets", [])}
    return out


def find_bin():
    for c in ("target/release/fdsprof.exe", "target/release/fdsprof"):
        if os.path.exists(c):
            return os.path.abspath(c)
    sys.exit("no fdsprof (cargo build --release -p nes-runner --bin fdsprof)")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("routine", nargs="?", help="entry address in hex, e.g. EAEA")
    ap.add_argument("--list", action="store_true", help="list routines by how many titles call them")
    ap.add_argument("--disks", type=int, default=6, help="how many callers to profile")
    ap.add_argument("--calls", type=int, default=6, help="how many calls per disk")
    ap.add_argument("--frames", type=int, default=3000)
    ap.add_argument("--trace", action="store_true", help="also dump the first call in order")
    ap.add_argument("--trace-max", type=int, default=0,
                    help="how much of the trace to print; a disk load is tens of thousands long")
    ap.add_argument("--only", help="profile just this title (a filename stem in out/corpus)")
    args = ap.parse_args()

    if not os.path.isdir(ROWS):
        sys.exit("no census rows at %s (run scripts/fds-census.py first)" % ROWS)
    cen = census()

    if args.list:
        tally = collections.Counter()
        for _, addrs in cen.items():
            for a in addrs:
                tally[a] += 1
        for a, n in sorted(tally.items(), key=lambda kv: (-kv[1], kv[0])):
            print("  $%04X  %3d titles" % (a, n))
        return
    if not args.routine:
        ap.error("give a routine address, or --list")

    target = int(args.routine.lstrip("$").lstrip("0x"), 16)
    binary = find_bin()
    bios = os.path.abspath(BIOS)
    if not os.path.exists(bios):
        sys.exit("this tool profiles the REAL BIOS and %s is missing" % BIOS)

    callers = sorted(t for t, addrs in cen.items() if target in addrs)
    if args.only:
        callers = [args.only]
    print("$%04X is entered by %d of the %d titles in the census\n"
          % (target, len(callers), len(cen)))
    if not callers:
        print("Nothing calls it. Either the census never reached the code that does,")
        print("or it is only ever reached from inside the BIOS.")
        return

    env = dict(os.environ, FDS_BIOS=bios)
    done = 0
    for title in callers:
        disk = os.path.join(CORPUS, title + ".fds")
        if not os.path.exists(disk):
            continue
        cmd = [binary, disk, "--routine", "%04X" % target,
               "--calls", str(args.calls), "--frames", str(args.frames)]
        if args.trace and done == 0:
            cmd.append("--trace")
            if args.trace_max:
                cmd += ["--trace-max", str(args.trace_max)]
        try:
            p = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=900)
        except subprocess.TimeoutExpired:
            print("%s: timed out\n" % title)
            continue
        print(p.stdout.rstrip())
        if p.stderr.strip():
            print("  stderr: %s" % p.stderr.strip())
        print()
        done += 1
        if done >= args.disks:
            break
    if done == 0:
        print("None of the callers are extracted under %s/" % CORPUS)


if __name__ == "__main__":
    main()
