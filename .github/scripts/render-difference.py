"""Mean channel difference between two renders, as `bundle --verify` measures it.

Usage: render-difference.py GRID SMOKE FLOOR. Exits 1 when the two renders are
closer than FLOOR, which is what a render that found no textures looks like
next to the procedural grid. Standard library only, so a runner needs nothing
installed: it reads the 8-bit, non-interlaced RGB or RGBA PNGs the app writes
and refuses anything else.
"""

import struct
import sys
import zlib


def pixels(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit(f"{path} is not a PNG")
    pos, idat, header = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if depth != 8 or color not in (2, 6) or interlace:
        sys.exit(f"{path} is not an 8-bit non-interlaced RGB or RGBA PNG")
    bpp = 4 if color == 6 else 3
    raw, stride = zlib.decompress(idat), width * bpp
    out, prev = bytearray(), bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind, row = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        for i in range(stride):
            a = row[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if kind == 1:
                row[i] = (row[i] + a) & 0xFF
            elif kind == 2:
                row[i] = (row[i] + b) & 0xFF
            elif kind == 3:
                row[i] = (row[i] + (a + b) // 2) & 0xFF
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                row[i] = (row[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 0xFF
        out += row
        prev = row
    if bpp == 3:
        out = bytearray(b for i in range(0, len(out), 3) for b in out[i:i + 3] + b"\xff")
    return (width, height), out


grid_size, grid = pixels(sys.argv[1])
smoke_size, smoke = pixels(sys.argv[2])
if grid_size != smoke_size:
    sys.exit(f"the renders are {grid_size} and {smoke_size}")
delta = sum(abs(x - y) for x, y in zip(grid, smoke)) / len(grid)
floor = float(sys.argv[3])
print(f"the two renders differ by {delta:.2f} of a channel step, floor {floor}")
sys.exit(0 if delta >= floor else 1)
