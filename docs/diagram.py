"""Shared drawing helpers for the diagrams in docs/.

Each render_*.py script draws into `out` and calls save(); the PNGs are
screenshots of those SVGs, see the docstring of each script.
"""
import math

out = []
INK, GREY, MUTED = "#1f2937", "#6b7280", "#9ca3af"
C_ROW, C_DRV, C_COL, C_DIR, C_SEL = "#2563eb", "#9333ea", "#16a34a", "#0d9488", "#ea580c"
C_SUP, C_GND = "#b45309", "#111827"
DASH = "7 5"


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def text(x, y, s, size=13, anchor="start", weight="normal", fill=INK, mono=False, italic=False):
    out.append('<text x="%g" y="%g" font-size="%g" text-anchor="%s" font-weight="%s" fill="%s" '
               'font-family="%s"%s>%s</text>' % (x, y, size, anchor, weight, fill,
                                                  "DejaVu Sans Mono" if mono else "DejaVu Sans",
                                                  ' font-style="italic"' if italic else "", esc(s)))


def line(x1, y1, x2, y2, color=INK, w=2, dash=None):
    out.append('<line x1="%g" y1="%g" x2="%g" y2="%g" stroke="%s" stroke-width="%g" stroke-linecap="round"%s/>'
               % (x1, y1, x2, y2, color, w, ' stroke-dasharray="%s"' % dash if dash else ""))


def path(d, color=INK, w=2, dash=None, fill="none"):
    out.append('<path d="%s" fill="%s" stroke="%s" stroke-width="%g" stroke-linejoin="round"%s/>'
               % (d, fill, color, w, ' stroke-dasharray="%s"' % dash if dash else ""))


def poly(pts, color=INK, w=2, dash=None):
    path("M " + " L ".join("%g %g" % p for p in pts), color, w, dash)


def rect(x, y, w, h, fill="#fff", stroke=INK, rx=4, sw=1.5, dash=None):
    out.append('<rect x="%g" y="%g" width="%g" height="%g" rx="%g" fill="%s" stroke="%s" stroke-width="%g"%s/>'
               % (x, y, w, h, rx, fill, stroke, sw, ' stroke-dasharray="%s"' % dash if dash else ""))


def dot(x, y, color=INK, r=3.5):
    out.append('<circle cx="%g" cy="%g" r="%g" fill="%s"/>' % (x, y, r, color))




def gnd(x, y):
    line(x, y, x, y + 10, C_GND, 2)
    for i, h in enumerate((10, 6, 2)):
        line(x - h, y + 10 + i * 4, x + h, y + 10 + i * 4, C_GND, 2)


def diode(x, y1, y2, up, color=INK, dash=None):
    """Vertical diode between y1 and y2 (y1 < y2), conducting upwards if up."""
    m = (y1 + y2) / 2
    line(x, y1, x, m - 6, color, 1.6, dash)
    line(x, m + 6, x, y2, color, 1.6, dash)
    if up:
        out.append('<polygon points="%g,%g %g,%g %g,%g" fill="%s"/>' % (x - 6, m + 6, x + 6, m + 6, x, m - 6, color))
        line(x - 6, m - 6, x + 6, m - 6, color, 2)
    else:
        out.append('<polygon points="%g,%g %g,%g %g,%g" fill="%s"/>' % (x - 6, m - 6, x + 6, m - 6, x, m + 6, color))
        line(x - 6, m + 6, x + 6, m + 6, color, 2)


def coil(x1, x2, y, color=INK, dash=None):
    n = 3
    w = (x2 - x1 - 8) / n
    d = "M %g %g L %g %g" % (x1, y, x1 + 4, y)
    for i in range(n):
        d += " a %g %g 0 0 1 %g 0" % (w / 2, w / 2, w)
    d += " L %g %g" % (x2, y)
    path(d, color, 1.8, dash)


def arrow(x1, y1, x2, y2, color, w=3):
    line(x1, y1, x2, y2, color, w)
    a = math.atan2(y2 - y1, x2 - x1)
    p = [(x2, y2), (x2 - 12 * math.cos(a - 0.4), y2 - 12 * math.sin(a - 0.4)),
         (x2 - 12 * math.cos(a + 0.4), y2 - 12 * math.sin(a + 0.4))]
    out.append('<polygon points="%s" fill="%s"/>' % (" ".join("%g,%g" % q for q in p), color))


def save(path, width, height):
    path.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d" viewBox="0 0 %d %d">'
                    '<rect width="100%%" height="100%%" fill="#ffffff"/>%s</svg>\n'
                    % (width, height, width, height, "".join(out)))
