#!/usr/bin/env python3
"""Draw docs/dot-flow.svg: the current through one dot for each colour, and the
order of direction, row enable and select for flipping it.

    python3 docs/render_dot_flow.py
    google-chrome --headless --screenshot=docs/dot-flow.png \
        --window-size=1640,1206 --force-device-scale-factor=1.25 docs/dot-flow.svg
"""
import math
from pathlib import Path

from diagram import *  # noqa: F401,F403


HOT = "#f97316"
YEL, BLK = "#facc15", "#111827"


def arrowhead(x, y, ang, color):
    p = [(x + 9 * math.cos(ang), y + 9 * math.sin(ang)),
         (x - 7 * math.cos(ang - 0.6), y - 7 * math.sin(ang - 0.6)),
         (x - 7 * math.cos(ang + 0.6), y - 7 * math.sin(ang + 0.6))]
    out.append('<polygon points="%s" fill="%s"/>' % (" ".join("%g,%g" % q for q in p), color))


def current(pts):
    """The path of the coil current, drawn under the wires, with an arrow on each long segment."""
    out.append('<path d="M %s" fill="none" stroke="%s" stroke-opacity="0.35" stroke-width="12" '
               'stroke-linejoin="round" stroke-linecap="round"/>' % (" L ".join("%g %g" % p for p in pts), HOT))
    for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
        if math.hypot(x2 - x1, y2 - y1) > 60:
            arrowhead((x1 + x2) / 2, (y1 + y2) / 2, math.atan2(y2 - y1, x2 - x1), HOT)


def tag(x, y, s, color):
    w = 10 + 7.4 * len(s)
    rect(x - w / 2, y - 10, w, 20, color, color, 10)
    text(x, y + 4.5, s, 11.5, "middle", "bold", "#fff")


def drv(x, y, title, on, note):
    rect(x, y, 150, 56, "#fff7ed" if on else "#f3f4f6", HOT if on else "#d1d5db", 6, 2.5 if on else 1.5)
    text(x + 75, y + 22, title, 12.5, "middle", "bold", INK if on else MUTED)
    text(x + 75, y + 41, note if on else "off", 11, "middle", fill=INK if on else MUTED)


def disc(x, y, colour):
    out.append('<circle cx="%g" cy="%g" r="22" fill="%s" stroke="#374151" stroke-width="2"/>' % (x, y, colour))


def panel(ox, oy, yellow):
    title = "Yellow: pin 18 low" if yellow else "Black: pin 18 high"
    rect(ox, oy, 760, 650, "#fff", "#d1d5db", 10, 1.2)
    text(ox + 20, oy + 32, title, 18, weight="bold")
    text(ox + 20, oy + 54, ("Row address and pin 11 (ROW-LOW) high, then a pulse on pin 19" if yellow else
                            "Row address and pin 10 (ROW-HIGH) high, then a pulse on pin 19"), 12.5, fill="#4b5563")
    y0 = oy + 80
    FX, CX = ox + 330, ox + 520           # diode column, column line
    yf, yj, yd, yb = y0 + 70, y0 + 190, y0 + 310, y0 + 380
    # current first, so the wires sit on top
    if yellow:
        current([(ox + 105, y0 + 6), (ox + 105, yf), (FX, yf), (FX, yj), (CX, yj), (CX, yb), (ox + 680, yb),
                 (ox + 680, yb + 60), (ox + 680, yb + 100)])
    else:
        current([(ox + 500, yb + 120), (ox + 500, yb), (CX, yb), (CX, yj), (FX, yj), (FX, yd), (ox + 105, yd),
                 (ox + 105, yd + 70)])
    # row drivers
    tag(ox + 105, y0 + 6, "VCOIL", C_SUP)
    line(ox + 105, y0 + 16, ox + 105, yf - 28, INK, 2)
    drv(ox + 30, yf - 28, "Row source driver", yellow, "row → coil supply")
    line(ox + 180, yf, FX, yf, INK, 2.5)
    text(ox + 196, yf - 8, "feeding line of the row", 11, fill=GREY)
    drv(ox + 30, yd - 28, "Row sink driver", not yellow, "row → ground")
    line(ox + 180, yd, FX, yd, INK, 2.5)
    text(ox + 196, yd + 18, "draining line of the row", 11, fill=GREY)
    line(ox + 105, yd + 28, ox + 105, yd + 44, INK, 2)
    gnd(ox + 105, yd + 44)
    # the dot: two diodes, a coil, the column line
    diode(FX, yf, yj, False, INK)
    diode(FX, yj, yd, False, INK)
    dot(FX, yf, INK, 4); dot(FX, yj, INK, 4); dot(FX, yd, INK, 4)
    coil(FX, CX, yj, INK)
    dot(CX, yj, INK, 4)
    text(FX - 14, (yf + yj) / 2 + 4, "feeding diode", 11, "end", fill=GREY)
    text(FX - 14, (yj + yd) / 2 + 4, "draining diode", 11, "end", fill=GREY)
    text((FX + CX) / 2, yj + 30, "coil of the dot", 11, "middle", fill=GREY)
    disc(CX + 58, yj - 52, YEL if yellow else BLK)
    text(CX + 90, yj - 47, "flips to " + ("yellow" if yellow else "black"), 13, weight="bold")
    line(CX, y0 - 10, CX, yb, INK, 2.5)
    text(CX + 8, y0 + 2, "column line", 11, fill=GREY)
    # column drivers
    line(ox + 500, yb, ox + 680, yb, INK, 2.5)
    dot(CX, yb, INK, 4)
    line(ox + 500, yb, ox + 500, yb + 20, INK, 2)
    line(ox + 680, yb, ox + 680, yb + 20, INK, 2)
    drv(ox + 425, yb + 20, "Column source", not yellow, "column → supply")
    drv(ox + 605, yb + 20, "Column sink", yellow, "column → ground")
    line(ox + 500, yb + 76, ox + 500, yb + 100, INK, 2)
    tag(ox + 500, yb + 110, "VCOIL", C_SUP)
    line(ox + 680, yb + 76, ox + 680, yb + 90, INK, 2)
    gnd(ox + 680, yb + 90)
    # what switched which driver on
    text(ox + 20, oy + 612, "Pin 18 picks which pair:", 12, weight="bold")
    text(ox + 190, oy + 612, ("row source and column sink." if yellow else "column source and row sink."), 12)
    text(ox + 20, oy + 632, "Pins 10/11 enable the row side, the pulse on pin 19 the column side.", 12, fill=GREY)


