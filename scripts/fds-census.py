#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Prodigy75000
"""Run `fdstrace` over the whole FDS corpus and roll the rows up into a census.

Step one of replacing `disksys.rom` (see docs/notes/FDS-HLE.md). The question
this answers is "which BIOS entry points does the library actually use", and it
has to be answered by measurement: a static scan of the disk images for the JSR
opcode returns 164 targets and most are data bytes that read as JSR.

The corpus lives OUTSIDE this repo, the way ColecoRust keeps its own:

    G:\\My Drive\\FamiRust\\Dumps\\Cylum's Famicom Disk System ROM Collection (02-16-2021).zip

114 games as 114 nested zips, one .fds each, sorted into Translations/ (40) and
Hacks/ (2) with the remaining 72 being original Japanese releases. All 114 get
traced, because a translation calls the same BIOS as the game it patches and is
a free cross-check, but the headline denominator is the 72 originals.

## What this cannot see

An unattended run reaches what an unattended run reaches. A game that wants a
button we do not press, or a disk side we do not have, contributes only the
routines it used before it stopped. That is why every row carries a verdict and
why the summary counts booted titles separately: a routine missing from this
census is a routine we did not observe, not a routine nothing calls.

## Usage

    cargo build --release -p nes-runner --bin fdstrace
    python scripts/fds-census.py                       # ~12 minutes, 114 titles
    python scripts/fds-census.py --from-rows           # redo the summary only
"""

import argparse
import collections
import glob
import io
import json
import os
import subprocess
import sys
import tempfile
import zipfile

DEFAULT_ZIP = r"G:\My Drive\FamiRust\Dumps\Cylum's Famicom Disk System ROM Collection (02-16-2021).zip"

# An ENTRY is control arriving at a routine: a call, a jump, or a vector.
#
# An `rts`/`rti` in game code landing in the BIOS is a RESUMPTION, not an entry.
# The BIOS called out to the game through the `$DFF6-$DFFF` RAM vectors and the
# game handed control back, so the address it lands on is wherever the BIOS
# happened to be, not anything published. Folding the two together inflated the
# first run of this census from 40 entry points to 119, and the extra were
# clustered in two narrow bands ($E9EB-$EA62 and $E14A-$E15B) that are plainly
# the middle of one routine rather than a hundred routines.
ENTRY_KINDS = ("jsr", "jmp", "jmp()", "brk", "nmi", "irq", "reset")

# Writes to $4024/$4025 at or above which a game counts as driving the drive
# itself rather than poking it at init. The cut is not arbitrary: on the
# 2026-09-29 run the distinct nonzero counts were 1, 2, 3, 8, 16, 61, 196, and
# then up into the thousands, so there is a wide gap either side of 16 and the
# cut does not sit on a slope. One title (Ginga Denshou) is the lone 16 and is
# the only genuinely ambiguous case.
DRIVES_OWN_MIN = 16


def group_of(name):
    """Which shelf of the archive a nested zip sits on."""
    if name.startswith("Translations/"):
        return "translation"
    if name.startswith("Hacks/"):
        return "hack"
    return "original"


def iter_disks(path):
    """Yield (title, group, fds_bytes, problem) for every disk in the corpus.

    Accepts either the nested archive or a plain directory of .fds files, so a
    single title can be re-run without unpacking 114 zips.
    """
    if os.path.isdir(path):
        for root, _, files in os.walk(path):
            for f in sorted(files):
                if f.lower().endswith(".fds"):
                    with open(os.path.join(root, f), "rb") as fh:
                        yield os.path.splitext(f)[0], "original", fh.read(), None
        return

    z = zipfile.ZipFile(path)
    for name in sorted(n for n in z.namelist() if n.lower().endswith(".zip")):
        title = os.path.splitext(os.path.basename(name))[0]
        try:
            inner = zipfile.ZipFile(io.BytesIO(z.read(name)))
        except zipfile.BadZipFile as e:
            yield title, group_of(name), None, "bad inner zip: %s" % e
            continue
        fds = [n for n in inner.namelist() if n.lower().endswith(".fds")]
        if len(fds) != 1:
            yield title, group_of(name), None, "%d .fds inside" % len(fds)
            continue
        yield title, group_of(name), inner.read(fds[0]), None


