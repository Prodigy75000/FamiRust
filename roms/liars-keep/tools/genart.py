# SPDX-License-Identifier: CC0-1.0
#
# Expands the art below into ../src/chr.s (the character ROM) and
# ../src/artmap.s (the constants and lookup tables that name it).
#
# Both outputs are checked in and readable on their own. This script exists so
# that the art can be edited as pictures instead of hex, and so that the tile
# numbers the code uses can never drift from the tiles the ROM actually holds:
# one run writes both files from one source of truth.
#
#   python tools/genart.py
#
# Pixel codes in every picture below: "." is colour 0 (the shared backdrop for
# backgrounds, transparent for sprites) and 1, 2, 3 are the three drawable
# colours of whichever palette the tile is drawn with.
import sys, io, os

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))

# The font is the demo cart's, which is the same author's work and also CC0.
# It is read back out of that cart's finished character ROM rather than copied
# into this file, so the two cartridges cannot drift apart, and rather than
# imported from its generator, because importing that generator would run it
# and rewrite the other cart as a side effect of building this one.
#
# The generated chr.s is checked in, so a clone holding only this directory
# still builds the ROM; it just cannot regenerate the art.
def load_font(path):
    """Pull tiles $00-$3F back out of a built .chr section, keyed by the
    character each one stands for."""
    lines = open(path, encoding="utf-8").read().split("\n")
    tiles, idx, i = {}, 0, 0
    while i < len(lines):
        ln = lines[i].split(";")[0].strip()
        if ln.startswith(".chrskip"):
            idx += int(ln.split()[1], 0)
        elif ln.startswith(".tile"):
            tiles[idx] = [lines[i + 1 + k].strip() for k in range(8)]
            idx += 1
            i += 9
        elif ln == ".prg":
            break
        i += 1
    return dict((chr(0x20 + k), v) for k, v in tiles.items() if k < 0x40)


FONT = load_font(os.path.join(ROOT, "roms", "famirust-demo", "src", "chr.s"))
if len(FONT) < 40:
    raise SystemExit("only found %d glyphs in the demo cart's character ROM" % len(FONT))

# ---------------------------------------------------------------------------
# Backgrounds. Every block is one 16x16 cell of the world: it is the unit the
# collision code works in, the unit the room grid stores, and the unit the
# attribute table can give its own palette to. Blocks are sliced into four 8x8
# tiles on the way out and identical tiles are shared, which is why several of
# these cost only two tiles each.
# ---------------------------------------------------------------------------
PAL_ROCK, PAL_BRICK, PAL_HOT = 1, 2, 3

SOLID   = 0x01   # you stand on it
HAZARD  = 0x02   # touching it kills you
CRUMBLE = 0x04   # solid, until you stand on it
GOAL    = 0x08   # touching it ends the room

B = {}

# Dungeon masonry. Two courses, offset, black mortar.
B["stone"] = [
"3333333333333333",
"2222222122222221",
"2222222122222221",
"2222222122222221",
"2222222122222221",
"2222222122222221",
"2222222122222221",
"1111111111111111",
"3333333333333333",
"2221222222212222",
"2221222222212222",
"2221222222212222",
"2221222222212222",
"2221222222212222",
"2221222222212222",
"1111111111111111",
]

# The honest platform. Lit along the top so it reads as something to stand on.
B["plat"] = [
"3333333333333333",
"2222222222222222",
"2221222222212222",
"2221222222212222",
"1111111111111111",
"2222222122222221",
"2222222122222221",
"2222222122222221",
"1111111111111111",
"2221222222212222",
"2221222222212222",
"2221222222212222",
"1111111111111111",
"2222222122222221",
"2222222122222221",
"1111111111111111",
]

# The one that gives you warning. Cracked, and it means it.
B["crumble"] = [
"3333333333333333",
"2222222222222222",
"22212.2222212222",
"2221.22222.12222",
"11111111.1111111",
"222222.122222221",
"2222.22122222.21",
"22222221.2222221",
"1111.11111.11111",
"2221222.22212222",
"2221.2222221.222",
"222122222.212222",
"111111.111111.11",
"2222222122.22221",
"22222221222222.1",
"1111111111111111",
]

