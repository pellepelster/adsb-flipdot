#!/usr/bin/env python3
"""Generates the flip-dot adapter board (Raspberry Pi HAT -> Mobitec 26-pin panel header).

Run inside the kicad/kicad:9.0 container:
  build_board.py prep     -> flipdot-hat.kicad_pcb (unrouted) + flipdot-hat.dsn
  build_board.py finish   -> imports flipdot-hat.ses, adds ground pours, saves the board
"""
import sys
import pcbnew
from pcbnew import VECTOR2I, FromMM, ToMM

FP = "/usr/share/kicad/footprints/"
NAME = "flipdot-hat"
W, H, R = 65.0, 56.5, 3.0  # Raspberry Pi HAT outline

# signal -> (Pi header pin, panel header pin, buffer, channel)
SIGNALS = [
    ("ROW0", 11, 5, "U1", 1), ("ROW1", 13, 7, "U1", 2), ("ROW2", 15, 6, "U1", 3),
    ("ROW3", 19, 8, "U1", 4), ("ROW4", 21, 9, "U1", 5),
    ("ROW-HIGH", 23, 10, "U1", 6), ("ROW-LOW", 33, 11, "U1", 7),
    ("COL0", 12, 13, "U2", 1), ("COL1", 16, 14, "U2", 2), ("COL2", 18, 15, "U2", 3),
    ("COL3", 22, 16, "U2", 4), ("COL4", 7, 17, "U2", 5), ("COL-H/L", 32, 18, "U2", 6),
    ("COL-SEL0", 36, 19, "U3", 1), ("COL-SEL1", 38, 20, "U3", 2), ("COL-SEL2", 40, 21, "U3", 3),
    ("COL-SEL3", 35, 22, "U3", 4), ("COL-SEL4", 37, 23, "U3", 5),
]
PI_GND = [6, 9, 14, 20, 25, 30, 34, 39]
PI_5V = [2, 4]


def P(x, y):
    return VECTOR2I(FromMM(x), FromMM(y))


