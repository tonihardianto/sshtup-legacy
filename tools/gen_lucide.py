#!/usr/bin/env python3
"""Turns Lucide SVG icons (https://lucide.dev, ISC licence) into flat polylines in `src/lucide_data.rs`.

    python3 tools/gen_lucide.py <dir with the .svg files> > src/lucide_data.rs

Lucide icons are 24x24 outlines made of paths, lines, circles and rectangles. Curves and arcs are flattened here, so the
app needs no SVG renderer: it strokes the polylines with round caps and joins (see src/icons.rs).
"""
import math, re, sys, os
import xml.etree.ElementTree as ET

# Icon variant name -> Lucide file (without .svg)
ICONS = [
    ("Host", "server"), ("Key", "key-round"), ("Forward", "arrow-left-right"), ("Snippet", "code-xml"),
    ("Shield", "shield-check"), ("Logs", "scroll-text"), ("Folder", "folder"), ("Settings", "settings"),
    ("Group", "layout-grid"), ("User", "user"), ("Bell", "bell"), ("Close", "x"), ("Terminal", "terminal"),
    ("Search", "search"), ("ChevLeft", "chevron-left"), ("ChevRight", "chevron-right"), ("ChevDown", "chevron-down"),
    ("ChevUp", "chevron-up"), ("File", "file"), ("Users", "users"), ("Upload", "upload"), ("Copy", "copy"),
    ("Stop", "square"), ("FolderPlus", "folder-plus"), ("Pencil", "pencil"), ("Pin", "pin"), ("PinOff", "pin-off"), ("Trash", "trash"), ("Power", "power"), ("Sliders", "settings-2"), ("List", "list"),
    ("Download", "download"), ("Eye", "eye"), ("EyeOff", "eye-off"), ("Alert", "triangle-alert"), ("Lock", "lock"),
    ("Sync", "refresh-cw"), ("Info", "info"),
]
NS = "{http://www.w3.org/2000/svg}"


def bez(p0, p1, p2, p3, n=14):
    out = []
    for i in range(1, n + 1):
        t = i / n
        u = 1 - t
        out.append((u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
                    u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1]))
    return out


