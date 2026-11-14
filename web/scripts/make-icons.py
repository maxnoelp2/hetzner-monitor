#!/usr/bin/env python3
"""Generate the PWA icons in web/public/icons/ (Python 3 standard library only).

Motif: a Midnight-Abyss (#171f44) background with an Aqua (#74ffed) heartbeat line, the
Lucide "activity" path (M22 12h-4l-3 9L9 3l-3 9H2) with round caps and joins.

    python3 -I web/scripts/make-icons.py

Writes icon.svg, icon-192.png, icon-512.png (rounded square, transparent corners),
icon-maskable-512.png (full-bleed square, line inside the 80 % safe zone) and
apple-touch-icon.png (180 px, opaque full-bleed square; iOS rounds the corners itself).
The PNG rasteriser draws signed-distance shapes and supersamples pixels near edges.
"""

import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "public" / "icons"

BG = (0x17, 0x1F, 0x44)  # Midnight Abyss
LINE = (0x74, 0xFF, 0xED)  # Aqua

# Lucide "activity" in its 24-unit grid: M22 12h-4l-3 9L9 3l-3 9H2.
ACTIVITY = [(22, 12), (18, 12), (15, 21), (9, 3), (6, 12), (2, 12)]
CENTER = (12, 12)

# Geometry on a 512-unit canvas.
CANVAS = 512
CORNER = 112  # rounded-square radius (about 22 %)
STROKE = 2.25  # stroke width in Lucide units (Lucide uses 2; slightly bolder for small sizes)
SCALE_ANY = 16.0  # line spans 320 of 512 units
SCALE_MASKABLE = 13.0  # farthest point incl. stroke ~145 units from centre, safe zone radius is 204.8

SUPERSAMPLE = 4  # 4x4 samples for pixels close to an edge


def line_points(scale):
    """The activity polyline in canvas units, centred."""
    half = CANVAS / 2
    return [(half + (x - CENTER[0]) * scale, half + (y - CENTER[1]) * scale) for x, y in ACTIVITY]


def sd_rounded_rect(px, py, half, radius):
    """Signed distance to a centred rounded square (negative inside)."""
    qx = abs(px) - (half - radius)
    qy = abs(py) - (half - radius)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - radius


def sd_segment(px, py, ax, ay, bx, by):
    """Distance from a point to a line segment."""
    dx, dy = bx - ax, by - ay
    t = ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)
    t = 0.0 if t < 0.0 else 1.0 if t > 1.0 else t
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def sd_polyline(px, py, segs, radius):
    """Signed distance to a stroked polyline with round caps and joins."""
    return min(sd_segment(px, py, *s) for s in segs) - radius


def coverage(sdf, x, y, unit):
    """Fraction of pixel (x, y) inside the shape; supersampled only near the edge."""
    d = sdf((x + 0.5) * unit, (y + 0.5) * unit)
    if d <= -unit:
        return 1.0
    if d >= unit:
        return 0.0
    n = SUPERSAMPLE
    hits = 0
    for sy in range(n):
        for sx in range(n):
            if sdf((x + (sx + 0.5) / n) * unit, (y + (sy + 0.5) / n) * unit) <= 0.0:
                hits += 1
    return hits / (n * n)


def render(size, scale, rounded):
    """Render one icon to RGBA rows."""
    unit = CANVAS / size  # canvas units per pixel
    half = CANVAS / 2
    pts = line_points(scale)
    segs = [(a[0], a[1], b[0], b[1]) for a, b in zip(pts, pts[1:])]
    radius = STROKE * scale / 2

    def bg_sdf(cx, cy):
        return sd_rounded_rect(cx - half, cy - half, half, CORNER)

    def line_sdf(cx, cy):
        return sd_polyline(cx, cy, segs, radius)

    pad = radius + 2 * unit
    x0, x1 = min(p[0] for p in pts) - pad, max(p[0] for p in pts) + pad
    y0, y1 = min(p[1] for p in pts) - pad, max(p[1] for p in pts) + pad

    rows = []
    for y in range(size):
        row = bytearray()
        cy = (y + 0.5) * unit
        for x in range(size):
            cx = (x + 0.5) * unit
            a_bg = coverage(bg_sdf, x, y, unit) if rounded else 1.0
            a_line = 0.0
            if x0 <= cx <= x1 and y0 <= cy <= y1:
                a_line = coverage(line_sdf, x, y, unit)
            rgb = [round(b + (c - b) * a_line) for b, c in zip(BG, LINE)]
            row += bytes(rgb) + bytes([round(255 * a_bg)])
        rows.append(bytes(row))
    return rows


def write_png(path, size, rows):
    """Write 8-bit RGBA rows as a PNG (filter type 0 on every row)."""

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + r for r in rows)
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.write_bytes(data)


def write_svg(path):
    pts = line_points(SCALE_ANY)
    d = "M" + " L".join(f"{x:g} {y:g}" for x, y in pts)
    color = "#%02x%02x%02x"
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CANVAS} {CANVAS}">\n'
        f'  <rect width="{CANVAS}" height="{CANVAS}" rx="{CORNER}" fill="{color % BG}"/>\n'
        f'  <path d="{d}" fill="none" stroke="{color % LINE}" stroke-width="{STROKE * SCALE_ANY:g}"'
        ' stroke-linecap="round" stroke-linejoin="round"/>\n'
        "</svg>\n"
    )
    path.write_text(svg, encoding="utf-8")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    write_svg(OUT / "icon.svg")
    targets = [
        ("icon-192.png", 192, SCALE_ANY, True),
        ("icon-512.png", 512, SCALE_ANY, True),
        ("icon-maskable-512.png", 512, SCALE_MASKABLE, False),
        ("apple-touch-icon.png", 180, SCALE_ANY, False),
    ]
    for name, size, scale, rounded in targets:
        write_png(OUT / name, size, render(size, scale, rounded))
        print(f"wrote {OUT / name}")
    print(f"wrote {OUT / 'icon.svg'}")


if __name__ == "__main__":
    main()
