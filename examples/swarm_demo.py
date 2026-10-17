#!/usr/bin/env python3
"""N scripted agents do the same work twice: with plain git worktrees, then against TreeYard.

Each agent i adds one new file, edits its own line of shared.txt (lines far apart, so git and
TreeYard both merge them cleanly), and some pairs of agents edit the same line of hot.txt (a real
conflict for the second one to land). Then every agent lands its change on main.

- git worktrees: `git worktree add` per agent, edit and commit in parallel, then a serial queue
  lands each branch with `git rebase main` + `git merge --ff-only` (no CI, like the TreeYard side).
- TreeYard (binary `treeyard`): `open_change` per agent through `treeyard call`, `apply` in parallel, then every
  agent calls `land` at once; the server rebases and merges.

A conflicted agent redoes its hot.txt edit on top of the new main and lands again (both sides).
At the end the script checks that main holds every agent's edit and prints one comparison table.
Standard library only. Usage: examples/swarm-demo.sh --help.
"""

import argparse
import concurrent.futures as cf
import json
import os
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request

LINES_PER_FILE = 40


def run(cmd, cwd=None, env=None, check=True):
    p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    if check and p.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)} failed ({p.returncode}): {p.stderr.strip()}")
    return p


def git(*args, cwd=None, check=True):
    return run(["git", *args], cwd=cwd, check=check)


def du_kb(path):
    """Allocated size of a tree in KiB (st_blocks), counting each hard-linked inode once."""
    seen, total = set(), 0
    for root, dirs, files in os.walk(path):
        for name in dirs + files:
            st = os.lstat(os.path.join(root, name))
            if (st.st_dev, st.st_ino) in seen:
                continue
            seen.add((st.st_dev, st.st_ino))
            total += st.st_blocks * 512
    return total / 1024


def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return 0.0
    k = max(0, min(len(xs) - 1, int(round(q * (len(xs) - 1)))))
    return xs[k]


class Plan:
    """What each agent edits. Same plan for both sides."""

    def __init__(self, agents, pairs):
        self.agents = agents
        # Agents 2k and 2k+1 (k < pairs) edit hot line k.
        self.pairs = min(pairs, agents // 2)

    def name(self, i):
        return f"agent-{i:02d}"

    def shared_old(self, i):
        return f"shared line {i:03d}"

    def shared_new(self, i):
        return f"shared line {i:03d} edited by {self.name(i)}"

    def hot_line(self, i):
        return i // 2 if i < 2 * self.pairs else None

    def hot_old(self, k):
        return f"hot line {k:03d}"

    def hot_new(self, i):
        return f"hot line {self.hot_line(i):03d} set by {self.name(i)}"

    def new_file(self, i):
        return f"agents/{self.name(i)}.txt", f"work of {self.name(i)}\n"

    def shared_text(self):
        # Three untouched lines between edited lines: git merge-file needs a gap to merge cleanly.
        out = []
        for i in range(self.agents):
            out += [self.shared_old(i), "", "", ""]
        return "\n".join(out) + "\n"

    def hot_text(self):
        return "".join(f"{self.hot_old(k)}\n\n\n\n" for k in range(max(1, self.pairs)))


def make_repo(path, files, plan):
    os.makedirs(path)
    git("init", "-q", "-b", "main", cwd=path)
    git("config", "user.email", "demo@example.invalid", cwd=path)
    git("config", "user.name", "demo", cwd=path)
    for f in range(files):
        d = os.path.join(path, "src", f"m{f // 100:02d}")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, f"f{f:04d}.txt"), "w") as fh:
            fh.write("".join(f"file {f} line {n} {'x' * 60}\n" for n in range(LINES_PER_FILE)))
    with open(os.path.join(path, "shared.txt"), "w") as fh:
        fh.write(plan.shared_text())
    with open(os.path.join(path, "hot.txt"), "w") as fh:
        fh.write(plan.hot_text())
    git("add", "-A", cwd=path)
    git("commit", "-q", "-m", "demo base", cwd=path)
    # Pack now: a detached auto-gc would move objects while the two sides read them.
    git("gc", "-q", cwd=path)
    git("config", "gc.auto", "0", cwd=path)


