#!/usr/bin/env python3
"""Scale run on a big repo: init, start, read, list, tree, grep and land through the API, with
the server's memory, next to plain git on a worktree of the same repo.

Stdlib only. Prints one JSON document and writes it to <dir>/results/scale-<tag>.json.

  python3 bench/scale.py --repo /path/to/clone --dir /tmp/scratch --tag after \
      [--binary target/release/treeyard] [--history 1000] [--lands 50] [--agents 16] [--git]

- init:   `treeyard init` wall time and peak RSS (`/usr/bin/time -l`).
- start:  `treeyard serve` until /health answers (Repo::open), twice: first and warm restart.
- read:   `read` of 200 sampled files (seeded); `list` of the root; `tree` of main.
- grep:   three patterns, first call (builds the lazy trigram index) and a repeat.
- land:   N sequential agents (open_change, apply one line to a sampled file, land), then
          `--agents` threads that land 5 changes each on different files.
- rss:    server RSS after start, after reads, after grep, after lands (`ps -o rss`).
- --git:  baseline on a git worktree of the same commit: worktree add, git status, git grep,
          commit of a one-line edit, git merge-tree + update-ref (a land without CI).
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import random
import re
import shutil
import socket
import statistics
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

# Git's automatic maintenance starts a detached `git repack` after commands on a big repository; many
# at once used all memory on a linux clone (39 repacks, 16 GB). Every git this script runs has it off.
os.environ.update({"GIT_CONFIG_COUNT": "2", "GIT_CONFIG_KEY_0": "maintenance.auto", "GIT_CONFIG_VALUE_0": "false",
                   "GIT_CONFIG_KEY_1": "gc.auto", "GIT_CONFIG_VALUE_1": "0"})

PATTERNS = [r"EXPORT_SYMBOL_GPL\(", r"struct file_operations [a-z_]+ = \{", r"TODO|FIXME"]


def sh(cmd, cwd=None, env=None):
    return subprocess.run(cmd, cwd=cwd, env=env, check=True, capture_output=True, text=True).stdout


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def post(url, op, body):
    req = urllib.request.Request(f"{url}/api/{op}", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=3600) as r:
        return json.loads(r.read())


def timed(f):
    t = time.perf_counter()
    v = f()
    return v, time.perf_counter() - t


def stats(xs):
    xs = sorted(xs)
    if not xs:
        return {}
    return {
        "n": len(xs),
        "p50_ms": round(statistics.median(xs) * 1000, 2),
        "p95_ms": round(xs[min(len(xs) - 1, int(len(xs) * 0.95))] * 1000, 2),
        "max_ms": round(xs[-1] * 1000, 2),
    }


def rss_mb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return round(int(out) / 1024, 1) if out else None


def time_l(cmd):
    """Runs cmd under /usr/bin/time -l; returns (wall s, peak RSS MB)."""
    t = time.perf_counter()
    p = subprocess.run(["/usr/bin/time", "-l", *cmd], capture_output=True, text=True)
    wall = time.perf_counter() - t
    if p.returncode != 0:
        raise SystemExit(f"{cmd} failed: {p.stderr[-2000:]}")
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    return round(wall, 3), round(int(m.group(1)) / 2**20, 1) if m else None


class Server:
    def __init__(self, binary, data):
        self.port = free_port()
        self.url = f"http://127.0.0.1:{self.port}"
        t = time.perf_counter()
        self.proc = subprocess.Popen([binary, "serve", "--data", str(data), "--addr", f"127.0.0.1:{self.port}"],
                                     stderr=subprocess.DEVNULL)
        while True:
            try:
                urllib.request.urlopen(f"{self.url}/health", timeout=1)
                break
            except OSError:
                if self.proc.poll() is not None:
                    raise SystemExit("server exited")
                time.sleep(0.02)
        self.start_s = round(time.perf_counter() - t, 3)

    def stop(self):
        self.proc.terminate()
        self.proc.wait()


def sample(repo, n, seed):
    all_files = [f for f in sh(["git", "-C", repo, "ls-files", "-z"]).split("\0") if f]
    lower = {}
    for f in all_files:
        lower[f.lower()] = lower.get(f.lower(), 0) + 1
    # Paths that differ only in case share one file on APFS: keep them out of the sample.
    files = [f for f in all_files if f.endswith((".c", ".h", ".rs", ".py", ".go", ".md")) and lower[f.lower()] == 1]
    rnd = random.Random(seed)
    return rnd.sample(files, min(n, len(files))), len(files)


def land_one(url, agent, path, line):
    ch = post(url, "open_change", {"agent": agent, "intent": f"scale edit {path}"})
    text = post(url, "read", {"path": path, "at": ch["id"]})["content"]
    post(url, "apply", {"change": ch["id"], "edits": [{"path": path, "content": text + line}]})
    r, t = timed(lambda: post(url, "land", {"change": ch["id"]}))
    if r["status"] != "landed":
        raise SystemExit(f"land of {path}: {r['status']}")
    return t


def run_treeyard(args, repo, out):
    d = Path(args.dir)
    data = d / f"data-{args.tag}"
    if data.exists():
        shutil.rmtree(data)
    branch = sh(["git", "-C", repo, "symbolic-ref", "--short", "HEAD"]).strip()
    # `--history 0` is left out, so a binary older than the history import (the "before" run) works.
    hist = ["--history", str(args.history)] if args.history else []
    wall, rss = time_l([args.binary, "init", "--data", str(data), "--from", repo, "--branch", branch, *hist])
    out["init"] = {"wall_s": wall, "peak_rss_mb": rss, "history": args.history}
    out["data_mb"] = round(int(sh(["du", "-sk", str(data)]).split()[0]) / 1024, 1)
    srv = Server(args.binary, data)
    try:
        out["start_s"] = srv.start_s
        out["rss_mb"] = {"start": rss_mb(srv.proc.pid)}
        paths, n_src = sample(repo, 200, args.seed)
        out["source_files"] = n_src
        reads = [timed(lambda p=p: post(srv.url, "read", {"path": p}))[1] for p in paths]
        out["read"] = stats(reads)
        _, out["list_root_ms"] = timed(lambda: post(srv.url, "list", {"path": ""}))
        out["list_root_ms"] = round(out["list_root_ms"] * 1000, 2)
        try:
            tree, t = timed(lambda: post(srv.url, "tree", {}))
            out["tree"] = {"files": tree["files"], "ms": round(t * 1000, 2)}
        except urllib.error.HTTPError as e:
            out["tree"] = {"unsupported": e.code}
        _, t = timed(lambda: post(srv.url, "status", {}))
        out["status_ms"] = round(t * 1000, 2)
        out["rss_mb"]["after_reads"] = rss_mb(srv.proc.pid)
        if not args.no_grep:
            g = {}
            for pat in PATTERNS:
                r1, t1 = timed(lambda: post(srv.url, "grep", {"pattern": pat, "max": 100}))
                _, t2 = timed(lambda: post(srv.url, "grep", {"pattern": pat, "max": 100}))
                g[pat] = {"first_ms": round(t1 * 1000, 2), "repeat_ms": round(t2 * 1000, 2),
                          "matches": len(r1.get("matches", []) if isinstance(r1, dict) else r1),
                          "postings_mb": ((r1.get("index") if isinstance(r1, dict) else None) or {}).get("postings_mb")}
            out["grep"] = g
            out["rss_mb"]["after_grep"] = rss_mb(srv.proc.pid)
        lands = [land_one(srv.url, f"seq-{i}", paths[i % len(paths)], f"/* scale {i} */\n")
                 for i in range(args.lands)]
        out["land_sequential"] = stats(lands)
        times, lock = [], threading.Lock()

        def agent(a):
            for k in range(5):
                p = paths[(args.lands + a * 5 + k) % len(paths)]
                t = land_one(srv.url, f"par-{a}", p, f"/* par {a}.{k} */\n")
                with lock:
                    times.append(t)

        t0 = time.perf_counter()
        th = [threading.Thread(target=agent, args=(a,)) for a in range(args.agents)]
        for x in th:
            x.start()
        for x in th:
            x.join()
        wall = time.perf_counter() - t0
        out["land_parallel"] = {**stats(times), "agents": args.agents, "wall_s": round(wall, 3),
                                "lands_per_s": round(len(times) / wall, 1)}
        out["rss_mb"]["after_lands"] = rss_mb(srv.proc.pid)
    finally:
        srv.stop()
    srv = Server(args.binary, data)
    out["restart_s"] = srv.start_s
    out["rss_mb"]["restart"] = rss_mb(srv.proc.pid)
    srv.stop()
    fsck = subprocess.run(["git", "--git-dir", str(data / "git"), "fsck", "--connectivity-only", "--no-progress"],
                          capture_output=True, text=True)
    out["fsck_connectivity_ok"] = fsck.returncode == 0


def run_git(args, repo, out):
    d = Path(args.dir)
    wt = d / f"wt-{args.tag}"
    if wt.exists():
        sh(["git", "-C", repo, "worktree", "remove", "--force", str(wt)])
    g = {}
    _, t = timed(lambda: sh(["git", "-C", repo, "worktree", "add", "--detach", "-q", str(wt), "HEAD"]))
    g["worktree_add_s"] = round(t, 3)
    _, t = timed(lambda: sh(["git", "-C", str(wt), "status", "--porcelain"]))
    g["status_s"] = round(t, 3)
    grep = {}
    for pat in PATTERNS:
        _, t = timed(lambda: subprocess.run(["git", "-C", str(wt), "grep", "-E", "-c", pat], capture_output=True))
        grep[pat] = round(t * 1000, 2)
    g["git_grep_ms"] = grep
    paths, _ = sample(repo, 200, args.seed)
    env = {**os.environ, "GIT_AUTHOR_NAME": "b", "GIT_AUTHOR_EMAIL": "b@x", "GIT_COMMITTER_NAME": "b",
           "GIT_COMMITTER_EMAIL": "b@x"}
    base = sh(["git", "-C", str(wt), "rev-parse", "HEAD"]).strip()
    main = base
    lands = []
    for i in range(args.lands):
        p = wt / paths[i % len(paths)]
        t0 = time.perf_counter()
        with open(p, "a") as f:
            f.write(f"/* scale {i} */\n")
        sh(["git", "-C", str(wt), "commit", "-q", "-a", "-m", f"scale {i}", "--no-verify"], env=env)
        head = sh(["git", "-C", str(wt), "rev-parse", "HEAD"]).strip()
        tree = sh(["git", "-C", str(wt), "merge-tree", "--write-tree", main, head]).split()[0]
        _ = tree
        sh(["git", "-C", str(wt), "update-ref", "refs/scale/main", head])
        main = head
        lands.append(time.perf_counter() - t0)
    g["edit_commit_merge_land"] = stats(lands)
    g["worktree_df_note"] = "worktree checkout size: " + sh(["du", "-sh", str(wt)]).split()[0]
    sh(["git", "-C", repo, "worktree", "remove", "--force", str(wt)])
    sh(["git", "-C", repo, "update-ref", "-d", "refs/scale/main"])
    out["git"] = g


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--dir", required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--binary", default=str(Path(__file__).resolve().parent.parent / "target/release/treeyard"))
    ap.add_argument("--history", type=int, default=0)
    ap.add_argument("--lands", type=int, default=50)
    ap.add_argument("--agents", type=int, default=16)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--no-grep", action="store_true")
    ap.add_argument("--git", action="store_true", help="also run the plain git baseline")
    ap.add_argument("--git-only", action="store_true")
    args = ap.parse_args(argv)
    repo = str(Path(args.repo).resolve())
    out = {
        "tag": args.tag, "repo": Path(repo).name, "rev": sh(["git", "-C", repo, "rev-parse", "HEAD"]).strip(),
        "files_at_head": len(sh(["git", "-C", repo, "ls-files", "-z"]).split("\0")) - 1,
        "commits": int(sh(["git", "-C", repo, "rev-list", "--count", "HEAD"]).strip()),
        "binary": args.binary,
        "machine": {"platform": platform.platform(), "cpu": platform.processor(), "cores": os.cpu_count(),
                    "load": os.getloadavg()},
    }
    if not args.git_only:
        run_treeyard(args, repo, out)
    if args.git or args.git_only:
        run_git(args, repo, out)
    res = Path(args.dir) / "results"
    res.mkdir(parents=True, exist_ok=True)
    (res / f"scale-{args.tag}.json").write_text(json.dumps(out, indent=2))
    json.dump(out, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
