#!/usr/bin/env python3
# Author: Jamius Siam
# Since: 31/10/2026
"""Generates framelet.ico, framelet-rec.ico (red record dot) and framelet.png without any imaging library.

Glyph: four rounded viewfinder corners around a centered dot, accent blue gradient on transparent.
Usage: python make_icons.py   (writes next to this script)
"""
import math
import os
import struct
import zlib

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
TOP, BOTTOM = (0x6C, 0xD2, 0xFF), (0x2F, 0x7B, 0xFF)  # accent gradient
RED = (0xFF, 0x3B, 0x30)


def seg_dist(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def render(size, dot_color=None):
    """Returns RGBA bytes. Geometry is defined on a 256 grid and scaled."""
    s = size / 256.0
    small = size <= 24
    stroke = 30.0 if small else 22.0
    lo = 30.0 if small else 36.0  # bracket outer edge (stroke center is inset by stroke/2)
    c0 = lo + stroke / 2
    arm = 62.0 if small else 56.0
    r = 30.0 if small else 34.0
    dot_r = 30.0 if small else 28.0
    out = bytearray()
    for y in range(size):
        for x in range(size):
            px, py = (x + 0.5) / s, (y + 0.5) / s
            fx = px if px < 128 else 256 - px  # fold into the top-left corner
            fy = py if py < 128 else 256 - py
            # L-shaped path: vertical arm, quarter arc, horizontal arm
            d = min(
                seg_dist(fx, fy, c0, c0 + r, c0, c0 + arm),
                seg_dist(fx, fy, c0 + r, c0, c0 + arm, c0),
            )
            if fx < c0 + r and fy < c0 + r:
                d = min(d, abs(math.hypot(fx - (c0 + r), fy - (c0 + r)) - r))
            cov = max(0.0, min(1.0, 0.5 - (d - stroke / 2) * s))
            dd = math.hypot(px - 128, py - 128) - dot_r
            dot_cov = max(0.0, min(1.0, 0.5 - dd * s))
            t = py / 256.0
            base = tuple(TOP[i] + (BOTTOM[i] - TOP[i]) * t for i in range(3))
            if dot_color is not None and dot_cov > 0:
                col = dot_color
            else:
                col = base
            a = max(cov, dot_cov)
            out += bytes((round(col[0]), round(col[1]), round(col[2]), round(a * 255)))
    return bytes(out)


def png(size, rgba):
    raw = b"".join(b"\x00" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def ico(images):
    head = struct.pack("<HHH", 0, 1, len(images))
    entries, blobs, offset = b"", b"", 6 + 16 * len(images)
    for size, data in images:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    return head + entries + blobs


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    for name, dot in (("framelet", None), ("framelet-rec", RED)):
        images = [(s, png(s, render(s, dot))) for s in SIZES]
        with open(os.path.join(here, name + ".ico"), "wb") as f:
            f.write(ico(images))
    with open(os.path.join(here, "framelet.png"), "wb") as f:
        f.write(png(256, render(256)))


if __name__ == "__main__":
    main()
