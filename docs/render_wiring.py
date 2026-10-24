#!/usr/bin/env python3
"""Draw docs/wiring.svg: how the Raspberry Pi, the HAT, the two power supplies and
the panels are wired.

    python3 docs/render_wiring.py
    google-chrome --headless --screenshot=docs/wiring.png \
        --window-size=1640,720 --force-device-scale-factor=1.25 docs/wiring.svg
"""
from pathlib import Path

from diagram import *  # noqa: F401,F403


PCB, PCB_EDGE = "#dcfce7", "#16a34a"
HAT, HAT_EDGE = "#e0e7ff", "#4f46e5"
USB = "#475569"
RIBBON, RIBBON_EDGE = "#d1d5db", "#6b7280"
WALL_X = 60


def supply(y, title, detail):
    """A plug-in power supply in the wall, with its cable leaving on the right.
    Returns where the cable starts."""
    rect(WALL_X + 14, y - 34, 120, 68, "#f3f4f6", INK, 8, 2)
    for dy in (-14, 14):
        rect(WALL_X - 4, y + dy - 4, 18, 8, MUTED, INK, 1, 1)
    text(WALL_X + 74, y - 4, title, 15, "middle", "bold")
    text(WALL_X + 74, y + 16, detail, 11.5, "middle", fill=GREY)
    return WALL_X + 134, y


def header(x, y, w, h, label=None, below=True):
    """A shrouded pin header seen from above."""
    rect(x, y, w, h, "#374151", INK, 2, 1)
    for i in range(int(w // 9)):
        for row in (0, 1):
            dot(x + 6 + i * 9, y + h / 2 - 3 + row * 6, "#facc15", 1.6)
    if label:
        text(x + w / 2, y + h + 18 if below else y - 8, label, 11.5, "middle", fill=GREY)


def ribbon(pts, label, lx, ly):
    """A flat ribbon cable along `pts`."""
    d = "M " + " L ".join("%g %g" % p for p in pts)
    out.append('<path d="%s" fill="none" stroke="%s" stroke-width="18" stroke-linejoin="round"/>'
               % (d, RIBBON_EDGE))
    out.append('<path d="%s" fill="none" stroke="%s" stroke-width="15" stroke-linejoin="round"/>'
               % (d, RIBBON))
    text(lx, ly, label, 13, "middle", "bold", fill=INK)
    text(lx, ly + 17, "26 wires: logic, 5 V and the 24 V for the coils", 11.5, "middle", fill=GREY)


def panel(x, y, n):
    """A Mobitec panel: 28 x 16 dots on a board with a 26-pin header near each end
    on the back. Returns the centres of the two headers."""
    w, h, strip = 360, 222, 46
    rect(x, y, w, h + strip, "#111827", INK, 4, 1.5)
    p = (w - 16) / 28
    for r in range(16):
        for c in range(28):
            out.append('<circle cx="%g" cy="%g" r="%g" fill="#374151"/>'
                       % (x + 8 + p / 2 + c * p, y + 8 + p / 2 + r * p, p * 0.42))
    rect(x - 6, y + h, w + 12, strip, PCB, PCB_EDGE, 3, 1.5)
    centres = []
    for hx in (x + 22, x + w - 22 - 120):
        rect(hx, y + h + 10, 120, 26, "none", INK, 2, 1.2, "4 3")
        centres.append((hx + 60, y + h + 23))
    text(x + w / 2, y - 14, "Matrix display %d" % n, 17, "middle", "bold")
    text(x + w / 2, y + h + strip + 20, "26-pin headers on the back", 11.5, "middle", fill=GREY)
    return centres


text(40, 46, "Wiring", 26, weight="bold")
text(40, 72, "The HAT sits on the GPIO header of the Pi. Each panel passes every signal from one of its "
             "headers to the other, so the second panel hangs off the first.", 14, fill="#4b5563")

# ---------- wall ----------
line(WALL_X, 130, WALL_X, 560, INK, 3)
for y in range(136, 560, 18):
    line(WALL_X - 14, y + 10, WALL_X, y, MUTED, 1.2)
text(WALL_X, 584, "wall sockets", 12, "middle", fill=GREY)

# ---------- Raspberry Pi and HAT ----------
PX, PY, PW, PH = 400, 150, 380, 400
rect(PX, PY, PW, PH, PCB, PCB_EDGE, 14, 2)
text(PX + PW - 20, PY + PH - 34, "Raspberry Pi", 18, "end", "bold", PCB_EDGE)
text(PX + PW - 20, PY + PH - 14, "under the HAT", 11.5, "end", fill=GREY)
port_y = PY + PH - 40
rect(PX - 10, port_y - 9, 26, 18, "#9ca3af", INK, 2, 1.2)
text(PX + 24, port_y + 4, "USB power", 11, fill=GREY)

HX, HY, HW, HH = PX + 20, PY + 20, PW - 40, 290
rect(HX, HY, HW, HH, HAT, HAT_EDGE, 12, 2)
header(HX + 40, HY + 12, HW - 80, 18)
text(HX + HW / 2, HY + 50, "40-pin GPIO header, onto the Pi", 11.5, "middle", fill=GREY)
text(HX + HW / 2, HY + 130, "flipdot HAT", 20, "middle", "bold", HAT_EDGE)
text(HX + HW / 2, HY + 152, "level shifters 3.3 V → 5 V", 12, "middle", fill=GREY)
jack_y = HY + 86
rect(HX - 12, jack_y - 16, 44, 32, "#1f2937", INK, 4, 1.2)
dot(HX + 10, jack_y, "#9ca3af", 5)
text(HX + 40, jack_y + 4, "12–24 V jack", 11, fill=GREY)
J2X, J2W = HX + 60, HW - 120
header(J2X, HY + HH - 46, J2W, 22, "26-pin header to the panel", below=False)

# ---------- power ----------
x0, y0 = supply(jack_y, "24 V supply", "coils of the dots")
line(x0, y0, HX - 12, jack_y, C_SUP, 5)
text((x0 + HX) / 2 - 6, jack_y - 12, "24 V", 13, "middle", "bold", C_SUP)
text((x0 + HX) / 2 - 6, jack_y + 22, "barrel plug", 11, "middle", fill=GREY)
text((x0 + HX) / 2 - 6, jack_y + 36, "centre +", 11, "middle", fill=GREY)

x0, y0 = supply(port_y, "USB supply", "5 V for the Pi")
line(x0, y0, PX - 10, port_y, USB, 5)
text((x0 + PX) / 2 - 6, port_y - 12, "USB cable", 13, "middle", "bold", USB)

# ---------- panels ----------
D1X, D2X, DY = 850, 1250, 150
h1 = panel(D1X, DY, 1)
h2 = panel(D2X, DY, 2)
RY = 640
jx = J2X + J2W / 2
ribbon([(jx, HY + HH - 24), (jx, RY), (h1[0][0], RY), h1[0]],
       "ribbon cable", (jx + h1[0][0]) / 2, RY + 34)
ribbon([h1[1], (h1[1][0], RY), (h2[0][0], RY), h2[0]],
       "ribbon cable", (h1[1][0] + h2[0][0]) / 2, RY + 34)
for (x, y) in (h1[0], h1[1], h2[0]):
    header(x - 52, y - 10, 104, 20)

W, H = 1640, 720
save(Path(__file__).with_name("wiring.svg"), W, H)
