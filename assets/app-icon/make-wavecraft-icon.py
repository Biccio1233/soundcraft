#!/usr/bin/env python3
"""Draw WaveCraft's app icon (original artwork): a teal waveform over a dark violet rounded tile.

Pure Python (zlib + struct), no image library: `python3 make-wavecraft-icon.py OUT.png [SIZE]`.
"""
import math
import struct
import sys
import zlib


def png(path, w, h, rows):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(r) for r in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "wavecraft-512.png"
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 512
    margin, radius = n * 0.09, n * 0.2
    top, bottom = (40, 18, 74), (12, 6, 26)
    wave = (78, 201, 176)
    rows = []
    for y in range(n):
        row = []
        for x in range(n):
            # Rounded tile with a vertical gradient.
            cx = min(max(x, margin + radius), n - margin - radius)
            cy = min(max(y, margin + radius), n - margin - radius)
            d = math.hypot(x - cx, y - cy) - radius
            alpha = max(0.0, min(1.0, 0.5 - d))
            t = (y - margin) / (n - 2 * margin)
            bg = [round(top[i] + (bottom[i] - top[i]) * max(0.0, min(1.0, t))) for i in range(3)]
            # Waveform: a decaying envelope of vertical bars.
            u = (x - margin) / (n - 2 * margin)
            env = 0.0
            if 0.08 < u < 0.92:
                k = (u - 0.08) / 0.84
                env = (0.25 + 0.75 * math.exp(-3.2 * k)) * abs(math.sin(k * math.pi * 7.0) * 0.6 + 0.4)
            half = env * n * 0.3
            bar = (int((x - margin) / (n * 0.022)) % 2) == 0
            color = bg
            if bar and env > 0 and abs(y - n / 2) <= half:
                color = list(wave)
            row += color + [round(255 * alpha)]
        rows.append(row)
    png(out, n, n, rows)


if __name__ == "__main__":
    main()
