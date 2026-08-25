# SPDX-License-Identifier: CC0-1.0
#
# Writes ../src/tables.s: the trigonometry and the NTSC 2A03 timer arithmetic
# the cart needs, worked out at build time so the 6502 never has to multiply or
# divide at run time.
#
# Nothing creative lives in the output. These are lookup tables for facts.
#
#   python tools/gentables.py         # rewrites src/tables.s in place

import io, math, os, sys

N = 64                      # entries per periodic table, one full turn
CPU = 1789773.0             # NTSC 2A03 clock, Hz

w = io.StringIO()
w.write("""; SPDX-License-Identifier: CC0-1.0
; FamiRust Demo Cart -- generated numeric tables.
;
; Nothing creative lives in this file: it is trigonometry and the NTSC 2A03
; timer arithmetic, worked out once at build time so the 6502 never has to
; multiply. Regenerate with tools/gentables.py.

; ---------------------------------------------------------------------------
; Ring positions, 64 evenly spaced angles. The values are already screen
; coordinates for an 8x8 sprite, including the fact that OAM Y is the sprite's
; top scanline minus one.
;   outer ring: centre (128,132), radii 60 x 44
;   inner ring: centre (128,132), radii 30 x 22
; ---------------------------------------------------------------------------
""")


def table(name, vals, per=8, comment=None):
    if comment:
        w.write("; %s\n" % comment)
    lo, hi = min(vals), max(vals)
    assert 0 <= lo and hi <= 255, "%s leaves byte range: %d..%d" % (name, lo, hi)
    w.write("%s:\n" % name)
    for i in range(0, len(vals), per):
        w.write("  .byte " + ",".join("%3d" % v for v in vals[i:i + per]) + "\n")
    w.write("\n")


def ring(cx, cy, rx, ry):
    xs = [cx + round(rx * math.cos(2 * math.pi * i / N)) for i in range(N)]
    ys = [cy + round(ry * math.sin(2 * math.pi * i / N)) for i in range(N)]
    return xs, ys


ring_x, ring_y = ring(124, 127, 60, 44)
inner_x, inner_y = ring(124, 127, 30, 22)
table("ring_x", ring_x)
table("ring_y", ring_y)
table("inner_x", inner_x)
table("inner_y", inner_y)

wave_y = [127 + round(46 * math.sin(2 * math.pi * i / N)) for i in range(N)]
table("wave_y", wave_y, comment="Vertical sine for the WAVE arrangement.")

# A shallow bob for decoration. Kept to 0..7 so a caller can add it to a base
# row and stay inside a two-tile band without checking for overflow.
bob = [round(3.5 + 3.5 * math.sin(2 * math.pi * i / N)) for i in range(N)]
assert min(bob) == 0 and max(bob) == 7, bob
table("bob", bob, comment="Shallow 0..7 bob for idle decoration.")

w.write("""; ---------------------------------------------------------------------------
; Pulse and triangle timer periods.
;
;   pulse frequency = CPU / (16 * (timer + 1))
;
; so timer = round(CPU / (16 * f)) - 1. The triangle's timer clocks at half the
; pulse rate, so writing a note from this table to the triangle sounds an
; octave lower; the music data compensates by naming the octave above.
;
; 48 notes, C2 up to B5. Index 0 is C2. N_<NOTE><OCTAVE> constants follow, so
; the pattern data reads like music rather than like arithmetic.
; ---------------------------------------------------------------------------
""")

NAMES = ["C", "CS", "D", "DS", "E", "F", "FS", "G", "GS", "A", "AS", "B"]
periods = []
for idx in range(48):
    octave = 2 + idx // 12
    semi = idx % 12
    # A4 = 440 Hz, and A is semitone 9 of its octave.
    n = (octave - 4) * 12 + (semi - 9)
    f = 440.0 * (2.0 ** (n / 12.0))
    p = int(round(CPU / (16.0 * f))) - 1
    assert 0 < p < 2048, "note %d needs an 11-bit timer, got %d" % (idx, p)
    periods.append(p)

w.write("note_lo:\n")
for i in range(0, 48, 8):
    w.write("  .byte " + ",".join("$%02X" % (p & 0xFF) for p in periods[i:i + 8]) + "\n")
w.write("note_hi:\n")
for i in range(0, 48, 8):
    w.write("  .byte " + ",".join("$%02X" % (p >> 8) for p in periods[i:i + 8]) + "\n")

w.write("\n; Note-name constants for the pattern data.\n")
for idx in range(48):
    w.write("N_%s%d = %d\n" % (NAMES[idx % 12], 2 + idx // 12, idx))
w.write("REST = $FF\n")
w.write("HOLD = $FE\n")

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "src", "tables.s")
open(out, "w", newline="\n").write(w.getvalue())
print("wrote", os.path.normpath(out))