def arc(p0, rx, ry, phi, fa, fs, p1, n=14):
    """SVG endpoint arc to points (W3C implementation notes, F.6.5)."""
    if p0 == p1:
        return []
    rx, ry = abs(rx), abs(ry)
    if rx == 0 or ry == 0:
        return [p1]
    ph = math.radians(phi)
    c, s = math.cos(ph), math.sin(ph)
    dx, dy = (p0[0] - p1[0]) / 2, (p0[1] - p1[1]) / 2
    x1, y1 = c * dx + s * dy, -s * dx + c * dy
    lam = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry)
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1
    den = rx * rx * y1 * y1 + ry * ry * x1 * x1
    co = math.sqrt(max(0, num / den)) * (-1 if fa == fs else 1)
    cxp, cyp = co * rx * y1 / ry, -co * ry * x1 / rx
    cx = c * cxp - s * cyp + (p0[0] + p1[0]) / 2
    cy = s * cxp + c * cyp + (p0[1] + p1[1]) / 2

    def ang(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    th1 = ang(1, 0, (x1 - cxp) / rx, (y1 - cyp) / ry)
    dth = ang((x1 - cxp) / rx, (y1 - cyp) / ry, (-x1 - cxp) / rx, (-y1 - cyp) / ry)
    if not fs and dth > 0:
        dth -= 2 * math.pi
    elif fs and dth < 0:
        dth += 2 * math.pi
    out = []
    steps = max(4, int(abs(dth) / (math.pi / 2) * n))
    for i in range(1, steps + 1):
        t = th1 + dth * i / steps
        x, y = rx * math.cos(t), ry * math.sin(t)
        out.append((c * x - s * y + cx, s * x + c * y + cy))
    out[-1] = p1
    return out


def parse_path(d):
    toks = re.findall(r"[MmLlHhVvCcSsQqTtAaZz]|-?(?:\d*\.\d+|\d+\.?)(?:e-?\d+)?", d)
    i = 0
    subs, cur, pos, start = [], [], (0.0, 0.0), (0.0, 0.0)
    last_c = None
    cmd = None

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    def flag():
        # arc flags may be glued to the next number ("0 01.5"): handled by the tokenizer only when separated
        return int(num())

    while i < len(toks):
        if re.match(r"[A-Za-z]", toks[i]):
            cmd = toks[i]
            i += 1
            if cmd in "Zz":
                if cur:
                    cur.append(start)
                    subs.append(cur)
                    cur = []
                pos = start
                continue
        rel = cmd.islower()
        C = cmd.upper()
        ox, oy = pos if rel else (0.0, 0.0)
        if C == "M":
            if cur:
                subs.append(cur)
            pos = (ox + num(), oy + num())
            start = pos
            cur = [pos]
            cmd = "l" if rel else "L"
        elif C == "L":
            pos = (ox + num(), oy + num())
            cur.append(pos)
        elif C == "H":
            pos = ((pos[0] if rel else 0) + num(), pos[1])
            cur.append(pos)
        elif C == "V":
            pos = (pos[0], (pos[1] if rel else 0) + num())
            cur.append(pos)
        elif C == "C":
            p1 = (ox + num(), oy + num()); p2 = (ox + num(), oy + num()); p3 = (ox + num(), oy + num())
            cur += bez(pos, p1, p2, p3)
            last_c = p2
            pos = p3
            continue
        elif C == "S":
            p1 = (2 * pos[0] - last_c[0], 2 * pos[1] - last_c[1]) if last_c else pos
            p2 = (ox + num(), oy + num()); p3 = (ox + num(), oy + num())
            cur += bez(pos, p1, p2, p3)
            last_c = p2
            pos = p3
            continue
        elif C == "Q":
            q = (ox + num(), oy + num()); p3 = (ox + num(), oy + num())
            p1 = (pos[0] + 2 / 3 * (q[0] - pos[0]), pos[1] + 2 / 3 * (q[1] - pos[1]))
            p2 = (p3[0] + 2 / 3 * (q[0] - p3[0]), p3[1] + 2 / 3 * (q[1] - p3[1]))
            cur += bez(pos, p1, p2, p3)
            pos = p3
        elif C == "A":
            rx, ry, ph = num(), num(), num()
            fa, fs = flag(), flag()
            p1 = (ox + num(), oy + num())
            cur += arc(pos, rx, ry, ph, fa, fs, p1)
            pos = p1
        else:
            raise SystemExit(f"unsupported path command {cmd}")
        last_c = None
    if cur:
        subs.append(cur)
    return subs


def circle(cx, cy, rx, ry, n=40):
    return [[(cx + rx * math.cos(2 * math.pi * k / n), cy + ry * math.sin(2 * math.pi * k / n)) for k in range(n + 1)]]


def rect(x, y, w, h, r):
    r = min(r, w / 2, h / 2)
    if r <= 0:
        return [[(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]]
    pts = [(x + r, y), (x + w - r, y)]
    pts += arc((x + w - r, y), r, r, 0, 0, 1, (x + w, y + r))
    pts += [(x + w, y + h - r)]
    pts += arc((x + w, y + h - r), r, r, 0, 0, 1, (x + w - r, y + h))
    pts += [(x + r, y + h)]
    pts += arc((x + r, y + h), r, r, 0, 0, 1, (x, y + h - r))
    pts += [(x, y + r)]
    pts += arc((x, y + r), r, r, 0, 0, 1, (x + r, y))
    return [pts]


def load(path):
    out = []
    for el in ET.parse(path).getroot().iter():
        t = el.tag.replace(NS, "")
        a = el.attrib
        f = lambda k, d=0.0: float(a.get(k, d))
        if t == "path":
            out += parse_path(a["d"])
        elif t == "line":
            out.append([(f("x1"), f("y1")), (f("x2"), f("y2"))])
        elif t == "polyline":
            v = [float(x) for x in re.findall(r"-?[\d.]+", a["points"])]
            out.append(list(zip(v[::2], v[1::2])))
        elif t == "polygon":
            v = [float(x) for x in re.findall(r"-?[\d.]+", a["points"])]
            p = list(zip(v[::2], v[1::2]))
            out.append(p + [p[0]])
        elif t == "circle":
            out += circle(f("cx"), f("cy"), f("r"), f("r"))
        elif t == "ellipse":
            out += circle(f("cx"), f("cy"), f("rx"), f("ry"))
        elif t == "rect":
            out += rect(f("x"), f("y"), f("width"), f("height"), f("rx", a.get("ry", 0)))
    return [s for s in out if len(s) >= 2]


def main(src):
    print("// Generated by tools/gen_lucide.py from Lucide icons (https://lucide.dev, ISC licence, see LICENSES/lucide.txt).")
    print("// Each icon is a list of polylines on a 24x24 grid, stroked 2 wide with round caps and joins. Do not edit by hand.")
    print()
    print("use crate::icons::Icon;")
    print()
    print("pub fn strokes(icon: Icon) -> &'static [&'static [(f32, f32)]] {")
    print("    match icon {")
    for name, file in ICONS:
        subs = load(os.path.join(src, file + ".svg"))
        body = ", ".join("&[" + ", ".join(f"({x:.2f}, {y:.2f})" for x, y in s) + "]" for s in subs)
        print(f"        Icon::{name} => &[{body}],")
    print("        _ => &[],")
    print("    }")
    print("}")


if __name__ == "__main__":
    main(sys.argv[1])