B["backbrick"] = [
"................",
".1....1....1....",
"................",
"....1....1....1.",
"................",
".1....1....1....",
"................",
"....1....1....1.",
"................",
".1....1....1....",
"................",
"....1....1....1.",
"................",
".1....1....1....",
"................",
"....1....1....1.",
]

B["spikeup"] = [
"...33......33...",
"...33......33...",
"..3223....3223..",
"..3223....3223..",
"..3223....3223..",
".332233..332233.",
".322223..322223.",
".322223..322223.",
"3322222333222233",
"3222222232222223",
"1111111111111111",
"2222222222222222",
"2222222222222222",
"2222222222222222",
"2222222222222222",
"1111111111111111",
]

B["spikedn"] = [
"1111111111111111",
"2222222222222222",
"2222222222222222",
"2222222222222222",
"2222222222222222",
"1111111111111111",
"3222222232222223",
"3322222333222233",
".322223..322223.",
".322223..322223.",
".332233..332233.",
"..3223....3223..",
"..3223....3223..",
"..3223....3223..",
"...33......33...",
"...33......33...",
]

B["lava"] = [
"3223322332233223",
"2112211221122112",
"2111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
"1111111111111111",
]

# Fire, in a palette of black, red, yellow, white. The colours have to run
# outward from the hot middle: white core, yellow body, red edge. Drawn the
# other way round, with white filling a yellow outline, it stops being fire and
# becomes a bright blob with a rim, which is how the first bonfire here ended up
# unrecognisable.
B["torch"] = [
".......1........",
"......121.......",
"......121.......",
".....12321......",
".....12321......",
"....1233321.....",
"....1233321.....",
".....12321......",
"......121.......",
".......1........",
"......111.......",
".....11111......",
".....11111......",
"......111.......",
"......111.......",
"................",
]

B["chain"] = [
"......22........",
".....2..2.......",
".....2..2.......",
"......22........",
"......22........",
".....2..2.......",
".....2..2.......",
"......22........",
"......22........",
".....2..2.......",
".....2..2.......",
"......22........",
"......22........",
".....2..2.......",
".....2..2.......",
"......22........",
]

B["rubble"] = [
"................",
"................",
"................",
"................",
"................",
"................",
"................",
"................",
"................",
"......11........",
".....1221.......",
"...11.11..11....",
"..1221..11221...",
".122221.122221..",
"1222222112222221",
"1111111111111111",
]

# The bonfire. Reaching it is how a room ends, which makes it the only thing in
# this cartridge that is exactly what it looks like, so it had better look like
# it. The first attempt did not: the flame was too narrow and pointed, and the
# logs underneath were drawn as a row of gaps, which together read to a
# playtester as "a shard on top of an ocarina". A wider flame over a solid mound
# of coals says campfire without needing a caption.
B["bonfire"] = [
"................",
".......1........",
"......121.......",
"......121.......",
".....12321......",
".....12321......",
"....1233321.....",
"....1233321.....",
"....1233321.....",
".....12321......",
"......121.......",
"................",
"...1111111111...",
"..11.111111.11..",
".111111111111...",
".11111111111....",
]

# name, art, palette, flags. The order fixes the block ids.
BLOCKS = [
    ("air",       None,        0,         0),
    ("stone",     "stone",     PAL_ROCK,  SOLID),
    ("plat",      "plat",      PAL_BRICK, SOLID),
    # The liar. Same picture, same palette, same everything except the one bit
    # that decides whether it holds you up. That only works because it shares
    # tiles with the block above rather than imitating it: there is nothing to
    # spot, not one pixel.
    ("liar",      "plat",      PAL_BRICK, 0),
    ("crumble",   "crumble",   PAL_BRICK, SOLID | CRUMBLE),
    ("rubble",    "rubble",    PAL_BRICK, 0),
    ("spikeup",   "spikeup",   PAL_ROCK,  HAZARD),
    ("spikedn",   "spikedn",   PAL_ROCK,  HAZARD),
    ("lava",      "lava",      PAL_HOT,   HAZARD),
    ("torch",     "torch",     PAL_HOT,   0),
    ("chain",     "chain",     PAL_ROCK,  0),
    ("backbrick", "backbrick", PAL_ROCK,  0),
    ("bonfire",   "bonfire",   PAL_HOT,   GOAL),
]