text(40, 46, "One dot: row enable, direction and fire", 26, weight="bold")
text(40, 72, "A dot flips when current runs through its coil. The direction of the current decides the colour; "
     "it only flows while both drivers of the path are on.", 14, fill="#4b5563")
panel(40, 100, True)
panel(840, 100, False)

# ---------- timing: one sequence under each panel ----------
ty = 800
text(40, ty, "Sequence for one dot", 18, weight="bold")
names = [("Direction, pin 18", C_DIR), ("Pin 11, ROW-LOW", C_DRV), ("Pin 10, ROW-HIGH", C_DRV),
         ("Select, pin 19", C_SEL), ("Coil current", HOT)]
ROW0, STEP = ty + 76, 56


def sequence(ox, yellow):
    rect(ox, ty + 14, 760, ROW0 + 4 * STEP + 58 - ty - 14, "#fff", "#d1d5db", 10, 1.2)
    text(ox + 20, ty + 40, "Yellow" if yellow else "Black", 14, weight="bold")
    for i, (n, c) in enumerate(names):
        text(ox + 20, ROW0 + i * STEP + 4, n, 12.5, weight="bold", fill=c)
    x0, x1 = ox + 175, ox + 740
    xs = [x0 + 30, x0 + 130, x0 + 240, x0 + 400, x0 + 490]   # direction, enable, fire, end of pulse, row off
    for i, (_, c) in enumerate(names):
        y = ROW0 + i * STEP
        hi, lo = y - 14, y + 14
        if i == 0:
            lvl = lo if yellow else hi
            poly([(x0, y), (xs[0] - 8, y), (xs[0], lvl), (x1, lvl)], c, 2.5)
            text(xs[0] + 12, lvl + 16, "low" if yellow else "high", 11, fill=c)
        elif i in (1, 2):
            if (i == 1) == yellow:
                poly([(x0, lo), (xs[1], lo), (xs[1], hi), (xs[4], hi), (xs[4], lo), (x1, lo)], c, 2.5)
            else:
                line(x0, lo, x1, lo, c, 2.5)
                text((xs[1] + xs[4]) / 2, lo - 6, "stays low", 11, "middle", fill=c)
        elif i == 3:
            poly([(x0, lo), (xs[2], lo), (xs[2], hi), (xs[3], hi), (xs[3], lo), (x1, lo)], c, 2.5)
            text((xs[2] + xs[3]) / 2, hi - 6, "about 190 µs", 11, "middle", weight="bold", fill=c)
        else:
            peak = hi if yellow else lo + 4
            line(x0, y, x1, y, "#e5e7eb", 1)
            poly([(x0, y), (xs[2], y), (xs[2] + 16, peak), (xs[3] - 8, peak), (xs[3] + 16, y), (x1, y)], c, 2.5)
            text((xs[2] + xs[3]) / 2, peak - 6 if yellow else y - 6, "row → column" if yellow else "column → row",
                 11, "middle", weight="bold", fill=c)
    for x, lab in zip(xs, ("1. direction", "2. row on", "3. fire", "", "4. row off")):
        line(x, ty + 52, x, ROW0 + 4 * STEP + 20, "#d1d5db", 1, "4 4")
        text(x + 4, ROW0 + 4 * STEP + 36, lab, 11.5, fill="#374151")


sequence(40, True)
sequence(840, False)
text(40, ROW0 + 4 * STEP + 84, "Never ROW-HIGH and ROW-LOW together, and never a long high on pin 19: a coil that stays energised "
     "overheats.",
     11.5, fill=GREY)

W, H = 1640, ROW0 + 4 * STEP + 106
save(Path(__file__).with_name("dot-flow.svg"), W, H)
