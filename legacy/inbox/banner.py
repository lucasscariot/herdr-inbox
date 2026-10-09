"""A slowly turning thread sculpture above a stationary wordmark, both made of lit Braille dots."""

from functools import lru_cache
from math import cos, pi, sin, sqrt

FPS = 10
TURN_SECONDS = 60
COLORS = (94, 130, 172, 208, 223, 220, 229, 237, 239, 60, 67, 110, 153, 195)
# The wordmark is drawn from the same lit Braille dots as the sculpture. Each
# glyph is a 9 x 16 dot bitmap built from rectangles and diagonal strokes.
GLYPH_WIDTH, GLYPH_HEIGHT, GLYPH_GAP, SPACE_WIDTH = 9, 16, 3, 5
WORDMARK = "HERDR INBOX"


def _diagonal(x0, x1, rows=range(GLYPH_HEIGHT)):
    """A three-dot-wide stroke from column x0 on the top row to x1 on the bottom row."""
    last = GLYPH_HEIGHT - 1
    return [(x0 + round((x1 - x0) * row / last), row, 3, 1) for row in rows]


_GLYPHS = {
    "H": [(0, 0, 3, 16), (6, 0, 3, 16), (0, 6, 9, 3)],
    "E": [(0, 0, 3, 16), (0, 0, 9, 3), (0, 6, 7, 3), (0, 13, 9, 3)],
    "R": [(0, 0, 3, 16), (0, 0, 8, 3), (6, 1, 3, 8), (0, 6, 9, 3)] + _diagonal(3, 6, range(9, 16)),
    "D": [(0, 0, 3, 16), (0, 0, 7, 3), (0, 13, 7, 3), (6, 2, 3, 12)],
    "I": [(3, 0, 3, 16), (0, 0, 9, 3), (0, 13, 9, 3)],
    "N": [(0, 0, 3, 16), (6, 0, 3, 16)] + _diagonal(0, 6),
    "B": [(0, 0, 3, 16), (0, 0, 8, 3), (0, 6, 8, 3), (0, 13, 8, 3), (6, 1, 3, 6), (6, 8, 3, 7)],
    "O": [(1, 0, 7, 3), (1, 13, 7, 3), (0, 1, 3, 14), (6, 1, 3, 14)],
    "X": _diagonal(0, 6) + _diagonal(6, 0),
}


def _wordmark_dots():
    dots, x = set(), 0
    for character in WORDMARK:
        if character == " ":
            x += SPACE_WIDTH + GLYPH_GAP
            continue
        for left, top, width, height in _GLYPHS[character]:
            for dx in range(width):
                for dy in range(height):
                    if 0 <= left + dx < GLYPH_WIDTH:
                        dots.add((x + left + dx, top + dy))
        x += GLYPH_WIDTH + GLYPH_GAP
    return dots, x - GLYPH_GAP


_DOTS, _DOT_WIDTH = _wordmark_dots()
_WIDTH = (_DOT_WIDTH + 1) // 2
WORDMARK_ROWS = GLYPH_HEIGHT // 4


