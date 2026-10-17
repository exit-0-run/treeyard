#!/usr/bin/env python3
"""Phase-0 baseline: what plain git worktrees cost a swarm of N agents, next to Rootstock.

Stdlib only. Every subcommand prints one JSON document and also writes it to <out>/<name>.json.

  workspaces  time and disk to make N ready workspaces (git worktree add + dependency setup)
  conflicts   late conflicts: real history replayed in waves of N agents, merged with git merge-tree
  queue       a serial merge queue: N changes arrive at once, each runs the repo's CI in turn
  rootstock   the same agents on the Rootstock API: init, N agents open a change and read, replay
  cow         Rootstock workspaces: N copy-on-write workspaces with a shared dependency layer

Sources are pinned (REPOS). `fetch` clones them into <oss> when missing; worktrees are made from
a local clone in <dir>/src/<repo>, so the clones in <oss> stay untouched.
"""

from __future__ import annotations

import argparse
import collections
import concurrent.futures as cf
import json
import os
import platform
import random
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPOS = {
    "ripgrep": {
        "url": "https://github.com/BurntSushi/ripgrep",
        "rev": "3fce3b5bb0236da2df6d99672afb8a719642eca7",
        "branch": "master",
        "seed": ["cargo build -q"],
        "setups": {
            "checkout": [],
            "cargo-cold": ["cargo build --offline -q"],
            "cargo-warm-clone": ["@clone target", "cargo build --offline -q"],
        },
        # The queue starts 50 commits back, so CI may fetch crates the seed lockfile lacks.
        "ci": "cargo build -q && cargo test -q",
        "ci_setup": "checkout",
        "cow": {"layer": "target=Cargo.lock", "setup": "cargo build --offline -q"},
    },
    "requests": {
        "url": "https://github.com/psf/requests",
        "rev": "611c6162cbc4ac2020a2f91c7cfa4f3abf9bbb60",
        "branch": "main",
        "seed": ["python3 -m venv .venv", ".venv/bin/pip install -q -r requirements-dev.txt"],
        "setups": {
            "checkout": [],
            "pip": ["python3 -m venv .venv", ".venv/bin/pip install -q -r requirements-dev.txt"],
            "uv": ["uv venv -q .venv", "uv pip install -q --python .venv/bin/python -r requirements-dev.txt"],
        },
        "ci": ".venv/bin/python -m pytest -q -x -p no:cacheprovider tests",
        "ci_setup": "uv",
        # uv's .venv holds absolute paths (shebangs, the editable install), so it is no layer.
        "cow": {"layer": None, "setup": "uv venv -q .venv && uv pip install -q --python .venv/bin/python -r requirements-dev.txt"},
    },
    "astro": {
        "url": "https://github.com/withastro/astro",
        "rev": "c75e89711a4935a0befea6f768842f6d6a9de2bb",
        "branch": "main",
        "seed": ["pnpm install --frozen-lockfile"],
        "setups": {
            "checkout": [],
            "pnpm-copy": ["pnpm install --offline --frozen-lockfile --config.package-import-method=copy"],
            "pnpm-store": ["pnpm install --offline --frozen-lockfile"],
        },
        "ci": "pnpm install --frozen-lockfile --prefer-offline && pnpm run build:ci:no-cache"
              " && pnpm -C packages/astro run test:unit",
        "ci_setup": "checkout",
        "cow": {"layer": "node_modules=pnpm-lock.yaml", "setup": "pnpm install --offline --frozen-lockfile"},
    },
}

