#!/usr/bin/env python3
"""Generate the tray icons in src-tauri/icons/tray-*.png.

The icon is Ferris, the Rust mascot — a crab. Public domain, and apt for a
Rust app. It carries the severity as its colour, because menu bar title text
cannot be coloured on either platform (spec §8.2).

Drawn from geometric primitives rather than a rasterised SVG, so this script
needs no image library and no external binary — only the standard library.
The shape was rendered and eyeballed at 44px before being written down:
claws up, two eyes bitten out of the body, three legs a side. If you change
the geometry, re-open one of the generated PNGs enlarged and confirm it still
reads as a crab, not a blob.

The five colours below are the current severity ramp (Task 13): four bands —
normal/warning/high/critical — plus the neutral "no data yet" icon. They were
run through the dataviz palette validator in both colourblind and
normal-vision modes; colourblind separation passes (worst adjacent pair ΔE
13.0 deutan), the normal-vision floor fails narrowly (14.9 against a
threshold of 15) — accepted deliberately, because colour is reinforcement
here (the percentage is always shown as text next to it), never the sole
carrier of identity. Do not retune the hues without re-running that
validator; there isn't room between green and red for a fifth reliably
distinct step, let alone a better-separated four.

Usage:
    python3 scripts/generate-tray-icons.py
"""
import struct
import zlib
import math
import pathlib

COLOURS = {
    "tray-neutral":  (0xA1, 0xA1, 0xAA),
    "tray-normal":   (0x16, 0xA3, 0x4A),
    "tray-warning":  (0xFA, 0xCC, 0x15),
    "tray-high":     (0xF9, 0x73, 0x16),
    "tray-critical": (0xDC, 0x26, 0x26),
}
SIZE, SS = 44, 4   # 44px canvas, 4x supersampling for smooth edges

ICONS_DIR = pathlib.Path(__file__).resolve().parent.parent / "src-tauri" / "icons"


def rot(px, py, cx, cy, deg):
    a = math.radians(deg)
    dx, dy = px - cx, py - cy
    return (dx * math.cos(a) + dy * math.sin(a), -dx * math.sin(a) + dy * math.cos(a))


def ell(px, py, cx, cy, rx, ry, deg=0.0):
    dx, dy = rot(px, py, cx, cy, deg)
    return (dx / rx) ** 2 + (dy / ry) ** 2 <= 1.0


def inside(px, py):
    # claws, with a pincer notch bitten out of each
    for cx in (7.5, 36.5):
        if ell(px, py, cx, 13, 6.0, 5.2):
            if not ell(px, py, cx + (2.6 if cx < 22 else -2.6), 10.0, 2.9, 2.2):
                return True
    # arms joining the claws to the body
    if ell(px, py, 13.5, 20.0, 6.0, 1.9, -38):
        return True
    if ell(px, py, 30.5, 20.0, 6.0, 1.9, 38):
        return True
    # three legs a side
    for s in (1, -1):
        cx = 22 + s * 8
        if ell(px, py, cx, 27.5, 6.2, 1.7, s * -18):
            return True
        if ell(px, py, cx - s * 1.0, 32.0, 5.8, 1.7, s * 12):
            return True
        if ell(px, py, cx - s * 2.0, 35.5, 5.0, 1.7, s * 38):
            return True
    # body, with the eyes bitten out
    if ell(px, py, 22, 26, 12.6, 8.6):
        if ell(px, py, 17.6, 21.5, 1.9, 1.9):
            return False
        if ell(px, py, 26.4, 21.5, 1.9, 1.9):
            return False
        return True
    return False


def write_png(path, rgb):
    rows = []
    for y in range(SIZE):
        row = bytearray()
        for x in range(SIZE):
            hits = sum(
                inside(x + (sx + .5) / SS, y + (sy + .5) / SS)
                for sy in range(SS) for sx in range(SS)
            )
            row += bytes((rgb[0], rgb[1], rgb[2], int(255 * hits / (SS * SS))))
        rows.append(bytes(row))
    raw = b"".join(b"\x00" + r for r in rows)

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    pathlib.Path(path).write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    ICONS_DIR.mkdir(parents=True, exist_ok=True)
    for name, rgb in COLOURS.items():
        out = ICONS_DIR / f"{name}.png"
        write_png(out, rgb)
        print("wrote", out)


if __name__ == "__main__":
    main()
