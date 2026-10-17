#!/usr/bin/env python3
"""Draws the README hero cards (one animated SVG per result) from the numbers in docs/benchmarks.md.

    python3 assets/hero/make.py
"""
import os

HERE = os.path.dirname(os.path.abspath(__file__))
W, H = 640, 280
CYCLE = 6.0  # seconds per loop
RUN = 0.7  # share of the loop the slow bar takes to fill

CARDS = [
    # file, big number, word, what, repo, git (value, label), treeyard (value, label)
    ("workspaces", "57×", "faster", "50 workspaces, ready to build", "ripgrep", (69.66, "69.7 s"), (1.22, "1.2 s")),
    ("disk", "161×", "less disk", "disk for 50 workspaces", "ripgrep", (7789, "7.8 GB"), (48.3, "48 MB")),
    ("astro", "13×", "faster", "50 workspaces + node_modules", "astro", (352.53, "353 s"), (26.19, "26 s")),
    ("blame", "43×", "faster", "blame a file", "ripgrep, warm", (24.76, "24.8 ms"), (0.58, "0.6 ms")),
    ("thrown-away", "111 → 0", "", "changes thrown away", "16 agents", (111, "111"), (0, "0")),
]

STYLE = """<style>
.bg{fill:#ffffff;stroke:#d5dcd6}.t{fill:#1c2420}.m{fill:#5b675f}.track{fill:#eef1ee}.git{fill:#9aa39c}.ty{fill:#2f6b45}.big{fill:#2f6b45}
text{font-family:-apple-system,"Segoe UI",Helvetica,Arial,sans-serif}
.n{font-size:92px;font-weight:800;letter-spacing:-2px}.w{font-size:34px;font-weight:700}.what{font-size:25px;font-weight:600}
.lab{font-size:22px;font-weight:700}.v{font-size:22px;font-weight:700;font-family:ui-monospace,Menlo,monospace}
@media (prefers-color-scheme: dark){.bg{fill:#161b22;stroke:#30363d}.t{fill:#e6edf3}.m{fill:#8d96a0}.track{fill:#21262d}.git{fill:#545d68}.ty{fill:#7fc496}.big{fill:#7fc496}}
</style>"""


HOLD = 0.25  # share of the loop that shows the final result before the race restarts


def bar(y, cls, label, frac, value_label, run):
    """A bar that shows its final width first (so a still image is complete), then races."""
    x0, x1 = 150, W - 150
    w = max((x1 - x0) * frac, 6)
    end = HOLD + 0.002 + max(run, 0.004)
    keys = f"0;{HOLD:.3f};{HOLD + 0.001:.3f};{end:.3f};1"
    return f"""<text class="t lab" x="32" y="{y + 25}">{label}</text>
<rect class="track" x="{x0}" y="{y}" width="{x1 - x0}" height="34" rx="6"/>
<rect class="{cls}" x="{x0}" y="{y}" width="{w:.1f}" height="34" rx="6">
  <animate attributeName="width" values="{w:.1f};{w:.1f};0;{w:.1f};{w:.1f}" keyTimes="{keys}" dur="{CYCLE}s" repeatCount="indefinite"/>
</rect>
<text class="t v" x="{x0 + w + 12:.1f}" y="{y + 25}">{value_label}
  <animate attributeName="opacity" values="1;1;0;0;1" keyTimes="{keys}" dur="{CYCLE}s" repeatCount="indefinite"/>
</text>"""


for name, big, word, what, repo, (g, gl), (ty, tl) in CARDS:
    frac_ty = ty / g
    svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-label="{big} {word}: {what}. git {gl}, TreeYard {tl}.">
{STYLE}
<rect class="bg" x="1" y="1" width="{W - 2}" height="{H - 2}" rx="18"/>
<text class="big" x="28" y="104"><tspan class="n">{big}</tspan><tspan class="w" dx="16">{word}</tspan></text>
<text class="m what" x="32" y="146">{what} · {repo}</text>
{bar(172, "git", "git", 1.0, gl, RUN)}
{bar(222, "ty", "TreeYard", frac_ty, tl, RUN * frac_ty)}
</svg>
"""
    open(os.path.join(HERE, f"{name}.svg"), "w").write(svg)
    print(name)
