#!/usr/bin/env python3
"""Renders a terminal transcript as an SVG terminal window, with lines that appear one by one.

    python3 scripts/demo-svg.py assets/demo.txt > assets/demo.svg

The input is plain text: a line that starts with "$ " is a command (it types in), other lines are
output. Lines appear in order; with prefers-reduced-motion the image is static. The output depends
only on the input (tests/docs.rs regenerates it and compares). Standard library only.
"""

import html
import sys

CHAR_W = 8.4  # px per character at 13px monospace, with room for wider fonts
LINE_H = 19
PAD = 18
TOP = 40


def render(lines):
    width = int(max(len(line) for line in lines) * CHAR_W + 2 * PAD)
    width = max(width, 640)
    height = TOP + LINE_H * len(lines) + PAD
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}" '
        'role="img" aria-label="Terminal: examples/swarm-demo.sh output">',
        "<style>",
        "text{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:13px;white-space:pre}",
        ".bg{fill:#0d1117}.bar{fill:#161b22}.o{fill:#c9d1d9}.c{fill:#7fc496}.p{fill:#d99a1e}.h{fill:#e6edf3;font-weight:700}",
        # Visible by default: a renderer without CSS animation shows the whole transcript.
        ".l{animation:show .01s linear both}",
        "@keyframes show{from{opacity:0}to{opacity:1}}",
        "@media (prefers-reduced-motion: reduce){.l{animation:none}}",
        "</style>",
        f'<rect class="bg" width="{width}" height="{height}" rx="10"/>',
        f'<rect class="bar" width="{width}" height="28" rx="10"/><rect class="bar" y="18" width="{width}" height="10"/>',
        '<circle cx="18" cy="14" r="5.5" fill="#ff5f56"/><circle cx="36" cy="14" r="5.5" fill="#ffbd2e"/>'
        '<circle cx="54" cy="14" r="5.5" fill="#27c93f"/>',
    ]
    t = 0.3
    for n, line in enumerate(lines):
        y = TOP + LINE_H * n + 6
        if line.startswith("$ "):
            delay, t = t, t + 0.9
            body = f'<tspan class="p">$ </tspan><tspan class="c">{html.escape(line[2:])}</tspan>'
        else:
            delay, t = t, t + 0.12
            cls = "h" if line.startswith("| Step") or line.startswith("swarm-demo:") else "o"
            body = f'<tspan class="{cls}">{html.escape(line)}</tspan>'
        out.append(f'<text class="l" x="{PAD}" y="{y}" style="animation-delay:{delay:.2f}s">{body}</text>')
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as fh:
        lines = fh.read().rstrip("\n").split("\n")
    sys.stdout.write(render(lines))


if __name__ == "__main__":
    main()
