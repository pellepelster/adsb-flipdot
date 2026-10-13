#!/usr/bin/env python3
"""Draw docs/filter-area.svg: the point of interest as a house, the filter box
around it and the runways of Hamburg Airport in the distance.

The numbers mirror the defaults of FilterParams in src/cli/filter.rs. Positions,
box and runway lengths are to scale; the house, the aircraft and the runway
width are drawn larger so they can be seen.

    python3 docs/render_filter_area.py
    google-chrome --headless --screenshot=docs/filter-area.png \
        --window-size=1600,1000 --force-device-scale-factor=2 docs/filter-area.svg
"""

import math
from pathlib import Path

LAT, LON = 53.7271256, 9.9065838
AREA_WIDTH_KM = 4.0
AREA_LENGTH_KM = 6.0
AREA_OFFSET_PERCENT = 80.0
VIEW_DIRECTION = 153.0
MAX_ALTITUDE_FT = 5000

EARTH_RADIUS_KM = 6371.0088
KM_PER_FOOT = 0.0003048

# Runway thresholds of EDDH, from ourairports.com.
RUNWAYS = [
    ("05", (53.618301, 9.963710), "23", (53.637100, 10.001800)),
    ("15", (53.654400, 9.975210), "33", (53.625099, 10.000500)),
]

WIDTH, HEIGHT = 1600, 1000
FONT = "DejaVu Sans, Verdana, sans-serif"


def add(a, b):
    return (a[0] + b[0], a[1] + b[1], a[2] + b[2])


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def mul(a, k):
    return (a[0] * k, a[1] * k, a[2] * k)


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def cross(a, b):
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def unit(a):
    return mul(a, 1 / math.sqrt(dot(a, a)))


def enu(lat, lon):
    """Kilometres east and north of the point of interest."""
    east = math.radians(lon - LON) * math.cos(math.radians(LAT)) * EARTH_RADIUS_KM
    north = math.radians(lat - LAT) * EARTH_RADIUS_KM
    return (east, north, 0.0)


UP = (0.0, 0.0, 1.0)
HEADING = math.radians(VIEW_DIRECTION)
AHEAD = (math.sin(HEADING), math.cos(HEADING), 0.0)
RIGHT = (math.cos(HEADING), -math.sin(HEADING), 0.0)


def local(right, ahead, up=0.0):
    """A point given relative to the house, along the view direction."""
    return add(add(mul(RIGHT, right), mul(AHEAD, ahead)), mul(UP, up))


CAMERA = local(-3.0, -10.0, 8.0)
CAM_AHEAD = unit(sub(local(0.4, 4.0), CAMERA))
CAM_RIGHT = unit(cross(CAM_AHEAD, UP))
CAM_UP = cross(CAM_RIGHT, CAM_AHEAD)
FOCAL = 1550.0
HORIZON_Y = HEIGHT / 2 + FOCAL * CAM_AHEAD[2] / math.hypot(CAM_AHEAD[0], CAM_AHEAD[1])


def depth(p):
    return dot(sub(p, CAMERA), CAM_AHEAD)


def project(p):
    d = sub(p, CAMERA)
    z = dot(d, CAM_AHEAD)
    return (
        WIDTH / 2 + FOCAL * dot(d, CAM_RIGHT) / z,
        HEIGHT / 2 - FOCAL * dot(d, CAM_UP) / z,
    )


def points(ps):
    return " ".join(f"{x:.1f},{y:.1f}" for x, y in map(project, ps))


out = []


def polygon(ps, **attrs):
    out.append(f'<polygon points="{points(ps)}"{fmt(attrs)}/>')


def line(a, b, **attrs):
    (x1, y1), (x2, y2) = project(a), project(b)
    out.append(f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}"{fmt(attrs)}/>')


def text(x, y, s, size=18, anchor="start", weight="normal", fill="#1f2a36", **attrs):
    out.append(
        f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" text-anchor="{anchor}" '
        f'font-weight="{weight}" fill="{fill}" stroke="#ffffff" stroke-width="{size / 4:.1f}" '
        f'stroke-opacity="0.85" stroke-linejoin="round" paint-order="stroke"{fmt(attrs)}>{s}</text>'
    )


def fmt(attrs):
    return "".join(f' {k.replace("_", "-")}="{v}"' for k, v in attrs.items())


def centroid(ps):
    return mul((sum(p[0] for p in ps), sum(p[1] for p in ps), sum(p[2] for p in ps)), 1 / len(ps))


# --- sky and ground ---------------------------------------------------------

out.append(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" '
    f'width="{WIDTH}" height="{HEIGHT}" font-family="{FONT}">'
    "<defs>"
    '<linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">'
    '<stop offset="0" stop-color="#9cc7e8"/><stop offset="1" stop-color="#e9f3fa"/></linearGradient>'
    '<linearGradient id="ground" x1="0" y1="0" x2="0" y2="1">'
    '<stop offset="0" stop-color="#b9d3a8"/><stop offset="1" stop-color="#9fc48b"/></linearGradient>'
    '<linearGradient id="haze" x1="0" y1="0" x2="0" y2="1">'
    '<stop offset="0" stop-color="#e9f3fa" stop-opacity="1"/>'
    '<stop offset="1" stop-color="#e9f3fa" stop-opacity="0"/></linearGradient>'
    "</defs>"
)
out.append(f'<rect width="{WIDTH}" height="{HEIGHT}" fill="url(#ground)"/>')