# ---------------------------------------------------------------------------
# The room alphabet. A room is typed as ASCII in the source, so this table is
# the entire authoring language. Characters that are not listed are air, which
# means a typo costs you a hole in the floor and not a corrupt room.
#
# The last five spawn something instead of being something: the cell itself
# keeps its block and an entity is created there when the room loads.
# ---------------------------------------------------------------------------
E_NONE, E_BAIT, E_SAW, E_CRUSHER, E_DARTR, E_DARTL = 0, 1, 2, 3, 4, 5

CHARS = [
    (".", "air",       E_NONE),
    ("#", "stone",     E_NONE),
    ("=", "plat",      E_NONE),
    ("~", "liar",      E_NONE),
    ("c", "crumble",   E_NONE),
    ("o", "rubble",    E_NONE),
    ("^", "spikeup",   E_NONE),
    ("v", "spikedn",   E_NONE),
    ("L", "lava",      E_NONE),
    ("*", "torch",     E_NONE),
    ("|", "chain",     E_NONE),
    ("%", "backbrick", E_NONE),
    ("D", "bonfire",   E_NONE),
    ("S", "air",       E_NONE),   # spawn point, found by scanning the grid
    ("B", "air",       E_BAIT),
    ("W", "air",       E_SAW),
    ("C", "air",       E_CRUSHER),
    (">", "stone",     E_DARTR),  # a wall that shoots
    ("<", "stone",     E_DARTL),
]

# ---------------------------------------------------------------------------
# Sprites. 16x16 metasprites are four tiles in the order TL, TR, BL, BR; the
# hero is drawn facing right only and mirrored by the OAM flip bit, which is
# why there is no left-facing art here.
# ---------------------------------------------------------------------------
S = {}

S["hero_idle"] = [
"................",
"....33333333....",
"...3222222223...",
"...3211111123...",
"...3213113123...",
"...3211111123...",
"....33111133....",
"......3113......",
"..322222222223..",
"..321222222123..",
"..322222222223..",
"..332222222233..",
"...322....223...",
"...322....223...",
"..3322....2233..",
"..3333....3333..",
]
S["hero_walk1"] = [
"................",
"....33333333....",
"...3222222223...",
"...3211111123...",
"...3213113123...",
"...3211111123...",
"....33111133....",
"......3113......",
"..322222222223..",
"..321222222123..",
"..322222222223..",
"..332222222233..",
"...322....223...",
"..322......223..",
".3322.......223.",
".3333.......2233",
]
S["hero_walk2"] = [
"................",
"....33333333....",
"...3222222223...",
"...3211111123...",
"...3213113123...",
"...3211111123...",
"....33111133....",
"......3113......",
"..322222222223..",
"..321222222123..",
"..322222222223..",
"..332222222233..",
"....322..223....",
"....322..223....",
"...3322..2233...",
"...3333..3333...",
]
S["hero_jump"] = [
"................",
"33..33333333..33",
"323.32222223.323",
"323.32111123.323",
"323.32131123.323",
"3233321111123333",
"3222233111133223",
"..32..3113..23..",
"..322222222223..",
"..321222222123..",
"..322222222223..",
"...3322222233...",
"....33222233....",
"...3322..2233...",
"...3333..3333...",
"................",
]
S["hero_dead"] = [
"................",
"................",
"................",
"................",
"................",
"....33333333....",
"...32222222223..",
"..3213113221223.",
".32222222222223.",
".32222222222223.",
".33333333333333.",
"..33........33..",
"................",
"................",
"................",
"................",
]

# The bait. It is drawn like a reward because that is the point of it.
S["flask"] = [
"................",
"......3333......",
"......3113......",
"......3113......",
".....332233.....",
"....33222233....",
"...3222222223...",
"..322222222223..",
"..321111111123..",
"..321111111123..",
"..322111111223..",
"..322211112223..",
"..322222222223..",
"...3322222233...",
".....333333.....",
"................",
]

S["saw"] = [
"......3333......",
"....33111133....",
"...3111111113...",
"..311111111113..",
".31111111111113.",
"3111111111111113",
"3111113333111113",
"3111133223311113",
"3111133223311113",
"3111113333111113",
"3111111111111113",
".31111111111113.",
"..311111111113..",
"...3111111113...",
"....33111133....",
"......3333......",
]

