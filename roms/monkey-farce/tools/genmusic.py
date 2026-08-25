# SPDX-License-Identifier: CC0-1.0
#
# Expands the tune below into ../src/music.s.
#
#   python tools/genmusic.py
#
# The song is written as notes, one row per sixteenth, because the alternative
# is a wall of period values and nobody has ever spotted a wrong note in a wall
# of period values. The generator turns them into the two timer tables the 2A03
# actually wants, and checks its own arithmetic on the way past.
#
# THE TUNE: D harmonic minor, 150 BPM, sixteen bars that loop. Four voices, in
# the arrangement that gives this style its drive:
#
#   pulse 1   the hook, mostly eighths, stated then answered then walked down
#   pulse 2   a sixteenth-note arpeggio of whatever chord is underneath it,
#             which is what makes the whole thing move rather than sit
#   triangle  root and octave on eighths, the pumping bass
#   noise     kick, snare, hats
#
# Original composition. Nothing here is transcribed from anything.
import sys, os, io

HERE = os.path.dirname(os.path.abspath(__file__))
CPU = 1789773.0            # NTSC 2A03 master clock / 12
LOW_OCTAVE = 2             # note index 0 is C2
OCTAVES = 5
TEMPO = 6                  # frames per row: 60/6 = 10 rows a second = 150 BPM
ROWS = 32                  # two bars of sixteenths
PATTERNS = 8               # 8 * 32 = 256 rows, so the row counter wraps and loops

REST, HOLD = 0xFF, 0xFE
NONE, KICK, SNARE, HAT = 0, 1, 2, 3

NAMES = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"]


def note_index(tok):
    """`A-4` -> an index into the note tables, counting up from C2."""
    if tok in (".", "..."):
        return HOLD
    if tok in ("-", "---"):
        return REST
    name, octave = tok[:2], int(tok[2:])
    if name not in NAMES:
        raise SystemExit("no such note: %r" % tok)
    i = (octave - LOW_OCTAVE) * 12 + NAMES.index(name)
    if not 0 <= i < OCTAVES * 12:
        raise SystemExit("%s is outside the %d octaves from C%d" % (tok, OCTAVES, LOW_OCTAVE))
    return i


def row_list(text, conv):
    toks = text.split()
    if len(toks) != ROWS:
        raise SystemExit("a pattern has %d rows, expected %d:\n  %s" % (len(toks), ROWS, text))
    return [conv(t) for t in toks]


def drum(tok):
    return {".": NONE, "K": KICK, "S": SNARE, "H": HAT}[tok]


# ---------------------------------------------------------------------------
# The song. Each entry is two bars.
#
# Harmony:  Dm  Dm | Bb  C  | Dm  Dm | F  A  and round again with the lead
# reaching higher the second time through.
# ---------------------------------------------------------------------------
LEAD = [
    # Dm, Dm: the hook. Statement, answer, and a walk back down to the root.
    """D-5 ... ... ...  F-5 ... E-5 ...  D-5 ... ... ...  A-4 ... C-5 ...
       D-5 ... ... ...  A-5 ... G-5 ...  F-5 ... E-5 ...  D-5 ... ... ...""",
    # Bb, C
    """A#4 ... ... ...  D-5 ... F-5 ...  A#5 ... ... ...  A-5 ... F-5 ...
       G-5 ... ... ...  E-5 ... C-5 ...  E-5 ... G-5 ...  A-5 ... ... ...""",
    # Dm, Dm again, an octave of reach higher
    """D-5 ... ... ...  F-5 ... A-5 ...  D-6 ... ... ...  A-5 ... F-5 ...
       E-5 ... ... ...  G-5 ... F-5 ...  E-5 ... D-5 ...  C#5 ... ... ...""",
    # F, A: the turnaround. C# is the raised seventh that makes it harmonic
    # minor rather than merely sad.
    """F-5 ... E-5 ...  D-5 ... C-5 ...  A-4 ... ... ...  D-5 ... F-5 ...
       E-5 ... C#5 ...  E-5 ... A-5 ...  F-5 ... E-5 ...  D-5 ... --- ...""",
    # Second time round: the hook again, then held higher and longer.
    """D-5 ... ... ...  F-5 ... E-5 ...  D-5 ... ... ...  A-4 ... C-5 ...
       D-5 ... F-5 ...  A-5 ... D-6 ...  C-6 ... A#5 ...  A-5 ... ... ...""",
    """A#5 ... ... ...  A-5 ... F-5 ...  D-5 ... F-5 ...  A#5 ... ... ...
       C-6 ... ... ...  A#5 ... G-5 ...  E-5 ... G-5 ...  C-6 ... ... ...""",
    """D-6 ... ... ...  A-5 ... F-5 ...  D-5 ... ... ...  F-5 ... A-5 ...
       D-6 ... C-6 ...  A#5 ... A-5 ...  G-5 ... F-5 ...  E-5 ... ... ...""",
    """F-5 ... E-5 ...  D-5 ... C#5 ...  D-5 ... E-5 ...  F-5 ... G-5 ...
       A-5 ... ... ...  E-5 ... C#5 ...  A-4 ... ... ...  --- ... ... ...""",
]

