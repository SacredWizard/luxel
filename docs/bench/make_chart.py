#!/usr/bin/env python3
"""Render the README benchmark charts (light + dark SVG each). No dependencies.

Run: python3 docs/bench/make_chart.py
Numbers come from the runs described in README "Benchmarks" (Apple Silicon).
Palette: validated categorical slots 1 (blue) and 2 (orange) plus a neutral
for "old behaviour"; light/dark steps are separate, not an automatic flip.
"""
import math
from pathlib import Path

FONT = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif"
THEMES = {
    "light": dict(bg="#fcfcfb", ink="#0b0b0b", muted="#6b6b68", grid="#e6e6e3",
                  blue="#2a78d6", orange="#eb6834", neutral="#a3a39f"),
    "dark": dict(bg="#1a1a19", ink="#f2f2f0", muted="#a3a39f", grid="#33332f",
                 blue="#3987e5", orange="#d95926", neutral="#6b6b68"),
}
LUXEL, LWC = "luxel", "lightweight-charts 4.2.0"


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def bar_path(x, y, w, h, r=4):
    """Bar with a 4px rounded data end (top), square at the baseline."""
    r = min(r, w / 2, h)
    return (f"M{x:.1f},{y + h:.1f} V{y + r:.1f} Q{x:.1f},{y:.1f} {x + r:.1f},{y:.1f} "
            f"H{x + w - r:.1f} Q{x + w:.1f},{y:.1f} {x + w:.1f},{y + r:.1f} V{y + h:.1f} Z")


def frame(t, w, h, title, subtitle, legend, aria):
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" '
           f'font-family="{FONT}" role="img" aria-label="{esc(aria)}">',
           f'<rect width="{w}" height="{h}" rx="12" fill="{t["bg"]}"/>',
           f'<text x="32" y="36" font-size="18" font-weight="600" fill="{t["ink"]}">{esc(title)}</text>',
           f'<text x="32" y="58" font-size="13" fill="{t["muted"]}">{esc(subtitle)}</text>']
    lx = 32
    for name, color in legend:
        out.append(f'<rect x="{lx}" y="70" width="12" height="12" rx="3" fill="{t[color]}"/>')
        out.append(f'<text x="{lx + 18}" y="80.5" font-size="13" fill="{t["ink"]}">{esc(name)}</text>')
        lx += 18 + len(name) * 7.4 + 24
    return out