grid = []
for k in range(-60, 61, 2):
    for along_east in (True, False):
        previous = None
        for t in range(-60, 61, 2):
            p = (float(t), float(k), 0.0) if along_east else (float(k), float(t), 0.0)
            if depth(p) < 2.0:
                previous = None
                continue
            x, y = project(p)
            if previous is not None:
                grid.append(f"M{previous[0]:.1f},{previous[1]:.1f}L{x:.1f},{y:.1f}")
            previous = (x, y)
out.append(f'<path d="{"".join(grid)}" stroke="#ffffff" stroke-opacity="0.35" stroke-width="1" fill="none"/>')
out.append(f'<rect y="{HORIZON_Y:.1f}" width="{WIDTH}" height="560" fill="url(#haze)"/>')
out.append(f'<rect width="{WIDTH}" height="{max(HORIZON_Y, 0):.1f}" fill="url(#sky)"/>')

# --- airport ----------------------------------------------------------------

runways = [(a, enu(*pa), b, enu(*pb)) for a, pa, b, pb in RUNWAYS]
ends = [p for _, pa, _, pb in runways for p in (pa, pb)]
middle = centroid(ends)
apron = sorted(ends, key=lambda p: math.atan2(p[1] - middle[1], p[0] - middle[0]))
polygon(
    [add(middle, mul(sub(p, middle), 1.25)) for p in apron],
    fill="#8fb37c",
    stroke="#8fb37c",
    stroke_width=30,
    stroke_linejoin="round",
)
for _, pa, _, pb in runways:
    line(pa, pb, stroke="#39424d", stroke_width=7, stroke_linecap="butt")
    line(pa, pb, stroke="#ffffff", stroke_width=1.2, stroke_dasharray="7 5")
for a, pa, b, pb in runways:
    for name, here, there in ((a, pa, pb), (b, pb, pa)):
        (x, y), (ox, oy) = project(here), project(there)
        n = math.hypot(x - ox, y - oy)
        text(
            x + (x - ox) / n * 20,
            y + (y - oy) / n * 16 + 6,
            name,
            size=16,
            anchor="middle",
            weight="bold",
        )
ax, ay = project(middle)
text(ax, min(project(p)[1] for p in ends) - 34, "Hamburg Airport (EDDH)", size=21, anchor="middle", weight="bold")

# --- box --------------------------------------------------------------------

half = AREA_WIDTH_KM / 2
ahead = AREA_LENGTH_KM * AREA_OFFSET_PERCENT / 100
behind = AREA_LENGTH_KM - ahead
top = MAX_ALTITUDE_FT * KM_PER_FOOT
corners = [(-half, -behind), (half, -behind), (half, ahead), (-half, ahead)]
floor = [local(r, a) for r, a in corners]
roof = [local(r, a, top) for r, a in corners]
sides = [[floor[i], floor[(i + 1) % 4], roof[(i + 1) % 4], roof[i]] for i in range(4)]
BOX_FILL = "#ffc928"
BOX_EDGE = "#d89a00"


def faces_camera(face):
    c = centroid(face)
    outward = sub(c, local(0.0, (ahead - behind) / 2, c[2]))
    return dot(outward, sub(CAMERA, c)) > 0


polygon(floor, fill=BOX_FILL, fill_opacity=0.22, stroke=BOX_EDGE, stroke_width=1.5, stroke_dasharray="8 6")
for face in sides:
    if not faces_camera(face):
        polygon(face, fill=BOX_FILL, fill_opacity=0.16, stroke=BOX_EDGE, stroke_width=1.5, stroke_opacity=0.7)

# --- inside the box: approach path, view direction, house, aircraft ---------

threshold, far_end = runways[1][1], runways[1][3]
outbound = unit(sub(threshold, far_end))


def on_approach(km_out):
    """A point on a 3 degree approach to runway 15."""
    return add(add(threshold, mul(outbound, km_out)), mul(UP, (km_out + 0.3) * math.tan(math.radians(3))))


line(on_approach(0.0), on_approach(17.0), stroke="#39424d", stroke_width=2, stroke_dasharray="3 7", stroke_linecap="round")

arrow_tip = local(0.0, 2.6)
line(local(0.0, 0.4), local(0.0, 2.3), stroke="#1f2a36", stroke_width=3)
polygon([arrow_tip, local(-0.16, 2.15), local(0.16, 2.15)], fill="#1f2a36")

