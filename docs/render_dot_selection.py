#!/usr/bin/env python3
"""Draw docs/dot-selection.svg: how the five bits of the row and the column address
split into a decoder group and a line, and which rows and columns each group covers.

    python3 docs/render_dot_selection.py
    google-chrome --headless --screenshot=docs/dot-selection.png \
        --window-size=1640,800 --force-device-scale-factor=1.25 docs/dot-selection.svg
"""
from pathlib import Path

from diagram import *  # noqa: F401,F403


TINT_R = ["#dbeafe", "#eff6ff"]
TINT_C = ["#dcfce7", "#f0fdf4"]




def bits(x, y, pins, color, tint, title):
    """Five bit boxes, bit 4 on the left, with the header pin under each."""
    text(x, y - 46, title, 16, weight="bold", fill=color)
    bw = 70
    for k in range(5):
        b = 4 - k
        bx = x + k * (bw + 8) + (16 if k >= 2 else 0)
        rect(bx, y - 30, bw, 70, tint[0] if k < 2 else "#fff", color, 6, 2)
        text(bx + bw / 2, y + 12, "bit %d" % b, 15, "middle", "bold", color)
        text(bx + bw / 2, y + 34, "pin %d" % pins[b], 11.5, "middle", weight="bold")
    x2 = x + 2 * (bw + 8) - 8
    path("M %g %g v 8 h %g v -8" % (x, y + 48, x2 - x), color, 1.5)
    text((x + x2) / 2, y + 74, "group", 12.5, "middle", "bold", color)
    text((x + x2) / 2, y + 90, "which decoder chip", 11.5, "middle", fill=GREY)
    x3 = x + 2 * (bw + 8) + 16
    x4 = x3 + 3 * (bw + 8) - 8
    path("M %g %g v 8 h %g v -8" % (x3, y + 48, x4 - x3), color, 1.5)
    text((x3 + x4) / 2, y + 74, "line in the group: 1–7", 12.5, "middle", "bold", color)
    text((x3 + x4) / 2, y + 90, "0 selects nothing", 11.5, "middle", fill=GREY)
    return x4


text(40, 46, "Dot selection: row and column address", 26, weight="bold")
text(40, 72, "Each address has five bits. The top two pick one of the decoder chips, the low three one of its seven lines.",
     14, fill="#4b5563")

# ---------- matrix ----------
MX, MY, P = 860, 150, 25
NC, NR = 28, 16
for g in range(3):
    r0, r1 = g * 7, min(g * 7 + 7, NR)
    rect(MX - 14, MY - 12 + r0 * P, NC * P + 4, (r1 - r0) * P, TINT_R[g % 2], TINT_R[g % 2], 0, 0)
for g in range(4):
    c0 = g * 7
    rect(MX - 12 + c0 * P, MY + NR * P - 6, 7 * P, 10, TINT_C[g % 2], "#16a34a", 0, 0.8)
    text(MX - 12 + c0 * P + 7 * P / 2, MY + NR * P + 22, "group %d" % g, 12, "middle", "bold", "#16a34a")
    text(MX - 12 + c0 * P + 7 * P / 2, MY + NR * P + 38, "columns %d–%d" % (c0 + 1, c0 + 7), 11, "middle", fill=GREY)
    text(MX - 12 + c0 * P + 7 * P / 2, MY + NR * P + 53, "address %d–%d" % (8 * g + 1, 8 * g + 7), 11, "middle", fill=GREY)
for g, (a, b) in enumerate(((1, 7), (8, 14), (15, 16))):
    yc = MY - 12 + ((a - 1) + (b - a + 1) / 2) * P
    text(MX - 26, yc - 8, "group %d" % g, 12, "end", "bold", C_ROW)
    text(MX - 26, yc + 8, "rows %d–%d" % (a, b) if a != b else "row %d" % a, 11, "end", fill=GREY)
    hi = 8 * g + (b - a + 1)
    text(MX - 26, yc + 23, "address %d–%d" % (8 * g + 1, hi), 11, "end", fill=GREY)
for r in range(NR):
    for c in range(NC):
        out.append('<circle cx="%g" cy="%g" r="8" fill="#1f2937" stroke="#111827" stroke-width="1"/>'
                   % (MX + c * P, MY + r * P))
for c in (1, 7, 8, 14, 15, 21, 22, 28):
    text(MX + (c - 1) * P, MY - 20, str(c), 10.5, "middle", fill=GREY)
for r in (1, 7, 8, 14, 15, 16):
    text(MX + NC * P - 2, MY + (r - 1) * P + 4, str(r), 10.5, fill=GREY)

# ---------- row address ----------
RX, RY = 60, 230
end = bits(RX, RY, {0: 5, 1: 7, 2: 6, 3: 8, 4: 9}, C_ROW, TINT_R, "Row address  (pins 9, 8, 6, 7, 5)")
text(RX, RY + 128, "Note the order on the header: bit 1 is pin 7, bit 2 is pin 6.", 12, fill=GREY)
arrow(end + 14, RY + 5, MX - 150, RY + 5, C_ROW, 2.5)

# ---------- column address ----------
CX, CY = 60, 660
end = bits(CX, CY, {0: 13, 1: 14, 2: 15, 3: 16, 4: 17}, C_COL, TINT_C, "Column address  (pins 17, 16, 15, 14, 13)")
xc = MX + NC * P / 2 - 12
poly([(end + 14, CY + 5), (xc, CY + 5)], C_COL, 2.5)
arrow(xc, CY + 5, xc, MY + NR * P + 64, C_COL, 2.5)

W, H = 1640, 800
save(Path(__file__).with_name("dot-selection.svg"), W, H)
