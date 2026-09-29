#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
"""Boot every disk in the corpus under both BIOSes and compare the handover.

Step three of replacing `disksys.rom` (see docs/notes/FDS-HLE.md). `fdsdiff`
stops each boot at handover, the instant the BIOS jumps to the game, and this
runs it over the whole corpus and counts.

## What counts as a pass, and what does not

The CONTRACT is the part a game can depend on:

  * it is handed control at all, through the $DFFC pseudo-vector
  * program RAM: the right files, chosen by the right rule, at the right
    addresses

REGISTERS are reported and not required. They looked like a contract at first,
because the three disks in the repo all hand over a=$10 x=$00 y=$FF sp=$FF
p=$20, but Xevious hands over x=$FF y=$00 p=$21. They are whatever the real
BIOS's last instruction left behind, they vary by disk, and so no game can be
depending on them.

WORK RAM is reported and not required. The real BIOS leaves about 700 bytes of
its own scratch in there, most of it with no published meaning, and reproducing
it byte for byte would mean deriving our implementation from theirs, which is
the one thing this may not do. The bytes that do have a meaning ($0100 to
$0103) are set, and the corpus is the judge of whether the rest matters.

Leftover BOOT ARTWORK is reported and not required. The real BIOS leaves its own
wordmark in pattern and nametable RAM; we leave ours. A game that loads its own
tiles overwrites both, and a game that does not was going to see somebody's boot
screen either way. That files of kind 1 and 2 land exactly where the real BIOS
puts them is checked in nes-core/tests/fds_hle_boot.rs, where it can be asked
about the file's own bytes rather than about the whole of video memory.

    cargo build --release -p nes-runner --bin fdsdiff
    python scripts/fds-hle-check.py
"""

import argparse
import collections
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import zipfile

DEFAULT_ZIP = r"G:\My Drive\FamiRust\Dumps\Cylum's Famicom Disk System ROM Collection (02-16-2021).zip"

ENTRY_RE = re.compile(r"entry \$(\w+) after (\d+) frames")
REG_RE = re.compile(r"a=\$(\w+) x=\$(\w+) y=\$(\w+) sp=\$(\w+) p=\$(\w+)")
NEVER_RE = re.compile(r"NEVER HANDED OVER")
DIFF_RE = re.compile(r"^  (work RAM|PRG RAM|CHR RAM|nametable RAM): (identical|(\d+) bytes)", re.M)


def group_of(name):
    if name.startswith("Translations/"):
        return "translation"
    if name.startswith("Hacks/"):
        return "hack"
    return "original"


def iter_disks(path):
    z = zipfile.ZipFile(path)
    for name in sorted(n for n in z.namelist() if n.lower().endswith(".zip")):
        title = os.path.splitext(os.path.basename(name))[0]
        try:
            inner = zipfile.ZipFile(io.BytesIO(z.read(name)))
        except zipfile.BadZipFile:
            continue
        fds = [n for n in inner.namelist() if n.lower().endswith(".fds")]
        if len(fds) == 1:
            yield title, group_of(name), inner.read(fds[0])


def check(binary, real, hle, data, frames, tmp):
    disk = os.path.join(tmp, "d.fds")
    with open(disk, "wb") as f:
        f.write(data)
    cmd = [binary, disk, "--bios", real, "--bios", hle, "--frames", str(frames)]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=900)
    except subprocess.TimeoutExpired:
        return {"verdict": "TIMEOUT"}
    out = p.stdout
    entries = ENTRY_RE.findall(out)
    regs = REG_RE.findall(out)
    row = {"nhandover": len(entries)}
    if NEVER_RE.search(out) or len(entries) < 2:
        # Which of the two failed to get there matters: ours failing is a bug,
        # the real BIOS failing means the disk needs something neither of us
        # does unattended, and the comparison says nothing either way.
        first = out.split("firmware")[0]
        row["verdict"] = "REAL-NO-HANDOVER" if NEVER_RE.search(first) else "HLE-NO-HANDOVER"
        return row
    row["entry_real"], row["frames_real"] = entries[0][0], int(entries[0][1])
    row["entry_hle"], row["frames_hle"] = entries[1][0], int(entries[1][1])
    diffs = dict((m[0], m[1]) for m in DIFF_RE.findall(out))
    row["diffs"] = diffs
    row["regs_match"] = len(regs) == 2 and regs[0] == regs[1]
    contract = row["entry_real"] == row["entry_hle"] and diffs.get("PRG RAM") == "identical"
    row["verdict"] = "CONTRACT" if contract else "DIFFER"
    return row


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--zip", default=DEFAULT_ZIP)
    ap.add_argument("--real", default="dumps/fds/disksys.rom")
    ap.add_argument("--hle", default="firmware/fds-hle/fds-hle.bin")
    ap.add_argument("--bin", dest="binary", default=None)
    ap.add_argument("--out", default="out/fds-hle-check.json")
    ap.add_argument("--frames", type=int, default=1500)
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    if args.binary is None:
        for c in ("target/release/fdsdiff.exe", "target/release/fdsdiff"):
            if os.path.exists(c):
                args.binary = c
                break
    if not args.binary or not os.path.exists(args.binary):
        sys.exit("no fdsdiff (cargo build --release -p nes-runner --bin fdsdiff)")
    binary = os.path.abspath(args.binary)
    real = os.path.abspath(args.real)
    hle = os.path.abspath(args.hle)

    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, (title, group, data) in enumerate(iter_disks(args.zip)):
            if args.limit and i >= args.limit:
                break
            row = check(binary, real, hle, data, args.frames, tmp)
            row["title"] = title
            row["group"] = group
            rows.append(row)
            print("%3d %-16s %-48s %s" % (i + 1, row["verdict"], title[:48], group),
                  flush=True)

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(rows, f, indent=1)

    print("\n=== handover against the real BIOS, %d disks ===" % len(rows))
    tally = collections.Counter((r["group"], r["verdict"]) for r in rows)
    kinds = sorted({v for _, v in tally})
    for g in sorted({r["group"] for r in rows}):
        n = sum(tally[(g, k)] for k in kinds)
        print("  %-12s %s  (%d)" % (g, "  ".join("%s=%d" % (k, tally[(g, k)]) for k in kinds), n))
    ok = sum(1 for r in rows if r["verdict"] == "CONTRACT")
    print("\n  %d of %d disks are handed the machine the real BIOS hands them." % (ok, len(rows)))
    same_chr = sum(1 for r in rows if r.get("diffs", {}).get("CHR RAM") == "identical")
    same_regs = sum(1 for r in rows if r.get("regs_match"))
    print("  of those, %d also match on pattern RAM and %d on the leftover registers."
          % (same_chr, same_regs))
    bad = [r for r in rows if r["verdict"] != "CONTRACT"]
    if bad:
        print("  not yet:")
        for r in bad:
            print("    %-16s %-46s %s" % (r["verdict"], r["title"][:46], r.get("diffs", "")))


if __name__ == "__main__":
    main()