polygon([local(-0.34, -0.3), local(0.38, -0.3), local(0.48, 0.34), local(-0.24, 0.34)], fill="#000000", fill_opacity=0.16)
w, d, wall, ridge = 0.26, 0.22, 0.3, 0.52
house = [
    ("#f7f1e3", [local(-w, -d), local(w, -d), local(w, -d, wall), local(-w, -d, wall)]),
    ("#f7f1e3", [local(-w, d), local(w, d), local(w, d, wall), local(-w, d, wall)]),
    ("#dcd4c0", [local(-w, -d), local(-w, d), local(-w, d, wall), local(-w, 0, ridge), local(-w, -d, wall)]),
    ("#dcd4c0", [local(w, -d), local(w, d), local(w, d, wall), local(w, 0, ridge), local(w, -d, wall)]),
    ("#c8452f", [local(-w, -d, wall), local(w, -d, wall), local(w, 0, ridge), local(-w, 0, ridge)]),
    ("#a83824", [local(-w, d, wall), local(w, d, wall), local(w, 0, ridge), local(-w, 0, ridge)]),
]
for fill, face in sorted(house, key=lambda item: -depth(centroid(item[1]))):
    polygon(face, fill=fill, stroke="#5a4636", stroke_width=1, stroke_linejoin="round")
polygon([local(-0.05, -d), local(0.05, -d), local(0.05, -d, 0.18), local(-0.05, -d, 0.18)], fill="#5a4636")

aircraft_at = on_approach(5.8)
inbound = mul(outbound, -1)
wing = cross(inbound, UP)
outline = [(0.5, 0), (0.42, 0.035), (0.1, 0.04), (-0.05, 0.45), (-0.15, 0.45), (-0.08, 0.04),
           (-0.38, 0.03), (-0.46, 0.17), (-0.52, 0.17), (-0.48, 0)]
outline += [(a, -b) for a, b in reversed(outline[1:-1])]
plane = [add(add(aircraft_at, mul(inbound, a * 0.8)), mul(wing, b * 0.8)) for a, b in outline]
shadow = [(p[0], p[1], 0.0) for p in plane]
polygon(shadow, fill="#000000", fill_opacity=0.14)
line(aircraft_at, (aircraft_at[0], aircraft_at[1], 0.0), stroke="#39424d", stroke_width=1, stroke_dasharray="2 4")
polygon(plane, fill="#27313d", stroke="#ffffff", stroke_width=0.8, stroke_linejoin="round")

# --- box, near side ---------------------------------------------------------

for face in sides:
    if faces_camera(face):
        polygon(face, fill=BOX_FILL, fill_opacity=0.16, stroke=BOX_EDGE, stroke_width=2.5, stroke_linejoin="round")
polygon(roof, fill=BOX_FILL, fill_opacity=0.14, stroke=BOX_EDGE, stroke_width=2.5, stroke_linejoin="round")

# --- labels -----------------------------------------------------------------

hx, hy = project(local(-w, -d, 0.1))
lx, ly = hx - 70, hy - 40
out.append(f'<path d="M{hx:.1f},{hy:.1f}L{lx + 10:.1f},{ly - 7:.1f}" stroke="#1f2a36" stroke-width="1.5" fill="none"/>')
text(lx, ly, f"{LAT}° N, {LON}° E", size=22, anchor="end", weight="bold")
text(lx, ly + 24, "--lat / --lon", size=16, anchor="end", fill="#4a5764")

vx, vy = project(arrow_tip)
text(vx + 16, vy + 4, f"--view-direction {VIEW_DIRECTION:g}°", size=17, fill="#1f2a36", weight="bold")
text(vx + 16, vy + 26, f"--area-offset {AREA_OFFSET_PERCENT:g} % ahead of the house", size=15, fill="#4a5764")

(x1, y1), (x2, y2) = project(floor[0]), project(floor[1])
text((x1 + x2) / 2, (y1 + y2) / 2 + 34, f"--area-width {AREA_WIDTH_KM:g} km", size=20, anchor="middle", weight="bold")

(x1, y1), (x2, y2) = project(floor[1]), project(floor[2])
text((x1 + x2) / 2 + 18, (y1 + y2) / 2 + 22, f"--area-length {AREA_LENGTH_KM:g} km", size=20, weight="bold")

(x1, y1), (x2, y2) = project(floor[0]), project(roof[0])
text(x1 - 16, (y1 + y2) / 2 + 6, f"--max-altitude {MAX_ALTITUDE_FT} ft", size=20, anchor="end", weight="bold")

north_from = local(-5.0, 1.5)
north_to = add(north_from, (0.0, 1.2, 0.0))
line(north_from, north_to, stroke="#1f2a36", stroke_width=2.5)
polygon([add(north_to, (0.0, 0.35, 0.0)), add(north_to, (-0.14, 0.0, 0.0)), add(north_to, (0.14, 0.0, 0.0))], fill="#1f2a36")
(nx, ny), (bx, by) = project(add(north_to, (0.0, 0.35, 0.0))), project(north_from)
n = math.hypot(nx - bx, ny - by)
text(nx + (nx - bx) / n * 18, ny + (ny - by) / n * 18 + 6, "N", size=18, anchor="middle", weight="bold")

out.append("</svg>")
Path(__file__).with_name("filter-area.svg").write_text("\n".join(out) + "\n")
