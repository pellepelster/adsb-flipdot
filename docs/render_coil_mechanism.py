#!/usr/bin/env python3
"""Draw docs/coil-mechanism.svg: side view of one dot, the disc magnet over the
coil's iron core, turned half a turn by reversing the current.

    python3 docs/render_coil_mechanism.py
    google-chrome --headless --screenshot=docs/coil-mechanism.png \
        --window-size=1180,500 --force-device-scale-factor=1.25 docs/coil-mechanism.svg
"""
import math
from pathlib import Path

from diagram import *  # noqa: F401,F403


YEL, BLK, HOT = "#facc15", "#111827", "#f97316"
NCOL, SCOL = "#dc2626", "#2563eb"


def rot(px, py, cx, cy, a):
    c, s = math.cos(a), math.sin(a)
    return cx + (px - cx) * c - (py - cy) * s, cy + (px - cx) * s + (py - cy) * c


def polyf(pts, fill, stroke="none", sw=0):
    out.append('<polygon points="%s" fill="%s" stroke="%s" stroke-width="%g" stroke-linejoin="round"/>'
               % (" ".join("%g,%g" % p for p in pts), fill, stroke, sw))


def box(cx, cy, x0, y0, x1, y1, a, fill, stroke="none", sw=0):
    polyf([rot(x, y, cx, cy, a) for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1))], fill, stroke, sw)


def pole_tag(x, y, n):
    out.append('<circle cx="%g" cy="%g" r="13" fill="%s"/>' % (x, y, NCOL if n else SCOL))
    text(x, y + 5, "N" if n else "S", 14, "middle", "bold", "#fff")


def flipdot(ox, oy, yellow):
    """Side view, axle into the page. The viewer looks from the top."""
    cx, cy = ox + 200, oy + 150
    tilt = math.radians(-8)
    a = tilt if yellow else tilt + math.pi
    # disc: yellow face on one side, black on the other, magnet along its length
    box(cx, cy, cx - 110, cy - 9, cx + 110, cy - 3, a, YEL, "#a16207", 1)
    box(cx, cy, cx - 110, cy - 3, cx + 110, cy + 3, a, "#e5e7eb")
    box(cx, cy, cx - 110, cy + 3, cx + 110, cy + 9, a, BLK)
    box(cx, cy, cx - 104, cy - 2.5, cx - 70, cy + 2.5, a, NCOL)     # N end of the disc magnet
    box(cx, cy, cx + 70, cy - 2.5, cx + 104, cy + 2.5, a, SCOL)     # S end
    out.append('<circle cx="%g" cy="%g" r="6" fill="#fff" stroke="#374151" stroke-width="2.5"/>' % (cx, cy))
    nx, ny = rot(cx - 87, cy, cx, cy, a)
    text(nx, ny - 16 if yellow else ny + 26, "N", 12, "middle", "bold", NCOL)
    sx, sy = rot(cx + 87, cy, cx, cy, a)
    text(sx, sy - 16 if yellow else sy + 26, "S", 12, "middle", "bold", SCOL)
    # viewer
    text(cx, oy + 30, "seen from the front: " + ("yellow" if yellow else "black"), 13, "middle", "bold")
    line(cx, oy + 40, cx, oy + 82, GREY, 1.5, "4 4")
    out.append('<polygon points="%g,%g %g,%g %g,%g" fill="%s"/>' % (cx - 6, oy + 76, cx + 6, oy + 76, cx, oy + 88, GREY))
    # U-shaped iron core with the coil on its base
    core = "#9ca3af"
    lx, rx, top, base = cx - 140, cx + 140, cy + 6, cy + 150
    rect(lx - 14, top, 28, base - top, core, "#6b7280", 3, 1.5)
    rect(rx - 14, top, 28, base - top, core, "#6b7280", 3, 1.5)
    rect(lx - 14, base - 28, rx - lx + 28, 28, core, "#6b7280", 3, 1.5)
    for k in range(9):
        x = cx - 80 + k * 20
        out.append('<ellipse cx="%g" cy="%g" rx="9" ry="22" fill="none" stroke="#b45309" stroke-width="3.5"/>'
                   % (x, base - 14))
    # pole faces: the core pulls the opposite end of the disc magnet
    pole_tag(lx, top - 22, not yellow)
    pole_tag(rx, top - 22, yellow)
    # current through the coil
    yy = base + 40
    if yellow:
        arrow_txt = "current row → column"
        poly([(cx - 110, yy), (cx + 110, yy)], HOT, 3)
        out.append('<polygon points="%g,%g %g,%g %g,%g" fill="%s"/>' % (cx + 110, yy - 7, cx + 110, yy + 7, cx + 124, yy, HOT))
    else:
        arrow_txt = "current column → row"
        poly([(cx - 110, yy), (cx + 110, yy)], HOT, 3)
        out.append('<polygon points="%g,%g %g,%g %g,%g" fill="%s"/>' % (cx - 110, yy - 7, cx - 110, yy + 7, cx - 124, yy, HOT))
    text(cx, yy + 24, arrow_txt, 13, "middle", "bold", HOT)
    text(cx, base - 40, "coil", 12, "middle", fill="#b45309")
    text(lx - 22, base - 50, "iron core", 11, "end", fill=GREY)


text(40, 46, "Inside one dot: how the coil flips the disc", 24, weight="bold")
text(40, 72, "Side view, the axle points into the page. A small magnet runs along the disc; the coil's iron core "
     "sits under its two ends.", 14, fill="#4b5563")
flipdot(40, 100, True)
flipdot(700, 100, False)
# the flip between the two
mx, my = 590, 250
path("M %g %g A 60 60 0 1 1 %g %g" % (mx - 50, my + 20, mx + 50, my + 20), INK, 2.5)
out.append('<polygon points="%g,%g %g,%g %g,%g" fill="%s"/>' % (mx + 42, my + 12, mx + 58, my + 14, mx + 50, my + 30, INK))
text(mx, my + 54, "reverse the", 12.5, "middle", weight="bold")
text(mx, my + 70, "current: the disc", 12.5, "middle", weight="bold")
text(mx, my + 86, "turns half a turn", 12.5, "middle", weight="bold")

W, H = 1180, 500
save(Path(__file__).with_name("coil-mechanism.svg"), W, H)
