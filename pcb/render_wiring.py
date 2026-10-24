#!/usr/bin/env python3
"""Draw pcb/wiring.svg: every connection of the HAT from Pi pin through the
74HCT541 buffers to the panel pin, plus the power and coil supply.

The signals come from the tables in build_board.py, read without importing it,
since that needs KiCad's pcbnew.

    python3 pcb/render_wiring.py
    google-chrome --headless --screenshot=pcb/wiring.png \
        --window-size=1600,1356 --force-device-scale-factor=1.5 pcb/wiring.svg
"""
import ast
from pathlib import Path

src = Path(__file__).with_name("build_board.py").read_text()
T = {}
for node in ast.parse(src).body:
    if isinstance(node, ast.Assign) and getattr(node.targets[0], "id", None) in ("SIGNALS", "PI_GND", "PI_5V"):
        T[node.targets[0].id] = ast.literal_eval(node.value)
SIGNALS, PI_GND, PI_5V = T["SIGNALS"], T["PI_GND"], T["PI_5V"]

BCM = {3: 2, 5: 3, 7: 4, 8: 14, 10: 15, 11: 17, 12: 18, 13: 27, 15: 22, 16: 23, 18: 24, 19: 10,
       21: 9, 22: 25, 23: 11, 24: 8, 26: 7, 27: 0, 28: 1, 29: 5, 31: 6, 32: 12, 33: 13, 35: 19,
       36: 16, 37: 26, 38: 20, 40: 21}


def group(sig):
    if sig.startswith("ROW-"):
        return "#9333ea", "row drive"
    if sig.startswith("ROW"):
        return "#2563eb", "row address"
    if sig == "COL-H/L":
        return "#0d9488", "column direction"
    if sig.startswith("COL-SEL"):
        return "#ea580c", "panel select"
    return "#16a34a", "column address"


P = 26                      # pin pitch
X_PI, X_U, X_J = 60, 640, 1300
UW = 170                    # chip body width
STUB = 34
TOP = 150
out = []


def text(x, y, s, size=13, anchor="start", weight="normal", fill="#1f2937", family="DejaVu Sans"):
    s = s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    out.append('<text x="%g" y="%g" font-size="%g" text-anchor="%s" font-weight="%s" fill="%s" '
               'font-family="%s">%s</text>' % (x, y, size, anchor, weight, fill, family, s))


def line(x1, y1, x2, y2, color="#1f2937", w=2, dash=None):
    out.append('<line x1="%g" y1="%g" x2="%g" y2="%g" stroke="%s" stroke-width="%g"%s/>'
               % (x1, y1, x2, y2, color, w, ' stroke-dasharray="%s"' % dash if dash else ""))


def rect(x, y, w, h, fill="#fff", stroke="#1f2937", rx=4, sw=1.5):
    out.append('<rect x="%g" y="%g" width="%g" height="%g" rx="%g" fill="%s" stroke="%s" stroke-width="%g"/>'
               % (x, y, w, h, rx, fill, stroke, sw))


def dot(x, y, color="#1f2937", r=3.5):
    out.append('<circle cx="%g" cy="%g" r="%g" fill="%s"/>' % (x, y, r, color))


def gnd(x, y, right=True):
    """A ground tag at the end of a pin stub."""
    d = 1 if right else -1
    line(x, y, x + d * 14, y, "#111827", 2)
    for i, h in enumerate((8, 5, 2)):
        line(x + d * (14 + i * 4), y - h, x + d * (14 + i * 4), y + h, "#111827", 2)


def tag(x, y, s, color, right=True):
    w = 9 + 7.6 * len(s)
    x0 = x if right else x - w
    rect(x0, y - 9, w, 18, color, color, 9)
    text(x0 + w / 2, y + 4.5, s, 12, "middle", "bold", "#fff")


def nc(x, y):
    line(x - 5, y - 5, x + 5, y + 5, "#9ca3af", 2)
    line(x - 5, y + 5, x + 5, y - 5, "#9ca3af", 2)