def edit_files(root, plan, i):
    """The agent's edits, done in a working directory (the git side)."""
    p, text = plan.new_file(i)
    os.makedirs(os.path.join(root, os.path.dirname(p)), exist_ok=True)
    with open(os.path.join(root, p), "w") as fh:
        fh.write(text)
    sub(os.path.join(root, "shared.txt"), plan.shared_old(i), plan.shared_new(i))
    if plan.hot_line(i) is not None:
        set_hot(os.path.join(root, "hot.txt"), plan, i)


def sub(path, old, new):
    with open(path) as fh:
        lines = fh.read().split("\n")
    lines = [new if line == old else line for line in lines]
    with open(path, "w") as fh:
        fh.write("\n".join(lines))


def set_hot(path, plan, i):
    k = plan.hot_line(i)
    with open(path) as fh:
        lines = fh.read().split("\n")
    prefix = f"hot line {k:03d}"
    lines = [plan.hot_new(i) if line.startswith(prefix) else line for line in lines]
    with open(path, "w") as fh:
        fh.write("\n".join(lines))


# ---------------------------------------------------------------- git worktrees


def side_git(base, scratch, plan):
    repo = os.path.join(scratch, "git", "repo")
    wts = os.path.join(scratch, "git", "worktrees")
    git("clone", "-q", base, repo)
    for k, v in (("user.email", "demo@example.invalid"), ("user.name", "demo"), ("gc.auto", "0")):
        git("config", k, v, cwd=repo)
    os.makedirs(wts)
    r = {"name": "git worktrees"}

    t = time.perf_counter()
    for i in range(plan.agents):
        git("worktree", "add", "-q", "-b", plan.name(i), os.path.join(wts, plan.name(i)), "main", cwd=repo)
    r["setup_s"] = time.perf_counter() - t
    r["disk_mb"] = du_kb(wts) / 1024

    def work(i):
        wt = os.path.join(wts, plan.name(i))
        edit_files(wt, plan, i)
        git("add", "-A", cwd=wt)
        git("commit", "-q", "-m", f"{plan.name(i)}: demo edit", cwd=wt)

    t = time.perf_counter()
    with cf.ThreadPoolExecutor(plan.agents) as ex:
        list(ex.map(work, range(plan.agents)))
    r["work_s"] = time.perf_counter() - t

    # A serial queue: each change rebases on the newest main, then main fast-forwards to it.
    lands, conflicts = [], 0
    t = time.perf_counter()
    for i in range(plan.agents):
        wt = os.path.join(wts, plan.name(i))
        if git("rebase", "-q", "main", cwd=wt, check=False).returncode != 0:
            conflicts += 1
            git("rebase", "--abort", cwd=wt)
            # The agent redoes its work on top of the new main.
            git("reset", "-q", "--hard", "main", cwd=wt)
            edit_files(wt, plan, i)
            git("add", "-A", cwd=wt)
            git("commit", "-q", "-m", f"{plan.name(i)}: demo edit (redone)", cwd=wt)
        git("merge", "-q", "--ff-only", plan.name(i), cwd=repo)
        lands.append(time.perf_counter() - t)
    r["land_s"] = time.perf_counter() - t
    r["lands"] = lands
    r["conflicts"] = conflicts
    r["warned"] = 0  # git has no signal before the merge
    r["show"] = lambda path: git("show", f"main:{path}", cwd=repo, check=False).stdout
    return r


# ---------------------------------------------------------------- TreeYard


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def side_treeyard(base, scratch, plan, binary):
    data = os.path.join(scratch, "treeyard", "data")
    os.makedirs(os.path.dirname(data))
    run([binary, "init", "--data", data, "--from", base, "--branch", "main"])
    port = free_port()
    url = f"http://127.0.0.1:{port}"
    log = open(os.path.join(scratch, "treeyard", "serve.log"), "w")
    server = subprocess.Popen([binary, "serve", "--data", data, "--addr", f"127.0.0.1:{port}"], stdout=log, stderr=log)
    try:
        for _ in range(200):
            try:
                urllib.request.urlopen(f"{url}/health", timeout=1).read()
                break
            except OSError:
                time.sleep(0.05)
        else:
            raise RuntimeError("treeyard serve did not start; see serve.log")
        return drive_treeyard(plan, binary, url, data)
    finally:
        server.terminate()
        server.wait()
        log.close()