def prep():
    board = pcbnew.NewBoard(NAME + ".kicad_pcb")
    ds = board.GetDesignSettings()
    ds.m_MinClearance = FromMM(0.2)
    ds.m_TrackMinWidth = FromMM(0.25)
    ds.m_CopperEdgeClearance = FromMM(0.4)
    ns = ds.m_NetSettings
    dflt = ns.GetDefaultNetclass()
    dflt.SetClearance(FromMM(0.25))
    dflt.SetTrackWidth(FromMM(0.3))
    dflt.SetViaDiameter(FromMM(0.8))
    dflt.SetViaDrill(FromMM(0.4))
    power = pcbnew.NETCLASS("Power")
    power.SetClearance(FromMM(0.25))
    power.SetTrackWidth(FromMM(0.8))
    power.SetViaDiameter(FromMM(1.0))
    power.SetViaDrill(FromMM(0.5))
    ns.SetNetclass("Power", power)
    for pat in ("+5V", "+12V", "VIN", "VIN_F"):
        ns.SetNetclassPatternAssignment(pat, "Power")

    nets = {}

    def net(name):
        if name not in nets:
            nets[name] = pcbnew.NETINFO_ITEM(board, name)
            board.Add(nets[name])
        return nets[name]

    def add(lib, fpname, ref, value, rot=0):
        fp = pcbnew.FootprintLoad(FP + lib + ".pretty", fpname)
        fp.SetReference(ref)
        fp.SetValue(value)
        board.Add(fp)
        fp.SetOrientationDegrees(rot)
        return fp

    def pad(fp, n):
        return fp.FindPadByNumber(str(n))

    def move_pad_to(fp, n, x, y):
        p = pad(fp, n).GetPosition()
        fp.Move(VECTOR2I(FromMM(x) - p.x, FromMM(y) - p.y))

    def connect(fp, n, netname):
        pad(fp, n).SetNet(net(netname))

    def at(fp, n):
        p = pad(fp, n).GetPosition()
        return round(ToMM(p.x), 2), round(ToMM(p.y), 2)

    # --- Raspberry Pi 40-pin connector (female socket soldered on the BOTTOM side) ---
    # A socket footprint flipped to the back gives the Pi pin positions seen from the top.
    j1 = add("Connector_PinSocket_2.54mm", "PinSocket_2x20_P2.54mm_Vertical", "J1", "Pi socket 2x20", 0)
    j1.Flip(j1.GetPosition(), pcbnew.FLIP_DIRECTION_LEFT_RIGHT)
    for rot in (0, 90, 180, 270):
        j1.SetOrientationDegrees(rot)
        move_pad_to(j1, 1, 8.37, 4.77)
        if at(j1, 2) == (8.37, 2.23) and at(j1, 3) == (10.91, 4.77) and at(j1, 40) == (56.63, 2.23):
            break
    else:
        raise SystemExit("J1 orientation not found")
    assert j1.GetLayer() == pcbnew.B_Cu

    # --- panel connector, same pinout as the panel so a straight ribbon works ---
    j2 = add("Connector_IDC", "IDC-Header_2x13_P2.54mm_Vertical", "J2", "Panel 2x13 IDC", 90)
    move_pad_to(j2, 1, 17.26, 49.54)
    assert at(j2, 2) == (17.26, 47.0) and at(j2, 3) == (19.8, 49.54), (at(j2, 2), at(j2, 3))

    # --- level shifters ---
    us = {}
    for ref, x in (("U1", 25.0), ("U2", 39.0), ("U3", 53.0)):
        u = add("Package_DIP", "DIP-20_W7.62mm", ref, "74HCT541", 0)
        move_pad_to(u, 1, x - 3.81, 13.0)
        us[ref] = u
        connect(u, 1, "GND")   # /OE1
        connect(u, 19, "GND")  # /OE2
        connect(u, 10, "GND")
        connect(u, 20, "+5V")
        c = add("Capacitor_THT", "C_Disc_D3.0mm_W1.6mm_P2.50mm", "C" + ref[1], "100n", 0)
        move_pad_to(c, 1, x + 3.81, 9.6)
        if at(c, 2)[0] > x + 3.81:  # keep the cap over the IC, not beside it
            c.SetOrientationDegrees(180)
            move_pad_to(c, 1, x + 3.81, 9.6)
        connect(c, 1, "+5V")
        connect(c, 2, "GND")
        c.Reference().SetPosition(P(x + 2.56, 7.75))

    used = {"U1": set(), "U2": set(), "U3": set()}
    for sig, pi_pin, panel_pin, ref, ch in SIGNALS:
        connect(j1, pi_pin, "PI_" + sig)
        connect(us[ref], ch + 1, "PI_" + sig)   # A(ch)
        connect(us[ref], 19 - ch, sig)          # Y(ch)
        connect(j2, panel_pin, sig)
        used[ref].add(ch)
    for ref, chans in used.items():
        for ch in range(1, 9):
            if ch not in chans:
                connect(us[ref], ch + 1, "GND")  # unused inputs tied low
    for n in PI_GND:
        connect(j1, n, "GND")
    for n in PI_5V:
        connect(j1, n, "+5V")
    connect(j2, 1, "GND")
    connect(j2, 2, "GND")
    connect(j2, 3, "+5V")
    connect(j2, 26, "+12V")

    # --- coil supply: jack -> Schottky (reverse polarity) -> bulk cap -> panel pin 26 ---
    j3 = add("Connector_BarrelJack", "BarrelJack_Horizontal", "J3", "12-24V 5.5x2.1 centre +", 0)
    move_pad_to(j3, 1, 13.75, 14.0)  # opening flush with the left board edge
    connect(j3, 1, "VIN")
    connect(j3, 2, "GND")
    connect(j3, 3, "GND")
    d1 = add("Diode_THT", "D_DO-41_SOD81_P10.16mm_Horizontal", "D1", "1N5819", 0)
    move_pad_to(d1, 1, 14.4, 24.5)   # pad 1 = cathode
    if at(d1, 2)[0] > 14.4:
        d1.SetOrientationDegrees(180)
        move_pad_to(d1, 1, 14.4, 24.5)
    connect(d1, 2, "VIN")
    connect(d1, 1, "+12V")
    c4 = add("Capacitor_THT", "CP_Radial_D10.0mm_P5.00mm", "C4", "470u 35V", 0)
    move_pad_to(c4, 1, 6.5, 34.0)    # pad 1 = +
    connect(c4, 1, "+12V")
    connect(c4, 2, "GND")
    c4.Reference().SetPosition(P(9.0, 40.8))

    for i, (x, y) in enumerate(((3.5, 3.5), (61.5, 3.5), (3.5, 52.5), (61.5, 52.5)), 1):
        h = add("MountingHole", "MountingHole_2.7mm_M2.5", "H%d" % i, "M2.5", 0)
        h.SetPosition(P(x, y))
        h.Reference().SetVisible(False)

    # --- outline with 3 mm corners ---
    def edge(shape):
        shape.SetLayer(pcbnew.Edge_Cuts)
        shape.SetWidth(FromMM(0.1))
        board.Add(shape)

    def line(x1, y1, x2, y2):
        s = pcbnew.PCB_SHAPE(board)
        s.SetShape(pcbnew.SHAPE_T_SEGMENT)
        s.SetStart(P(x1, y1))
        s.SetEnd(P(x2, y2))
        edge(s)

    def arc(sx, sy, mx, my, ex, ey):
        s = pcbnew.PCB_SHAPE(board)
        s.SetShape(pcbnew.SHAPE_T_ARC)
        s.SetArcGeometry(P(sx, sy), P(mx, my), P(ex, ey))
        edge(s)

    k = R * (1 - 0.5 ** 0.5)
    line(R, 0, W - R, 0); line(W, R, W, H - R); line(W - R, H, R, H); line(0, H - R, 0, R)
    arc(0, R, k, k, R, 0); arc(W - R, 0, W - k, k, W, R)
    arc(W, H - R, W - k, H - k, W - R, H); arc(R, H, k, H - k, 0, H - R)

    # --- silkscreen notes ---
    def text(s, x, y, size=1.0, layer=pcbnew.F_SilkS):
        t = pcbnew.PCB_TEXT(board)
        t.SetText(s)
        t.SetLayer(layer)
        t.SetPosition(P(x, y))
        t.SetTextSize(VECTOR2I(FromMM(size), FromMM(size)))
        t.SetTextThickness(FromMM(size * 0.15))
        if layer == pcbnew.B_SilkS:
            t.SetMirrored(True)
        board.Add(t)

    text("Mobitec flip-dot adapter  rev A", 38.5, 41.5, 1.0)
    text("TO PANEL  (pin 1 left)", 32.5, 54.9, 1.0)
    text("12-24V centre +", 13.5, 8.1, 0.8)
    text("ROW", 25.0, 38.6, 1.0); text("COL", 39.0, 38.6, 1.0); text("COL-SEL", 53.0, 38.6, 1.0)

    board.BuildListOfNets()
    pcbnew.SaveBoard(NAME + ".kicad_pcb", board)
    ok = pcbnew.ExportSpecctraDSN(board, NAME + ".dsn")
    print("saved board, DSN export:", ok, "nets:", len(nets))
    for fp in board.GetFootprints():
        b = fp.GetBoundingBox(False, False)
        print("  %-3s %6.2f %6.2f .. %6.2f %6.2f" % (fp.GetReference(), ToMM(b.GetLeft()), ToMM(b.GetTop()), ToMM(b.GetRight()), ToMM(b.GetBottom())))