# --- title ---
text(X_PI, 48, "Flip-dot HAT wiring", 28, weight="bold")
text(X_PI, 76, "Raspberry Pi  →  74HCT541 level shifters (3.3 V in, 5 V out, not inverting)  →  "
     "Mobitec 26-pin panel header", 15, fill="#4b5563")
text(X_PI, 98, "Pin numbers are the physical pins of each connector and chip. "
     "Every chip gets a 100 nF capacitor from pin 20 to pin 10, close to the chip.", 13, fill="#6b7280")

# --- legend ---
lx = X_PI
for color, name in [("#2563eb", "row address"), ("#9333ea", "row drive"), ("#16a34a", "column address"),
                    ("#0d9488", "column direction"), ("#ea580c", "panel select")]:
    line(lx, 122, lx + 28, 122, color, 4)
    text(lx + 36, 127, name, 13)
    lx += 36 + 9 * len(name) + 30

# --- chips with their signals ---
chips = {}
y = TOP + 20
for ref in ("U1", "U2", "U3"):
    chips[ref] = y
    y += 10 * P + 70
bottom = y - 70

rect(X_PI, TOP, 250, bottom - TOP + 10, "#f0fdf4", "#166534")
text(X_PI + 12, TOP + 22, "Raspberry Pi 40-pin header", 14, weight="bold", fill="#166534")
rect(X_J, TOP, 250, bottom - TOP + 10, "#fefce8", "#854d0e")
text(X_J + 238, TOP + 22, "Panel 26-pin header", 14, "end", "bold", "#854d0e")

for ref, y0 in chips.items():
    ys = [y0 + 25 + i * P for i in range(10)]
    xl, xr = X_U, X_U + UW
    rect(xl, y0, UW, 10 * P + 24, "#f3f4f6", "#111827", 6, 2)
    out.append('<path d="M %g %g a 10 10 0 0 0 20 0" fill="none" stroke="#111827" stroke-width="2"/>'
               % (xl + UW / 2 - 10, y0))
    text(xl + UW / 2, y0 + 10 * P / 2 + 8, ref, 22, "middle", "bold")
    text(xl + UW / 2, y0 + 10 * P / 2 + 30, "74HCT541", 14, "middle", fill="#4b5563")
    left = ["/OE1"] + ["A%d" % i for i in range(1, 9)] + ["GND"]
    right = ["VCC", "/OE2"] + ["Y%d" % i for i in range(1, 9)]
    for i in range(10):
        line(xl - STUB, ys[i], xl, ys[i], "#111827", 2)
        line(xr, ys[i], xr + STUB, ys[i], "#111827", 2)
        text(xl - 4, ys[i] - 4, str(i + 1), 11, "end", fill="#6b7280")
        text(xr + 4, ys[i] - 4, str(20 - i), 11, fill="#6b7280")
        text(xl + 7, ys[i] + 4.5, left[i], 12, family="DejaVu Sans Mono")
        text(xr - 7, ys[i] + 4.5, right[i], 12, "end", family="DejaVu Sans Mono")
    gnd(xl - STUB, ys[0], False)
    gnd(xl - STUB, ys[9], False)
    tag(xr + STUB, ys[0], "+5V", "#dc2626")
    gnd(xr + STUB, ys[1], True)
    chans = {ch: (sig, pi, panel) for sig, pi, panel, r, ch in SIGNALS if r == ref}
    for ch in range(1, 9):
        ya, yy = ys[ch], ys[ch + 1]
        if ch not in chans:
            gnd(xl - STUB, ya, False)
            nc(xr + STUB, yy)
            continue
        sig, pi, panel = chans[ch]
        color = group(sig)[0]
        # Pi pin -> A input
        dot(X_PI + 250, ya, color, 5)
        text(X_PI + 14, ya + 4.5, "pin %d" % pi, 13, weight="bold")
        text(X_PI + 236, ya + 4.5, "GPIO%d" % BCM[pi], 12, "end", fill="#4b5563", family="DejaVu Sans Mono")
        line(X_PI + 250, ya, xl - STUB, ya, color, 3)
        text((X_PI + 250 + xl - STUB) / 2, ya - 5, sig, 12, "middle", "bold", color)
        # Y output -> panel pin
        dot(X_J, yy, color, 5)
        text(X_J + 236, yy + 4.5, "pin %d" % panel, 13, "end", "bold")
        line(xr + STUB, yy, X_J, yy, color, 3)
        text((xr + STUB + X_J) / 2, yy - 5, sig, 12, "middle", "bold", color)

