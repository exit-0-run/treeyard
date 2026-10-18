#!/usr/bin/env python3
"""Phase-4 derived data on a real repo: blame, history, grep and symbols through the TreeYard
API, each checked against (or timed next to) plain git on a checkout.

Stdlib only. Prints one JSON document and writes it to <dir>/results/derived-<repo>.json.

  python3 bench/derived.py --repo /path/to/clone --dir /tmp/scratch [--history 500] [--files 200]

- blame:   for sampled files touched in the imported history, every line's commit and original
           line number from `blame` must equal `git blame --first-parent --porcelain`.
- log:     `log path=<dir>` from the changed-path index vs the same walk without it (scan) vs
           `git log --first-parent -- <dir>`; the entries must be the same commits.
- grep:    `grep` with the trigram index vs the scan of every file vs `git grep` on a
           checkout; matches must be equal.
- symbols: first lookup (parses every source blob) and a warm lookup.
- land:    (--only land [--auth]) 300 sequential lands: land latency with auth and audit.
- bisect:  (requests only; --only bisect --bisect-test CMD) 40 agents land through the API,
           one plants a bug; the server bisects with the unit tests; revert removes it.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import random
import socket
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

# Git's automatic maintenance starts a detached `git repack` after commands on a big repository; many
# at once used all memory on a linux clone (39 repacks, 16 GB). Every git this script runs has it off.
os.environ.update({"GIT_CONFIG_COUNT": "2", "GIT_CONFIG_KEY_0": "maintenance.auto", "GIT_CONFIG_VALUE_0": "false",
                   "GIT_CONFIG_KEY_1": "gc.auto", "GIT_CONFIG_VALUE_1": "0"})

PATTERNS = [
    r"fn\s+main",
    r"TODO|FIXME",
    r"func \(s \*Server\)",
    r"Copyright 20[0-9][0-9]",
    r"(?i)deprecated",
    r"panic\(",
    r"import \"net/http\"",
    r"\bunsafe\b",
    r"errors\.New\(\"[a-z]+ not found",
    r"[A-Z][a-z]+Handler",
]


def sh(cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


TOKEN = None  # set by --auth: every call sends it, like an agent with a token


def post(url, op, body):
    headers = {"Content-Type": "application/json"}
    if TOKEN:
        headers["Authorization"] = f"Bearer {TOKEN}"
    req = urllib.request.Request(f"{url}/api/{op}", data=json.dumps(body).encode(), headers=headers)
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
        "sum_s": round(sum(xs), 3),
    }


def git_blame(repo, path):
    out = subprocess.run(
        ["git", "-C", repo, "blame", "--first-parent", "--porcelain", "HEAD", "--", path],
        check=True,
        capture_output=True,
    ).stdout.decode("utf-8", "replace")
    lines = []
    for line in out.split("\n"):
        parts = line.split(" ")
        if len(parts[0]) == 40 and len(parts) >= 3 and all(c in "0123456789abcdef" for c in parts[0]):
            lines.append((parts[0], int(parts[1])))
    return lines


def rs_lines(v):
    out = []
    for h in v["hunks"]:
        commit = v["origins"][h["origin"]]["commit"]
        out.extend((commit, h["orig_start"] + k) for k in range(h["count"]))
    return out


def cmd_blame(args, url, repo, out):
    touched = sh(["git", "-C", repo, "log", "--first-parent", "--format=", "--name-only", f"-n{args.history}"])
    head = set(sh(["git", "-C", repo, "ls-files"]).split("\n"))
    files = sorted({p for p in touched.split("\n") if p and p in head})
    random.Random(args.seed).shuffle(files)
    files = sorted(files[: args.files])
    eq_files = eq_lines = lines = 0
    cold, warm, gitt = [], [], []
    bad = []
    for p in files:
        try:
            v, t1 = timed(lambda: post(url, "blame", {"path": p}))
        except Exception as e:  # binary or unreadable: count as a mismatch with a note
            bad.append({"path": p, "error": str(e)[:200]})
            continue
        _, t2 = timed(lambda: post(url, "blame", {"path": p}))
        g, t3 = timed(lambda: git_blame(repo, p))
        cold.append(t1)
        warm.append(t2)
        gitt.append(t3)
        r = rs_lines(v)
        same = sum(1 for a, b in zip(r, g) if a == b)
        lines += len(g)
        eq_lines += same if len(r) == len(g) else 0
        if r == g:
            eq_files += 1
        elif len(bad) < 10:
            first = next((i for i, (a, b) in enumerate(zip(r, g)) if a != b), min(len(r), len(g)))
            bad.append({"path": p, "lines": [len(r), len(g)], "first_diff_line": first + 1,
                        "treeyard": r[first] if first < len(r) else None, "git": g[first] if first < len(g) else None})
    out["blame"] = {
        "files": len(files), "files_equal": eq_files, "lines": lines, "lines_equal": eq_lines,
        "treeyard_cold": stats(cold), "treeyard_warm": stats(warm), "git_blame": stats(gitt), "mismatches": bad,
    }


def cmd_log(args, url, repo, out):
    files = sh(["git", "-C", repo, "-c", "core.quotePath=false", "ls-files"]).split("\n")
    dirs = sorted({p.split("/")[0] for p in files if "/" in p} | {"/".join(p.split("/")[:2]) for p in files if p.count("/") >= 2})
    random.Random(args.seed).shuffle(dirs)
    rows = []
    for d in dirs[:12]:
        a, ti = timed(lambda: post(url, "log", {"path": d, "limit": 50}))
        b, ts = timed(lambda: post(url, "log", {"path": d, "limit": 50, "scan": True}))
        g, tg = timed(lambda: sh(["git", "-C", repo, "log", "--first-parent", "--format=%H", "-n50", "--", d]))
        # The import changeset (agent "import") stands for all older history: leave it out.
        ci = [e["commit"] for e in a["entries"] if e["agent"] != "import"]
        cs = [e["commit"] for e in b["entries"] if e["agent"] != "import"]
        gl = [c for c in g.split("\n") if c]
        # git's list goes past the import; compare the part TreeYard holds.
        rows.append({"path": d, "index_ms": round(ti * 1000, 2), "scan_ms": round(ts * 1000, 2),
                     "git_ms": round(tg * 1000, 2), "entries": len(ci), "index_eq_scan": ci == cs,
                     "eq_git_prefix": ci == gl[: len(ci)], "scanned_index": a["scanned"], "scanned_walk": b["scanned"]})
    out["log"] = rows


def cmd_grep(args, url, repo, out):
    _, t_build = timed(lambda: post(url, "grep", {"pattern": "treeyard-warmup-xyz", "max": 1}))
    rows = []
    for pat in PATTERNS:
        idx, ti = timed(lambda: post(url, "grep", {"pattern": pat, "max": 100000}))
        _, ti2 = timed(lambda: post(url, "grep", {"pattern": pat, "max": 100000}))
        scan, ts = timed(lambda: post(url, "grep", {"pattern": pat, "max": 100000, "index": False}))
        flags = ["-i", "-E", "--", pat[4:]] if pat.startswith("(?i)") else ["-E", "--", pat]
        g, tg = timed(lambda: subprocess.run(["git", "-C", repo, "grep", "-n", *flags], capture_output=True, text=True))
        rows.append({
            "pattern": pat, "matches": len(idx["matches"]), "equal_to_scan": idx["matches"] == scan["matches"],
            "candidates": idx["index"].get("candidates"), "files": idx["index"].get("files"),
            "index_ms": round(min(ti, ti2) * 1000, 2), "scan_ms": round(ts * 1000, 2),
            "git_grep_ms": round(tg * 1000, 2), "git_grep_exit": g.returncode,
        })
    out["grep"] = {"first_call_builds_index_s": round(t_build, 3), "postings_mb": idx["index"].get("postings_mb"), "patterns": rows,
                   "speedup_p50": round(statistics.median(r["scan_ms"] / max(r["index_ms"], 0.01) for r in rows), 1)}


def cmd_symbols(args, url, repo, out):
    _, t_first = timed(lambda: post(url, "symbols", {"name": "main", "max": 5}))
    names = ["main", "New", "Error", "String", "parse", "Serve", "run", "init"]
    warm = []
    hits = {}
    for n in names:
        v, t = timed(lambda: post(url, "symbols", {"name": n, "max": 10000}))
        warm.append(t)
        hits[n] = len(v["symbols"])
    out["symbols"] = {"first_call_parses_s": round(t_first, 3), "warm": stats(warm), "hits": hits}


def cmd_land(args, url, repo, out):
    """300 sequential lands of one new file each: the cost of auth and the audit row."""
    lat = []
    for i in range(300):
        ch = post(url, "open_change", {"agent": "bench", "intent": f"note {i}"})
        post(url, "apply", {"change": ch["id"], "edits": [{"path": f"notes/n{i}.md", "content": f"{i}\n"}]})
        r, t = timed(lambda: post(url, "land", {"change": ch["id"]}))
        assert r["status"] == "landed", r
        lat.append(t)
    rows = len(post(url, "audit", {"op": "land", "limit": 1000})["entries"])
    out["land"] = {"auth": bool(TOKEN), "lands": stats(lat), "audit_rows_for_land": rows}


def cmd_bisect(args, url, repo, out):
    """40 agents land changes through the API; agent 27 plants a real bug (swapped user and
    password in requests' get_auth_from_url). The server bisects with the repo's own unit
    tests, then the culprit is reverted through the API."""
    good = post(url, "status", {})["main"]
    planted = None
    for i in range(40):
        ch = post(url, "open_change", {"agent": f"agent-{i:02}", "intent": f"change {i}", "model": "sim"})
        if i == 27:
            edit = {"path": "src/requests/utils.py", "replace": {
                "old": "auth = (unquote(parsed.username), unquote(parsed.password))",
                "new": "auth = (unquote(parsed.password), unquote(parsed.username))"}}
        else:
            edit = {"path": f"notes/agent-{i:02}.md", "content": f"note {i}\n"}
        post(url, "apply", {"change": ch["id"], "edits": [edit]})
        landed = post(url, "land", {"change": ch["id"]})
        assert landed["status"] == "landed", landed
        if i == 27:
            planted = landed["changeset"]
    b, t = timed(lambda: post(url, "bisect", {"good": good}))
    found = b["first_bad"]["changeset"] == planted
    rv = post(url, "revert", {"changeset": b["first_bad"]["changeset"], "agent": "fixer"})
    after = None
    try:
        post(url, "bisect", {"good": good})
    except urllib.error.HTTPError as e:
        after = json.loads(e.read()).get("error", "")[:120]
    out["bisect"] = {"candidates": b["candidates"], "tests": b["tests"], "found_planted": found,
                     "first_bad_agent": b["first_bad"]["agent"], "seconds": round(t, 2),
                     "test_seconds": [round(x["seconds"], 2) for x in b["steps"]],
                     "revert": rv["status"], "bisect_after_revert": after}


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, help="a git clone (read only)")
    ap.add_argument("--dir", required=True, help="scratch dir: data dir and results go here")
    ap.add_argument("--binary", default=str(Path(__file__).resolve().parent.parent / "target/release/treeyard"))
    ap.add_argument("--history", type=int, default=500)
    ap.add_argument("--files", type=int, default=200)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--only", default="blame,log,grep,symbols")
    ap.add_argument("--bisect-test", help="[bisect] test command for the bisect part")
    ap.add_argument("--auth", action="store_true", help="create a token (auth on) and send it on every call")
    args = ap.parse_args(argv)
    repo = str(Path(args.repo).resolve())
    name = Path(repo).name
    d = Path(args.dir)
    data = d / f"data-{name}-{args.history}"
    out = {
        "repo": name, "rev": sh(["git", "-C", repo, "rev-parse", "HEAD"]).strip(), "history": args.history,
        "files_at_head": len(sh(["git", "-C", repo, "ls-files"]).splitlines()),
        "machine": {"platform": platform.platform(), "cpu": platform.processor(), "cores": os.cpu_count()},
    }
    if not data.exists():
        _, t = timed(lambda: sh([args.binary, "init", "--data", str(data), "--from", repo,
                                 "--branch", sh(["git", "-C", repo, "symbolic-ref", "--short", "HEAD"]).strip(),
                                 "--history", str(args.history)]))
        out["init_s"] = round(t, 3)
        if args.bisect_test:
            (data / "treeyard.toml").write_text(f"[bisect]\ntest = {json.dumps(args.bisect_test)}\ntimeout_s = 300\n")
    if args.auth:
        global TOKEN
        TOKEN = sh([args.binary, "token", "create", "--data", str(data), "--name", "bench", "--write", "**"]).strip()
    port = free_port()
    url = f"http://127.0.0.1:{port}"
    proc = subprocess.Popen([args.binary, "serve", "--data", str(data), "--addr", f"127.0.0.1:{port}"],
                            stderr=subprocess.DEVNULL)
    try:
        for _ in range(200):
            try:
                urllib.request.urlopen(f"{url}/health", timeout=1)
                break
            except OSError:
                time.sleep(0.05)
        for part in args.only.split(","):
            {"blame": cmd_blame, "log": cmd_log, "grep": cmd_grep, "symbols": cmd_symbols,
             "bisect": cmd_bisect, "land": cmd_land}[part](args, url, repo, out)
    finally:
        proc.terminate()
        proc.wait()
    (d / "results").mkdir(parents=True, exist_ok=True)
    (d / "results" / f"derived-{name}.json").write_text(json.dumps(out, indent=2))
    json.dump(out, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