# Sixteenth-note arpeggios. Four notes to a beat, up and back down.
def arp(a, b, c, beats):
    one = "%s %s %s %s " % (a, b, c, b)
    return (one * beats).strip()


HARMONY = [
    arp("D-4", "F-4", "A-4", 8),
    arp("A#3", "D-4", "F-4", 4) + " " + arp("C-4", "E-4", "G-4", 4),
    arp("D-4", "F-4", "A-4", 8),
    arp("F-3", "A-3", "C-4", 4) + " " + arp("A-3", "C#4", "E-4", 4),
    arp("D-4", "F-4", "A-4", 8),
    arp("A#3", "D-4", "F-4", 4) + " " + arp("C-4", "E-4", "G-4", 4),
    arp("D-4", "F-4", "A-4", 4) + " " + arp("D-4", "G-4", "A#4", 4),
    arp("F-3", "A-3", "C-4", 4) + " " + arp("A-3", "C#4", "E-4", 4),
]


def bassline(root_low, root_high, bars):
    """Root, octave, root, octave on eighths: the pump. Four of those to a bar."""
    return (("%s ... %s ... " % (root_low, root_high)) * (bars * 4)).strip()


BASS = [
    bassline("D-2", "D-3", 2),
    bassline("A#2", "A#3", 1) + " " + bassline("C-3", "C-4", 1),
    bassline("D-2", "D-3", 2),
    bassline("F-2", "F-3", 1) + " " + bassline("A-2", "A-3", 1),
    bassline("D-2", "D-3", 2),
    bassline("A#2", "A#3", 1) + " " + bassline("C-3", "C-4", 1),
    bassline("D-2", "D-3", 1) + " " + bassline("G-2", "G-3", 1),
    bassline("F-2", "F-3", 1) + " " + bassline("A-2", "A-3", 1),
]

BEAT = "K . H . S . H . K . H . S . H . K . H . S . H . K . H . S . H ."
FILL = "K . H . S . H . K . H . S . H . K . H . S . H . S S S S S . S ."
DRUMS = [BEAT, BEAT, BEAT, FILL, BEAT, BEAT, BEAT, FILL]

for name, lst in (("lead", LEAD), ("harmony", HARMONY), ("bass", BASS), ("drums", DRUMS)):
    if len(lst) != PATTERNS:
        raise SystemExit("%s has %d patterns, expected %d" % (name, len(lst), PATTERNS))

song_lead = [n for p in LEAD for n in row_list(p, note_index)]
song_harm = [n for p in HARMONY for n in row_list(p, note_index)]
song_bass = [n for p in BASS for n in row_list(p, note_index)]
song_drum = [n for p in DRUMS for n in row_list(p, drum)]
assert len(song_lead) == PATTERNS * ROWS == 256

