#!/usr/bin/env python3
"""Renders assets/hero/treeyard-race.gif: git vs TreeYard bars racing, from the same numbers as the cards.

Needs macOS QuickLook (qlmanage) and ffmpeg.  python3 assets/hero/make_gif.py
"""
import os, shutil, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
W, H, FPS, RACE, HOLD = 1200, 675, 15, 3.0, 2.5
ROWS = [
    ("Spin up 50 workspaces", (69.66, "69.7 s"), (1.22, "1.2 s"), "57×", "faster"),
    ("Disk for 50 workspaces", (7789, "7.8 GB"), (48.3, "48 MB"), "161×", "less disk"),
    ("Blame a file", (24.76, "24.8 ms"), (0.58, "0.6 ms"), "43×", "faster"),
    ("Changes thrown away, 16 agents", (111, "111"), (0, "0"), "111→0", ""),
]
BG, FG, MUTED, TRACK, GIT, TY = "#0d1117", "#e6edf3", "#8d96a0", "#21262d", "#545d68", "#7fc496"
FONT = '-apple-system, "SF Pro Display", Helvetica, Arial, sans-serif'
MONO = 'ui-monospace, Menlo, monospace'


def frame(t):
    """t in seconds from the start."""
    p = min(t / RACE, 1.0)
    x0, x1 = 48, 800
    # QuickLook draws an SVG wider than about 768 px in a fixed window and crops it, so frames
    # declare half size and are rendered at 4x.
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W // 2}" height="{H / 2}" viewBox="0 0 {W} {H}">',
           f'<rect width="{W}" height="{H}" fill="{BG}"/>',
           f'<text x="48" y="78" fill="{FG}" font-family=\'{FONT}\' font-size="46" font-weight="800">TreeYard vs plain git worktrees</text>',
           f'<text x="48" y="118" fill="{MUTED}" font-family=\'{FONT}\' font-size="24">50 AI coding agents on one repo · measured on real repositories</text>']
    for i, (label, (g, gl), (ty, tl), big, word) in enumerate(ROWS):
        y = 158 + i * 120
        out.append(f'<text x="48" y="{y + 18}" fill="{FG}" font-family=\'{FONT}\' font-size="26" font-weight="700">{label}</text>')
        for j, (val, lab, color, name) in enumerate([(g, gl, GIT, "git"), (ty, tl, TY, "TreeYard")]):
            by = y + 32 + j * 38
            frac = val / g
            done = p >= frac
            w = (x1 - x0) * min(p, frac)
            out.append(f'<rect x="{x0}" y="{by}" width="{x1 - x0}" height="30" rx="6" fill="{TRACK}"/>')
            if w > 0.5:
                out.append(f'<rect x="{x0}" y="{by}" width="{max(w, 6):.1f}" height="30" rx="6" fill="{color}"/>')
            if done:
                tx = x0 + max(w, 6) + 12
                out.append(f'<text x="{tx:.1f}" y="{by + 23}" fill="{FG}" font-family="{MONO}" font-size="22" font-weight="700">{name} {lab}</text>')
            elif j == 0 and w > 140:
                out.append(f'<text x="{x0 + 12}" y="{by + 23}" fill="{BG}" font-family="{MONO}" font-size="20" font-weight="700">git…</text>')
        if p >= 1.0:
            out.append(f'<text x="1152" y="{y + 82}" text-anchor="end" fill="{TY}" font-family=\'{FONT}\' font-size="66" font-weight="800">{big}</text>')
            if word:
                out.append(f'<text x="1152" y="{y + 108}" text-anchor="end" fill="{MUTED}" font-family=\'{FONT}\' font-size="22">{word}</text>')
    out.append(f'<text x="48" y="{H - 28}" fill="{MUTED}" font-family=\'{FONT}\' font-size="22">TreeYard · git for agent swarms · 15-core CPU, 48 GB RAM</text>')
    out.append('</svg>')
    return "\n".join(out)


def main():
    tmp = tempfile.mkdtemp(prefix="ty-gif-")
    n = int((RACE + HOLD) * FPS)
    paths = []
    for k in range(n):
        p = os.path.join(tmp, f"f{k:04d}.svg")
        open(p, "w").write(frame(k / FPS))
        paths.append(p)
    png = os.path.join(tmp, "png")
    os.makedirs(png)
    for i in range(0, len(paths), 20):
        subprocess.run(["qlmanage", "-t", "-s", str(2 * W), "-o", png, *paths[i:i + 20]], capture_output=True, timeout=120)
    out = os.path.join(HERE, "treeyard-race.gif")
    pat = os.path.join(png, "f%04d.svg.png")
    vf = f"crop={2 * W}:{2 * H}:0:0,scale={W}:{H}:flags=lanczos,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=none"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", pat, "-vf", vf, "-loop", "0", out], check=True)
    shutil.rmtree(tmp)
    print(out, os.path.getsize(out) // 1024, "KB,", n, "frames")


if __name__ == "__main__":
    main()