def trace_one(binary, bios, data, frames, extra, tmpdir):
    """Run the tracer on one disk image; return its JSON row or an error dict."""
    disk = os.path.join(tmpdir, "disk.fds")
    out = os.path.join(tmpdir, "row.json")
    with open(disk, "wb") as f:
        f.write(data)
    if os.path.exists(out):
        os.remove(out)
    env = dict(os.environ, FDS_BIOS=bios)
    cmd = [binary, disk, "--frames", str(frames), "--json", out] + extra
    try:
        p = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=900)
    except subprocess.TimeoutExpired:
        return {"error": "timeout"}
    if not os.path.exists(out):
        return {"error": (p.stderr or p.stdout or "no output").strip()[:300]}
    with open(out, encoding="utf-8") as f:
        return json.load(f)


def safe_name(title):
    return "".join(c if c.isalnum() or c in " -_." else "_" for c in title)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--zip", default=DEFAULT_ZIP, help="corpus archive or a directory of .fds")
    ap.add_argument("--bios", default="dumps/fds/disksys.rom")
    ap.add_argument("--bin", dest="binary", default=None,
                    help="tracer path (default target/release/fdstrace[.exe])")
    ap.add_argument("--out", default="out/fds-census")
    ap.add_argument("--frames", type=int, default=1800, help="~30s per title")
    ap.add_argument("--limit", type=int, default=0, help="stop after N titles (smoke test)")
    ap.add_argument("--no-mash", action="store_true", help="do not tap START")
    ap.add_argument("--no-autoswap", action="store_true", help="do not turn the disk over")
    ap.add_argument("--from-rows", action="store_true",
                    help="rebuild the summary from rows already on disk, trace nothing")
    args = ap.parse_args()

    rows_dir = os.path.join(args.out, "rows")
    if args.from_rows:
        paths = sorted(glob.glob(os.path.join(rows_dir, "*.json")))
        if not paths:
            sys.exit("no rows under %s" % rows_dir)
        rows = []
        for p in paths:
            with open(p, encoding="utf-8") as f:
                rows.append(json.load(f))
        print("rebuilt from %d rows" % len(rows))
    else:
        rows = trace_all(args, rows_dir)

    write_summary(rows, os.path.join(args.out, "SUMMARY.md"), args)
    print("\nwrote %s" % os.path.join(args.out, "SUMMARY.md"))


def trace_all(args, rows_dir):
    if args.binary is None:
        for cand in ("target/release/fdstrace.exe", "target/release/fdstrace"):
            if os.path.exists(cand):
                args.binary = cand
                break
    if args.binary is None or not os.path.exists(args.binary):
        sys.exit("no tracer (cargo build --release -p nes-runner --bin fdstrace)")
    # Absolute, because a relative path plus a rewritten env is enough to make
    # CreateProcess fail with a bare "cannot find the file specified".
    args.binary = os.path.abspath(args.binary)
    args.bios = os.path.abspath(args.bios)
    if not os.path.exists(args.bios):
        sys.exit("no BIOS at %s" % args.bios)
    extra = []
    if not args.no_mash:
        extra.append("--mash")
    if not args.no_autoswap:
        extra.append("--autoswap")

    os.makedirs(rows_dir, exist_ok=True)
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, (title, group, data, problem) in enumerate(iter_disks(args.zip)):
            if args.limit and i >= args.limit:
                break
            if problem is not None:
                row = {"title": title, "group": group, "verdict": "UNREADABLE",
                       "error": problem}
            else:
                row = trace_one(args.binary, args.bios, data, args.frames, extra, tmp)
                row["title"] = title
                row["group"] = group
                if "error" in row:
                    row["verdict"] = "ERROR"
            rows.append(row)
            with open(os.path.join(rows_dir, safe_name(title) + ".json"), "w",
                      encoding="utf-8") as f:
                json.dump(row, f, indent=1)
            print("%3d %-12s %-52s %s"
                  % (i + 1, row.get("verdict", "?"), title[:52], group), flush=True)
    return rows