# --- power ---
py = bottom + 60
text(X_PI, py, "Power", 20, weight="bold")
py += 36


def power_row(y, what, net, color):
    text(X_PI + 14, y + 5, what, 14)
    line(X_PI + 380, y, X_PI + 440, y, color, 3)
    tag(X_PI + 440, y, net, color)


power_row(py, "Pi pins " + ", ".join(map(str, PI_5V)), "+5V", "#dc2626")
power_row(py + 32, "Pi pins " + ", ".join(map(str, PI_GND)), "GND", "#111827")
power_row(py + 64, "Panel pin 3  (5 V for the panel's logic)", "+5V", "#dc2626")
power_row(py + 96, "Panel pins 1, 2", "GND", "#111827")
text(X_PI + 14, py + 133, "Panel pins 4, 12, 24, 25: not connected", 14, fill="#6b7280")

# coil supply: jack -> D1 -> +12V -> panel 26, C4 to GND
cx, cy = X_U + 40, py + 10
text(cx, cy - 16, "Coil supply 12–24 V, centre positive", 14, weight="bold")
rect(cx, cy, 120, 70, "#f3f4f6", "#111827")
text(cx + 60, cy + 30, "J3", 15, "middle", "bold")
text(cx + 60, cy + 50, "5.5 × 2.1 jack", 12, "middle", fill="#4b5563")
tip, sleeve = cy + 20, cy + 52
text(cx + 126, tip - 5, "centre +", 11, fill="#4b5563")
line(cx + 120, tip, cx + 230, tip, "#b45309", 3)
# diode, anode left
dx = cx + 230
out.append('<polygon points="%g,%g %g,%g %g,%g" fill="#111827"/>' % (dx, tip - 11, dx, tip + 11, dx + 20, tip))
line(dx + 20, tip - 11, dx + 20, tip + 11, "#111827", 3)
text(dx + 10, tip - 18, "D1 1N5819", 12, "middle", "bold")
line(dx + 20, tip, dx + 200, tip, "#b45309", 3)
dot(dx + 110, tip, "#b45309", 4.5)
# C4 to ground
line(dx + 110, tip, dx + 110, tip + 26, "#b45309", 3)
line(dx + 96, tip + 26, dx + 124, tip + 26, "#111827", 3)
out.append('<path d="M %g %g q 14 -8 28 0" fill="none" stroke="#111827" stroke-width="3"/>'
           % (dx + 96, tip + 38))
text(dx + 92, tip + 30, "+", 13, "end", "bold")
text(dx + 132, tip + 37, "C4 470 µF 35 V", 12, "start", "bold")
line(dx + 110, tip + 34, dx + 110, sleeve + 16, "#111827", 3)
line(cx + 120, sleeve, cx + 150, sleeve, "#111827", 3)
line(cx + 150, sleeve, cx + 150, sleeve + 16, "#111827", 3)
line(cx + 150, sleeve + 16, dx + 110, sleeve + 16, "#111827", 3)
tag(dx + 200, tip, "+12V → panel pin 26", "#b45309")
gnd(dx + 110, sleeve + 16)
text(cx, sleeve + 50, "D1 blocks a reversed plug; C4 buffers the current of the coil pulses.", 12, fill="#6b7280")

W, H = X_J + 300, py + 170
svg = ('<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d" viewBox="0 0 %d %d">'
       '<rect width="100%%" height="100%%" fill="#ffffff"/>%s</svg>' % (W, H, W, H, "".join(out)))
Path(__file__).with_name("wiring.svg").write_text(svg + "\n")