# ---------------------------------------------------------------------------
# The timer values. A pulse channel divides the CPU clock by 16 per full cycle
# and the triangle by 32, so the same written note needs two different numbers
# and the triangle's table is not the pulse table shifted: it is the same
# frequency reached a different way.
# ---------------------------------------------------------------------------
def periods(divisor):
    out = []
    for i in range(OCTAVES * 12):
        freq = 440.0 * (2.0 ** ((i + (LOW_OCTAVE - 4) * 12 - 9) / 12.0))
        p = int(round(CPU / (divisor * freq))) - 1
        out.append(p)
    return out


pulse = periods(16.0)
tri = periods(32.0)

# A9 is 440 Hz by definition, so the table has one value in it that can be
# checked against something other than itself.
a4 = note_index("A-4")
back = CPU / (16.0 * (pulse[a4] + 1))
if abs(back - 440.0) > 1.0:
    raise SystemExit("A4 came out at %.1f Hz, not 440" % back)
for name, tbl, lim in (("pulse", pulse, 0x7FF), ("triangle", tri, 0x7FF)):
    for i, p in enumerate(tbl):
        if not 8 <= p <= lim:
            raise SystemExit("%s period for %s%d is %d, outside 8..%d"
                             % (name, NAMES[i % 12], LOW_OCTAVE + i // 12, p, lim))

# Every note the song actually uses must be inside the table, which row_list
# already guarantees, and the bass must not ask the triangle for something it
# cannot voice.
used = set(n for n in song_lead + song_harm + song_bass if n < 0xFE)
if not used:
    raise SystemExit("the song contains no notes at all")

# ---------------------------------------------------------------------------
w = io.StringIO()
w.write("""; SPDX-License-Identifier: CC0-1.0
; MONKEY FARCE -- the tune. GENERATED by tools/genmusic.py; edit the notes
; there, not the numbers here.
;
; D harmonic minor, 150 BPM, sixteen bars on a loop. Four voices: the hook on
; pulse 1, a sixteenth-note arpeggio on pulse 2, root-and-octave bass on the
; triangle, and kick, snare and hats on the noise channel.
;
; The song is %d rows long on purpose. A byte counter wraps at exactly that, so
; looping the tune costs an INX and nothing else.

MUSIC_ROWS  = %d
MUSIC_TEMPO = %d
NOTE_REST   = $%02X
NOTE_HOLD   = $%02X
DRUM_NONE   = %d
DRUM_KICK   = %d
DRUM_SNARE  = %d
DRUM_HAT    = %d

""" % (len(song_lead), len(song_lead), TEMPO, REST, HOLD, NONE, KICK, SNARE, HAT))


def table(label, values, comment, hexfmt=True):
    w.write("%s:  ; %s\n" % (label, comment))
    for i in range(0, len(values), 8):
        row = values[i:i + 8]
        w.write("  .byte " + ", ".join(("$%02X" % v) if hexfmt else str(v) for v in row) + "\n")
    w.write("\n")


table("note_lo", [p & 0xFF for p in pulse], "pulse timer, low byte, C2 upwards")
table("note_hi", [p >> 8 for p in pulse], "pulse timer, high three bits")
table("tri_lo", [p & 0xFF for p in tri], "triangle timer, low byte")
table("tri_hi", [p >> 8 for p in tri], "triangle timer, high three bits")
table("song_lead", song_lead, "pulse 1: the hook")
table("song_harm", song_harm, "pulse 2: the arpeggio")
table("song_bass", song_bass, "triangle: root and octave")
table("song_drum", song_drum, "noise: kick, snare, hats")

out = os.path.join(HERE, "..", "src", "music.s")
open(out, "w", newline="\n").write(w.getvalue())
print("wrote %s (%d rows, %d frames a loop, A4 checks out at %.1f Hz)"
      % (os.path.normpath(out), len(song_lead), len(song_lead) * TEMPO, back))