def grouped_bars(t, *, title, subtitle, aria, groups, series, ymax, ystep, unit,
                 fmt, ref=None, w=760, h=380):
    """series: [(name, color_key, [value or None per group])]; None = did not finish."""
    L, R, T, B = 64, 24, 100, 52
    pw, ph = w - L - R, h - T - B
    ys = lambda v: T + ph - min(v, ymax) / ymax * ph
    out = frame(t, w, h, title, subtitle, [(n, c) for n, c, _ in series], aria)
    v = 0
    while v <= ymax + 1e-9:
        y = ys(v)
        out.append(f'<line x1="{L}" x2="{w - R}" y1="{y:.1f}" y2="{y:.1f}" stroke="{t["grid"]}"/>')
        out.append(f'<text x="{L - 10}" y="{y + 4:.1f}" font-size="12" text-anchor="end" '
                   f'fill="{t["muted"]}">{v:g}{unit}</text>')
        v += ystep
    if ref:
        val, label = ref
        y = ys(val)
        out.append(f'<line x1="{L}" x2="{w - R}" y1="{y:.1f}" y2="{y:.1f}" stroke="{t["muted"]}" '
                   f'stroke-width="1.5" stroke-dasharray="5 4"/>')
        out.append(f'<text x="{w - R}" y="{y - 6:.1f}" font-size="12" text-anchor="end" '
                   f'fill="{t["muted"]}">{esc(label)}</text>')
    gw = pw / len(groups)
    bw = min(64, (gw - 40) / len(series))
    for g, gname in enumerate(groups):
        cx = L + gw * g + gw / 2
        x0 = cx - (bw * len(series) + 2 * (len(series) - 1)) / 2
        for i, (name, color, vals) in enumerate(series):
            x = x0 + i * (bw + 2)
            val = vals[g]
            if val is None:
                for dy, txt in ((-44, "✕"), (-26, "did not"), (-10, "finish")):
                    out.append(f'<text x="{x + bw / 2:.1f}" y="{ys(0) + dy:.1f}" font-size="12" '
                               f'text-anchor="middle" fill="{t["muted"]}">{txt}</text>')
                continue
            y = ys(val)
            hgt = max(ys(0) - y, 2)
            out.append(f'<path d="{bar_path(x, ys(0) - hgt, bw, hgt)}" fill="{t[color]}">'
                       f'<title>{esc(name)}, {esc(gname)}: {esc(fmt(val))}</title></path>')
            inside = hgt > 40
            ly, ink = (ys(0) - hgt + 20, "#ffffff") if inside else (ys(0) - hgt - 8, t["ink"])
            out.append(f'<text x="{x + bw / 2:.1f}" y="{ly:.1f}" font-size="13" font-weight="600" '
                       f'text-anchor="middle" fill="{ink}">{esc(fmt(val))}</text>')
        out.append(f'<text x="{cx:.1f}" y="{h - B + 24:.1f}" font-size="13" text-anchor="middle" '
                   f'fill="{t["ink"]}">{esc(gname)}</text>')
    out.append(f'<line x1="{L}" x2="{w - R}" y1="{ys(0):.1f}" y2="{ys(0):.1f}" stroke="{t["muted"]}"/>')
    out.append("</svg>")
    return "\n".join(out)


def flat_lines(t, *, title, subtitle, aria, xs, xlabels, series, ymax, ystep, unit, note,
               w=760, h=350):
    """Line chart with a log-scaled x axis (data size); series: [(name, color, [y])]."""
    L, R, T, B = 72, 40, 100, 62
    pw, ph = w - L - R, h - T - B
    lx0, lx1 = math.log10(xs[0]), math.log10(xs[-1])
    xp = lambda x: L + (math.log10(x) - lx0) / (lx1 - lx0) * pw
    yp = lambda v: T + ph - v / ymax * ph
    out = frame(t, w, h, title, subtitle, [(n, c) for n, c, _ in series], aria)
    v = 0
    while v <= ymax + 1e-9:
        y = yp(v)
        out.append(f'<line x1="{L}" x2="{w - R}" y1="{y:.1f}" y2="{y:.1f}" stroke="{t["grid"]}"/>')
        out.append(f'<text x="{L - 10}" y="{y + 4:.1f}" font-size="12" text-anchor="end" '
                   f'fill="{t["muted"]}">{v:g}{unit}</text>')
        v += ystep
    for x, lab in zip(xs, xlabels):
        out.append(f'<text x="{xp(x):.1f}" y="{h - B + 24:.1f}" font-size="13" text-anchor="middle" '
                   f'fill="{t["ink"]}">{esc(lab)}</text>')
    for name, color, ys_ in series:
        pts = " ".join(f"{xp(x):.1f},{yp(y):.1f}" for x, y in zip(xs, ys_))
        out.append(f'<polyline points="{pts}" fill="none" stroke="{t[color]}" stroke-width="2" '
                   f'stroke-linejoin="round" stroke-linecap="round"/>')
        for x, y in zip(xs, ys_):
            out.append(f'<circle cx="{xp(x):.1f}" cy="{yp(y):.1f}" r="5" fill="{t[color]}" '
                       f'stroke="{t["bg"]}" stroke-width="2"><title>{esc(name)}: {y:g}{unit}</title></circle>')
    out.append(f'<line x1="{L}" x2="{w - R}" y1="{yp(0):.1f}" y2="{yp(0):.1f}" stroke="{t["muted"]}"/>')
    out.append(f'<text x="{L + pw / 2:.1f}" y="{h - 10:.1f}" font-size="12" text-anchor="middle" '
               f'fill="{t["muted"]}">bars in the series (log scale)</text>')
    out.append(f'<text x="{w - R}" y="{T - 2:.1f}" font-size="12" text-anchor="end" '
               f'fill="{t["muted"]}">{esc(note)}</text>')
    out.append("</svg>")
    return "\n".join(out)


