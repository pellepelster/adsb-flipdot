#!/usr/bin/env python3
"""Draw docs/mobitec/panel-overview.svg: the groups of header pins, the row and
column decoders as blocks and the dot matrix they drive.

    python3 docs/render_panel_overview.py
    google-chrome --headless --screenshot=docs/mobitec/panel-overview.png \
        --window-size=1640,1010 --force-device-scale-factor=1.25 docs/mobitec/panel-overview.svg
"""
from pathlib import Path

from diagram import *  # noqa: F401,F403

W, H = 1640, 1010
ROWC, COLC, ENC, DIRC, SELC = C_ROW, C_COL, C_DRV, C_DIR, C_SEL
YEL = "#facc15"




text(40, 46, "Mobitec panel: from header pins to dots", 26, weight="bold")
text(40, 72, "One 28 × 16 panel. What each group of input pins does; the decoders and drivers on the panel are drawn as blocks.",
     14, fill="#4b5563")

# ---------- header ----------
HX, HY, HW = 40, 110, 300
groups = [
    ("Row address", "pins 5, 7, 6, 8, 9", "ROW0–ROW4", ROWC, 150),
    ("Row enable", "pins 10, 11", "ROW-HIGH, ROW-LOW", ENC, 270),
    ("Column address", "pins 13–17", "COL0–COL4", COLC, 550),
    ("Direction", "pin 18", "COL-H/L", DIRC, 660),
    ("Select (fire)", "pin 19", "COL-SEL0", SELC, 760),
]
rect(HX, HY, HW, 770, "#fefce8", "#854d0e")
text(HX + 14, HY + 26, "26-pin header", 16, weight="bold", fill="#854d0e")
for name, pins, sigs, color, y in groups:
    rect(HX + 12, y - 26, HW - 24, 62, "#fff", color, 6, 2)
    text(HX + 26, y - 4, name, 14, weight="bold", fill=color)
    text(HX + 26, y + 16, pins, 13)
    text(HX + HW - 26, y + 16, sigs, 11.5, "end", fill=GREY, mono=True)
y = 835
for s in ("pins 1, 2 GND · 3 +5 V · 26 coil supply",
          "pins 20–24: select of the next panels"):
    text(HX + 14, y, s, 11.5, fill=GREY)
    y += 18

# ---------- matrix ----------
MX, MY, P = 900, 150, 24
NC, NR = 28, 16
R_SEL, C_SEL_ = 3, 8          # example dot
rect(MX - 16, MY - 16, NC * P + 8, NR * P + 8, "#111827", "#111827", 6)
for r in range(NR):
    for c in range(NC):
        on = (r, c) == (R_SEL - 1, C_SEL_ - 1)
        hl = r == R_SEL - 1 or c == C_SEL_ - 1
        out.append('<circle cx="%g" cy="%g" r="%g" fill="%s" stroke="%s" stroke-width="%g"/>' % (
            MX + c * P, MY + r * P, 8.5, YEL if on else ("#374151" if hl else "#1f2937"),
            "#fff" if on else ("#6b7280" if hl else "#374151"), 3 if on else 1.2))
line(MX - 16, MY + (R_SEL - 1) * P, MX + NC * P - 8, MY + (R_SEL - 1) * P, ROWC, 2)
line(MX + (C_SEL_ - 1) * P, MY - 16, MX + (C_SEL_ - 1) * P, MY + NR * P - 8, COLC, 2)
for r in (1, 8, 16):
    text(MX + NC * P + 2, MY + (r - 1) * P + 4, "row %d" % r, 11, fill=GREY)
for c in (1, 8, 15, 22, 28):
    text(MX + (c - 1) * P, MY - 24, str(c), 11, "middle", fill=GREY)
text(MX + NC * P / 2 - 8, MY - 42, "columns", 12, "middle", fill=GREY)

# ---------- row side ----------
RDX = 640
rect(RDX, MY - 16, 200, NR * P + 8, "#eff6ff", ROWC, 8, 2)
text(RDX + 100, MY + 40, "Row decoder", 15, "middle", weight="bold", fill=ROWC)
text(RDX + 100, MY + 60, "+ row drivers", 13, "middle", fill=ROWC)
for i, s in enumerate(("Picks 1 of 16 rows:", "address 1–7 → rows 1–7", "9–15 → rows 8–14",
                       "17, 18 → rows 15, 16", "", "On only while pin 10", "or pin 11 is high")):
    text(RDX + 100, MY + 100 + i * 19, s, 12, "middle")
