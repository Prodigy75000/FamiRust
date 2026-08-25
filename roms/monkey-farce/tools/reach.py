# SPDX-License-Identifier: CC0-1.0
#
# Can you actually finish this room?
#
#   python tools/reach.py            # every room
#   python tools/reach.py 4          # just that one
#
# A game built on lying to the player has to be scrupulously honest about one
# thing: there is a way through. Eyeballing a room does not establish that. The
# jump in this cartridge clears exactly two blocks of height and about three and
# a half of distance, and a platform placed one row too high is indistinguishable
# on paper from one placed correctly right up until nobody can finish the level.
#
# So this reads rooms.s, ports the physics out of main.s, and searches. From
# every block you can stand on it simulates real jumps, frame by frame, with the
# same constants, the same collision box and the same order of operations as the
# 6502, and asks which blocks you can be standing on next. Then it says whether
# the door is in that set, and whether every banana is.
#
# What it does NOT model: saws, crushers and darts. Those make a room hard. This
# answers the different and more important question of whether it is possible,
# which is a property of the walls alone.
import sys, os, re

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "src")

# ---------------------------------------------------------------------------
# Everything below has to match main.s. Read out of it rather than retyped, so
# that retuning the jump retunes the answer.
# ---------------------------------------------------------------------------
def constants(path):
    txt = open(path, encoding="utf-8").read()
    out = {}
    for m in re.finditer(r"^([A-Z_][A-Z_0-9]*)\s*=\s*(\d+)\s*(?:;.*)?$", txt, re.M):
        out[m.group(1)] = int(m.group(2))
    return out


K = constants(os.path.join(SRC, "main.s"))
GRAV, VY_MAX, JUMP_V = K["GRAV"], K["VY_MAX"], K["JUMP_V"]
WALK_ACC, AIR_ACC, FRICTION, WALK_MAX = K["WALK_ACC"], K["AIR_ACC"], K["FRICTION"], K["WALK_MAX"]
HB_L, HB_R, HB_T, HB_B = K["HB_L"], K["HB_R"], K["HB_T"], K["HB_B"]
PLAY_TOP, GRID_W, GRID_H = K["PLAY_TOP"], K["GRID_W"], K["GRID_H"]
NAME_LEN, COYOTE = K["NAME_LEN"], K["COYOTE_FRAMES"]
DART_SPEED = K["DART_SPEED"]

# How much warning the room owes you before the first thing shoots at you. A
# dart crossing three blocks takes about this long, which is enough to see it
# leave the wall. Less than this and the room has killed you before you have
# finished reading its name.
SAFE_FRAMES = 45

BF_SOLID, BF_HAZARD, BF_CRUMBLE, BF_GOAL = 0x01, 0x02, 0x04, 0x08

# Block flags and the room alphabet, read out of the generated artmap so the
# two cannot disagree about which character is which kind of block.
def artmap():
    txt = open(os.path.join(SRC, "artmap.s"), encoding="utf-8").read()

    def table(name):
        m = re.search(r"^%s:.*?\n((?:  \.byte .*\n)+)" % name, txt, re.M)
        vals = []
        for line in m.group(1).strip().split("\n"):
            vals += [int(v.strip()[1:], 16) for v in line.split(".byte")[1].split(",")]
        return vals

    return table("blk_flags"), table("charmap")


FLAGS, CHARMAP = artmap()


def flags_of(ch):
    """The flags of the block a room character stands for."""
    return FLAGS[CHARMAP[ord(ch) - 0x20]]


# ---------------------------------------------------------------------------
# The rooms, read as the pictures they are typed as.
# ---------------------------------------------------------------------------
def rooms():
    txt = open(os.path.join(SRC, "rooms.s"), encoding="utf-8").read()
    out = []
    for m in re.finditer(r"^room_(\d+):\n((?:\s*\.str \"[^\"]*\"\n)+)", txt, re.M):
        lines = re.findall(r'\.str "([^"]*)"', m.group(2))
        name, grid = lines[0], lines[1:]
        assert len(name) == NAME_LEN, "room %s name is %d chars" % (m.group(1), len(name))
        assert len(grid) == GRID_H, "room %s has %d rows" % (m.group(1), len(grid))
        for r in grid:
            assert len(r) == GRID_W, "room %s row %r is %d wide" % (m.group(1), r, len(r))
        out.append((int(m.group(1)), name.strip(), grid))
    return out