def write_summary(rows, path, args):
    verdicts = collections.Counter((r["group"], r.get("verdict", "?")) for r in rows)
    groups = sorted({r["group"] for r in rows})
    kinds = sorted({v for (_, v) in verdicts})

    used = collections.defaultdict(lambda: collections.defaultdict(set))
    used_boot = collections.defaultdict(set)
    kind_of = {}
    cycles = collections.defaultdict(int)
    calls = collections.defaultdict(int)
    resumes = collections.defaultdict(set)
    reads_beyond = []
    drives_own = []
    only_vectors = 0
    for r in rows:
        if "targets" not in r:
            continue
        booted = r.get("verdict") == "BOOT"
        for t in r["targets"]:
            a, k = t["to"], t["kind"]
            if k not in ENTRY_KINDS:
                resumes[a].add(r["title"])
                continue
            used[a][r["group"]].add(r["title"])
            if booted:
                used_boot[a].add(r["title"])
            calls[a] += t["count"]
            cycles[a] += t["cycles_in_window"]
            if k in ("nmi", "irq", "reset") or a not in kind_of:
                kind_of[a] = k
        # Everything outside $FFFA-$FFFF, which every game reads because that is
        # where the CPU fetches its vectors from.
        beyond = [(lo, hi) for lo, hi in r.get("read_outside_ranges", []) if lo < 0xfffa]
        if beyond:
            reads_beyond.append((r["title"], r["group"], r.get("verdict"), beyond,
                                 sum(hi - lo + 1 for lo, hi in beyond)))
        else:
            only_vectors += 1
        if r.get("diskreg_writes", {}).get("by_game", 0) > 0:
            drives_own.append((r["title"], r["diskreg_writes"]["by_game"]))

    n_orig = sum(1 for r in rows if r["group"] == "original")
    boot_orig = sum(1 for r in rows if r["group"] == "original" and r.get("verdict") == "BOOT")

    L = []
    L.append("# FDS BIOS census\n")
    L.append("Generated by `scripts/fds-census.py`, which runs `fdstrace` over every")
    L.append("disk in the corpus and folds the rows together. Do not hand-edit.\n")
    L.append("Corpus: `%s`" % args.zip)
    L.append("Run: %d frames per title (~%.0fs), mash=%s autoswap=%s\n"
             % (args.frames, args.frames / 60.0, not args.no_mash, not args.no_autoswap))
    L.append("**This is a census of what an unattended run reached.** A routine absent")
    L.append("from it is a routine we did not observe, not a routine nothing calls: a")
    L.append("game that wants a button we never press, or a disk side this dump does not")
    L.append("carry, contributes only what it used before it stopped.\n")

    L.append("## Verdicts\n")
    L.append("| group | " + " | ".join(kinds) + " | total |")
    L.append("|---|" + "---|" * (len(kinds) + 1))
    for g in groups:
        n = sum(verdicts[(g, k)] for k in kinds)
        L.append("| %s | %s | %d |" % (g, " | ".join(str(verdicts[(g, k)]) for k in kinds), n))
    L.append("")

    L.append("## BIOS entry points\n")
    L.append("%d distinct entry points across %d titles. `originals` is the headline"
             % (len(used), len(rows)))
    L.append("denominator (%d titles, %d of which booted); `booted` counts only titles"
             % (n_orig, boot_orig))
    L.append("that reached their own code, which is the stronger observation.\n")
    L.append("`cyc/call` is CPU cycles per uninterrupted stay inside the window, not the")
    L.append("cost of the routine: the BIOS can hand control to a game hook through the")
    L.append("`$DFF6-$DFFF` RAM vectors, and that closes the accounting early.\n")
    L.append("| entry | kind | originals | translations | hacks | booted | calls | cyc/call |")
    L.append("|---|---|---|---|---|---|---|---|")
    order = sorted(used, key=lambda a: (-len(used[a]["original"]), -len(used_boot[a]), a))
    for a in order:
        c = calls[a]
        L.append("| `$%04X` | %s | %d/%d | %d | %d | %d | %d | %d |"
                 % (a, kind_of.get(a, "?"), len(used[a]["original"]), n_orig,
                    len(used[a]["translation"]), len(used[a]["hack"]),
                    len(used_boot[a]), c, cycles[a] // c if c else 0))
    L.append("")

    L.append("## Returns into the BIOS, which are not entry points\n")
    L.append("%d addresses an `rts`/`rti` in game code landed on. These are the BIOS"
             % len(resumes))
    L.append("resuming after it called out through the `$DFF6-$DFFF` RAM vectors, so the")
    L.append("address is wherever the BIOS happened to be rather than anything published.")
    L.append("They are kept out of the table above because counting them as entry points")
    L.append("trebled it, and kept here because they are the evidence about how the")
    L.append("vector dispatch behaves.\n")
    L.append("| address | titles |")
    L.append("|---|---|")
    for a in sorted(resumes, key=lambda a: (-len(resumes[a]), a))[:40]:
        L.append("| `$%04X` | %d |" % (a, len(resumes[a])))
    if len(resumes) > 40:
        L.append("| ... %d more | |" % (len(resumes) - 40))
    L.append("")

    L.append("## BIOS bytes the GAME reads directly\n")
    L.append("Counted while the CPU was executing OUTSIDE `$E000-$FFFF`, with")
    L.append("`$FFFA-$FFFF` dropped because every game reads the vectors.\n")
    L.append("This is the constraint on what a synthetic image may contain. A byte only")
    L.append("the BIOS reads goes away with the routine that read it; a byte the game")
    L.append("reads has to still be there.\n")
    L.append("**%d of %d titles read nothing but the vectors.** The rest:\n"
             % (only_vectors, len(rows)))
    if not reads_beyond:
        L.append("(none)\n")
    else:
        L.append("| title | group | verdict | bytes | ranges |")
        L.append("|---|---|---|---|---|")
        for t, g, v, rs, n in sorted(reads_beyond, key=lambda x: -x[4]):
            L.append("| %s | %s | %s | %d | %s |"
                     % (t, g, v, n, " ".join("`$%04X-$%04X`" % (lo, hi) for lo, hi in rs)))
        L.append("")

    L.append("## Titles that drive `$4024`/`$4025` themselves\n")
    L.append("These keep the hardware transfer engine load-bearing no matter how good the")
    L.append("HLE gets, so the HLE stays strictly additive.\n")
    L.append("Volume is what separates the two cases, so the table is cut at %d. A game"
             % DRIVES_OWN_MIN)
    L.append("that writes these registers once or twice at init is poking the drive; a")
    L.append("game that writes them hundreds of times is running its own read loop and")
    L.append("needs the real hardware underneath it whatever the BIOS does.\n")
    heavy = [(t, n) for t, n in drives_own if n >= DRIVES_OWN_MIN]
    light = [(t, n) for t, n in drives_own if n < DRIVES_OWN_MIN]
    if not heavy:
        L.append("None observed.\n")
    else:
        L.append("| title | writes |")
        L.append("|---|---|")
        for t, n in sorted(heavy, key=lambda x: -x[1]):
            L.append("| %s | %d |" % (t, n))
        L.append("")
    L.append("%d further titles wrote between 1 and %d times, which is an init poke.\n"
             % (len(light), DRIVES_OWN_MIN - 1))

    L.append("## Per title\n")
    L.append("| title | group | verdict | entries | sides | swaps | $402x by game |")
    L.append("|---|---|---|---|---|---|---|")
    for r in sorted(rows, key=lambda r: (r["group"], r["title"])):
        ents = "-"
        if "targets" in r:
            ents = sum(1 for t in r["targets"] if t["kind"] in ENTRY_KINDS)
        L.append("| %s | %s | %s | %s | %s | %s | %s |"
                 % (r["title"], r["group"], r.get("verdict", "?"), ents,
                    r.get("sides", "-"), r.get("swaps", "-"),
                    r.get("diskreg_writes", {}).get("by_game", "-")))
    L.append("")

    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(L))


if __name__ == "__main__":
    main()
