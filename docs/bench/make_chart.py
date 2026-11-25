#!/usr/bin/env python3
"""Render the README benchmark chart (light + dark SVG). No dependencies.

Data: visible Chrome window, 120 Hz display, scripted zoom sweep
(see README "Benchmarks"). Value = 1000 / p95 frame time, i.e. the frame
rate of the slowest 5% of frames. Run: python3 docs/bench/make_chart.py
"""
from pathlib import Path

SIZES = ["100K bars", "1M bars", "10M bars"]
# p95 frame time in ms; None = did not finish
SERIES = [
    ("luxel", [9.1, 9.0, 9.0]),
    ("lightweight-charts 4.2.0", [41.1, 516.0, None]),
]
THEMES = {
    "light": dict(bg="#fcfcfb", ink="#0b0b0b", muted="#6b6b68", grid="#e6e6e3",
                  colors=["#2a78d6", "#eb6834"]),
    "dark": dict(bg="#1a1a19", ink="#f2f2f0", muted="#a3a39f", grid="#33332f",
                 colors=["#3987e5", "#d95926"]),
}
W, H = 760, 380
L, R, T, B = 56, 24, 96, 56          # plot margins
YMAX = 130                            # fps axis max
FONT = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif"


def bar_path(x, y, w, h, r=4):
    """Bar with 4px rounded top (data end), square at the baseline."""
    r = min(r, w / 2, h)
    return (f"M{x:.1f},{y + h:.1f} V{y + r:.1f} Q{x:.1f},{y:.1f} {x + r:.1f},{y:.1f} "
            f"H{x + w - r:.1f} Q{x + w:.1f},{y:.1f} {x + w:.1f},{y + r:.1f} V{y + h:.1f} Z")


def render(theme):
    t = THEMES[theme]
    pw, ph = W - L - R, H - T - B
    ys = lambda v: T + ph - v / YMAX * ph
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" '
           f'font-family="{FONT}" role="img" aria-label="Benchmark: frame rate of the slowest 5% of frames while zooming. '
           f'luxel holds about 110 fps at 100K, 1M and 10M bars; lightweight-charts drops to 24 fps at 100K, '
           f'2 fps at 1M and does not finish at 10M.">',
           f'<rect width="{W}" height="{H}" rx="12" fill="{t["bg"]}"/>',
           f'<text x="{L}" y="34" font-size="18" font-weight="600" fill="{t["ink"]}">'
           f'Frame rate of the slowest 5% of frames while zooming</text>',
           f'<text x="{L}" y="56" font-size="13" fill="{t["muted"]}">'
           f'Higher is better · visible Chrome window, 120 Hz display, same scripted zoom sweep</text>']
    # legend
    lx = L
    for i, (name, _) in enumerate(SERIES):
        out.append(f'<rect x="{lx}" y="68" width="12" height="12" rx="3" fill="{t["colors"][i]}"/>')
        out.append(f'<text x="{lx + 18}" y="78.5" font-size="13" fill="{t["ink"]}">{name}</text>')
        lx += 18 + len(name) * 7.4 + 24
    # grid + y axis
    for v in range(0, YMAX + 1, 30):
        y = ys(v)
        out.append(f'<line x1="{L}" x2="{W - R}" y1="{y:.1f}" y2="{y:.1f}" stroke="{t["grid"]}" stroke-width="1"/>')
        out.append(f'<text x="{L - 10}" y="{y + 4:.1f}" font-size="12" text-anchor="end" fill="{t["muted"]}">{v}</text>')
    out.append(f'<text x="14" y="{T + ph / 2:.1f}" font-size="12" fill="{t["muted"]}" '
               f'transform="rotate(-90 14 {T + ph / 2:.1f})" text-anchor="middle">fps</text>')
    # 120 Hz reference
    y120 = ys(120)
    out.append(f'<line x1="{L}" x2="{W - R}" y1="{y120:.1f}" y2="{y120:.1f}" stroke="{t["muted"]}" '
               f'stroke-width="1.5" stroke-dasharray="5 4"/>')
    out.append(f'<text x="{W - R}" y="{y120 - 6:.1f}" font-size="12" text-anchor="end" fill="{t["muted"]}">'
               f'120 Hz display limit</text>')
    # bars
    gw = pw / len(SIZES)
    bw, gap = 64, 2
    for g, size in enumerate(SIZES):
        cx = L + gw * g + gw / 2
        x0 = cx - bw - gap / 2
        for i, (_, vals) in enumerate(SERIES):
            x = x0 + i * (bw + gap)
            ms = vals[g]
            if ms is None:
                for dy, txt in ((-44, "✕"), (-26, "did not"), (-10, "finish")):
                    out.append(f'<text x="{x + bw / 2:.1f}" y="{ys(0) + dy:.1f}" font-size="12" '
                               f'text-anchor="middle" fill="{t["muted"]}">{txt}</text>')
                continue
            fps = 1000.0 / ms
            y, h = ys(fps), ys(0) - ys(fps)
            out.append(f'<path d="{bar_path(x, y, bw, max(h, 2))}" fill="{t["colors"][i]}">'
                       f'<title>{SERIES[i][0]}, {size}: {fps:.0f} fps (p95 frame {ms:g} ms)</title></path>')
            label = f"{fps:.0f} fps" if fps >= 10 else f"{fps:.1f} fps"
            inside = h > 40  # tall bars: label inside the top, white on the fill
            ly, ink = (y + 20, "#ffffff") if inside else (y - 8, t["ink"])
            out.append(f'<text x="{x + bw / 2:.1f}" y="{ly:.1f}" font-size="13" font-weight="600" '
                       f'text-anchor="middle" fill="{ink}">{label}</text>')
        out.append(f'<text x="{cx:.1f}" y="{H - B + 24:.1f}" font-size="13" text-anchor="middle" '
                   f'fill="{t["ink"]}">{size}</text>')
    out.append(f'<line x1="{L}" x2="{W - R}" y1="{ys(0):.1f}" y2="{ys(0):.1f}" stroke="{t["muted"]}" stroke-width="1"/>')
    out.append('</svg>')
    return "\n".join(out)


if __name__ == "__main__":
    here = Path(__file__).parent
    for theme in THEMES:
        (here / f"benchmark-{theme}.svg").write_text(render(theme))
        print(f"wrote {here / f'benchmark-{theme}.svg'}")
