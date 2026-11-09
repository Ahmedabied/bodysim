"""Draw one simulated day as an SVG: cortisol, core temperature and glucose, for one person.

    cargo run --release --example day -- 2 > day.csv
    python3 scripts/plot_day.py day.csv docs/day.svg [person] [day]

Standard library only. The day defaults to the second one (the first starts from the initial state).
"""

import csv
import sys

PANELS = [
    ("cortisol_nmol_l", "Cortisol", "nmol/L"),
    ("core_c", "Core temperature", "C"),
    ("glucose_mg_dl", "Glucose", "mg/dL"),
]
MEALS = [(7.5, "breakfast"), (12.5, "lunch"), (19.0, "dinner")]
W, PANEL_H, LEFT, RIGHT, TOP, GAP = 760, 150, 70, 24, 40, 46
INK, MUTED, GRID, LINE = "#1f2328", "#6e7781", "#e6e8eb", "#2f6fb0"


def nice_range(lo, hi):
    pad = (hi - lo) * 0.12 or 1.0
    return lo - pad, hi + pad


def main(src, dst, person="adult-a", day=1):
    rows = [r for r in csv.DictReader(open(src, encoding="utf-8")) if r["person"] == person]
    rows = [r for r in rows if day * 24 <= float(r["hour"]) <= (day + 1) * 24]
    hours = [float(r["hour"]) - day * 24 for r in rows]
    height = TOP + len(PANELS) * (PANEL_H + GAP) + 10
    plot_w = W - LEFT - RIGHT
    x = lambda h: LEFT + plot_w * h / 24
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {height}" width="{W}" height="{height}" '
           f'font-family="system-ui, -apple-system, Segoe UI, sans-serif" font-size="12" fill="{INK}">',
           f'<rect width="{W}" height="{height}" fill="#ffffff"/>',
           f'<text x="{LEFT}" y="22" font-size="14" font-weight="600">One simulated day of {person}</text>']
    for i, (key, title, unit) in enumerate(PANELS):
        y0 = TOP + i * (PANEL_H + GAP) + 18
        vals = [float(r[key]) for r in rows]
        lo, hi = nice_range(min(vals), max(vals))
        y = lambda v: y0 + PANEL_H * (1 - (v - lo) / (hi - lo))
        out.append(f'<text x="{LEFT}" y="{y0 - 8}" font-weight="600">{title} <tspan fill="{MUTED}" '
                   f'font-weight="400">({unit})</tspan></text>')
        for h in range(0, 25, 3):
            out.append(f'<line x1="{x(h):.1f}" y1="{y0}" x2="{x(h):.1f}" y2="{y0 + PANEL_H}" stroke="{GRID}"/>')
            if i == len(PANELS) - 1:
                out.append(f'<text x="{x(h):.1f}" y="{y0 + PANEL_H + 16}" text-anchor="middle" fill="{MUTED}">{h:02d}:00</text>')
        for v in (min(vals), max(vals)):
            out.append(f'<line x1="{LEFT}" y1="{y(v):.1f}" x2="{W - RIGHT}" y2="{y(v):.1f}" stroke="{GRID}" stroke-dasharray="3 3"/>')
            out.append(f'<text x="{LEFT - 8}" y="{y(v) + 4:.1f}" text-anchor="end" fill="{MUTED}">{v:.1f}</text>')
        if key == "glucose_mg_dl":
            for h, name in MEALS:
                out.append(f'<line x1="{x(h):.1f}" y1="{y0}" x2="{x(h):.1f}" y2="{y0 + PANEL_H}" stroke="{MUTED}" stroke-width="1"/>')
                out.append(f'<text x="{x(h) + 4:.1f}" y="{y0 + 12}" fill="{MUTED}">{name}</text>')
        pts = " ".join(f"{x(h):.1f},{y(v):.1f}" for h, v in zip(hours, vals))
        out.append(f'<polyline points="{pts}" fill="none" stroke="{LINE}" stroke-width="2" stroke-linejoin="round"/>')
    out.append("</svg>")
    open(dst, "w", encoding="utf-8").write("\n".join(out) + "\n")


if __name__ == "__main__":
    a = sys.argv[1:]
    if len(a) < 2:
        sys.exit(__doc__)
    main(a[0], a[1], a[2] if len(a) > 2 else "adult-a", int(a[3]) if len(a) > 3 else 1)
