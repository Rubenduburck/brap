#!/usr/bin/env python3
"""Renders the `overhead` Criterion results as benches/overhead.svg for the
README, and prints the README table.

    cargo bench --bench overhead && python3 benches/plot.py
"""

import html
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
RESULTS = ROOT / "target" / "criterion" / "overhead"
OUTPUT = ROOT / "benches" / "overhead.svg"

BASELINE = "baseline"
# Operation order, top to bottom: the clock, then explicit records, then
# whole-function measurement.
OPERATIONS = [
    "stamp!",
    "count!",
    "count! tls",
    "record_log2!",
    "record_log2! tls",
    "measure",
    "measure_fine",
    "measure tls",
    "measure_fine tls",
]
PLOTTED_THREADS = 16

WIDTH, LABEL_WIDTH, VALUE_WIDTH = 640, 160, 72
TOP, ROW, BAR = 40, 30, 16


def time_ns(name, threads):
    """Criterion's headline estimate: the slope, or the mean without one."""
    path = RESULTS / name / str(threads) / "new" / "estimates.json"
    estimates = json.loads(path.read_text())
    return (estimates.get("slope") or estimates["mean"])["point_estimate"]


def net_ns(name, threads):
    """Cost of the operation alone, without the call it is attached to."""
    return max(time_ns(name, threads) - time_ns(BASELINE, threads), 0.0)


def format_ns(value):
    """Differences below the benchmark's resolution read as such, not as 0."""
    return "< 0.1 ns" if value < 0.05 else f"{value:.1f} ns"


def bar_path(x, y, length):
    """A bar square at the baseline with a 4px rounded data end."""
    radius = min(4, length / 2)
    return (
        f"M{x},{y}h{length - radius:.1f}"
        f"a{radius},{radius} 0 0 1 {radius},{radius}v{BAR - 2 * radius}"
        f"a{radius},{radius} 0 0 1 -{radius},{radius}h-{length - radius:.1f}z"
    )


def render(results):
    height = TOP + ROW * len(results) + 12
    scale = (WIDTH - LABEL_WIDTH - VALUE_WIDTH) / max(value for _, value in results)
    rows = []
    for index, (name, value) in enumerate(results):
        y = TOP + index * ROW
        length = max(value * scale, 2)
        middle = y + BAR / 2
        rows.append(
            f'<text class="label" x="{LABEL_WIDTH - 12}" y="{middle}">{name}</text>'
            f'<path class="bar" d="{bar_path(LABEL_WIDTH, y, length)}"/>'
            f'<text class="value" x="{LABEL_WIDTH + length + 6:.1f}" y="{middle}">{html.escape(format_ns(value))}</text>'
        )
    title = f"Cost per measurement, {PLOTTED_THREADS} threads recording at once (ns)"
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}" role="img" aria-label="{title}">
<style>
  .surface {{ fill: #fcfcfb; }}
  .title, .value {{ fill: #0b0b0b; }}
  .label {{ fill: #52514e; }}
  .bar {{ fill: #2a78d6; }}
  .axis {{ stroke: #c3c2b7; stroke-width: 1; }}
  @media (prefers-color-scheme: dark) {{
    .surface {{ fill: #1a1a19; }}
    .title, .value {{ fill: #ffffff; }}
    .label {{ fill: #c3c2b7; }}
    .bar {{ fill: #3987e5; }}
    .axis {{ stroke: #52514e; }}
  }}
  text {{ font: 13px ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; dominant-baseline: central; }}
  .title {{ font-weight: 600; dominant-baseline: auto; }}
  .label {{ text-anchor: end; font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 12px; }}
  .value {{ font-variant-numeric: tabular-nums; }}
</style>
<rect class="surface" width="100%" height="100%" rx="6"/>
<text class="title" x="16" y="24">{title}</text>
<line class="axis" x1="{LABEL_WIDTH}" x2="{LABEL_WIDTH}" y1="{TOP - 6}" y2="{height - 12}"/>
{chr(10).join(rows)}
</svg>
"""


def main():
    plotted = [(name, net_ns(name, PLOTTED_THREADS)) for name in OPERATIONS]
    OUTPUT.write_text(render(plotted))
    print(f"| Operation | 1 thread | {PLOTTED_THREADS} threads |")
    print("|---|---|---|")
    for name, value in plotted:
        print(f"| `{name}` | {format_ns(net_ns(name, 1))} | {format_ns(value)} |")


if __name__ == "__main__":
    main()