CHARTS = {
    # 1. hero: worst-5% frame rate while zooming (visible Chrome, 120 Hz)
    "benchmark": lambda t: grouped_bars(
        t, title="Frame rate of the slowest 5% of frames while zooming",
        subtitle="Higher is better · visible Chrome window, 120 Hz display, same scripted zoom sweep",
        aria="Bar chart. Frame rate of the slowest 5% of frames while zooming. luxel holds about 110 fps "
             "at 100K, 1M and 10M bars; lightweight-charts drops to 24 fps at 100K, 1.9 fps at 1M and "
             "does not finish at 10M.",
        groups=["100K bars", "1M bars", "10M bars"],
        series=[(LUXEL, "blue", [1000 / 9.1, 1000 / 9.0, 1000 / 9.0]),
                (LWC, "orange", [1000 / 41.1, 1000 / 516, None])],
        ymax=120, ystep=30, unit="", ref=(120, "120 Hz display limit"),
        fmt=lambda v: f"{v:.0f} fps" if v >= 10 else f"{v:.1f} fps"),
    # 2. load time
    "load": lambda t: grouped_bars(
        t, title="Time to load the data",
        subtitle="Lower is better · luxel builds its LOD pyramid; lightweight-charts runs setData()",
        aria="Bar chart. Time to load the data. luxel: 1 ms at 100K bars, 9 ms at 1M, 52 ms at 10M. "
             "lightweight-charts: 37 ms at 100K, 446 ms at 1M, did not finish at 10M.",
        groups=["100K bars", "1M bars", "10M bars"],
        series=[(LUXEL, "blue", [1, 9, 52]), (LWC, "orange", [37, 446, None])],
        ymax=500, ystep=100, unit=" ms", fmt=lambda v: f"{v:.0f} ms"),
    # 3. CPU per frame vs data size (native)
    "cpu": lambda t: flat_lines(
        t, title="CPU time per frame stays under 3 µs as the data grows 500×",
        subtitle="Native, release build · visible-range query + instance generation, 2000 px plot",
        aria="Line chart. luxel CPU time per frame from 100K to 50M bars stays between 0.9 and 2.3 "
             "microseconds, whether the whole series or the last 500 bars is visible.",
        xs=[1e5, 1e6, 1e7, 5e7], xlabels=["100K", "1M", "10M", "50M"],
        series=[("whole series visible", "blue", [2.3, 2.2, 1.2, 1.7]),
                ("last 500 bars visible", "orange", [1.4, 1.1, 0.9, 1.9])],
        ymax=10, ystep=2, unit=" µs", note="for scale: one 120 Hz frame is 8,333 µs"),
    # 4. axis smoothness A/B
    "axis": lambda t: grouped_bars(
        t, title="Price axis while zooming: snapping vs luxel's spring",
        subtitle="Lower is better · 1M-bar zoom sweep, measured every frame, % of the axis height",
        aria="Bar chart. Largest one-frame axis jump: snapping 222%, spring 34%. 99th-percentile "
             "per-frame jerk: snapping 171%, spring 10%.",
        groups=["largest one-frame jump", "p99 per-frame jerk"],
        series=[("snapping (before)", "neutral", [222, 171]), ("luxel spring", "blue", [34, 10])],
        ymax=240, ystep=60, unit="%", fmt=lambda v: f"{v:.0f}%", h=340),
}

if __name__ == "__main__":
    here = Path(__file__).parent
    for name, make in CHARTS.items():
        for theme, t in THEMES.items():
            path = here / f"{name}-{theme}.svg"
            path.write_text(make(t))
            print(f"wrote {path.name}")
