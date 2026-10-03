"""Draws the app and tray icons (no image libraries needed).

    python3 packaging/make_icons.py

Writes app/src-tauri/icons/{icon.png,32x32.png,128x128.png,128x128@2x.png} and the tray icons
tray-{idle,recording,paused,error,dictating}.png. Run `npx tauri icon app/src-tauri/icons/icon.png` from
app/ui afterwards to regenerate icon.icns / icon.ico for bundling.
"""

import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "app" / "src-tauri" / "icons"
SS = 4  # supersampling per axis


def png(path, w, h, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * w * 4:(y + 1) * w * 4]) for y in range(h))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.write_bytes(data)


def rounded_rect(x, y, cx, cy, hw, hh, r):
    qx, qy = abs(x - cx) - hw + r, abs(y - cy) - hh + r
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def render(size, layers):
    """layers: list of (sdf(x, y) in unit coords, (r, g, b, a)); painted in order."""
    px = [0] * (size * size * 4)
    for j in range(size):
        for i in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sj in range(SS):
                for si in range(SS):
                    x = (i + (si + 0.5) / SS) / size
                    y = (j + (sj + 0.5) / SS) / size
                    col = [0.0, 0.0, 0.0, 0.0]
                    for sdf, (r, g, b, a) in layers:
                        if sdf(x, y) <= 0:
                            fa = a / 255
                            out_a = fa + col[3] * (1 - fa)
                            if out_a > 0:
                                for k, c in enumerate((r, g, b)):
                                    col[k] = (c * fa + col[k] * col[3] * (1 - fa)) / out_a
                            col[3] = out_a
                    for k in range(3):
                        acc[k] += col[k] * col[3]
                    acc[3] += col[3]
            n = SS * SS
            a = acc[3] / n
            o = (j * size + i) * 4
            if a > 0:
                for k in range(3):
                    px[o + k] = round(acc[k] / acc[3])
            px[o + 3] = round(a * 255)
    return px


def bars(color, scale=1.0, cx=0.5, cy=0.5):
    """Three rounded bars, the middle one tallest: a quiet level meter."""
    w = 0.11 * scale
    out = []
    for k, h in ((-1, 0.36), (0, 0.6), (1, 0.36)):
        x0 = cx + k * 0.2 * scale
        hh = h * scale / 2
        out.append((lambda x, y, x0=x0, hh=hh: rounded_rect(x, y, x0, cy, w / 2, hh, w / 2), color))
    return out


def circle(cx, cy, r, color):
    return (lambda x, y: math.hypot(x - cx, y - cy) - r, color)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    bg = (lambda x, y: rounded_rect(x, y, 0.5, 0.5, 0.42, 0.42, 0.1), (30, 36, 48, 255))
    app = [bg] + bars((242, 244, 248, 255), 0.85)
    for name, size in (("icon.png", 512), ("128x128@2x.png", 256), ("128x128.png", 128), ("32x32.png", 32)):
        png(OUT / name, size, size, render(size, app))

    ink = (40, 40, 40, 255)
    badge_ring = (255, 255, 255, 255)
    states = {
        "idle": [],
        "recording": [circle(0.78, 0.78, 0.2, badge_ring), circle(0.78, 0.78, 0.15, (230, 57, 70, 255))],
        "paused": [circle(0.78, 0.78, 0.2, badge_ring), circle(0.78, 0.78, 0.15, (240, 170, 30, 255))],
        "error": [circle(0.78, 0.78, 0.2, badge_ring), circle(0.78, 0.78, 0.15, (120, 120, 120, 255))],
        "dictating": [circle(0.78, 0.78, 0.2, badge_ring), circle(0.78, 0.78, 0.15, (59, 111, 224, 255))],
    }
    plate = circle(0.45, 0.45, 0.44, (242, 244, 248, 230))
    for state, badge in states.items():
        layers = [plate] + bars(ink, 0.75, 0.45, 0.45) + badge
        png(OUT / f"tray-{state}.png", 64, 64, render(64, layers))
    print("icons written to", OUT)


if __name__ == "__main__":
    main()