def drive_treeyard(plan, binary, url, data):
    env = dict(os.environ, TREEYARD_URL=url)
    env.pop("TREEYARD_TOKEN", None)

    def call(op, params):
        p = run([binary, "call", op, json.dumps(params)], env=env)
        return json.loads(p.stdout)

    r = {"name": "TreeYard"}
    disk0 = du_kb(data)
    ids = {}
    t = time.perf_counter()
    for i in range(plan.agents):
        paths = ["shared.txt", plan.new_file(i)[0]] + (["hot.txt"] if plan.hot_line(i) is not None else [])
        ch = call("open_change", {"agent": plan.name(i), "intent": "demo edit", "paths": paths})
        ids[i] = ch["id"]
    r["setup_s"] = time.perf_counter() - t

    warned = set()

    def work(i):
        p, text = plan.new_file(i)
        edits = [
            {"path": p, "content": text},
            {"path": "shared.txt", "replace": {"old": plan.shared_old(i) + "\n", "new": plan.shared_new(i) + "\n"}},
        ]
        if plan.hot_line(i) is not None:
            k = plan.hot_line(i)
            edits.append({"path": "hot.txt", "replace": {"old": plan.hot_old(k) + "\n", "new": plan.hot_new(i) + "\n"}})
        out = call("apply", {"change": ids[i], "edits": edits})
        if any(c.get("kind") == "conflict" for c in out.get("collisions", [])):
            warned.add(i)

    t = time.perf_counter()
    with cf.ThreadPoolExecutor(plan.agents) as ex:
        list(ex.map(work, range(plan.agents)))
    r["work_s"] = time.perf_counter() - t

    lands, conflicts = [], [0]

    def land(i):
        out = call("land", {"change": ids[i]})
        while out["status"] == "conflict":
            conflicts[0] += 1
            if os.environ.get("SWARM_DEMO_DEBUG"):
                tasks = [(c["path"], c["kind"], c.get("theirs_by")) for c in out["change"]["conflicts"]]
                print(f"debug: {plan.name(i)} conflict {tasks}", file=sys.stderr)
            # The agent takes main's side of the conflict, redoes its one-line edit on it as a
            # small `replace` (not a whole-file write, which would undo later lands), and lands again.
            call("resolve", {"change": ids[i], "path": "hot.txt", "take": "theirs"})
            text = call("read", {"path": "hot.txt", "at": ids[i]})["content"]
            k = plan.hot_line(i)
            old = next(line for line in text.split("\n") if line.startswith(f"hot line {k:03d}"))
            edit = {"path": "hot.txt", "replace": {"old": old + "\n", "new": plan.hot_new(i) + "\n"}}
            call("apply", {"change": ids[i], "edits": [edit]})
            out = call("land", {"change": ids[i]})
        if out["status"] != "landed":
            raise RuntimeError(f"land of {plan.name(i)}: {out['status']}")
        lands.append(time.perf_counter() - t)

    t = time.perf_counter()
    with cf.ThreadPoolExecutor(plan.agents) as ex:
        list(ex.map(land, range(plan.agents)))
    r["land_s"] = time.perf_counter() - t
    r["lands"] = lands
    r["conflicts"] = conflicts[0]
    r["warned"] = len({plan.hot_line(i) for i in warned if plan.hot_line(i) is not None})
    r["disk_mb"] = (du_kb(data) - disk0) / 1024
    gd = os.path.join(data, "git")
    r["show"] = lambda path: git("--git-dir", gd, "show", f"main:{path}", check=False).stdout
    return r


# ---------------------------------------------------------------- report


def check(plan, r):
    """Edits of agents that main does not hold."""
    lost = 0
    shared = r["show"]("shared.txt")
    hot = r["show"]("hot.txt")
    for i in range(plan.agents):
        p, text = plan.new_file(i)
        ok = r["show"](p) == text and plan.shared_new(i) in shared.split("\n")
        k = plan.hot_line(i)
        if k is not None:
            # One of the pair wins the hot line (the last to land); the line must hold one of them.
            pair = [j for j in range(plan.agents) if plan.hot_line(j) == k]
            ok = ok and any(plan.hot_new(j) in hot.split("\n") for j in pair)
        lost += 0 if ok else 1
    return lost