arrow(RDX + 200, MY + (R_SEL - 1) * P, MX - 18, MY + (R_SEL - 1) * P, ROWC, 3)
for r in range(NR):
    line(RDX + 200, MY + r * P, RDX + 214, MY + r * P, ROWC, 1.5)

# ---------- column side ----------
CDY = MY + NR * P + 60
rect(MX - 16, CDY, NC * P + 8, 150, "#f0fdf4", COLC, 8, 2)
text(MX + NC * P / 2 - 8, CDY + 28, "Column decoder + column drivers", 15, "middle", weight="bold", fill=COLC)
for i, s in enumerate(("Picks 1 of 28 columns: address 1–7 → columns 1–7, 9–15 → 8–14,",
                       "17–23 → 15–21, 25–31 → 22–28. Addresses 0, 8, 16, 24 pick nothing.",
                       "On only for the moment pin 19 rises: that pulse is what flips the dot.")):
    text(MX + NC * P / 2 - 8, CDY + 56 + i * 20, s, 12, "middle")
arrow(MX + (C_SEL_ - 1) * P, CDY, MX + (C_SEL_ - 1) * P, MY + NR * P - 6, COLC, 3)
for c in range(NC):
    line(MX + c * P, CDY, MX + c * P, CDY - 12, COLC, 1.5)

# ---------- wires from the header ----------
def route(y, color, pts, w=3):
    poly([(HX + HW - 12, y)] + pts, color, w)


route(groups[0][4], ROWC, [(560, groups[0][4]), (560, MY + 60), (RDX, MY + 60)])
arrow(560, MY + 60, RDX, MY + 60, ROWC)
route(groups[1][4], ENC, [(580, groups[1][4]), (580, MY + 300), (RDX - 2, MY + 300)])
arrow(580, MY + 300, RDX, MY + 300, ENC)
route(groups[2][4], COLC, [(MX - 120, groups[2][4]), (MX - 120, CDY + 40)])
arrow(MX - 120, CDY + 40, MX - 16, CDY + 40, COLC)
route(groups[4][4], SELC, [(MX - 80, groups[4][4]), (MX - 80, CDY + 110)])
arrow(MX - 80, CDY + 110, MX - 16, CDY + 110, SELC)
# direction goes to both
yd = groups[3][4]
route(yd, DIRC, [(520, yd)])
poly([(520, yd), (520, MY + 340), (RDX, MY + 340)], DIRC, 3)
arrow(520, MY + 340, RDX, MY + 340, DIRC)
poly([(520, yd), (MX - 100, yd), (MX - 100, CDY + 75)], DIRC, 3)
arrow(MX - 100, CDY + 75, MX - 16, CDY + 75, DIRC)
dot(520, yd, DIRC, 5)
text(540, yd + 26, "Direction goes to both decoders:", 12, weight="bold", fill=DIRC)
text(540, yd + 42, "it sets which way the current", 12, fill=DIRC)
text(540, yd + 58, "runs through the coil", 12, fill=DIRC)

# ---------- example and colours ----------
ex, ey = 380, 830
rect(ex - 10, ey - 24, 270, 116, "#fff", "#d1d5db", 6, 1.2)
text(ex, ey, "Example (the yellow dot)", 13, weight="bold")
for i, s in enumerate(("row address 3 → row 3", "column address 9 → column 8",
                       "ROW-LOW, pin 18 low → yellow", "ROW-HIGH, pin 18 high → black", "then one pulse on pin 19")):
    text(ex, ey + 20 + i * 17, s, 12)
text(MX - 16, CDY + 186, "Colours measured on our panel: pin 18 low with ROW-LOW shows yellow, pin 18 high with ROW-HIGH black.",
     11.5, fill=GREY)

bx, by = MX - 16, CDY + 214
rect(bx, by, NC * P + 8, 150, "#faf5ff", ENC, 8, 1.5)
text(bx + 16, by + 26, "ROW-HIGH and ROW-LOW (pins 10, 11)", 14, weight="bold", fill=ENC)
for i, s_ in enumerate((
        "On our panel both pins go into one NOR gate that switches the row decoder on. Either one high is",
        "enough, and both do the same: the direction of the current comes from pin 18 alone.",
        "The names come from older Mobitec panels with FP2800 drivers. There ROW-HIGH connects the row",
        "to the coil supply and ROW-LOW connects it to ground, and both high together shorts the supply.",
        "Drive them the way that suits both: ROW-LOW with pin 18 low, ROW-HIGH with pin 18 high, never both.")):
    text(bx + 16, by + 50 + i * 19, s_, 12, fill="#374151")

save(Path(__file__).parent / "mobitec" / "panel-overview.svg", W, H)