def finish():
    board = pcbnew.LoadBoard(NAME + ".kicad_pcb")
    board.GetDesignSettings().m_MinResolvedSpokes = 1
    for t in list(board.GetTracks()):
        board.Remove(t)
    for z in list(board.Zones()):
        board.Remove(z)
    ok = pcbnew.ImportSpecctraSES(board, NAME + ".ses")
    print("SES import:", ok, "tracks:", len(board.GetTracks()))
    gnd = board.FindNet("GND")
    for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
        z = pcbnew.ZONE(board)
        z.SetLayer(layer)
        z.SetNet(gnd)
        z.SetLocalClearance(FromMM(0.3))
        z.SetMinThickness(FromMM(0.25))
        z.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
        z.SetThermalReliefGap(FromMM(0.3))
        z.SetThermalReliefSpokeWidth(FromMM(0.5))
        z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
        o = z.Outline()
        o.NewOutline()
        for x, y in ((0, 0), (W, 0), (W, H), (0, H)):
            o.Append(FromMM(x), FromMM(y))
        board.Add(z)
    pcbnew.ZONE_FILLER(board).Fill(board.Zones())
    pcbnew.SaveBoard(NAME + ".kicad_pcb", board)
    print("saved routed board")


if __name__ == "__main__":
    {"prep": prep, "finish": finish}[sys.argv[1]]()