LOCKFILES = {"Cargo.lock", "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "poetry.lock", "uv.lock"}


def sh(cmd, cwd=None, check=True, env=None, capture=True):
    """Runs a command (list = exec, str = shell); returns stdout."""
    p = subprocess.run(
        cmd, cwd=cwd, shell=isinstance(cmd, str), env=env, text=True,
        stdout=subprocess.PIPE if capture else None, stderr=subprocess.PIPE if capture else None,
    )
    if check and p.returncode != 0:
        raise RuntimeError(f"{cmd!r} in {cwd} failed ({p.returncode}): {(p.stderr or '')[-2000:]}")
    return p.stdout or ""


def git(repo, *args, check=True):
    return sh(["git", "-C", str(repo), *args], check=check).strip()


def used_bytes(path):
    s = os.statvfs(path)
    return (s.f_blocks - s.f_bfree) * s.f_frsize


def du_kb(paths):
    """Allocated KB per path, plus the total of one `du` over all (hard links counted once)."""
    paths = [str(p) for p in paths]
    if not paths:
        return [], 0
    each = [int(sh(["du", "-sk", p]).split()[0]) for p in paths]
    total = int(sh(["du", "-skc", *paths]).strip().splitlines()[-1].split()[0])
    return each, total


def pct(xs, p):
    xs = sorted(xs)
    return xs[round((len(xs) - 1) * p)] if xs else 0.0


def summary(xs):
    xs = list(xs)
    if not xs:
        return {}
    return {"n": len(xs), "p50": round(pct(xs, 0.5), 4), "p95": round(pct(xs, 0.95), 4),
            "max": round(max(xs), 4), "mean": round(statistics.fmean(xs), 4)}


def machine():
    info = {"os": platform.platform(), "python": platform.python_version(), "cpus": os.cpu_count()}
    if sys.platform == "darwin":
        info["cpu"] = sh(["sysctl", "-n", "machdep.cpu.brand_string"]).strip()
        info["mem_gb"] = int(sh(["sysctl", "-n", "hw.memsize"])) // 2**30
    info["git"] = sh(["git", "--version"]).strip()
    return info


def emit(args, name, doc):
    doc = {"bench": name, "machine": machine(), "argv": sys.argv[1:], **doc}
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{name}.json").write_text(json.dumps(doc, indent=2) + "\n")
    print(json.dumps(doc, indent=2))


def source(args, name):
    """A local clone of the pinned repo, made from <oss>/<name> (cloned from the URL if missing)."""
    spec = REPOS[name]
    oss = Path(args.oss) / name
    if not oss.exists():
        sh(["git", "clone", "-q", spec["url"], str(oss)])
    src = Path(args.dir) / "src" / name
    if not src.exists():
        src.parent.mkdir(parents=True, exist_ok=True)
        sh(["git", "clone", "-q", "--local", "--no-checkout", str(oss), str(src)])
        git(src, "branch", "-f", "bench-base", spec["rev"])
    return src, spec


def seed(args, name):
    """A worktree with dependencies installed once (warms the caches; reused by warm setups)."""
    src, spec = source(args, name)
    s = Path(args.dir) / "seed" / name
    if not (s / ".rootstock-bench-seeded").exists():
        if s.exists():
            shutil.rmtree(s)
            git(src, "worktree", "prune")
        s.parent.mkdir(parents=True, exist_ok=True)
        git(src, "worktree", "add", "-q", "--detach", str(s), spec["rev"])
        t = time.monotonic()
        for step in spec["seed"]:
            sh(step, cwd=s)
        (s / ".rootstock-bench-seeded").write_text(f"{time.monotonic() - t:.2f}\n")
    return s, float((s / ".rootstock-bench-seeded").read_text())


def run_step(step, ws, seed_dir):
    if step.startswith("@clone "):
        rel = step.split(" ", 1)[1]
        cp = ["cp", "-c", "-R", "-p"] if sys.platform == "darwin" else ["cp", "-R", "-p", "--reflink=auto"]
        sh([*cp, str(seed_dir / rel), str(ws / rel)])
    else:
        sh(step, cwd=ws)


def make_workspace(src, rev, ws, steps, seed_dir):
    t0 = time.monotonic()
    git(src, "worktree", "add", "-q", "--detach", str(ws), rev)
    t1 = time.monotonic()
    for step in steps:
        run_step(step, ws, seed_dir)
    t2 = time.monotonic()
    return {"git_s": t1 - t0, "deps_s": t2 - t1, "total_s": t2 - t0}


def remove_workspaces(src, root):
    if root.exists():
        shutil.rmtree(root)
    git(src, "worktree", "prune")


def cmd_workspaces(args):
    src, spec = source(args, args.repo)
    setups = args.setup.split(",") if args.setup else list(spec["setups"])
    seed_dir, seed_s = seed(args, args.repo) if any(spec["setups"][s] for s in setups) else (None, 0.0)
    runs = []
    for setup in setups:
        steps = spec["setups"][setup]
        for n in [int(x) for x in args.n.split(",")]:
            root = Path(args.dir) / "ws" / f"{args.repo}-{setup}-{n}"
            remove_workspaces(src, root)
            root.mkdir(parents=True)
            before = used_bytes(root)
            t = time.monotonic()
            with cf.ThreadPoolExecutor(max_workers=min(n, args.jobs)) as pool:
                res = list(pool.map(lambda i: make_workspace(src, spec["rev"], root / f"a{i:02}", steps, seed_dir), range(n)))
            wall = time.monotonic() - t
            physical = used_bytes(root) - before
            each, dedup = du_kb(sorted(root.iterdir()))
            run = {
                "repo": args.repo, "setup": setup, "steps": steps, "n": n, "jobs": min(n, args.jobs),
                "wall_s": round(wall, 2), "per_workspace_s": summary(r["total_s"] for r in res),
                "git_s": summary(r["git_s"] for r in res), "deps_s": summary(r["deps_s"] for r in res),
                "du_per_workspace_mb": round(statistics.fmean(each) / 1024, 1),
                "du_total_kb": sum(each), "du_total_mb": round(sum(each) / 1024, 1),
                "du_dedup_total_mb": round(dedup / 1024, 1),
                "df_delta_mb": round(physical / 2**20, 1),
            }
            print(json.dumps(run), file=sys.stderr)
            runs.append(run)
            if not args.keep:
                remove_workspaces(src, root)
    emit(args, f"workspaces-{args.repo}", {"repo": args.repo, "rev": spec["rev"], "seed_s": seed_s, "runs": runs})


# ---- late conflicts --------------------------------------------------------------------------

DETERMINISTIC = {"GIT_AUTHOR_NAME": "bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                 "GIT_COMMITTER_NAME": "bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid",
                 "GIT_AUTHOR_DATE": "1700000000 +0000", "GIT_COMMITTER_DATE": "1700000000 +0000"}


class Git:
    """Plumbing on one repo: merge-tree, commit-tree and tree lookups, no checkout."""

    def __init__(self, repo):
        self.repo = str(repo)
        self.env = {**os.environ, **DETERMINISTIC}

    def run(self, *args, check=True):
        p = subprocess.run(["git", "-C", self.repo, "-c", "merge.renames=false", "-c", "diff.renames=false", *args],
                           text=True, capture_output=True, env=self.env)
        if check and p.returncode not in (0, 1):
            raise RuntimeError(f"git {args}: {p.stderr}")
        return p

    def tree(self, rev):
        return self.run("rev-parse", f"{rev}^{{tree}}").stdout.strip()

    def merge(self, base, ours, theirs, *extra):
        """(tree or None, conflicted paths) for the 3-way merge of theirs onto ours."""
        p = self.run("merge-tree", "--write-tree", "--name-only", "--no-messages", *extra,
                     f"--merge-base={base}", ours, theirs)
        lines = p.stdout.splitlines()
        if p.returncode == 0:
            return lines[0], []
        paths = []
        for line in lines[1:]:
            if not line:
                break
            paths.append(line)
        return None, paths

    def commit(self, tree, parent, msg):
        return self.run("commit-tree", tree, "-p", parent, "-m", msg).stdout.strip()

    def paths(self, commit):
        out = self.run("diff-tree", "--no-commit-id", "-r", "--name-only", f"{commit}^", commit).stdout
        return set(out.split())

    def lines(self, commit):
        out = self.run("diff-tree", "--no-commit-id", "-r", "--numstat", f"{commit}^", commit).stdout
        n = 0
        for row in out.splitlines():
            a, d, _ = row.split("\t", 2)
            n += (int(a) if a != "-" else 0) + (int(d) if d != "-" else 0)
        return n


def replay_git(g, commits, agents, seed):
    """Worktree model: each agent works on its commit's true parent, unaware of the others; the
    wave lands in random order through git; a conflict is seen only at merge time, thrown away and
    redone on the newer main. Mirrors `rootstock replay` (src/replay.rs), with git as the merger."""
    rng = random.Random(seed)
    parent = {c: p for p, c in zip(commits, commits[1:])}
    main = commits[0]
    n = collections.Counter({k: 0 for k in ("changes", "empty", "faithful_waves", "forced", "landed",
                                             "late_conflicts", "merges", "no_merge", "redone",
                                             "unresolved", "visible_before_merge", "waves")})
    conflicted_paths = collections.Counter()
    thrown_lines, total_lines = 0, 0
    first_divergence = None  # once main differs from history it rarely comes back
    for start in range(1, len(commits), agents):
        touched = {c: g.paths(c) for c in commits[start:start + agents]}
        wave = [c for c in touched if touched[c]]
        n["waves"] += 1
        n["empty"] += len(touched) - len(wave)
        n["changes"] += len(wave)
        order = wave[:]
        rng.shuffle(order)

        def land(c):
            nonlocal main
            p = parent[c]
            if p == main:
                tree, paths = g.tree(c), []
                n["no_merge"] += 1
            else:
                n["merges"] += 1
                tree, paths = g.merge(p, main, c)
            if tree is None:
                return paths
            main = g.commit(tree, main, f"land {c[:12]}")
            n["landed"] += 1
            return []

        deferred = []
        for c in order:
            lines = g.lines(c)
            total_lines += lines
            paths = land(c)
            if paths:
                n["late_conflicts"] += 1
                thrown_lines += lines
                conflicted_paths.update(paths)
                if any(touched[c] & touched[o] for o in wave if o != c):
                    n["visible_before_merge"] += 1
                deferred.append(c)
        deferred.sort(key=commits.index)
        while deferred:
            still, progress = [], False
            for c in deferred:
                n["redone"] += 1
                if land(c):
                    still.append(c)
                else:
                    progress = True
            if still and not progress:
                c = still.pop(0)
                tree, _ = g.merge(parent[c], main, c, "-Xtheirs")
                if tree is None:
                    n["unresolved"] += 1
                else:
                    main = g.commit(tree, main, f"forced {c[:12]}")
                    n["forced"] += 1
                    n["landed"] += 1
            deferred = still
        if g.tree(main) == g.tree(commits[min(start + agents, len(commits)) - 1]):
            n["faithful_waves"] += 1
        elif first_divergence is None:
            first_divergence = n["waves"]
    lock = sum(v for k, v in conflicted_paths.items() if k.rsplit("/", 1)[-1] in LOCKFILES)
    return {
        "agents": agents, "seed": seed, **dict(sorted(n.items())),
        "late_conflict_rate": round(n["late_conflicts"] / max(1, n["changes"]), 4),
        "lines_in_conflicted_changes": thrown_lines, "lines_total": total_lines,
        "thrown_away_share": round(thrown_lines / max(1, total_lines), 4),
        "final_tree_equal": g.tree(main) == g.tree(commits[-1]), "first_divergent_wave": first_divergence,
        "conflicted_path_hits": sum(conflicted_paths.values()), "lockfile_path_hits": lock,
        "top_conflicted_paths": conflicted_paths.most_common(5),
    }


def history(g, rev, count):
    out = g.run("rev-list", "--first-parent", "-n", str(count + 1), rev).stdout.split()
    return out[::-1]


def cmd_conflicts(args):
    src, spec = source(args, args.repo)
    g = Git(src)
    commits = history(g, spec["rev"], args.commits)
    runs = []
    for agents in [int(x) for x in args.agents.split(",")]:
        for seed in [int(x) for x in args.seeds.split(",")]:
            t = time.monotonic()
            r = replay_git(g, commits, agents, seed)
            r["seconds"] = round(time.monotonic() - t, 2)
            print(json.dumps({k: r[k] for k in ("agents", "seed", "late_conflicts", "faithful_waves", "waves")}),
                  file=sys.stderr)
            runs.append(r)
    emit(args, f"conflicts-{args.repo}", {"repo": args.repo, "rev": spec["rev"], "from": commits[0],
                                          "commits": len(commits) - 1, "runs": runs})


# ---- serial merge queue ----------------------------------------------------------------------

def cmd_queue(args):
    """N changes enter the queue at t=0. The queue lands them one at a time: check out main plus
    the change (history order, so main + change equals the original commit), run CI, land. A
    change waits for every change ahead of it. One queue of max(N) runs; a wave of n < max(N) is
    its first n changes, so smaller N are prefixes of the same run."""
    src, spec = source(args, args.repo)
    g = Git(src)
    seed_dir, _ = seed(args, args.repo)
    ws = Path(args.dir) / "queue" / args.repo
    remove_workspaces(src, ws)
    ws.parent.mkdir(parents=True, exist_ok=True)
    ns = [int(x) for x in args.n.split(",")]
    commits = history(g, spec["rev"], max(ns))
    make_workspace(src, commits[0], ws, spec["setups"][spec["ci_setup"]], seed_dir)
    t = time.monotonic()
    sh(spec["ci"], cwd=ws, check=False)
    warm_s = time.monotonic() - t
    t0, ci, starts, done, ok = time.monotonic(), [], [], [], []
    for c in commits[1:]:
        starts.append(time.monotonic() - t0)
        git(ws, "checkout", "-q", "--detach", c)
        t = time.monotonic()
        ok.append(subprocess.run(spec["ci"], cwd=ws, shell=True, capture_output=True).returncode == 0)
        ci.append(time.monotonic() - t)
        done.append(time.monotonic() - t0)
        print(json.dumps({"k": len(ci), "ci_s": round(ci[-1], 2), "ok": ok[-1]}), file=sys.stderr)
    runs = []
    for n in ns:
        runs.append({"n": n, "ci_s": summary(ci[:n]), "wait_s": summary(starts[:n]),
                     "time_to_land_s": summary(done[:n]), "queue_drain_s": round(done[n - 1], 2),
                     "landings_per_hour": round(3600 * n / done[n - 1], 1), "ci_failed": n - sum(ok[:n])})
    remove_workspaces(src, ws)
    emit(args, f"queue-{args.repo}", {"repo": args.repo, "rev": spec["rev"], "ci": spec["ci"],
                                      "ci_setup": spec["ci_setup"], "first_ci_run_s": round(warm_s, 2),
                                      "from": commits[0], "ci_each_s": [round(x, 2) for x in ci], "runs": runs})


# ---- rootstock -------------------------------------------------------------------------------

def post(url, op, body):
    req = urllib.request.Request(f"{url}/api/{op}", data=json.dumps(body).encode(),
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read())


def exists(url, path):
    try:
        post(url, "read", {"path": path})
        return True
    except urllib.error.HTTPError:
        return False


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def rss_mb(pid):
    return round(int(sh(["ps", "-o", "rss=", "-p", str(pid)]).strip() or 0) / 1024, 1)


def cmd_rootstock(args):
    src, spec = source(args, args.repo)
    binary = Path(args.bin).resolve()
    data = Path(args.dir) / "rs" / args.repo
    if data.exists():
        shutil.rmtree(data)
    data.parent.mkdir(parents=True, exist_ok=True)
    t = time.monotonic()
    sh([str(binary), "init", "--data", str(data), "--from", str(src), "--branch", "bench-base"])
    init_s = time.monotonic() - t
    _, init_kb = du_kb([data])
    port = free_port()
    url = f"http://127.0.0.1:{port}"
    proc = subprocess.Popen([str(binary), "serve", "--data", str(data), "--addr", f"127.0.0.1:{port}"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    result = {"repo": args.repo, "rev": spec["rev"], "init_s": round(init_s, 2), "data_mb": round(init_kb / 1024, 1)}
    try:
        t = time.monotonic()
        while True:
            try:
                urllib.request.urlopen(f"{url}/health", timeout=1).read()
                break
            except OSError:
                if time.monotonic() - t > 120:
                    raise
                time.sleep(0.05)
        result["serve_ready_s"] = round(time.monotonic() - t, 2)
        files = post(url, "status", {})["files"]
        result["files"] = files
        readme = next(p for p in ("README.md", "README.rst", "README") if exists(url, p))
        runs = []
        for n in [int(x) for x in args.n.split(",")]:
            before = used_bytes(data)

            def agent(i):
                t0 = time.monotonic()
                ch = post(url, "open_change", {"agent": f"agent-{i:02}", "intent": f"bench {n}/{i}"})
                t1 = time.monotonic()
                post(url, "read", {"path": readme, "at": ch["id"]})
                post(url, "grep", {"pattern": "TODO", "at": ch["id"], "max": 20})
                return ch["id"], t1 - t0, time.monotonic() - t0

            t = time.monotonic()
            with cf.ThreadPoolExecutor(max_workers=min(n, args.jobs)) as pool:
                res = list(pool.map(agent, range(n)))
            wall = time.monotonic() - t
            delta = used_bytes(data) - before
            for cid, _, _ in res:
                post(url, "abandon", {"change": cid})
            run = {"n": n, "wall_s": round(wall, 3), "open_change_s": summary(r[1] for r in res),
                   "ready_s": summary(r[2] for r in res), "df_delta_mb": round(delta / 2**20, 2),
                   "server_rss_mb": rss_mb(proc.pid)}
            print(json.dumps(run), file=sys.stderr)
            runs.append(run)
        result["agents"] = runs
    finally:
        proc.send_signal(signal.SIGTERM)
        proc.wait()
    replays = []
    for agents in [int(x) for x in args.agents.split(",")] if args.agents else []:
        rdata = Path(args.dir) / "rs" / f"{args.repo}-replay-{agents}"
        if rdata.exists():
            shutil.rmtree(rdata)
        out = sh([str(binary), "replay", "--src", str(src), "--from", f"bench-base~{args.commits}",
                  "--to", "bench-base", "--agents", str(agents), "--data", str(rdata)])
        r = json.loads(out)
        r.pop("diverged", None)
        replays.append(r)
        print(json.dumps({k: r.get(k) for k in ("agents", "first_conflicts", "flagged_early", "land_ms")}),
              file=sys.stderr)
        shutil.rmtree(rdata)
    result["replay"] = replays
    emit(args, f"rootstock-{args.repo}", result)


# ---- rootstock workspaces --------------------------------------------------------------------

def serve(binary, data):
    port = free_port()
    url = f"http://127.0.0.1:{port}"
    proc = subprocess.Popen([str(binary), "serve", "--data", str(data), "--addr", f"127.0.0.1:{port}"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t = time.monotonic()
    while True:
        try:
            urllib.request.urlopen(f"{url}/health", timeout=1).read()
            return proc, url
        except OSError:
            if time.monotonic() - t > 120:
                proc.kill()
                raise
            time.sleep(0.05)


def cmd_cow(args):
    """N Rootstock workspaces next to the worktree baseline: the first one builds the dependency
    layer (not counted), then N more are made at once (at most --jobs), with the setup command
    (`build`: ready to build and test) and without it (`files`: files plus the layer clone).
    Each new workspace then edits one tracked file and syncs it to the server."""
    src, spec = source(args, args.repo)
    cow = spec["cow"]
    binary = Path(args.bin).resolve()
    root = Path(args.dir) / "cow" / args.repo
    if root.exists():
        sh(["chmod", "-R", "u+w", str(root)], check=False)
        shutil.rmtree(root)
    root.mkdir(parents=True)
    data, home = root / "data", root / "node"
    sh([str(binary), "init", "--data", str(data), "--from", str(src), "--branch", "bench-base"])
    proc, url = serve(binary, data)
    env = {**os.environ, "ROOTSTOCK_URL": url, "ROOTSTOCK_HOME": str(home)}
    layer = ["--layer", cow["layer"]] if cow.get("layer") else ["--no-layers"]

    def create(name, setup):
        t = time.monotonic()
        argv = [str(binary), "workspace", "create", name, "--agent", name, *layer]
        if setup:
            argv += ["--setup", cow["setup"]]
        out = json.loads(sh(argv, env=env))
        wall = time.monotonic() - t
        ws = Path(out["dir"])
        target = next(p for p in ("README.md", "README.rst", "README") if (ws / p).exists())
        with open(ws / target, "a") as f:
            f.write(f"\nedited by {name}\n")
        t = time.monotonic()
        synced = json.loads(sh([str(binary), "workspace", "sync", name], env=env))
        sync_s = time.monotonic() - t
        assert synced["paths"] == [target], synced
        return {"wall_s": wall, "sync_s": sync_s, **out["timing"], "layers": out["layers"]}

    result = {"repo": args.repo, "rev": spec["rev"], "layer": cow.get("layer"), "setup": cow["setup"]}
    try:
        t = time.monotonic()
        first = create("first", True)
        result["first"] = {"wall_s": round(time.monotonic() - t, 2), **first}
        _, base_kb = du_kb(sorted(p for p in (home / "bases").iterdir() if p.is_dir()))
        result["base_du_mb"] = round(base_kb / 1024, 1)
        runs = []
        for mode in args.modes.split(","):
            for n in [int(x) for x in args.n.split(",")]:
                names = [f"{mode}-{n}-{i:02}" for i in range(n)]
                fill_s = None
                if mode.startswith("spare"):
                    # The warm pool (what `rootstock daemon --spares N` keeps): filled first, not counted.
                    t = time.monotonic()
                    sh([str(binary), "workspace", "spare", "--count", str(n), *layer], env=env)
                    fill_s = round(time.monotonic() - t, 2)
                before = used_bytes(root)
                t = time.monotonic()
                with cf.ThreadPoolExecutor(max_workers=min(n, args.jobs)) as pool:
                    res = list(pool.map(lambda nm: create(nm, mode.endswith("build")), names))
                wall = time.monotonic() - t
                physical = used_bytes(root) - before
                each, dedup = du_kb([home / "ws" / nm for nm in names])
                run = {"repo": args.repo, "mode": mode, "n": n, "jobs": min(n, args.jobs), "wall_s": round(wall, 2),
                       "per_workspace_s": summary(r["wall_s"] for r in res),
                       "clone_s": summary(r["clone_s"] for r in res), "layers_s": summary(r["layers_s"] for r in res),
                       "setup_s": summary(r["setup_s"] for r in res), "sync_s": summary(r["sync_s"] for r in res),
                       "layer_restored": sum(1 for r in res if all(lay["restored"] for lay in r["layers"])),
                       "du_per_workspace_mb": round(statistics.fmean(each) / 1024, 1),
                       "du_total_mb": round(sum(each) / 1024, 1), "du_dedup_total_mb": round(dedup / 1024, 1),
                       "df_delta_mb": round(physical / 2**20, 1), "pool_fill_s": fill_s,
                       "from_spare": sum(1 for r in res if r.get("spare"))}
                if not args.keep:
                    t = time.monotonic()
                    with cf.ThreadPoolExecutor(max_workers=min(n, args.jobs)) as pool:
                        list(pool.map(lambda nm: sh([str(binary), "workspace", "remove", nm, "--abandon"], env=env), names))
                    run["remove_wall_s"] = round(time.monotonic() - t, 2)
                print(json.dumps(run), file=sys.stderr)
                runs.append(run)
        result["runs"] = runs
    finally:
        proc.send_signal(signal.SIGTERM)
        proc.wait()
    emit(args, f"cow-{args.repo}", result)


def cmd_history(args):
    """Cost of history queries an agent runs in its own checkout: path log and blame on the files
    the last <commits> commits touched most, plus one N-way burst (N agents ask at once)."""
    src, spec = source(args, args.repo)
    g = Git(src)
    commits = history(g, spec["rev"], args.commits)
    hits = collections.Counter()
    for c in commits[1:]:
        hits.update(g.paths(c))
    tip = set(git(src, "ls-tree", "-r", "--name-only", spec["rev"]).splitlines())
    files = [p for p, _ in hits.most_common() if p in tip][: args.files]

    def timed(cmd):
        t = time.monotonic()
        sh(cmd)
        return time.monotonic() - t

    log_s = [timed(["git", "-C", str(src), "log", "--format=%H", spec["rev"], "--", f]) for f in files]
    blame_s = [timed(["git", "-C", str(src), "blame", "--porcelain", spec["rev"], "--", f]) for f in files]
    bursts = []
    for n in [int(x) for x in args.n.split(",")]:
        jobs = [files[i % len(files)] for i in range(n)]
        t = time.monotonic()
        with cf.ThreadPoolExecutor(max_workers=n) as pool:
            each = list(pool.map(lambda f: timed(["git", "-C", str(src), "blame", "--porcelain", spec["rev"], "--", f]), jobs))
        bursts.append({"n": n, "wall_s": round(time.monotonic() - t, 3), "blame_s": summary(each)})
    emit(args, f"history-{args.repo}", {"repo": args.repo, "rev": spec["rev"], "files": files,
                                        "history_commits": int(git(src, "rev-list", "--count", spec["rev"])),
                                        "log_s": summary(log_s), "blame_s": summary(blame_s), "bursts": bursts})


def table(head, rows):
    out = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    out += ["| " + " | ".join(str(c) for c in row) + " |" for row in rows]
    return "\n".join(out)


def cmd_report(args):
    """Markdown tables from the result JSON files (the raw numbers in docs/benchmarks.md)."""
    res = {p.stem: json.loads(p.read_text()) for p in sorted(Path(args.out).glob("*.json"))}
    parts = []
    ws = [r for k, d in res.items() if k.startswith("workspaces-") for r in d["runs"]]
    if ws:
        parts.append("### Workspaces (git worktree add + setup)\n\n" + table(
            ["repo", "setup", "N", "jobs", "wall s", "per ws p50 s", "per ws p95 s", "git p50 s",
             "du per ws MB", "du sum MB", "du dedup MB", "df delta MB"],
            [[r["repo"], r["setup"], r["n"], r["jobs"], r["wall_s"], r["per_workspace_s"]["p50"],
              r["per_workspace_s"]["p95"], r["git_s"]["p50"], r["du_per_workspace_mb"], r["du_total_mb"],
              r["du_dedup_total_mb"], r["df_delta_mb"]] for r in ws]))
    rows = []
    for k, d in res.items():
        if k.startswith("conflicts-"):
            by = collections.defaultdict(list)
            for r in d["runs"]:
                by[r["agents"]].append(r)
            for agents, rs in sorted(by.items()):
                def m(key):
                    return "/".join(str(r[key]) for r in rs)
                rows.append([d["repo"], d["commits"], agents, len(rs), m("changes"), m("late_conflicts"),
                             m("visible_before_merge"), m("redone"), m("forced"), m("unresolved"),
                             m("thrown_away_share"), m("lockfile_path_hits"), m("faithful_waves") + " of " + str(rs[0]["waves"]),
                             "/".join(str(r.get("first_divergent_wave")) for r in rs), m("final_tree_equal")])
    if rows:
        parts.append("### Late conflicts (git merge-tree, one value per seed)\n\n" + table(
            ["repo", "commits", "agents", "seeds", "changes", "late conflicts", "shared a path before merge",
             "redone", "forced", "unresolved", "lines thrown away share", "lockfile hits", "faithful waves",
             "first divergent wave", "final tree equal"], rows))
    rows = [[d["repo"], r["n"], r["ci_s"].get("p50"), r["wait_s"]["mean"], r["wait_s"]["p95"], r["wait_s"]["max"],
             r["queue_drain_s"], r["landings_per_hour"], r["ci_failed"]]
            for k, d in res.items() if k.startswith("queue-") for r in d["runs"]]
    if rows:
        parts.append("### Serial merge queue (real CI per change)\n\n" + table(
            ["repo", "N", "CI p50 s", "wait mean s", "wait p95 s", "wait max s", "drain s", "landings/h",
             "CI failed"], rows))
    rows = [[r["repo"], r["mode"], r["n"], r["jobs"], r["wall_s"], r["per_workspace_s"]["p50"],
             r["per_workspace_s"]["p95"], r["clone_s"]["p50"], r["layers_s"]["p50"], r["setup_s"]["p50"],
             r["sync_s"]["p50"], r["layer_restored"], r["du_per_workspace_mb"], r["df_delta_mb"],
             round(r["df_delta_mb"] / r["n"], 1), r.get("pool_fill_s") or "", r.get("remove_wall_s", "")]
            for k, d in res.items() if k.startswith("cow-") for r in d["runs"]]
    if rows:
        parts.append("### Rootstock workspaces (copy-on-write clone + dependency layer)\n\n" + table(
            ["repo", "mode", "N", "jobs", "wall s", "per ws p50 s", "per ws p95 s", "clone p50 s", "layer p50 s",
             "setup p50 s", "sync p50 s", "layer reused", "du per ws MB", "df delta MB", "df per ws MB",
             "pool fill s (not counted)", "remove all s"], rows))
    rows, rrows = [], []
    for k, d in res.items():
        if k.startswith("rootstock-"):
            for r in d["agents"]:
                rows.append([d["repo"], d["init_s"], d["data_mb"], d["files"], r["n"], r["wall_s"],
                             r["open_change_s"]["p50"], r["ready_s"]["p50"], r["ready_s"]["p95"], r["df_delta_mb"],
                             r["server_rss_mb"]])
            for r in d["replay"]:
                g = r.get("git_check") or {}
                rrows.append([d["repo"], r["commits"], r["agents"], r["first_conflicts"], r["flagged_early"],
                              r["redone"], r["forced"], f'{r["faithful_waves"]} of {r["waves"]}', r["final_tree_equal"],
                              round(r["land_ms"]["p50"], 2), round(r["land_ms"]["p95"], 2), round(r["land_ms"]["max"], 2),
                              g.get("same_tree"), len(g.get("server_clean_git_conflict", [])), g.get("server_conflict_git_clean")])
    if rows:
        parts.append("### Rootstock API agents (no workspace)\n\n" + table(
            ["repo", "init s", "data MB", "files", "N", "wall s", "open_change p50 s", "ready p50 s", "ready p95 s",
             "df delta MB", "server RSS MB"], rows))
    if rrows:
        parts.append("### Rootstock replay (seed 1)\n\n" + table(
            ["repo", "commits", "agents", "first conflicts", "flagged by overlaps", "redone", "forced", "faithful waves",
             "final tree equal", "land p50 ms", "land p95 ms", "land max ms", "merges same tree as git",
             "server clean, git conflict", "server conflict, git clean"], rrows))
    rows = [[d["repo"], d["history_commits"], len(d["files"]), d["log_s"]["p50"], d["log_s"]["max"],
             d["blame_s"]["p50"], d["blame_s"]["max"]] + [f'{b["wall_s"]} / {b["blame_s"]["p95"]}' for b in d["bursts"]]
            for k, d in res.items() if k.startswith("history-")]
    if rows:
        heads = [f'N={b["n"]} wall / p95 s' for b in next(d for k, d in res.items() if k.startswith("history-"))["bursts"]]
        parts.append("### History queries in a checkout (git log / blame on the most-changed files)\n\n" + table(
            ["repo", "commits in history", "files", "log p50 s", "log max s", "blame p50 s", "blame max s", *heads], rows))
    print("\n\n".join(parts))


def cmd_fetch(args):
    for name in args.repos.split(","):
        src, spec = source(args, name)
        print(name, src, spec["rev"])


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dir", default=os.environ.get("BENCH_DIR", "/tmp/rootstock-bench"),
                    help="scratch: clones, seeds, workspaces (default $BENCH_DIR or /tmp/rootstock-bench)")
    ap.add_argument("--oss", default=os.environ.get("BENCH_OSS"), help="where pinned repos are cloned (default <dir>/oss)")
    ap.add_argument("--out", default=None, help="result JSON dir (default <dir>/results)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    f = sub.add_parser("fetch")
    f.add_argument("--repos", default=",".join(REPOS))
    w = sub.add_parser("workspaces")
    w.add_argument("--repo", choices=REPOS, required=True)
    w.add_argument("--n", default="1,10,50")
    w.add_argument("--setup", help="comma list of setups (default: all for the repo)")
    w.add_argument("--jobs", type=int, default=8, help="workspaces made at the same time")
    w.add_argument("--keep", action="store_true")
    c = sub.add_parser("conflicts")
    c.add_argument("--repo", choices=REPOS, required=True)
    c.add_argument("--commits", type=int, default=500)
    c.add_argument("--agents", default="1,10,50")
    c.add_argument("--seeds", default="1,2,3")
    q = sub.add_parser("queue")
    q.add_argument("--repo", choices=REPOS, required=True)
    q.add_argument("--n", default="1,10,50")
    r = sub.add_parser("rootstock")
    r.add_argument("--repo", choices=REPOS, required=True)
    r.add_argument("--bin", default="target/release/rootstock")
    r.add_argument("--n", default="1,10,50")
    r.add_argument("--jobs", type=int, default=50)
    r.add_argument("--agents", default="1,10,50", help="replay agent counts ('' to skip replay)")
    r.add_argument("--commits", type=int, default=500)
    h = sub.add_parser("history")
    h.add_argument("--repo", choices=REPOS, required=True)
    h.add_argument("--commits", type=int, default=500)
    h.add_argument("--files", type=int, default=10)
    h.add_argument("--n", default="1,10,50")
    k = sub.add_parser("cow")
    k.add_argument("--repo", choices=REPOS, required=True)
    k.add_argument("--bin", default="target/release/rootstock")
    k.add_argument("--n", default="1,10,50")
    k.add_argument("--modes", default="files,build,spare-files,spare-build",
                   help="files (no setup), build (setup command); spare-*: the same from a warm pool")
    k.add_argument("--jobs", type=int, default=8)
    k.add_argument("--keep", action="store_true")
    sub.add_parser("report")
    args = ap.parse_args(argv)
    args.oss = args.oss or str(Path(args.dir) / "oss")
    args.out = args.out or str(Path(args.dir) / "results")
    {"fetch": cmd_fetch, "workspaces": cmd_workspaces, "conflicts": cmd_conflicts, "queue": cmd_queue,
     "rootstock": cmd_rootstock, "cow": cmd_cow, "history": cmd_history, "report": cmd_report}[args.cmd](args)


if __name__ == "__main__":
    main()