class Room:
    def __init__(self, grid):
        self.g = grid
        self.spawn = None
        self.goal = []
        for by in range(GRID_H):
            for bx in range(GRID_W):
                c = grid[by][bx]
                if c == "S":
                    self.spawn = (bx, by)
                if flags_of(c) & BF_GOAL:
                    self.goal.append((bx, by))

    def at(self, px, py):
        """Flags of the block covering a pixel, matching block_at in main.s:
        above the room is wall, below it is lava."""
        if py < PLAY_TOP:
            return BF_SOLID
        row = (py - PLAY_TOP) >> 4
        if row >= GRID_H:
            return BF_HAZARD
        col = (px >> 4) & 15
        return flags_of(self.g[row][col])

    def solid(self, px, py):
        # A cracked block is solid, and stays solid long enough to jump off, so
        # the search is allowed to use it. It is not allowed to rest there.
        return bool(self.at(px, py) & (BF_SOLID | BF_CRUMBLE))


# ---------------------------------------------------------------------------
# The physics, in the order main.s runs it.
# ---------------------------------------------------------------------------
def clamp(v, lo, hi):
    return max(lo, min(hi, v))


class Hero:
    def __init__(self, x, y):
        self.x, self.y = x << 8, y << 8
        self.vx = self.vy = 0
        self.ground = False
        self.coyote = 0

    def px(self):
        return self.x >> 8

    def py(self):
        return self.y >> 8

    def step(self, room, dx, jump_press, jump_hold):
        # 1. horizontal input
        if dx:
            self.vx = clamp(self.vx + dx * (WALK_ACC if self.ground else AIR_ACC),
                            -WALK_MAX, WALK_MAX)
        elif self.vx > 0:
            self.vx = max(0, self.vx - FRICTION)
        elif self.vx < 0:
            self.vx = min(0, self.vx + FRICTION)

        # 2. jump, and the cut that makes its height a choice
        if jump_press and self.coyote > 0:
            self.vy = -JUMP_V
            self.coyote = 0
            self.ground = False
        elif not jump_hold and self.vy < -256:
            self.vy = -256

        # 3. gravity
        self.vy = min(self.vy + GRAV, VY_MAX)

        # 4. horizontal move
        nx = self.x + self.vx
        if self.vx > 0:
            for oy in (HB_T, HB_B):
                if room.solid((nx >> 8) + HB_R, self.py() + oy):
                    nx = ((((nx >> 8) + HB_R) & 0xF0) - HB_R - 1) << 8
                    self.vx = 0
                    break
        elif self.vx < 0:
            for oy in (HB_T, HB_B):
                if room.solid((nx >> 8) + HB_L, self.py() + oy):
                    nx = (((((nx >> 8) + HB_L) & 0xF0) + 16) - HB_L) << 8
                    self.vx = 0
                    break
        self.x = nx

        # 5. vertical move
        ny = self.y + self.vy
        self.ground = False
        if self.vy > 0:
            for ox in (HB_L, HB_R):
                if room.solid(self.px() + ox, (ny >> 8) + HB_B):
                    top = ((((ny >> 8) + HB_B) - PLAY_TOP) & 0xF0) + PLAY_TOP
                    ny = (top - HB_B - 1) << 8
                    self.vy = 0
                    self.ground = True
                    break
        elif self.vy < 0:
            for ox in (HB_L, HB_R):
                if room.solid(self.px() + ox, (ny >> 8) + HB_T):
                    ry = (ny >> 8) + HB_T
                    top = (max(0, ry - PLAY_TOP) & 0xF0) + PLAY_TOP
                    ny = (top + 16 - HB_T) << 8
                    self.vy = 0
                    break
        self.y = ny

        # 6. the standing probe
        if self.vy >= 0:
            for ox in (HB_L, HB_R):
                if room.solid(self.px() + ox, self.py() + HB_B + 1):
                    self.ground = True
                    break

        # 7. coyote time
        self.coyote = COYOTE if self.ground else max(0, self.coyote - 1)

    def touching(self, room):
        """OR of the flags of every block the box overlaps, as hero_flags does."""
        f = 0
        for ox in (HB_L, HB_R):
            for oy in (HB_T, HB_B):
                f |= room.at(self.px() + ox, self.py() + oy)
        return f