S["crusher"] = [
"3333333333333333",
"3222222222222223",
"3211111111111123",
"3211111111111123",
"3222222222222223",
"3222222222222223",
"3211111111111123",
"3222222222222223",
"3333333333333333",
"32.32.32.32.32.3",
"3.2.2.2.2.2.2.23",
"3333333333333333",
"..3..3..3..3..3.",
"................",
"................",
"................",
]

# 8x8 sprites.
S8 = {}
S8["dart"] = [
"........",
"3.......",
"33...3..",
"3222233.",
"32222233",
"3222233.",
"33...3..",
"3.......",
]
S8["skull"] = [
"..3333..",
".322223.",
"32133123",
"32133123",
"32211223",
"32311323",
".322223.",
"..3333..",
]

# ---------------------------------------------------------------------------
# Validation. Every mistake this catches is one that would otherwise show up as
# unreadable garbage on the screen with no clue where it came from.
# ---------------------------------------------------------------------------
def check(name, grid, size):
    if len(grid) != size:
        raise SystemExit("%s is %d rows tall, expected %d" % (name, len(grid), size))
    for y, r in enumerate(grid):
        if len(r) != size:
            raise SystemExit("%s row %d is %d wide, expected %d" % (name, y, len(r), size))
        bad = [c for c in r if c not in ".123"]
        if bad:
            raise SystemExit("%s row %d contains %r; pixels are . 1 2 3 only"
                             % (name, y, bad[0]))

for n, g in B.items():
    check("block " + n, g, 16)
for n, g in S.items():
    check("sprite " + n, g, 16)
for n, g in S8.items():
    check("sprite " + n, g, 8)

known = set(B) | set([None])
for name, art, pal, flags in BLOCKS:
    if art not in known:
        raise SystemExit("block %s names missing art %r" % (name, art))
blocknames = [b[0] for b in BLOCKS]
roomchars = [c[0] for c in CHARS]
for ch, blk, ent in CHARS:
    if blk not in blocknames:
        raise SystemExit("room character %r names missing block %r" % (ch, blk))
    if not (0x20 <= ord(ch) < 0x80):
        raise SystemExit("room character %r is outside the mappable range" % ch)
if len(set(roomchars)) != len(roomchars):
    raise SystemExit("a room character is listed twice")
for bad in ['"', ',']:
    if bad in roomchars:
        raise SystemExit("%r cannot be a room character; .str would break" % bad)

# ---------------------------------------------------------------------------
# Tile pool. Identical 8x8s are shared, which is what makes the liar free: it
# does not copy the honest platform's look, it is issued the same tile numbers.
# ---------------------------------------------------------------------------
class Pool(object):
    def __init__(self, base):
        self.base = base
        self.tiles = []
        self.index = {}
        self.names = {}

    def add(self, rows, name=None):
        key = tuple(rows)
        if key not in self.index:
            self.index[key] = self.base + len(self.tiles)
            self.tiles.append(rows)
        i = self.index[key]
        if name is not None:
            self.names.setdefault(i, name)
        return i


def slice16(grid):
    """Cut a 16x16 picture into its TL, TR, BL, BR 8x8 tiles."""
    out = []
    for oy in (0, 8):
        for ox in (0, 8):
            out.append([grid[oy + y][ox:ox + 8] for y in range(8)])
    return out


bg = Pool(0x40)
blk_tiles = {}
for name, art, pal, flags in BLOCKS:
    if art is None:
        blk_tiles[name] = [0, 0, 0, 0]      # tile $00 is the blank space glyph
        continue
    quads = slice16(B[art])
    blk_tiles[name] = [bg.add(q, "%s_%d" % (art, i)) for i, q in enumerate(quads)]

MS_ORDER = ["hero_idle", "hero_walk1", "hero_walk2", "hero_jump", "hero_dead",
            "flask", "saw", "crusher"]
sp = Pool(0)
spr_tiles = {}
for name in MS_ORDER:
    quads = slice16(S[name])
    spr_tiles[name] = [sp.add(q, "%s_%d" % (name, i)) for i, q in enumerate(quads)]
for name in ["dart", "skull"]:
    spr_tiles[name] = [sp.add(S8[name], name)]

if bg.base + len(bg.tiles) > 256:
    raise SystemExit("background tiles overflow pattern table 0")
if len(sp.tiles) > 256:
    raise SystemExit("sprite tiles overflow pattern table 1")