@lru_cache(maxsize=32)
def _wordmark(width):
    """Rasterize the lettering as Braille cells, lit brighter toward the top like the knot."""
    left = max(0, (width - _WIDTH) // 2)
    cells = {}
    for x, y in _DOTS:
        cell = x // 2, y // 4
        bit = ((0, 3), (1, 4), (2, 5), (6, 7))[y % 4][x % 2]
        cells[cell] = cells.get(cell, 0) | (1 << bit)
    rows = []
    for row in range(WORDMARK_ROWS):
        shade = 7 + round((1 - 0.11 * row) * 6)
        runs = []
        for column in range(_WIDTH):
            mask = cells.get((column, row))
            if not mask:
                continue
            character = chr(0x2800 + mask)
            if runs and runs[-1][0] + len(runs[-1][1]) == left + column:
                start, text = runs[-1]
                runs[-1] = start, text + character
            else:
                runs.append((left + column, character))
        rows.append(tuple((start, text, shade) for start, text in runs))
    return tuple(rows)


def pose(elapsed):
    """Turn once a minute around a fixed, gently tilted axis.

    Poses are quantized to animation frames so a full turn fits the frame cache
    and the second minute costs nothing to render.
    """
    step = round(elapsed * FPS) % (FPS * TURN_SECONDS)
    return round(2 * pi * step / (FPS * TURN_SECONDS), 4), -0.12


def _cloud():
    """Sample a trefoil's tube surface with a normal for each lit point."""
    points = []
    for index in range(420):
        t = 2 * pi * index / 420
        c, s = cos(2 * t), sin(2 * t)
        radius = 1.15 + 0.48 * cos(3 * t)
        center = radius * c, radius * s, 0.48 * sin(3 * t)
        dx = -1.44 * sin(3 * t) * c - 2 * radius * s
        dy = -1.44 * sin(3 * t) * s + 2 * radius * c
        dz = 1.44 * cos(3 * t)
        length = sqrt(dx * dx + dy * dy + dz * dz)
        tx, ty, tz = dx / length, dy / length, dz / length
        length = sqrt(tx * tx + ty * ty)
        side = -ty / length, tx / length, 0
        up = -tz * side[1], tz * side[0], tx * side[1] - ty * side[0]
        for sample in range(30):
            a = 2 * pi * sample / 30
            c, s = cos(a), sin(a)
            normal = tuple(side[j] * c + up[j] * s for j in range(3))
            point = tuple(center[j] + 0.24 * normal[j] for j in range(3))
            points.append((point, normal))
    return tuple(points)


_CLOUD = _cloud()


def _sculpture(width, height, yaw, pitch):
    # Braille gives two by four independently lit dots per terminal cell.
    dot_width, dot_height = width * 2, height * 4
    dots = {}
    cy, sy = cos(0.6 + yaw), sin(0.6 + yaw)
    cp, sp = cos(-0.40 + pitch * 0.7), sin(-0.40 + pitch * 0.7)
    cr, sr = cos(0.18), sin(0.18)
    scale = min((dot_height - 6) / 4.5, (dot_width - 6) / 4.5)

    def rotate(point):
        x, y, z = point
        y, z = y * cp - z * sp, y * sp + z * cp
        x, z = x * cy + z * sy, -x * sy + z * cy
        return x * cr - y * sr, x * sr + y * cr, z

    for point, normal in _CLOUD:
        x, y, z = rotate(point)
        nx, ny, nz = rotate(normal)
        perspective = 6 / (6 - z)
        px = round((dot_width - 1) / 2 + x * perspective * scale)
        py = round((dot_height - 1) / 2 + y * perspective * scale)
        if not (0 <= px < dot_width and 0 <= py < dot_height):
            continue
        previous = dots.get((px, py))
        if previous and previous[0] >= z:
            continue
        diffuse = max(0, -0.4 * nx - 0.5 * ny + 0.77 * nz)
        specular = max(0, -0.21 * nx - 0.27 * ny + 0.94 * nz) ** 20
        light = min(1, 0.12 + 0.62 * diffuse + 0.75 * specular)
        dots[(px, py)] = z, light

    cells = {}
    for (x, y), (_, light) in dots.items():
        cell = x // 2, y // 4
        bit = ((0, 3), (1, 4), (2, 5), (6, 7))[y % 4][x % 2]
        mask, brightest = cells.get(cell, (0, 0))
        cells[cell] = mask | (1 << bit), max(brightest, light)
    rows = []
    for row in range(height):
        runs = []
        for column in range(width):
            cell = cells.get((column, row))
            if not cell:
                continue
            mask, light = cell
            shade, character = 7 + round(light * 6), chr(0x2800 + mask)
            if runs and runs[-1][0] + len(runs[-1][1]) == column and runs[-1][2] == shade:
                start, text, _ = runs[-1]
                runs[-1] = start, text + character, shade
            else:
                runs.append((column, character, shade))
        rows.append(tuple(runs))
    return tuple(rows)


@lru_cache(maxsize=FPS * TURN_SECONDS + 16)
def frame(width, height, yaw, pitch):
    """Rotate the sculpture while keeping the wordmark centered and still."""
    rows = list(_sculpture(width, height - WORDMARK_ROWS - 1, yaw, pitch)) + [()]
    rows.extend(_wordmark(width))
    return tuple(rows)