# ---------------------------------------------------------------------------
# The search
# ---------------------------------------------------------------------------
# Every way of leaving a block: walk or run, left or right or straight up, with
# the jump held anywhere from a tap to the full climb, or not jumped at all.
PLANS = [(dx, run, hold)
         for dx in (-1, 0, 1)
         for run in (0, -1, 1)
         for hold in (0, 3, 6, 10, 14, 18)]

MAX_FRAMES = 90


def standable(room, bx, by):
    """Could he be at rest in this cell? The cell has to be clear and whatever
    is under it has to hold him."""
    if room.at(bx * 16 + 8, PLAY_TOP + by * 16 + 8) & (BF_SOLID | BF_HAZARD):
        return False
    return room.solid(bx * 16 + 8, PLAY_TOP + (by + 1) * 16 + 8)


def cells_under(h):
    """Every grid cell the hero's box is overlapping right now. A banana is
    collected by touching it, not by landing on it, so this is what decides
    whether one is gettable."""
    out = set()
    for ox in (HB_L, HB_R):
        for oy in (HB_T, HB_B):
            py = h.py() + oy
            if py < PLAY_TOP:
                continue
            row = (py - PLAY_TOP) >> 4
            if row < GRID_H:
                out.add(((h.px() + ox) >> 4, row))
    return out


def outcomes(room, bx, by):
    """Every cell he can be standing in after one departure from this one, and
    every cell he passes through on the way."""
    found, brushed = set(), set()
    for dx, run, hold in PLANS:
        h = Hero(bx * 16, PLAY_TOP + by * 16)
        h.vx = run * WALK_MAX
        h.ground = True
        h.coyote = COYOTE
        for f in range(MAX_FRAMES):
            h.step(room, dx, f == 0 and hold > 0, f < hold)
            t = h.touching(room)
            if t & BF_HAZARD:
                break
            brushed |= cells_under(h)
            if t & BF_GOAL:
                # Reaching a door is not the end of the simulation, because a
                # door only opens if you press UP in it. You are free to stand
                # in one, think better of it, and walk on, so everything past
                # the door is still reachable and a banana beside it is still
                # gettable.
                found.add(("GOAL",))
            if h.py() > PLAY_TOP + GRID_H * 16:
                break
            if h.ground and f > 2:
                cell = ((h.px() + 8) >> 4, (h.py() + 8 - PLAY_TOP) >> 4)
                if cell != (bx, by):
                    found.add(cell)
                    break
    return found, brushed


def solve(room):
    if room.spawn is None:
        return None, set()
    # The spawn is a cell in the air; fall to whatever is beneath it first.
    start = room.spawn
    while start[1] + 1 < GRID_H and not standable(room, *start):
        start = (start[0], start[1] + 1)
    seen, queue, won, brushed = {start}, [start], False, set()
    while queue:
        cell = queue.pop()
        landings, passed = outcomes(room, *cell)
        brushed |= passed
        for nxt in landings:
            if nxt == ("GOAL",):
                won = True
            elif nxt not in seen and 0 <= nxt[0] < GRID_W and 0 <= nxt[1] < GRID_H:
                seen.add(nxt)
                queue.append(nxt)
    return won, seen, brushed


# The cartridge keeps room 6 within one entity of this, so it is worth saying
# out loud: over the limit the extras are silently not spawned, and a room that
# quietly drops its last saw looks finished and is not the room that was drawn.
ENT_MAX = 8
SPAWNERS = "BWC><"