# The premise of the whole cartridge, checked where it can actually be checked.
# Give the liar so much as one pixel of its own and the pool stops sharing, the
# tile numbers diverge, and this fails. It is also re-asserted inside the ROM
# through LIAR_TILES_MATCH below, so a stale checked-in chr.s cannot hide it.
LIAR_TILES_MATCH = int(blk_tiles["liar"] == blk_tiles["plat"])
if not LIAR_TILES_MATCH:
    raise SystemExit(
        "the liar no longer shares the honest platform's tiles (%r vs %r); "
        "there is now something to see, and the game is about nothing"
        % (blk_tiles["liar"], blk_tiles["plat"]))

# ---------------------------------------------------------------------------
# Emit
# ---------------------------------------------------------------------------
def pad(rows):
    out = [(r.replace("0", ".") + "........")[:8] for r in rows]
    while len(out) < 8:
        out.append("........")
    return out[:8]


def emit_tile(w, label, rows):
    w.write(".tile %s\n" % label)
    for r in pad(rows):
        w.write(r + "\n")
    w.write(".endtile\n")


chr_out = io.StringIO()
chr_out.write("""; SPDX-License-Identifier: CC0-1.0
; LIAR'S KEEP -- character ROM. GENERATED by tools/genart.py; edit the art
; there, not here.
;
; Every tile is drawn as the picture it is: "." is colour 0 and 1, 2, 3 are the
; three drawable colours of whichever palette the tile is rendered with.
; Nothing in this file is derived from, traced from, or measured against any
; commercial cartridge.

.chr

; ---------------------------------------------------------------------------
; Pattern table 0, $00-$3F: the font, laid out so a tile index is its ASCII
; code minus $20. That is what lets the source write screen text, and room
; layouts, as text.
;
; The gaps are emitted by the loop below, including the one where the space
; character would be, so there is no skip written by hand here to double it.
; ---------------------------------------------------------------------------
""")

ORDER = [chr(c) for c in range(0x20, 0x60)]
NAMES = {' ': 'space', '!': 'bang', '"': 'quote', "'": 'apos', '(': 'lparen',
         ')': 'rparen', '*': 'star_c', '$': 'dollar', '+': 'plus', ',': 'comma',
         '-': 'dash', '.': 'dot', '/': 'slash', ':': 'colon', ';': 'semi',
         '<': 'lt', '=': 'eq', '>': 'gt', '?': 'query', '[': 'lbrack',
         ']': 'rbrack'}
run = 0
for i, ch in enumerate(ORDER):
    if ch not in FONT:
        run += 1
        continue
    if run:
        chr_out.write(".chrskip %d\n" % run)
        run = 0
    nm = NAMES.get(ch) or (('digit_' + ch) if ch.isdigit() else ('glyph_' + ch))
    chr_out.write("\n; %r -> tile $%02X\n" % (ch, i))
    emit_tile(chr_out, "chr_" + nm, FONT[ch])
if run:
    chr_out.write(".chrskip %d\n" % run)

chr_out.write("""
; ---------------------------------------------------------------------------
; Pattern table 0, $40 up: the world. Each 16x16 block of the room was sliced
; into four of these and identical slices were shared, so the count here is
; smaller than four times the number of blocks.
; ---------------------------------------------------------------------------
""")
for i, rows in enumerate(bg.tiles):
    idx = bg.base + i
    chr_out.write("\n; tile $%02X  %s\n" % (idx, bg.names.get(idx, "")))
    emit_tile(chr_out, "bgt_%s" % bg.names.get(idx, "t%02x" % idx), rows)

chr_out.write("\n; Pad out pattern table 0.\n.chrskip %d\n"
              % (256 - (bg.base + len(bg.tiles))))

chr_out.write("""
; ---------------------------------------------------------------------------
; Pattern table 1 ($1000): sprites. Indices are table-relative, which is what
; OAM stores once PPUCTRL points sprites at the second table.
; ---------------------------------------------------------------------------
""")
for i, rows in enumerate(sp.tiles):
    chr_out.write("\n; sprite tile $%02X  %s\n" % (i, sp.names.get(i, "")))
    emit_tile(chr_out, "spr_%s" % sp.names.get(i, "t%02x" % i), rows)
chr_out.write("\n; Pad out pattern table 1.\n.chrskip %d\n" % (256 - len(sp.tiles)))
chr_out.write("\n.prg\n")

