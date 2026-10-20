#!/usr/bin/env python3
"""Renders assets/hero/landing-race.gif (and .mp4 for X): 50 agents land 250 changes on one branch,
plain git vs gitai vs TreeYard, from the numbers in docs/benchmarks.md. The clock runs 132 s of the test
in RACE seconds; each bar fills while that server works and stops at its run time.

Needs macOS QuickLook (qlmanage) and ffmpeg.  python3 assets/hero/make_landing_gif.py [--scale 2]
"""
import os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
SCALE = float(sys.argv[sys.argv.index("--scale") + 1]) if "--scale" in sys.argv else 1.0
W, H, FPS, RACE, HOLD = 1200, 675, 20, 5.0, 3.5
END = 132.0  # seconds of the test the clock shows
ROWS = [  # name, run time (s), landed, p99, colour
    ("plain git", 132.0, "93 landed", "p99 30.2 s", "#545d68"),
    ("gitai", 22.2, "200 landed", "p99 5.9 s", "#8b7fd6"),
    ("TreeYard", 1.1, "197 landed", "p99 0.29 s", "#7fc496"),
]
BG, FG, MUTED, TRACK, TY = "#0d1117", "#e6edf3", "#8d96a0", "#21262d", "#7fc496"
FONT = '-apple-system, "SF Pro Display", Helvetica, Arial, sans-serif'
MONO = 'ui-monospace, Menlo, monospace'


def frame(t):
    clock = min(t / RACE, 1.0) * END
    x0, x1 = 230, 1000
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W // 2}" height="{H / 2}" viewBox="0 0 {W} {H}">',
           f'<rect width="{W}" height="{H}" fill="{BG}"/>',
           f'<text x="48" y="80" fill="{FG}" font-family=\'{FONT}\' font-size="44" font-weight="800">50 agents land 250 changes on one branch</text>',
           f'<text x="48" y="120" fill="{MUTED}" font-family=\'{FONT}\' font-size="24">Same machine · 20 % of the changes conflict on purpose</text>',
           f'<text x="1152" y="190" text-anchor="end" fill="{FG}" font-family="{MONO}" font-size="34" font-weight="700">{clock:6.1f} s</text>']
    for i, (name, run, landed, p99, color) in enumerate(ROWS):
        y = 230 + i * 120
        done = clock >= run
        frac = 1.0 if done else clock / run
        w = (x1 - x0) * frac
        bold = "800" if name == "TreeYard" else "700"
        out.append(f'<text x="48" y="{y + 40}" fill="{FG}" font-family=\'{FONT}\' font-size="32" font-weight="{bold}">{name}</text>')
        out.append(f'<rect x="{x0}" y="{y + 10}" width="{x1 - x0}" height="44" rx="8" fill="{TRACK}"/>')
        if w > 0.5:
            out.append(f'<rect x="{x0}" y="{y + 10}" width="{max(w, 8):.1f}" height="44" rx="8" fill="{color}"/>')
        if done:
            out.append(f'<text x="{x1 + 18}" y="{y + 44}" fill="{color if name == "TreeYard" else FG}" font-family="{MONO}" font-size="32" font-weight="800">{run:g} s</text>')
            out.append(f'<text x="{x0}" y="{y + 86}" fill="{MUTED}" font-family=\'{FONT}\' font-size="22">{landed} · {p99}</text>')
        else:
            out.append(f'<text x="{x0}" y="{y + 86}" fill="{MUTED}" font-family=\'{FONT}\' font-size="22">landing…</text>')
    if clock >= END:
        out.append(f'<text x="600" y="{H - 52}" text-anchor="middle" fill="{TY}" font-family=\'{FONT}\' font-size="46" font-weight="800">120× faster than git · 20× faster than gitai</text>')
    else:
        out.append(f'<text x="600" y="{H - 52}" text-anchor="middle" fill="{MUTED}" font-family=\'{FONT}\' font-size="24">TreeYard · open-source git server for agent swarms</text>')
    out.append('</svg>')
    return "\n".join(out)


def main():
    tmp = tempfile.mkdtemp(prefix="ty-landing-")
    n = int((RACE + HOLD) * FPS)
    paths = []
    for k in range(n):
        p = os.path.join(tmp, f"f{k:04d}.svg")
        open(p, "w").write(frame(k / FPS))
        paths.append(p)
    png = os.path.join(tmp, "png")
    os.makedirs(png)
    # QuickLook crops an SVG wider than about 768 px, so frames declare half size and render at 4x.
    for i in range(0, len(paths), 20):
        subprocess.run(["qlmanage", "-t", "-s", str(2 * W), "-o", png, *paths[i:i + 20]], capture_output=True, timeout=120)
    ow, oh = int(W * SCALE) // 2 * 2, int(H * SCALE) // 2 * 2
    pat = os.path.join(png, "f%04d.svg.png")
    gif = os.path.join(HERE, "landing-race.gif")
    vf = f"crop={2 * W}:{2 * H}:0:0,scale={ow}:{oh}:flags=lanczos,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=none"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", pat, "-vf", vf, "-loop", "0", gif], check=True)
    mp4 = os.path.join(HERE, "landing-race.mp4")
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", pat, "-vf",
                    f"crop={2 * W}:{2 * H}:0:0,scale={ow}:{oh}:flags=lanczos,format=yuv420p", "-c:v", "libx264",
                    "-crf", "18", "-movflags", "+faststart", mp4], check=True)
    shutil.rmtree(tmp)
    for f in (gif, mp4):
        print(f, os.path.getsize(f) // 1024, "KB")


if __name__ == "__main__":
    main()