def spawn_ambush(grid, start):
    """Shooters that can put a dart into you before you have had time to
    understand there is a room.

    A shooter fires along its own row at its own height, and the player spends
    the first second of a room standing exactly where they were put. Drawing a
    `>` in the wall beside the `S` is therefore not a hard opening, it is a
    coin flip taken out of the player's hands, and it is very easy to do by
    accident because in the picture the two characters are simply next to each
    other."""
    out = []
    sx, sy = start
    for by in range(GRID_H):
        if by != sy:
            continue
        for bx in range(GRID_W):
            c = grid[by][bx]
            if c not in "><":
                continue
            step = 1 if c == ">" else -1
            x = bx + step
            while 0 <= x < GRID_W:
                if x == sx:
                    frames = max(0, (abs(x - bx) - 1) * 16) // DART_SPEED
                    if frames < SAFE_FRAMES:
                        out.append((bx, by, frames))
                    break
                if flags_of(grid[by][x]) & BF_SOLID:
                    break
                x += step
    return out


def main():
    want = int(sys.argv[1]) if len(sys.argv) > 1 else None
    bad = 0
    for num, name, grid in rooms():
        if want is not None and num != want:
            continue
        room = Room(grid)
        won, seen, brushed = solve(room)
        ents = sum(row.count(c) for row in grid for c in SPAWNERS)
        # A banana you cannot possibly touch is not a hard banana, it is a bug
        # that looks like content, and it is the single easiest mistake to make
        # in this format: the bait goes where it reads well, not where the jump
        # arc goes.
        # Two bars, because they are different failures. A banana outside
        # `brushed` cannot be had at all. A banana that is only in `brushed` can
        # be had, but by a maximum-height jump from one exact spot, which in
        # the hand feels the same as impossible and is what the playtest
        # actually complained about. The bar is: stand in its cell, or stand
        # directly under it, and the rest of the risk comes from what is below.
        bananas = [(bx, by) for by in range(GRID_H) for bx in range(GRID_W)
                  if grid[by][bx] == "B"]
        lost = [c for c in bananas if c not in brushed]
        awkward = [c for c in bananas
                   if c in brushed and c not in seen and (c[0], c[1] + 1) not in seen]
        start = room.spawn
        while start and start[1] + 1 < GRID_H and not standable(room, *start):
            start = (start[0], start[1] + 1)
        ambush = spawn_ambush(grid, start) if start else []
        mark = "OK  " if won and not lost and not awkward and not ambush else "DEAD"
        if not won or lost or awkward or ambush:
            bad += 1
        print("%s room %d  %-14s  %d cells reachable of %d standable, %d/%d entities, %d/%d bananas"
              % (mark, num, name, len(seen),
                 sum(standable(room, x, y) for y in range(GRID_H) for x in range(GRID_W)),
                 ents, ENT_MAX, len(bananas) - len(lost) - len(awkward), len(bananas)))
        if not won:
            print("      the door cannot be reached")
        for bx, by in lost:
            print("      the banana at column %d row %d cannot be touched at all" % (bx, by))
        for bx, by in awkward:
            print("      the banana at column %d row %d needs a maximum jump from one"
                  " exact spot" % (bx, by))
        for bx, by, frames in ambush:
            print("      the shooter at column %d row %d reaches the spawn in %d"
                  " frames" % (bx, by, frames))
        if ents > ENT_MAX:
            print("      room %d asks for %d entities and only %d will spawn"
                  % (num, ents, ENT_MAX))
        if not won or lost or awkward or ambush or os.environ.get("SHOW"):
            # Print the room with the reachable cells marked, which is the whole
            # diagnosis: you can see exactly where the search ran out of floor.
            for by in range(GRID_H):
                row = "".join(
                    ("o" if (bx, by) in seen and grid[by][bx] == "."
                     else "," if (bx, by) in brushed and grid[by][bx] == "."
                     else grid[by][bx])
                    for bx in range(GRID_W))
                print("      " + row)
    if bad:
        print("\n%d room(s) cannot be finished." % bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