def table(plan, files, sides, out):
    g, s = sides
    rows = [
        (f"Give {plan.agents} agents a place to work (s)", f"{g['setup_s']:.2f}", f"{s['setup_s']:.2f}"),
        (f"Extra disk for {plan.agents} agents (MB)", f"{g['disk_mb']:.1f}", f"{s['disk_mb']:.1f}"),
        ("Edit and commit, all agents at once (s)", f"{g['work_s']:.2f}", f"{s['work_s']:.2f}"),
        ("Land every change (s)", f"{g['land_s']:.2f} (serial queue)", f"{s['land_s']:.2f} (all at once)"),
        (
            "Change is on main after, p50 / p95 (ms)",
            f"{1000 * statistics.median(g['lands']):.0f} / {1000 * pct(g['lands'], 0.95):.0f}",
            f"{1000 * statistics.median(s['lands']):.0f} / {1000 * pct(s['lands'], 0.95):.0f}",
        ),
        (
            "Same-line conflicts warned before land",
            f"{g['warned']} of {plan.pairs}",
            f"{s['warned']} of {plan.pairs}",
        ),
        ("Conflicts at land (agent redoes the edit)", str(g["conflicts"]), str(s["conflicts"])),
        ("Edits missing from main", f"{g['lost']} of {plan.agents}", f"{s['lost']} of {plan.agents}"),
    ]
    w0 = max(len(r[0]) for r in rows + [("Step", "", "")])
    w1 = max(len(r[1]) for r in rows + [("", "git worktrees", "")])
    w2 = max(len(r[2]) for r in rows + [("", "", "TreeYard")])
    print(f"swarm-demo: {plan.agents} agents, {files} files, {plan.pairs} same-line pairs", file=out)
    print(f"| {'Step':<{w0}} | {'git worktrees':<{w1}} | {'TreeYard':<{w2}} |", file=out)
    print(f"|{'-' * (w0 + 2)}|{'-' * (w1 + 2)}|{'-' * (w2 + 2)}|", file=out)
    for a, b, c in rows:
        print(f"| {a:<{w0}} | {b:<{w1}} | {c:<{w2}} |", file=out)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--agents", type=int, default=10)
    ap.add_argument("--files", type=int, default=2000, help="files in the synthetic repo")
    ap.add_argument("--pairs", type=int, default=2, help="pairs of agents that edit the same line")
    ap.add_argument("--bin", default=os.environ.get("TREEYARD_BIN", "treeyard"))
    ap.add_argument("--dir", help="scratch directory (default: a new temporary directory, deleted after)")
    ap.add_argument("--json", help="also write raw numbers to this file")
    a = ap.parse_args()
    if a.agents < 2:
        ap.error("--agents must be at least 2")
    binary = shutil.which(a.bin) or a.bin
    if not os.path.exists(binary):
        ap.error(f"treeyard binary not found: {a.bin} (build with `make release` or pass --bin)")
    plan = Plan(a.agents, a.pairs)
    scratch = a.dir or tempfile.mkdtemp(prefix="swarm-demo-")
    os.makedirs(scratch, exist_ok=True)
    try:
        base = os.path.join(scratch, "base")
        make_repo(base, a.files, plan)
        sides = (side_git(base, scratch, plan), side_treeyard(base, scratch, plan, binary))
        for r in sides:
            r["lost"] = check(plan, r)
        table(plan, a.files, sides, sys.stdout)
        if a.json:
            keep = ("setup_s", "disk_mb", "work_s", "land_s", "lands", "conflicts", "warned", "lost")
            raw = {r["name"]: {k: r[k] for k in keep} for r in sides}
            raw["agents"], raw["files"], raw["pairs"] = plan.agents, a.files, plan.pairs
            with open(a.json, "w") as fh:
                json.dump(raw, fh, indent=1)
        return 1 if any(r["lost"] for r in sides) else 0
    finally:
        if not a.dir:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