# ---- artmap.s -------------------------------------------------------------
m = io.StringIO()
m.write("""; SPDX-License-Identifier: CC0-1.0
; LIAR'S KEEP -- what the art is called. GENERATED by tools/genart.py.
;
; The code never writes a tile number. It writes a block id and looks the tiles
; up here, so the art and the game cannot disagree about what a thing is.

""")
for i, (name, art, pal, flags) in enumerate(BLOCKS):
    m.write("BLK_%-10s = %d\n" % (name.upper(), i))
m.write("NUM_BLOCKS   = %d\n\n" % len(BLOCKS))
m.write("BF_SOLID   = $%02X\nBF_HAZARD  = $%02X\nBF_CRUMBLE = $%02X\nBF_GOAL    = $%02X\n\n"
        % (SOLID, HAZARD, CRUMBLE, GOAL))
for nm, v in [("E_NONE", E_NONE), ("E_BAIT", E_BAIT), ("E_SAW", E_SAW),
              ("E_CRUSHER", E_CRUSHER), ("E_DARTR", E_DARTR), ("E_DARTL", E_DARTL)]:
    m.write("%-10s = %d\n" % (nm, v))
m.write("E_COUNT    = %d\n\n" % 6)

for i, nm in enumerate(MS_ORDER):
    m.write("MS_%-10s = %d\n" % (nm.upper(), i))
m.write("MS_COUNT   = %d\n" % len(MS_ORDER))
m.write("SPR_DART   = %d\nSPR_SKULL  = %d\n\n"
        % (spr_tiles["dart"][0], spr_tiles["skull"][0]))


def table(label, values, comment=""):
    m.write("%s:%s\n" % (label, ("  ; " + comment) if comment else ""))
    for i in range(0, len(values), 8):
        m.write("  .byte " + ", ".join("$%02X" % v for v in values[i:i + 8]) + "\n")
    m.write("\n")


for q, nm in enumerate(["tl", "tr", "bl", "br"]):
    table("blk_" + nm, [blk_tiles[b[0]][q] for b in BLOCKS],
          "background tile in the %s corner of each block" % nm.upper())
table("blk_flags", [b[3] for b in BLOCKS], "solid / hazard / crumble / goal")
table("blk_pal", [b[2] for b in BLOCKS], "which background palette the block wants")

# Spelled out so the ROM can assert on them. A table cannot be indexed at
# assembly time, so the two blocks the cartridge is actually about get their
# flags and palette named individually and checked in main.s.
byname = dict((b[0], b) for b in BLOCKS)
for who in ("plat", "liar"):
    m.write("BLKF_%-5s = $%02X\n" % (who.upper(), byname[who][3]))
    m.write("BLKP_%-5s = %d\n" % (who.upper(), byname[who][2]))
m.write("LIAR_TILES_MATCH = %d\n\n" % LIAR_TILES_MATCH)

# charmap: 96 entries covering $20..$7F, indexed by the byte .str emits.
cm_blk = [0] * 96
cm_ent = [0] * 96
for ch, blk, ent in CHARS:
    cm_blk[ord(ch) - 0x20] = blocknames.index(blk)
    cm_ent[ord(ch) - 0x20] = ent
table("charmap", cm_blk, "room character (the byte .str emits) -> block id")
table("charent", cm_ent, "room character -> entity spawned in that cell")
m.write("CH_SPAWN = $%02X  ; the S that marks where the room starts you\n\n"
        % (ord("S") - 0x20))

for q, nm in enumerate(["tl", "tr", "bl", "br"]):
    table("ms_" + nm, [spr_tiles[k][q] for k in MS_ORDER],
          "sprite tile in the %s corner of each metasprite" % nm.upper())

out_chr = os.path.join(HERE, "..", "src", "chr.s")
out_map = os.path.join(HERE, "..", "src", "artmap.s")
open(out_chr, "w", newline="\n").write(chr_out.getvalue())
open(out_map, "w", newline="\n").write(m.getvalue())
print("wrote %s (%d background tiles, %d sprite tiles)"
      % (os.path.normpath(out_chr), len(bg.tiles), len(sp.tiles)))
print("wrote %s (%d blocks, %d room characters)"
      % (os.path.normpath(out_map), len(BLOCKS), len(CHARS)))
