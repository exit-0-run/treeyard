#!/usr/bin/env python3
"""T2 experiment kit: plain git worktrees (arm A) against `treeyard connect` (arm B), 5 agents at once.

Standard library only. Subcommands (see bench/t2/README.md and PREREGISTRATION.md):

  setup     --scratch S                 pinned clones, the requests venv, a warm cargo build
  generate                              tasks/ from taskdefs.py (prompts, hidden tests, patches)
  validate  --scratch S [--repo R]      each test fails on the pin and passes with its reference
  run       --scratch S --arm A|B --repo R --set disjoint|colliding --rep N --harness H
  rehearse  --scratch S [--repo R]      oracle and noop runs of every cell (no model, no cost)
  collect   [--results DIR]             metrics.json and a summary table from the run files
  pilot     --scratch S [--yes-real]    the 12 pilot runs in the frozen order (without --yes-real: the order only)
  budget    --pilot DIR                 full budget from the pilot run files (formula only)
  recount   --runs DIR [--results DIR]  tokens of old runs from their transcripts, old count against new

Development set (dev/README.md, for tuning the arm B block): `--repo click` on setup, generate,
validate, rehearse and run uses dev/ instead; `run --arm B --block FILE` swaps the instruction block.

Harnesses: oracle (applies the reference patches), noop (changes nothing, one tool call),
stub (scripted misbehaviour for the stop tests), real (3 Claude Code, 2 Codex).
The `real` harness needs the owner's yes and a budget; this kit never starts it on its own.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
TASKS_DIR = HERE / "tasks"
RESULTS = HERE / "results"
HOOK = HERE / "astro_hook.mjs"
FROZEN_BLOCK = HERE / "block-b.md"  # the block that `treeyard connect` writes; arm B refuses any other
REPOS = json.loads((HERE / "repos.json").read_text())
# The development set (dev/README.md): its own repos.json, taskdefs.py, tasks/ and results/, for tuning
# the arm B block. `use_dev` points the kit at it; without a dev repo name nothing here changes.
DEV = HERE / "dev"
DEV_REPOS = json.loads((DEV / "repos.json").read_text()) if (DEV / "repos.json").is_file() else {}
TASKDEFS = HERE
VENV = "requests-venv"
TASK_TOTAL = 30
SETS = ("disjoint", "colliding")
ARMS = ("A", "B")
AGENTS = 5
# Agent slots per repetition: 3 Claude Code, 2 Codex, rotated by one slot per repetition (preregistration v2).
# v1 had an OpenCode slot; its free tier ran out of quota during the v1 pilot (results/v1-exploratory).
MIX = ["claude", "claude", "claude", "codex", "codex"]

# Frozen per-agent limits (PREREGISTRATION.md). Tests pass smaller ones.
LIMITS = {"max_turns": 80, "max_tokens": 4_000_000, "timeout_s": 1800, "claude_budget_usd": 3.0}
HEALTH_EVERY_S = 2.0

HANDBACK_A = (
    "Your branch `{branch}` does not merge cleanly into `main`: git reports a conflict. Run `git merge main` "
    "in this directory, resolve every conflict so that your work and the work on `main` are both kept, "
    "and commit the merge."
)
HANDBACK_B = (
    "Your change does not land cleanly on main: it has a conflict with work that landed first. The files "
    "in this directory now hold that work with conflict markers. Resolve every conflict so that your work "
    "and the landed work are both kept, then run `treeyard workspace land {name} --url {url}`."
)

CARGO = os.environ.get("CARGO") or shutil.which("cargo") or "/opt/homebrew/opt/rustup/bin/cargo"


def treeyard_bin():
    return Path(os.environ.get("T2_TREEYARD") or ROOT / "target" / "debug" / "treeyard")


class Stop(Exception):
    """An anomaly: the whole run stops with a non-zero exit and a reason line."""


def sh(args, cwd=None, env=None, check=True, timeout=600, input=None):
    r = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout, input=input)
    if check and r.returncode != 0:
        raise RuntimeError(f"command failed ({r.returncode}): {' '.join(map(str, args))[:200]}\n{r.stdout[-2000:]}{r.stderr[-2000:]}")
    return r


def git(*args, cwd, check=True, env=None):
    e = dict(os.environ if env is None else env)
    e.update(GIT_AUTHOR_NAME="t2", GIT_AUTHOR_EMAIL="t2@example.com", GIT_COMMITTER_NAME="t2",
             GIT_COMMITTER_EMAIL="t2@example.com", GIT_CONFIG_NOSYSTEM="1")
    return sh(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", *args], cwd=cwd, env=e, check=check)


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


# ===================================================================================== tasks


def use_dev():
    """Points the kit at the development set: dev/repos.json, dev/taskdefs.py, dev/tasks, dev/results."""
    global REPOS, TASKS_DIR, RESULTS, TASKDEFS, VENV, TASK_TOTAL
    REPOS, TASKS_DIR, RESULTS, TASKDEFS, VENV = DEV_REPOS, DEV / "tasks", DEV / "results", DEV, "dev-venv"
    TASK_TOTAL = 10 * len(DEV_REPOS)


def select_kit(repos):
    """`use_dev` when the named repositories are dev ones; T2 and dev names do not mix in one command."""
    dev = [r for r in repos or [] if r in DEV_REPOS]
    if dev and len(dev) != len(repos):
        raise SystemExit("T2 and development repositories do not mix in one command")
    if dev:
        use_dev()
    return bool(dev)


def block_text(path):
    """The instruction block in `path`, between the connect markers (added when the file has none)."""
    text = Path(path).read_text().rstrip("\n")
    if "<!-- treeyard:connect:start -->" not in text:
        text = f"<!-- treeyard:connect:start -->\n{text}\n<!-- treeyard:connect:end -->"
    return text


def put_block(repo, block):
    """Writes `block` in place of the marked block that `treeyard connect` put in AGENTS.md and CLAUDE.md."""
    for name in ("AGENTS.md", "CLAUDE.md"):
        f = repo / name
        if f.is_file():
            f.write_text(re.sub(r"<!-- treeyard:connect:start -->.*?<!-- treeyard:connect:end -->", lambda _: block,
                                f.read_text(), count=1, flags=re.S))


def load_taskdefs():
    if TASKDEFS != HERE:
        import importlib.util

        spec = importlib.util.spec_from_file_location("t2_dev_taskdefs", TASKDEFS / "taskdefs.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod
    sys.path.insert(0, str(HERE))
    import taskdefs  # noqa: E402

    return taskdefs


def apply_edits(files, tasks):
    """Applies the edits of `tasks` (in order) to `files` ({path: text}); returns the new dict.

    Insertions at one anchor are grouped: their texts go next to the anchor in task order."""
    out = dict(files)
    groups = {}
    for t in tasks:
        for e in t["edits"]:
            if e["op"] == "replace":
                text = out[e["file"]]
                if text.count(e["old"]) != 1:
                    raise ValueError(f"{t['id']}: `old` occurs {text.count(e['old'])} times in {e['file']}")
                out[e["file"]] = text.replace(e["old"], e["new"])
            else:
                groups.setdefault((e["file"], e["anchor"], e["op"]), []).append(e["text"])
    for (f, anchor, op), texts in groups.items():
        text = out[f]
        if text.count(anchor) != 1:
            raise ValueError(f"anchor occurs {text.count(anchor)} times in {f}: {anchor[:60]!r}")
        joined = "".join(texts)
        out[f] = text.replace(anchor, joined + anchor if op == "insert_before" else anchor + joined)
    return out


def unified(before, after):
    """A git-style patch for the files that differ."""
    parts = []
    for f in sorted(set(before) | set(after)):
        a, b = before.get(f, ""), after.get(f, "")
        if a == b:
            continue
        parts.append(f"diff --git a/{f} b/{f}\n")
        parts.extend(difflib.unified_diff(a.splitlines(True), b.splitlines(True), f"a/{f}", f"b/{f}", n=3))
    return "".join(parts)


def test_path(repo, task):
    tid = task["id"].replace("-", "_")
    if "test_path" in REPOS[repo]:
        return REPOS[repo]["test_path"].format(tid=tid)
    if repo == "requests":
        return f"tests/test_t2_{tid}.py"
    if repo == "ripgrep":
        crate_dir = {"grep-cli": "cli", "ignore": "ignore", "globset": "globset", "grep-printer": "printer",
                     "grep-searcher": "searcher"}[task["test_crate"]]
        return f"crates/{crate_dir}/tests/t2_{tid}.rs"
    return f"packages/internal-helpers/test/t2_{tid}.test.ts"


def pinned_files(src, rev, paths):
    return {p: git("show", f"{rev}:{p}", cwd=src).stdout for p in paths}


def generate(scratch):
    """Writes tasks/<repo>/<set>/<k>-<id>/ and tasks/MANIFEST.json from taskdefs.py and the pinned clones."""
    td = load_taskdefs()
    manifest = {}
    for repo, sets in td.TASKS.items():
        src = scratch / "t2" / "src" / repo
        rev = REPOS[repo]["rev"]
        for s in SETS:
            tasks = sets[s]
            assert len(tasks) == AGENTS, (repo, s)
            paths = sorted({e["file"] for t in tasks for e in t["edits"]})
            if s == "disjoint":
                touched = [sorted({e["file"] for e in t["edits"]}) for t in tasks]
                flat = [p for ps in touched for p in ps]
                assert len(flat) == len(set(flat)), f"{repo} disjoint tasks share a file"
            base = pinned_files(src, rev, paths)
            set_dir = TASKS_DIR / repo / s
            if set_dir.exists():
                shutil.rmtree(set_dir)
            for k, t in enumerate(tasks, 1):
                d = set_dir / f"{k}-{t['id']}"
                d.mkdir(parents=True)
                prompt = f"# {t['title']}\n\n{t['prompt']}{td.SUFFIX}"
                if "treeyard" in prompt.lower():
                    raise SystemExit(f"{t['id']}: the prompt names TreeYard")
                (d / "prompt.md").write_text(prompt)
                (d / "reference.patch").write_text(unified(base, apply_edits(base, [t])))
                hidden = d / "hidden" / test_path(repo, t)
                hidden.parent.mkdir(parents=True)
                hidden.write_text(t["test"])
                if s == "colliding":
                    before = apply_edits(base, tasks[: k - 1])
                    after = apply_edits(base, tasks[:k])
                    (d / f"resolved-{k}.patch").write_text(unified(before, after))
                meta = {"repo": repo, "set": s, "k": k, "id": t["id"], "title": t["title"],
                        "files": sorted({e["file"] for e in t["edits"]}), "test": test_path(repo, t)}
                if "test_crate" in t:
                    meta["test_crate"] = t["test_crate"]
                (d / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")
                for f in sorted(p for p in d.rglob("*") if p.is_file()):
                    manifest[str(f.relative_to(HERE))] = sha256(f)
    (TASKS_DIR / "MANIFEST.json").write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n")
    print(f"generated {sum(1 for k in manifest if k.endswith('prompt.md'))} tasks, {len(manifest)} files")


def task_dirs(repo, s):
    d = TASKS_DIR / repo / s
    out = sorted((p for p in d.iterdir() if p.is_dir()), key=lambda p: int(p.name.split("-")[0]))
    return out


def meta(task_dir):
    return json.loads((task_dir / "meta.json").read_text())


# ===================================================================================== scoring


def repo_env(repo, scratch):
    env = dict(os.environ)
    env["TMPDIR"] = str(scratch / "t2" / "tmp")
    (scratch / "t2" / "tmp").mkdir(parents=True, exist_ok=True)
    if repo == "requests":
        env["PYTHONPATH"] = "src"
        env["PYTHONDONTWRITEBYTECODE"] = "1"
    if repo == "ripgrep":
        env["CARGO_TARGET_DIR"] = str(scratch / "t2" / "env" / "rg-target")
        env["PATH"] = str(Path(CARGO).parent) + os.pathsep + env.get("PATH", "")
    env.update(REPOS[repo].get("env", {}))
    return env


def fill(cmd, scratch, m=None):
    venv = scratch / "t2" / "env" / VENV
    vals = {"python": str(venv / "bin" / "python"), "hook": str(HOOK), "cargo": CARGO}
    if m:
        vals["test"] = m["test"]
        vals["test_crate"] = m.get("test_crate", "")
        vals["test_name"] = Path(m["test"]).stem
        vals["test_rel"] = str(Path(m["test"]).relative_to(REPOS[m["repo"]]["workdir"])) if REPOS[m["repo"]]["workdir"] != "." else m["test"]
    return [a.format(**vals) for a in cmd]


def run_cmd(repo, cmd, tree, scratch, timeout=900):
    wd = tree / REPOS[repo]["workdir"]
    t0 = time.time()
    try:
        r = subprocess.run(cmd, cwd=wd, env=repo_env(repo, scratch), capture_output=True, text=True, timeout=timeout)
        rc, out = r.returncode, (r.stdout + r.stderr)
    except subprocess.TimeoutExpired:
        rc, out = 124, f"timeout {timeout} s"
    return rc, out, round(time.time() - t0, 2)


def decide(repo, task_dir, tree, scratch):
    """Copies the hidden test of one task into `tree` and runs its deciding command. True = done."""
    m = meta(task_dir)
    dst = tree / m["test"]
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(task_dir / "hidden" / m["test"], dst)
    rc, out, secs = run_cmd(repo, fill(REPOS[repo]["decide"], scratch, m), tree, scratch)
    return rc == 0, out, secs


def suite(repo, tree, scratch):
    rc, out, secs = run_cmd(repo, fill(REPOS[repo]["suite"], scratch), tree, scratch, timeout=1800)
    return rc == 0, out, secs


def reset_tree(tree, rev):
    git("reset", "-q", "--hard", rev, cwd=tree)
    git("clean", "-q", "-fdx", cwd=tree)


def apply_patch(tree, patch, check=True):
    return git("apply", "--whitespace=nowarn", str(patch), cwd=tree, check=check)


def validate(scratch, repos):
    lines, ok_all = [], True
    for repo in repos:
        rev = REPOS[repo]["rev"]
        tree = scratch / "t2" / "validate" / repo
        if not tree.exists():
            tree.parent.mkdir(parents=True, exist_ok=True)
            git("clone", "-q", "--no-checkout", str(scratch / "t2" / "src" / repo), str(tree), cwd=scratch)
            git("checkout", "-q", "--detach", rev, cwd=tree)
        reset_tree(tree, rev)
        ok, _, secs = suite(repo, tree, scratch)
        lines.append(f"{repo} pinned {rev[:10]} suite: {'pass' if ok else 'FAIL'} ({secs} s)")
        ok_all &= ok
        for s in SETS:
            dirs = task_dirs(repo, s)
            for d in dirs:
                m = meta(d)
                reset_tree(tree, rev)
                fail_ok, out1, s1 = decide(repo, d, tree, scratch)
                reset_tree(tree, rev)
                apply_patch(tree, d / "reference.patch")
                pass_ok, out2, s2 = decide(repo, d, tree, scratch)
                good = (not fail_ok) and pass_ok
                ok_all &= good
                lines.append(f"{repo} {s} {m['k']} {m['id']}: pinned {'pass' if fail_ok else 'fail'} ({s1} s), "
                             f"reference {'pass' if pass_ok else 'fail'} ({s2} s) -> {'ok' if good else 'BAD'}")
                if not good:
                    lines.append("    " + (out2 if not pass_ok else out1)[-600:].replace("\n", "\n    "))
            # The whole set on one tree: colliding = resolved-1..5 in the oracle order, disjoint = all references.
            reset_tree(tree, rev)
            for d in dirs:
                p = d / (f"resolved-{meta(d)['k']}.patch" if s == "colliding" else "reference.patch")
                apply_patch(tree, p)
            sok, _, ssecs = suite(repo, tree, scratch)
            done = [decide(repo, d, tree, scratch)[0] for d in dirs]
            good = sok and all(done)
            ok_all &= good
            how = "resolved-1..5" if s == "colliding" else "reference 1..5"
            lines.append(f"{repo} {s} all 5 ({how}): suite {'pass' if sok else 'FAIL'} ({ssecs} s), "
                         f"tests {sum(done)}/5 pass -> {'ok' if good else 'BAD'}")
            if s == "colliding":
                pairs = pair_conflicts(scratch, repo, dirs)
                lines.append(f"{repo} colliding: {pairs} of 10 pairs of reference patches conflict in git")
            else:
                pairs = pair_conflicts(scratch, repo, dirs)
                lines.append(f"{repo} disjoint: {pairs} of 10 pairs of reference patches conflict in git")
    n = sum(1 for x in lines if x.endswith("-> ok") and " all 5 " not in x)
    lines.append(f"tasks that fail on the pin and pass with the reference: {n}/{TASK_TOTAL}")
    lines.append("validate: " + ("ok" if ok_all else "FAILED"))
    text = "\n".join(lines) + "\n"
    RESULTS.mkdir(exist_ok=True)
    if set(repos) == set(REPOS):
        (RESULTS / "validate.txt").write_text(text)
    print(text, end="")
    return ok_all


def pair_conflicts(scratch, repo, dirs):
    """How many of the 10 task pairs conflict when both reference patches merge on the pinned commit."""
    rev = REPOS[repo]["rev"]
    work = scratch / "t2" / "pairs" / repo
    if not work.exists():
        work.parent.mkdir(parents=True, exist_ok=True)
        git("clone", "-q", "--no-checkout", str(scratch / "t2" / "src" / repo), str(work), cwd=scratch)
        git("checkout", "-q", "--detach", rev, cwd=work)
    commits = []
    for d in dirs:
        reset_tree(work, rev)
        apply_patch(work, d / "reference.patch")
        git("add", "-A", cwd=work)
        git("commit", "-q", "-m", d.name, cwd=work)
        commits.append(git("rev-parse", "HEAD", cwd=work).stdout.strip())
        git("checkout", "-q", "--detach", rev, cwd=work)
    n = 0
    for i in range(len(commits)):
        for j in range(i + 1, len(commits)):
            r = git("merge-tree", "--write-tree", commits[i], commits[j], cwd=work, check=False)
            n += r.returncode != 0
    return n


def pair_table(scratch, repo, s):
    """{(i, j): True when the reference patches of tasks i and j conflict} (1-based task numbers)."""
    dirs = task_dirs(repo, s)
    rev = REPOS[repo]["rev"]
    work = scratch / "t2" / "pairs" / repo
    if not work.exists():
        pair_conflicts(scratch, repo, dirs)
    commits = []
    for d in dirs:
        reset_tree(work, rev)
        apply_patch(work, d / "reference.patch")
        git("add", "-A", cwd=work)
        git("commit", "-q", "-m", d.name, cwd=work)
        commits.append(git("rev-parse", "HEAD", cwd=work).stdout.strip())
        git("checkout", "-q", "--detach", rev, cwd=work)
    out = {}
    for i in range(5):
        for j in range(i + 1, 5):
            out[f"{i + 1}-{j + 1}"] = git("merge-tree", "--write-tree", commits[i], commits[j], cwd=work, check=False).returncode != 0
    return out


# ===================================================================================== agents


# Token fields of one agent. tokens_in is every input token, cached or not (preregistration section 4);
# the split is the price-free cost: fresh input (not read from a cache; Claude's cache writes are in it
# and also in tokens_cache_write), cached input, output.
TOKEN_KEYS = ("tokens_in", "tokens_out", "tokens_fresh_in", "tokens_cached_in", "tokens_cache_write")
SUM_KEYS = ("turns", "tool_calls", *TOKEN_KEYS, "cost_usd", "wall_s")


def new_stats(prior=None):
    """Counters of one agent process. `prior` carries the cumulative state of a resumed session (both CLIs
    report session totals on resume: Claude's `total_cost_usd`, Codex's `turn.completed` usage)."""
    st = {"turns": 0, "tool_calls": 0, **{k: 0 for k in TOKEN_KEYS}, "cost_usd": 0.0, "session": None,
          "models": [], "killed": None, "exit": None,
          # private (dropped from the run file): committed totals, the live segment of the current
          # Claude invocation (usage per message id), seen tool_use ids, session totals for deltas
          "_done": {k: 0 for k in ("turns", *TOKEN_KEYS)}, "_msgs": {}, "_tools": set(),
          "_cost_cum": {}, "_codex_cum": {}}
    for key in ("_cost_cum", "_codex_cum"):
        st[key] = dict((prior or {}).get(key) or {})
    return st


def _claude_usage(u):
    fresh = int(u.get("input_tokens") or 0) + int(u.get("cache_creation_input_tokens") or 0)
    cached = int(u.get("cache_read_input_tokens") or 0)
    return {"tokens_in": fresh + cached, "tokens_out": int(u.get("output_tokens") or 0), "tokens_fresh_in": fresh,
            "tokens_cached_in": cached, "tokens_cache_write": int(u.get("cache_creation_input_tokens") or 0)}


def _refresh(st):
    """tokens/turns = committed totals + the live segment (Claude messages since the last `result`)."""
    seg = {k: 0 for k in TOKEN_KEYS}
    for u in st["_msgs"].values():
        for k, v in _claude_usage(u).items():
            seg[k] += v
    for k in TOKEN_KEYS:
        st[k] = st["_done"][k] + seg[k]
    st["turns"] = st["_done"]["turns"] + len(st["_msgs"])


def parse_line(harness, line, st):
    """Updates the counters in `st` from one transcript line: turns, tool calls, tokens, cost, session, model.

    Claude Code (stream-json) emits one `assistant` event per content block, all with the same message id
    and the same usage: usage and turns count once per message id while it runs, and the `result` event of
    the invocation replaces them with its own `usage` and `num_turns` (the unit of `--max-turns`).
    `total_cost_usd` is the session total, so a resumed session adds only the difference.
    Codex (`--json`) reports usage only at `turn.completed`, as the thread total: a resume adds the difference."""
    try:
        ev = json.loads(line)
    except ValueError:
        return
    if not isinstance(ev, dict):
        return
    t = ev.get("type")
    if "_done" not in st:  # a plain dict from an old caller
        st.update({k: v for k, v in new_stats().items() if k not in st})
    if harness in ("script", "stub"):
        if t == "turn":
            st["turns"] += 1
            st["_done"]["turns"] += 1
        elif t == "tool_use":
            st["tool_calls"] += 1
        elif t == "usage":
            fresh, out = int(ev.get("input_tokens", 0)), int(ev.get("output_tokens", 0))
            for k, v in (("tokens_in", fresh), ("tokens_fresh_in", fresh), ("tokens_out", out)):
                st[k] += v
                st["_done"][k] += v
            st["cost_usd"] += float(ev.get("cost_usd", 0))
    elif harness == "claude":
        if t == "system" and ev.get("session_id"):
            st["session"] = ev["session_id"]
            if ev.get("model") and ev["model"] not in st["models"]:
                st["models"].append(ev["model"])
        if t == "assistant":
            msg = ev.get("message") or {}
            mid = msg.get("id") or ev.get("uuid") or f"event-{len(st['_msgs'])}"
            for c in msg.get("content") or []:
                if isinstance(c, dict) and c.get("type") == "tool_use":
                    tid = c.get("id") or f"{mid}-{len(st['_tools'])}"
                    if tid not in st["_tools"]:
                        st["_tools"].add(tid)
                        st["tool_calls"] += 1
            if msg.get("model") and msg["model"] not in st["models"] and not msg["model"].startswith("<"):
                st["models"].append(msg["model"])
            st["_msgs"][mid] = msg.get("usage") or {}
            _refresh(st)
        if t == "result":
            u = ev.get("usage")
            seg = _claude_usage(u) if isinstance(u, dict) else None
            if seg is None:
                seg = {k: 0 for k in TOKEN_KEYS}
                for mu in st["_msgs"].values():
                    for k, v in _claude_usage(mu).items():
                        seg[k] += v
            for k in TOKEN_KEYS:
                st["_done"][k] += seg[k]
            st["_done"]["turns"] += int(ev["num_turns"]) if isinstance(ev.get("num_turns"), int) else len(st["_msgs"])
            st["_msgs"] = {}
            _refresh(st)
            since = ev.get("model_usage_since_last_result")
            if isinstance(since, dict) and since:
                st["cost_usd"] += sum(float((m or {}).get("costUSD") or 0) for m in since.values())
            else:
                sid = ev.get("session_id") or st.get("session") or ""
                total = float(ev.get("total_cost_usd") or 0)
                prev = st["_cost_cum"].get(sid, 0.0)
                st["cost_usd"] += total - prev if total >= prev else total
            sid = ev.get("session_id") or st.get("session") or ""
            st["_cost_cum"][sid] = float(ev.get("total_cost_usd") or 0)
    elif harness == "codex":
        if t == "thread.started":
            st["session"] = ev.get("thread_id")
        item = ev.get("item") or {}
        if t == "item.started" and item.get("type") in ("mcp_tool_call", "command_execution", "file_change", "web_search"):
            st["tool_calls"] += 1
            st["turns"] += 1
            st["_done"]["turns"] += 1
        if t == "turn.completed":
            u = ev.get("usage") or {}
            cur = {"in": int(u.get("input_tokens", 0)), "cached": int(u.get("cached_input_tokens", 0)),
                   "out": int(u.get("output_tokens", 0))}
            tid = st.get("session") or ""
            prev = st["_codex_cum"].get(tid) or {"in": 0, "cached": 0, "out": 0}
            if cur["in"] < prev["in"] or cur["out"] < prev["out"]:
                prev = {"in": 0, "cached": 0, "out": 0}  # not a running total: take it as is
            d = {k: cur[k] - prev[k] for k in cur}
            st["_codex_cum"][tid] = cur
            for k, v in (("tokens_in", d["in"]), ("tokens_out", d["out"]), ("tokens_cached_in", d["cached"]),
                         ("tokens_fresh_in", d["in"] - d["cached"])):
                st[k] += v
                st["_done"][k] += v
    elif harness == "opencode":
        part = ev.get("part") or {}
        if ev.get("sessionID"):
            st["session"] = ev["sessionID"]
        if t == "tool_use":
            st["tool_calls"] += 1
        if t == "step_finish":
            st["turns"] += 1
            st["_done"]["turns"] += 1
            tok = part.get("tokens") or {}
            cache = tok.get("cache") or {}
            fresh = int(tok.get("input", 0)) + int(cache.get("write", 0))
            vals = {"tokens_in": fresh + int(cache.get("read", 0)), "tokens_out": int(tok.get("output", 0)) + int(tok.get("reasoning", 0)),
                    "tokens_fresh_in": fresh, "tokens_cached_in": int(cache.get("read", 0)), "tokens_cache_write": int(cache.get("write", 0))}
            for k, v in vals.items():
                st[k] += v
                st["_done"][k] += v
            st["cost_usd"] += float(part.get("cost") or 0)


def count_transcript(harness, lines):
    """The counters of a whole transcript file (all invocations of one agent, hand-backs included)."""
    st = new_stats()
    for line in lines:
        parse_line(harness, line, st)
    return public_stats(st)


def public_stats(st):
    out = {k: v for k, v in st.items() if not k.startswith("_")}
    out["cost_usd"] = round(out.get("cost_usd", 0.0), 6)
    return out


def merge_stats(prev, st):
    """The counters of all invocations of one agent: sums, the first limit kill, the newest session."""
    if not prev:
        return dict(st)
    out = dict(st)
    for key in SUM_KEYS:
        out[key] = round(prev.get(key, 0) + st.get(key, 0), 6)
    out["session"] = st.get("session") or prev.get("session")
    out["killed"] = prev.get("killed") or st.get("killed")
    out["models"] = list(dict.fromkeys((prev.get("models") or []) + (st.get("models") or [])))
    for key in ("_cost_cum", "_codex_cum"):
        out[key] = {**(prev.get(key) or {}), **(st.get(key) or {})}
    return out


def legacy_count(harness, lines):
    """Tokens and turns as the runner counted them before the fix (every Claude stream event, every
    Codex `turn.completed` in full). Only for `recount`, to restate old run files."""
    tin = tout = turns = 0
    for line in lines:
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if not isinstance(ev, dict):
            continue
        if harness == "claude" and ev.get("type") == "assistant":
            u = (ev.get("message") or {}).get("usage") or {}
            turns += 1
            tin += int(u.get("input_tokens", 0)) + int(u.get("cache_read_input_tokens", 0)) + int(u.get("cache_creation_input_tokens", 0))
            tout += int(u.get("output_tokens", 0))
        elif harness == "codex" and ev.get("type") == "turn.completed":
            u = ev.get("usage") or {}
            tin += int(u.get("input_tokens", 0))
            tout += int(u.get("output_tokens", 0))
        elif harness == "codex" and ev.get("type") == "item.started" and (ev.get("item") or {}).get("type") in (
                "mcp_tool_call", "command_execution", "file_change", "web_search"):
            turns += 1
    return {"tokens_in": tin, "tokens_out": tout, "turns": turns}


def transcript_harness(path):
    """claude, codex or script, from the first events of a transcript file."""
    with open(path, errors="replace") as f:
        for line in f:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            t = ev.get("type") if isinstance(ev, dict) else None
            if t in ("system", "assistant", "result"):
                return "claude"
            if t in ("thread.started", "turn.started", "turn.completed", "item.started", "item.completed"):
                return "codex"
            if t in ("turn", "tool_use", "usage", "note"):
                return "script"
    return None


def codex_live_usage(codex_home, thread_id):
    """The thread total that Codex writes to its rollout file after every model call (`token_count`
    events), or None. `--json` prints usage only at `turn.completed`; this is the live source."""
    if not codex_home or not thread_id:
        return None
    files = sorted(Path(codex_home).glob(f"sessions/**/rollout-*{thread_id}.jsonl"))
    if not files:
        return None
    last = None
    try:
        with open(files[-1], errors="replace") as f:
            for line in f:
                if '"token_count"' not in line:
                    continue
                try:
                    p = json.loads(line).get("payload") or {}
                except ValueError:
                    continue
                total = ((p.get("info") or {}).get("total_token_usage")) if p.get("type") == "token_count" else None
                if total:
                    last = total
    except OSError:
        return None
    if not last:
        return None
    return {"in": int(last.get("input_tokens", 0)), "cached": int(last.get("cached_input_tokens", 0)),
            "out": int(last.get("output_tokens", 0))}


def codex_models(codex_home):
    """Model names in the Codex rollout files (`turn_context`); `--json` does not print the model."""
    out = []
    for f in sorted(Path(codex_home).glob("sessions/**/rollout-*.jsonl")) if codex_home else []:
        try:
            for line in open(f, errors="replace"):
                if '"turn_context"' in line:
                    m = (json.loads(line).get("payload") or {}).get("model")
                    if m and m not in out:
                        out.append(m)
        except (OSError, ValueError):
            pass
    return out


def supervise(name, harness, cmd, cwd, env, out_path, limits, registry, prior=None, codex_home=None):
    """Runs one agent in its own process group and kills the group at a limit. Returns its counters.

    The limits hold over all invocations of the agent: `prior` is the agent's counters before this one
    (a hand-back resumes the session within what is left). Codex tokens come from its rollout file
    every HEALTH_EVERY_S while it runs (`codex_live_usage`), else only at `turn.completed`."""
    st = new_stats(prior)
    base = {k: (prior or {}).get(k, 0) for k in ("turns", "tokens_in", "tokens_out", "wall_s")}
    t0 = time.time()
    env = dict(env, PWD=str(cwd))  # OpenCode takes its project from $PWD
    left_s = limits["timeout_s"] - base["wall_s"]
    with open(out_path, "a") as out, open(str(out_path) + ".err", "a") as err:
        if left_s <= 0:
            st["killed"] = f"timeout {limits['timeout_s']} s"
            st["exit"] = None
            st["wall_s"] = 0.0
            return public_stats(st)
        p = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=err, stdin=subprocess.DEVNULL,
                             text=True, start_new_session=True)
        registry[name] = p
        lock = threading.Lock()

        def kill(why):
            with lock:
                if st["killed"] is None:
                    st["killed"] = why
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass

        def over(tokens):
            if base["turns"] + st["turns"] > limits["max_turns"]:
                return f"turns > {limits['max_turns']}"
            if base["tokens_in"] + base["tokens_out"] + tokens > limits["max_tokens"]:
                return f"tokens > {limits['max_tokens']}"
            return None

        timer = threading.Timer(left_s, kill, args=(f"timeout {limits['timeout_s']} s",))
        timer.start()
        done = threading.Event()
        if harness == "codex" and codex_home:
            start_cum = dict(st["_codex_cum"])

            def probe():
                while not done.wait(HEALTH_EVERY_S):
                    live = codex_live_usage(codex_home, st.get("session"))
                    if live is None:
                        continue
                    prev = start_cum.get(st.get("session") or "") or {"in": 0, "out": 0}
                    tokens = max(live["in"] - prev.get("in", 0), 0) + max(live["out"] - prev.get("out", 0), 0)
                    st["live_tokens"] = tokens
                    why = over(max(tokens, st["tokens_in"] + st["tokens_out"]))
                    if why:
                        kill(why)
                        return

            threading.Thread(target=probe, daemon=True).start()
        for line in p.stdout:
            out.write(line)
            out.flush()
            parse_line(harness, line, st)
            why = over(st["tokens_in"] + st["tokens_out"])
            if why:
                kill(why)
            if st["killed"]:
                break
        p.stdout.close()
        st["exit"] = p.wait()
        done.set()
        timer.cancel()
    st["wall_s"] = round(time.time() - t0, 2)
    return public_stats(st) | {"_cost_cum": st["_cost_cum"], "_codex_cum": st["_codex_cum"]}


def kill_all(registry):
    for p in list(registry.values()):
        if p.poll() is None:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass


def script_cmd(kind, ctx, k, extra=()):
    return [sys.executable, "-I", str(HERE / "t2.py"), f"agent-{kind}", "--ctx", str(ctx), "--k", str(k), *extra]


# ----------------------------------------------------------------------- scripted agents


def emit(ev):
    print(json.dumps(ev), flush=True)


def tool(cmd, cwd, env=None):
    emit({"type": "tool_use", "name": cmd[0] if cmd[0] != str(treeyard_bin()) else "treeyard " + cmd[1]})
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env=env)
    emit({"type": "tool_result", "exit": r.returncode, "out": (r.stdout + r.stderr)[-400:]})
    return r


def agent_noop(ctx, k):
    c = json.loads(Path(ctx).read_text())
    emit({"type": "turn"})
    if c["arm"] == "A":
        tool(["git", "status", "--short"], c["agents"][k - 1]["cwd"])
    else:
        tool([str(treeyard_bin()), "workspace", "status", c["agents"][k - 1]["workspace"], "--url", c["url"], "--home", c["home"]],
             c["agents"][k - 1]["cwd"])
    emit({"type": "usage", "input_tokens": 0, "output_tokens": 0, "cost_usd": 0})


def agent_oracle(ctx, k, resume=False):
    """Applies reference.patch. Arm A: commits; on a hand-back merges main and applies resolved-k.patch.
    Arm B: syncs, waits for tasks 1..k-1 to land (the fixed oracle order), lands, and on a land
    conflict writes main + resolved-k.patch and lands again."""
    c = json.loads(Path(ctx).read_text())
    a = c["agents"][k - 1]
    td = Path(a["task_dir"])
    cwd = a["cwd"]
    emit({"type": "turn"})
    resolved = td / f"resolved-{k}.patch"
    if c["arm"] == "A":
        if not resume:
            tool(["git", "apply", str(td / "reference.patch")], cwd)
            tool(["git", "add", "-A"], cwd)
            tool(["git", "-c", "user.name=oracle", "-c", "user.email=oracle@example.com", "commit", "-q", "-m", a["title"]], cwd)
            return
        tool(["git", "-c", "user.name=oracle", "-c", "user.email=oracle@example.com", "merge", "-q", "--no-edit", "main"], cwd)
        meta_ = meta(td)
        for f in meta_["files"]:
            show = subprocess.run(["git", "show", f"main:{f}"], cwd=cwd, capture_output=True, text=True)
            Path(cwd, f).write_text(show.stdout)
        tool(["git", "apply", str(resolved if resolved.exists() else td / "reference.patch")], cwd)
        tool(["git", "add", "-A"], cwd)
        tool(["git", "-c", "user.name=oracle", "-c", "user.email=oracle@example.com", "commit", "-q", "--no-edit", "-m", "merge main"], cwd)
        return
    ty = str(treeyard_bin())
    base = [ "--url", c["url"], "--home", c["home"]]
    name = a["workspace"]
    tool(["git", "apply", str(td / "reference.patch")], cwd)
    r = tool([ty, "workspace", "sync", name, *base], cwd)
    warned = any(x.get("kind") in ("overlap", "conflict") for x in json.loads(r.stdout or "{}").get("collisions", []))
    emit({"type": "note", "warned": warned})
    landed_dir = Path(c["run_dir"]) / "landed"
    while not all((landed_dir / str(j)).exists() for j in range(1, k)):
        time.sleep(0.2)
    for attempt in range(3):
        r = tool([ty, "workspace", "land", name, "--wait-s", "0", *base], cwd)
        status = (json.loads(r.stdout or "{}").get("land") or {}).get("status")
        if status == "landed":
            landed_dir.mkdir(exist_ok=True)
            (landed_dir / str(k)).write_text("")
            return
        if status != "conflict":
            emit({"type": "note", "land_status": status})
            return
        # Resolve: main's content of the task files plus this task's resolved patch.
        tmp = Path(c["run_dir"]) / f"oracle-{k}-{attempt}"
        tmp.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=tmp)
        for f in meta(td)["files"]:
            rd = tool([ty, "call", "--url", c["url"], "read", json.dumps({"path": f})], cwd)
            p = tmp / f
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(json.loads(rd.stdout)["content"])
        tool(["git", "apply", str(resolved if resolved.exists() else td / "reference.patch")], tmp)
        for f in meta(td)["files"]:
            shutil.copyfile(tmp / f, Path(cwd) / f)
    emit({"type": "note", "land_status": "gave up"})


def agent_stub(ctx, k, resume=False):
    """Scripted misbehaviour for the stop tests; the mode comes from T2_STUB_MODES (comma list per slot)."""
    modes = os.environ.get("T2_STUB_MODES", "ok,ok,ok,ok,ok").split(",")
    mode = modes[k - 1] if k - 1 < len(modes) else "ok"
    if mode == "ok" or resume:
        emit({"type": "turn"})
        emit({"type": "tool_use", "name": "noop"})
    elif mode == "loop-turns":
        while True:
            emit({"type": "turn"})
            emit({"type": "tool_use", "name": "noop"})
            time.sleep(0.01)
    elif mode == "tokens":
        while True:
            emit({"type": "turn"})
            emit({"type": "tool_use", "name": "noop"})
            emit({"type": "usage", "input_tokens": 1000, "output_tokens": 100})
            time.sleep(0.01)
            # turns stay under the turn limit in the test: tokens hit their limit first
    elif mode == "sleep":
        emit({"type": "turn"})
        emit({"type": "tool_use", "name": "noop"})
        time.sleep(3600)
    elif mode == "no-tools":
        emit({"type": "turn"})
        emit({"type": "usage", "input_tokens": 10, "output_tokens": 10})
    elif mode == "edit-conflict":
        # Arm A only: every agent rewrites line 1 and commits, so the merges conflict; on the
        # hand-back it does nothing, so the runner must step in.
        cwd = json.loads(Path(ctx).read_text())["agents"][k - 1]["cwd"]
        f = Path(cwd) / "a.txt"
        lines = f.read_text().splitlines(True)
        lines[0] = f"agent {k}\n"
        f.write_text("".join(lines))
        tool(["git", "-c", "user.name=stub", "-c", "user.email=stub@example.com", "commit", "-q", "-am", f"stub {k}"], cwd)
    elif mode == "slow-ok":
        emit({"type": "turn"})
        emit({"type": "tool_use", "name": "noop"})
        time.sleep(float(os.environ.get("T2_STUB_SLOW_S", "3")))


# ----------------------------------------------------------------------- real harness wiring


def real_command(harness, prompt, a, c, run_dir, limits, resume_msg=None, session=None):
    """The command line of one real agent. Config homes point to scratch copies (lesson of T1):
    the runner never runs a CLI against the user's real config directory."""
    env = dict(os.environ)
    env.pop("CLAUDECODE", None)
    env.pop("CLAUDE_CODE_ENTRYPOINT", None)
    home = Path(run_dir) / "home" / a["agent"]
    home.mkdir(parents=True, exist_ok=True)
    text = resume_msg or prompt
    if harness == "claude":
        env["CLAUDE_CONFIG_DIR"] = str(home)
        env["DISABLE_AUTOUPDATER"] = "1"
        env["DISABLE_TELEMETRY"] = "1"
        tok = "<dry-run>" if os.environ.get("T2_DRY") else claude_token()
        if tok is None:
            raise Stop("claude: no OAuth token in the Keychain item of the installed Claude Code")
        env["CLAUDE_CODE_OAUTH_TOKEN"] = tok
        servers = {}
        if c["arm"] == "B":
            servers["treeyard"] = {"command": c["mcp_command"], "args": c["mcp_args"] + ["--agent", a["agent"]],
                                   "env": {"TREEYARD_URL": c["url"]}}
        (home / "mcp.json").write_text(json.dumps({"mcpServers": servers}, indent=2))
        mcp = str(home / "mcp.json")
        cmd = ["claude", "-p", text, "--mcp-config", mcp, "--strict-mcp-config",
               "--max-turns", str(limits["max_turns"]), "--max-budget-usd", str(limits["claude_budget_usd"]),
               "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits",
               "--allowedTools", "Bash,Read,Edit,Write,Glob,Grep" + (",mcp__treeyard__*" if c["arm"] == "B" else "")]
        if session:
            cmd += ["--resume", session]
        return cmd, env
    if harness == "codex":
        env["CODEX_HOME"] = str(home)
        auth = Path.home() / ".codex" / "auth.json"
        if not os.environ.get("T2_DRY") and not (home / "auth.json").exists():
            if not auth.exists():
                raise Stop("codex: no login (auth.json) for the installed Codex CLI")
            shutil.copyfile(auth, home / "auth.json")  # read-only use of the real login; deleted after the run
            os.chmod(home / "auth.json", 0o600)
        cmd = ["codex", "exec"]
        if session:
            cmd += ["resume", session]
        # `codex exec resume` has no -s flag: the sandbox mode goes in as a config value, which both forms take.
        cmd += ["--json", "--skip-git-repo-check", "-c", "sandbox_mode=\"workspace-write\"", "-c", "approval_policy=\"never\""]
        if c["arm"] == "B":
            margs = c["mcp_args"] + ["--agent", a["agent"]]
            cmd += ["-c", f"mcp_servers.treeyard.command={json.dumps(c['mcp_command'])}", "-c", f"mcp_servers.treeyard.args={json.dumps(margs)}",
                    "-c", f"mcp_servers.treeyard.env={{TREEYARD_URL={json.dumps(c['url'])}}}",
                    "-c", "mcp_servers.treeyard.default_tools_approval_mode=\"approve\""]
        cmd.append(text)
        return cmd, env
    if harness == "opencode":
        x = home / "xdg"
        for d in ("config/opencode", "data", "cache", "state"):
            (x / d).mkdir(parents=True, exist_ok=True)
        cfg = {"$schema": "https://opencode.ai/config.json", "agent": {"build": {"steps": limits["max_turns"]}}}
        if c["arm"] == "B":
            cfg["mcp"] = {"treeyard": {"type": "local", "command": [c["mcp_command"], *c["mcp_args"], "--agent", a["agent"]],
                                       "enabled": True, "environment": {"TREEYARD_URL": c["url"]}}}
        (x / "config" / "opencode" / "opencode.json").write_text(json.dumps(cfg, indent=2))
        env.update(XDG_CONFIG_HOME=str(x / "config"), XDG_DATA_HOME=str(x / "data"), XDG_CACHE_HOME=str(x / "cache"),
                   XDG_STATE_HOME=str(x / "state"))
        cmd = ["opencode", "run", "--standalone", "--format", "json", "--auto", "--model",
               os.environ.get("T2_OPENCODE_MODEL", "opencode/deepseek-v4.1-flash-free")]
        if session:
            cmd += ["--session", session]
        cmd.append(text)
        return cmd, env
    raise ValueError(harness)


def claude_token():
    r = subprocess.run(["security", "find-generic-password", "-s", "Claude Code-credentials", "-w"], capture_output=True, text=True)
    if r.returncode != 0:
        return None
    try:
        return json.loads(r.stdout)["claudeAiOauth"]["accessToken"]
    except (ValueError, KeyError, TypeError):
        return None


USER_CONFIG = [".claude.json", ".claude/settings.json", ".claude/CLAUDE.md", ".codex/config.toml", ".config/opencode/opencode.json"]


def user_config_hashes():
    """One value per user config file: its sha256. `~/.claude.json` is the exception: every open Claude Code
    session of the operator writes it (counters, session state), so its value is the count of entries that
    name a run directory. A run must leave that count as it was."""
    out = {}
    for rel in USER_CONFIG:
        p = Path.home() / rel
        if not p.is_file():
            out["~/" + rel] = "missing"
        elif rel == ".claude.json":
            out["~/" + rel] = "run-path entries: %d" % p.read_text(errors="replace").count("/t2/runs/")
        else:
            out["~/" + rel] = sha256(p)
    return out


# ===================================================================================== one run


class Run:
    def __init__(self, scratch, arm, repo, s, rep, harness, limits=None, agents=None, tasks_dir=None, repo_spec=None,
                 run_dir=None, health_url=None, results=None, block_file=None):
        self.scratch, self.arm, self.repo, self.set, self.rep, self.harness = scratch, arm, repo, s, rep, harness
        self.limits = dict(LIMITS, **(limits or {}))
        self.n_agents = AGENTS if agents is None else agents
        self.tasks_dir = tasks_dir or TASKS_DIR
        self.spec = repo_spec or REPOS[repo]
        self.name = f"{arm}-{repo}-{s}-r{rep}-{harness}"
        # --block: another instruction block for arm B (dev set only); the run name carries its file stem.
        self.block_file = Path(block_file) if block_file else None
        if self.block_file:
            self.name += f"-{self.block_file.stem}"
        self.run_dir = run_dir or scratch / "t2" / "runs" / self.name
        self.results = results or RESULTS
        self.health_url = health_url
        self.registry = {}
        self.stop_reason = None
        self.log = []

    # -------------------------------------------------------------- setup

    def note(self, msg):
        self.log.append(f"{time.strftime('%H:%M:%S')} {msg}")

    def tasks(self):
        d = self.tasks_dir / self.repo / self.set
        return sorted((p for p in d.iterdir() if p.is_dir()), key=lambda p: int(p.name.split("-")[0]))

    def make_repo(self):
        repo = self.run_dir / "repo"
        src = Path(self.spec.get("local_src") or self.scratch / "t2" / "src" / self.repo)
        git("clone", "-q", "--no-checkout", str(src), str(repo), cwd=self.run_dir)
        git("checkout", "-q", "-B", "main", self.spec["rev"], cwd=repo)
        git("remote", "remove", "origin", cwd=repo)
        git("config", "user.name", "t2", cwd=repo)
        git("config", "user.email", "t2@example.com", cwd=repo)
        return repo

    def harness_of(self, k):
        if self.harness != "real":
            return self.harness
        return MIX[(k - 1 + self.rep - 1) % len(MIX)]

    def setup(self):
        if self.run_dir.exists():
            shutil.rmtree(self.run_dir)
        self.run_dir.mkdir(parents=True)
        self.repo_dir = self.make_repo()
        self.base = git("rev-parse", "HEAD", cwd=self.repo_dir).stdout.strip()
        tasks = self.tasks()
        if len(tasks) != AGENTS:
            raise Stop(f"agent count {len(tasks)} tasks, want {AGENTS}")
        tasks = tasks[: self.n_agents]
        self.agents = []
        counts = {}
        ctx = {"arm": self.arm, "run_dir": str(self.run_dir), "agents": []}
        if self.arm == "B":
            ty = treeyard_bin()
            r = sh([str(ty), "connect", "--harness", "claude,codex,opencode", "--agents", str(AGENTS), "--no-mirror"], cwd=self.repo_dir)
            self.note("connect: " + r.stdout.strip().splitlines()[0])
            st = json.loads(sh([str(ty), "connect", "status", "--json"], cwd=self.repo_dir).stdout)
            self.url = st["url"]
            if self.url.endswith(":7420"):
                raise Stop("connect picked port 7420 (the live server); refused")
            self.health_url = self.url + "/health"
            self.home = self.run_dir / "node"
            ctx.update(url=self.url, home=str(self.home))
            if self.block_file:
                put_block(self.repo_dir, block_text(self.block_file))
            block = (self.repo_dir / "AGENTS.md").read_text()
            m = re.search(r"<!-- treeyard:connect:start -->.*?<!-- treeyard:connect:end -->", block, re.S)
            self.block = m.group(0) if m else block
            if self.block_file:
                if self.block != block_text(self.block_file):
                    raise Stop(f"instruction block differs from {self.block_file.name} (--block); refused")
            elif self.block != FROZEN_BLOCK.read_text().rstrip("\n"):
                raise Stop("instruction block differs from bench/t2/block-b.md (frozen); refused")
            # The MCP command that connect registered, but with a fixed agent name per slot instead of
            # the name pool, so each change maps to one slot.
            mc, margs, _ = parse_codex_config(self.repo_dir)
            ctx["mcp_command"] = mc
            ctx["mcp_args"] = [x for i, x in enumerate(margs) if x not in ("--agent-pool", "--pool-dir")
                               and (i == 0 or margs[i - 1] not in ("--agent-pool", "--pool-dir"))]
        for k, td in enumerate(tasks, 1):
            m = json.loads((td / "meta.json").read_text())
            h = self.harness_of(k)
            slot = MIX[(k - 1 + self.rep - 1) % len(MIX)]
            counts[slot] = counts.get(slot, 0) + 1
            agent = f"{slot}-{counts[slot]}"
            a = {"k": k, "agent": agent, "harness": h, "task_dir": str(td), "title": m["title"], "id": m["id"]}
            if self.arm == "A":
                wt = self.run_dir / "wt" / f"agent-{k}"
                git("worktree", "add", "-q", "-b", f"agent-{k}", str(wt), "main", cwd=self.repo_dir)
                a["cwd"] = str(wt)
            else:
                env = dict(os.environ, TREEYARD_HOME=str(self.home))
                r = sh([str(treeyard_bin()), "workspace-hook", "create", "--name", f"agent-{k}", "--agent", agent,
                        "--intent", m["title"], "--url", self.url, "--home", str(self.home)], cwd=self.repo_dir, env=env)
                a["cwd"] = r.stdout.strip().splitlines()[-1]
                a["workspace"] = f"agent-{k}"
            self.agents.append(a)
        ctx["agents"] = self.agents
        self.ctx = self.run_dir / "ctx.json"
        self.ctx.write_text(json.dumps(ctx, indent=2))
        self.ctx_data = ctx
        if len(self.agents) != AGENTS:
            raise Stop(f"agent count {len(self.agents)}, want {AGENTS}")
        if self.arm == "B":
            wss = json.loads(sh([str(treeyard_bin()), "call", "--url", self.url, "changes", "{}"]).stdout)
            n_open = len(wss.get("changes", wss) if isinstance(wss, dict) else wss)
            if n_open != AGENTS:
                raise Stop(f"agent count: {n_open} open changes after setup, want {AGENTS}")

    def prompt(self, a):
        text = Path(a["task_dir"], "prompt.md").read_text()
        if self.arm == "B":
            return self.block + "\n\n" + text
        return text

    def command(self, a, resume_msg=None, session=None):
        h = a["harness"]
        if h in ("oracle", "noop", "stub"):
            extra = ["--resume"] if resume_msg else []
            return script_cmd(h, self.ctx, a["k"], extra), dict(os.environ), "script" if h != "stub" else "stub"
        cmd, env = real_command(h, self.prompt(a), a, self.ctx_data, self.run_dir, self.limits, resume_msg, session)
        return cmd, env, h

    # -------------------------------------------------------------- monitor

    def healthy(self):
        if not self.health_url:
            return True
        try:
            with urllib.request.urlopen(self.health_url, timeout=2) as r:
                return r.status == 200
        except OSError:
            return False

    def monitor(self, done):
        while not done.is_set():
            if not self.healthy():
                self.stop_reason = f"server down: {self.health_url} does not answer"
                kill_all(self.registry)
                return
            done.wait(HEALTH_EVERY_S)

    def run_agents(self, which=None, resume=None):
        """Starts the agents (all, or `which`) at once; stops the run on an anomaly."""
        which = which or self.agents
        if resume is None and len(which) != AGENTS:
            raise Stop(f"agent count {len(which)}, want {AGENTS}")
        if not self.healthy():
            raise Stop(f"server down: {self.health_url} does not answer")
        results = {}
        done = threading.Event()
        mon = threading.Thread(target=self.monitor, args=(done,), daemon=True)
        mon.start()

        def one(a):
            msg = (resume or {}).get(a["k"])
            cmd, env, parser = self.command(a, resume_msg=msg, session=(a.get("stats") or {}).get("session"))
            out = self.run_dir / f"agent-{a['k']}.jsonl"
            st = supervise(f"agent-{a['k']}", parser, cmd, a["cwd"], env, out, self.limits, self.registry,
                           prior=a.get("stats"), codex_home=env.get("CODEX_HOME"))
            results[a["k"]] = st
            # The anomaly is an agent that never acts; a hand-back answered in words is not one.
            if resume is None and st["tool_calls"] == 0 and self.stop_reason is None and st["killed"] is None:
                self.stop_reason = f"zero tool calls: agent-{a['k']} ({a['agent']}) made no tool call"
                kill_all(self.registry)

        threads = [threading.Thread(target=one, args=(a,)) for a in which]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        done.set()
        mon.join()
        for a in which:
            a["stats"] = merge_stats(a.get("stats"), results.get(a["k"], {}))
            a["invocations"] = a.get("invocations", 0) + 1
        if self.stop_reason:
            raise Stop(self.stop_reason)

    # -------------------------------------------------------------- finalize, arm A

    def own_diff_a(self, k, onto):
        # Three dots: from the merge base, so only the agent's own work counts (not the reverse of main's).
        return git("diff", f"{onto}...agent-{k}", cwd=self.repo_dir).stdout

    def finalize_a(self):
        merges, interventions = [], 0
        for a in self.agents:
            k = a["k"]
            wt = a["cwd"]
            if git("status", "--porcelain", cwd=wt).stdout.strip():
                git("add", "-A", cwd=wt)
                git("commit", "-q", "-m", f"agent-{k}: work left in the worktree", cwd=wt)
            onto = git("rev-parse", "main", cwd=self.repo_dir).stdout.strip()
            rec = {"k": k, "conflict": False, "handback": None, "intervention": False}
            r = git("merge", "-q", "--no-ff", "--no-edit", f"agent-{k}", cwd=self.repo_dir, check=False)
            if r.returncode != 0:
                rec["conflict"] = True
                rec["conflict_files"] = conflict_files(self.repo_dir)
                git("merge", "--abort", cwd=self.repo_dir, check=False)
                # One hand-back to the agent, within its limits.
                self.run_agents([a], resume={k: HANDBACK_A.format(branch=f"agent-{k}")})
                if git("status", "--porcelain", cwd=wt).stdout.strip():
                    git("add", "-A", cwd=wt)
                    git("commit", "-q", "-m", f"agent-{k}: work left after the hand-back", cwd=wt)
                r2 = git("merge", "-q", "--no-ff", "--no-edit", f"agent-{k}", cwd=self.repo_dir, check=False)
                markers = has_markers(self.repo_dir) if r2.returncode == 0 else True
                rec["handback"] = "resolved" if r2.returncode == 0 and not markers else "failed"
                if r2.returncode != 0 or markers:
                    if r2.returncode != 0:
                        git("merge", "--abort", cwd=self.repo_dir, check=False)
                    else:
                        git("reset", "-q", "--hard", onto, cwd=self.repo_dir)
                    # The stand-in for a person: take the agent's side of every conflicting hunk.
                    interventions += 1
                    rec["intervention"] = True
                    r3 = git("merge", "-q", "--no-ff", "--no-edit", "-X", "theirs", f"agent-{k}", cwd=self.repo_dir, check=False)
                    if r3.returncode != 0:
                        git("checkout", "--theirs", "--", ".", cwd=self.repo_dir, check=False)
                        git("add", "-A", cwd=self.repo_dir)
                        git("commit", "-q", "--no-edit", cwd=self.repo_dir, check=False)
            rec["own_diff"] = self.own_diff_a(k, onto)
            merges.append(rec)
        return merges, interventions

    # -------------------------------------------------------------- finalize, arm B

    def call(self, op, args):
        r = sh([str(treeyard_bin()), "call", "--url", self.url, op, json.dumps(args)], check=False)
        try:
            return json.loads(r.stdout)
        except ValueError:
            return {"error": (r.stdout + r.stderr)[-400:]}

    def changes_of(self, agent):
        out = []
        for state in ("open", "landed"):
            d = self.call("changes", {"state": state})
            for ch in d.get("changes", []) if isinstance(d, dict) else d:
                if ch.get("agent") == agent:
                    out.append(ch)
        return out

    def finalize_b(self):
        lands, interventions = [], 0
        ty = str(treeyard_bin())
        for a in self.agents:
            k = a["k"]
            rec = {"k": k, "conflict": False, "handback": None, "intervention": False, "runner_landed": []}
            for ch in [c for c in self.changes_of(a["agent"]) if c.get("state") == "open"]:
                cid = ch["id"]
                is_ws = ch.get("session") == f"workspace:{a['workspace']}"
                if is_ws:
                    r = sh([ty, "workspace", "land", a["workspace"], "--wait-s", "0", "--url", self.url, "--home", str(self.home)], check=False)
                    try:
                        land = json.loads(r.stdout).get("land") or {}
                    except ValueError:
                        land = {"status": "error", "detail": (r.stdout + r.stderr)[-300:]}
                else:
                    land = self.call("land", {"change": cid})
                status = land.get("status") or land.get("error", "error")
                rec["runner_landed"].append({"change": cid, "status": status})
                if status == "conflict":
                    rec["conflict"] = True
                    self.run_agents([a], resume={k: HANDBACK_B.format(name=a["workspace"], url=self.url)})
                    if self.call("change", {"change": cid}).get("state") != "landed":
                        if is_ws:
                            sh([ty, "workspace", "land", a["workspace"], "--wait-s", "0", "--url", self.url, "--home", str(self.home)], check=False)
                    info = self.call("change", {"change": cid})
                    if info.get("state") == "landed":
                        rec["handback"] = "resolved"
                    else:
                        rec["handback"] = "failed"
                        interventions += 1
                        rec["intervention"] = True
                        for cf in info.get("conflicts") or []:
                            self.call("resolve", {"change": cid, "path": cf["path"], "take": "ours"})
                        rec["runner_landed"].append({"change": cid, "status": self.call("land", {"change": cid}).get("status")})
            lands.append(rec)
        return lands, interventions

    def own_diffs_b(self):
        out = {}
        for a in self.agents:
            parts = []
            for ch in self.changes_of(a["agent"]):
                d = self.call("diff", {"change": ch["id"]})
                for f in d.get("files", []):
                    body = "\n".join(f.get("patch", "").splitlines()[2:])
                    parts.append(f"+++ b/{f['path']}\n{body}")
            out[a["k"]] = "\n".join(parts)
        return out

    # -------------------------------------------------------------- score

    def final_tree(self):
        tree = self.run_dir / "final"
        if self.arm == "A":
            git("clone", "-q", "--branch", "main", str(self.repo_dir), str(tree), cwd=self.run_dir)
        else:
            git("clone", "-q", f"{self.url}/git", str(tree), cwd=self.run_dir)
        return tree

    def score(self, tree):
        out = {}
        if self.spec.get("suite"):
            ok, text, secs = run_cmd_spec(self.spec, self.spec["suite"], tree, self.scratch)
            out["suite_green"] = ok
            out["suite_s"] = secs
        done = {}
        for td in self.tasks()[: self.n_agents]:
            m = json.loads((td / "meta.json").read_text())
            dst = tree / m["test"]
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(td / "hidden" / m["test"], dst)
            ok, text, secs = run_cmd_spec(self.spec, self.spec["decide"], tree, self.scratch, m)
            done[m["id"]] = ok
        out["done"] = done
        out["tasks_on_main"] = sum(done.values())
        return out

    # -------------------------------------------------------------- warnings (arm B)

    def warnings(self, pairs_truth):
        ev = self.call("events", {"since": 0, "limit": 100000}).get("events", [])
        audit = sorted(self.call("audit", {"limit": 100000}).get("entries", []), key=lambda r: r["seq"])
        k_of = {}
        for a in self.agents:
            for ch in self.changes_of(a["agent"]):
                k_of[ch["id"]] = a["k"]
        first_land = {}
        for r in audit:
            if r.get("op") == "land" and r.get("change") not in first_land:
                first_land[r["change"]] = r["seq"]
        warned, pairs_warned, reacted, kinds = set(), set(), set(), {}
        land_conflicts = sum(1 for e in ev if e.get("kind") == "conflict" and e.get("cause") == "land")
        for e in ev:
            if e.get("kind") != "collision" or (e.get("file") or {}).get("kind") not in ("overlap", "conflict"):
                continue
            ch, other = e.get("change"), e.get("other")
            if ch not in k_of or other not in k_of:
                continue
            # The apply that caused the event: the last apply of `ch` in or before the event's second.
            trig = [r for r in audit if r.get("change") == ch and r.get("op") == "apply" and r["time_ms"] // 1000 <= e["time"]]
            tseq = trig[-1]["seq"] if trig else 0
            for c_ in (ch, other):
                if first_land.get(c_, 10**18) > tseq:
                    warned.add(k_of[c_])
                    if any(r.get("change") == c_ and r.get("op") in ("apply", "resolve") and r["seq"] > tseq for r in audit):
                        reacted.add(k_of[c_])
            pair = tuple(sorted((k_of[ch], k_of[other])))
            pairs_warned.add(pair)
            kinds.setdefault(f"{pair[0]}-{pair[1]}", set()).add(e["file"]["kind"])
        false_alarms = sum(1 for (i, j) in pairs_warned if not pairs_truth.get(f"{i}-{j}", False))
        return {"warned_before_land": len(warned), "reacted": len(reacted), "warned_pairs": sorted(pairs_warned),
                "warned_pair_kinds": {p: sorted(v) for p, v in sorted(kinds.items())},
                "true_conflict_pairs": sorted(p for p, v in pairs_truth.items() if v),
                "false_alarms": false_alarms, "land_conflicts": land_conflicts}

    def dry_run(self):
        """Arm setup and the 5 real-agent command lines, with no agent started (wiring check, free)."""
        os.environ["T2_DRY"] = "1"
        lines = []
        try:
            self.setup()
            for a in self.agents:
                cmd, env, parser = self.command(a)
                shown = ["<prompt>" if x == self.prompt(a) else x for x in cmd]
                homes = {k: v for k, v in env.items() if k in ("CLAUDE_CONFIG_DIR", "CODEX_HOME", "XDG_CONFIG_HOME")}
                lines.append(json.dumps({"k": a["k"], "agent": a["agent"], "harness": a["harness"], "cwd": a["cwd"],
                                         "command": shown, "config_homes": homes, "prompt_sha256":
                                         hashlib.sha256(self.prompt(a).encode()).hexdigest()}))
        finally:
            os.environ.pop("T2_DRY", None)
            if self.arm == "B" and (self.repo_dir / ".git" / "treeyard").exists():
                sh([str(treeyard_bin()), "disconnect", "--purge"], cwd=self.repo_dir, check=False)
        return lines

    # -------------------------------------------------------------- the whole run

    def execute(self, pairs_truth=None):
        t0 = time.time()
        result = {"run": self.name, "arm": self.arm, "repo": self.repo, "set": self.set, "rep": self.rep,
                  "harness": self.harness, "limits": self.limits, "started": time.strftime("%Y-%m-%dT%H:%M:%S")}
        if self.harness == "real":
            result["user_config_before"] = user_config_hashes()
        try:
            self.setup()
            result["agents"] = [{k: a[k] for k in ("k", "agent", "harness", "id")} for a in self.agents]
            if self.arm == "B":
                result["block_sha256"] = hashlib.sha256(self.block.encode()).hexdigest()
                if self.block_file:
                    result["block_file"] = self.block_file.name
            t_agents = time.time()
            self.run_agents()
            if self.arm == "A":
                merges, interventions = self.finalize_a()
                own = {m["k"]: m.pop("own_diff") for m in merges}
                result["merges"] = merges
                result["merge_conflicts"] = sum(m["conflict"] for m in merges)
                result.update(warned_before_land=0, reacted=0, false_alarms=0, land_conflicts=0)
            else:
                lands, interventions = self.finalize_b()
                result["lands"] = lands
                result.update(self.warnings(pairs_truth or {}))
                result["merge_conflicts"] = result["land_conflicts"]
                own = self.own_diffs_b()
            result["interventions"] = interventions
            result["wall_s"] = round(time.time() - t_agents, 2)
            result["setup_s"] = round(t_agents - t0, 2)
            tree = self.final_tree()
            result["lost_lines"] = lost_lines(own, tree)
            result.update(self.score(tree))
            result["status"] = "done"
        except Stop as e:
            kill_all(self.registry)
            result["status"] = "stopped"
            result["stop_reason"] = str(e)
            result["wall_s"] = round(time.time() - t0, 2)
        finally:
            kill_all(self.registry)
            if self.arm == "B" and getattr(self, "repo_dir", None) and (self.repo_dir / ".git" / "treeyard").exists():
                sh([str(treeyard_bin()), "disconnect", "--purge"], cwd=self.repo_dir, check=False)
            for p in (self.run_dir / "home").glob("*/auth.json") if (self.run_dir / "home").exists() else []:
                p.unlink()
        result["agent_stats"] = [{"k": a["k"], "harness": a["harness"], "invocations": a.get("invocations", 0),
                                  **{k: v for k, v in (a.get("stats") or {}).items() if k != "session" and not k.startswith("_")}}
                                 for a in getattr(self, "agents", [])]
        result["tokens_in"] = sum(s.get("tokens_in", 0) for s in result["agent_stats"])
        result["tokens_out"] = sum(s.get("tokens_out", 0) for s in result["agent_stats"])
        result["cost_usd"] = round(sum(s.get("cost_usd", 0) for s in result["agent_stats"]), 6)
        result["tool_calls"] = sum(s.get("tool_calls", 0) for s in result["agent_stats"])
        result["limit_kills"] = [f"agent-{s['k']}: {s['killed']}" for s in result["agent_stats"] if s.get("killed")]
        if self.harness == "real":
            result["user_config_after"] = user_config_hashes()
            result["user_config_unchanged"] = result["user_config_before"] == result["user_config_after"]
        result["log"] = self.log
        self.results.mkdir(parents=True, exist_ok=True)
        (self.results / f"{self.name}.json").write_text(json.dumps(result, indent=1, sort_keys=True) + "\n")
        return result


def run_cmd_spec(spec, cmd, tree, scratch, m=None):
    for repo, s in REPOS.items():
        if s is spec:
            rc, out, secs = run_cmd(repo, fill(cmd, scratch, m), tree, scratch)
            return rc == 0, out, secs
    # A test repository (stop tests): commands run as given, in the tree.
    vals = {"test": m["test"]} if m else {}
    r = subprocess.run([c.format(**vals) for c in cmd], cwd=tree, capture_output=True, text=True, timeout=120)
    return r.returncode == 0, r.stdout + r.stderr, 0.0


def parse_codex_config(repo):
    text = (repo / ".codex" / "config.toml").read_text()
    cmd = re.search(r'^command = "(.*)"$', text, re.M).group(1)
    args = json.loads(re.search(r"^args = (\[.*\])$", text, re.M).group(1))
    url = re.search(r'^TREEYARD_URL = "(.*)"$', text, re.M).group(1)
    return cmd, args, url


def conflict_files(tree):
    return sorted(set(git("diff", "--name-only", "--diff-filter=U", cwd=tree, check=False).stdout.split()))


def has_markers(tree):
    r = git("grep", "-l", "-E", "^(<<<<<<<|>>>>>>>)( |$)", cwd=tree, check=False)
    return bool(r.stdout.strip())


def lost_lines(own, tree):
    """Lines that an agent added (its own diff) and that final main does not have."""
    lost = {}
    for k, diff in own.items():
        n, path = 0, None
        for line in diff.splitlines():
            if line.startswith("+++ "):
                path = line[6:] if line.startswith("+++ b/") else None
                continue
            if line.startswith("+") and path:
                text = line[1:].strip()
                if len(text) < 4 or not re.search(r"\w", text):
                    continue
                f = tree / path
                if not f.exists() or text not in f.read_text(errors="replace"):
                    n += 1
        lost[str(k)] = n
    return {"per_agent": lost, "total": sum(lost.values())}


# ===================================================================================== collect, budget


METRICS = ["suite_green", "tasks_on_main", "merge_conflicts", "warned_before_land", "reacted", "interventions",
           "wall_s", "lost_lines", "tokens_in", "tokens_out", "cost_usd", "false_alarms", "tool_calls"]


def collect(results_dir, out_json, out_txt, only=None):
    rows = []
    for f in sorted(Path(results_dir).glob("*.json")):
        if f.name in ("metrics.json",):
            continue
        r = json.loads(f.read_text())
        if "run" not in r or (only and r["harness"] not in only):
            continue
        rows.append(r)
    table = []
    for r in rows:
        row = {"run": r["run"], "status": r.get("status"), "stop_reason": r.get("stop_reason")}
        for m in METRICS:
            v = r.get(m)
            row[m] = v["total"] if isinstance(v, dict) and "total" in v else v
        table.append(row)
    Path(out_json).write_text(json.dumps({"runs": table}, indent=1) + "\n")
    hdr = ["run", "status", "suite", "done", "merge/land conflicts", "warned", "reacted", "interv", "lost", "false alarms", "tool calls", "tokens", "cost", "wall s"]
    lines = ["| " + " | ".join(hdr) + " |", "|" + "---|" * len(hdr)]
    for r in table:
        lines.append("| " + " | ".join(str(x) for x in [
            r["run"], r["status"] if r["status"] == "done" else f"stopped: {r['stop_reason']}",
            {True: "green", False: "red", None: "-"}[r["suite_green"]], f"{r['tasks_on_main']}/5" if r["tasks_on_main"] is not None else "-",
            r["merge_conflicts"], r["warned_before_land"], r["reacted"], r["interventions"], r["lost_lines"], r["false_alarms"],
            r["tool_calls"], (r["tokens_in"] or 0) + (r["tokens_out"] or 0), r["cost_usd"], r["wall_s"]]) + " |")
    text = "\n".join(lines) + "\n"
    Path(out_txt).write_text(text)
    print(text, end="")
    return table


def ref_size(repo):
    n = 0
    for s in SETS:
        for d in task_dirs(repo, s):
            n += sum(1 for line in (d / "reference.patch").read_text().splitlines() if line[:1] in "+-" and line[:3] not in ("+++", "---"))
    return n


def budget(pilot_dir):
    """Full budget = mean pilot cost per run x 12 runs per repo x sum of repo factors x 1.25.
    A repo factor is its reference diff size over the pilot repo's (pilot repo factor = 1)."""
    runs = [json.loads(f.read_text()) for f in sorted(Path(pilot_dir).glob("*.json")) if f.name != "metrics.json"]
    runs = [r for r in runs if r.get("harness") == "real" and r.get("status") == "done"]
    if not runs:
        raise SystemExit("no finished real pilot runs in " + str(pilot_dir))
    pilot_repo = runs[0]["repo"]
    mean_cost = sum(r["cost_usd"] for r in runs) / len(runs)
    mean_tokens = sum(r["tokens_in"] + r["tokens_out"] for r in runs) / len(runs)
    factors = {repo: ref_size(repo) / ref_size(pilot_repo) for repo in REPOS}
    total = mean_cost * 12 * sum(factors.values()) * 1.25
    out = {"pilot_runs": len(runs), "pilot_repo": pilot_repo, "mean_cost_usd_per_run": round(mean_cost, 4),
           "mean_tokens_per_run": round(mean_tokens), "repo_factors": {k: round(v, 3) for k, v in factors.items()},
           "full_budget_usd": round(total, 2), "formula": "mean cost per run x 12 x sum(repo factors) x 1.25"}
    print(json.dumps(out, indent=1))
    return out


def recount(runs_dir, results_dirs=()):
    """Tokens of old runs from their transcripts, counted the old way and the new way (`parse_line`).
    Prints one row per run directory and the B / A ratio per set; changes no file."""
    recorded = {}
    for d in results_dirs or ():
        for f in Path(d).glob("*.json"):
            try:
                r = json.loads(f.read_text())
            except ValueError:
                continue
            if isinstance(r, dict) and "run" in r and "tokens_in" in r:
                recorded.setdefault(r["run"], r.get("tokens_in", 0) + r.get("tokens_out", 0))
    rows = []
    for run in sorted(p for p in Path(runs_dir).iterdir() if p.is_dir() and list(p.glob("agent-*.jsonl"))):
        harness_of = {}
        ctx = run / "ctx.json"
        if ctx.is_file():
            try:
                harness_of = {a["k"]: a["harness"] for a in json.loads(ctx.read_text()).get("agents", [])}
            except (ValueError, KeyError):
                pass
        row = {"run": run.name, "old": 0, "new": 0, "recorded": recorded.get(run.name), "by_harness": {}}
        for t in sorted(run.glob("agent-*.jsonl"), key=lambda p: int(p.stem.split("-")[1])):
            k = int(t.stem.split("-")[1])
            h = harness_of.get(k) or transcript_harness(t)
            if h not in ("claude", "codex"):
                continue
            lines = t.read_text(errors="replace").splitlines()
            old = legacy_count(h, lines)
            new = count_transcript(h, lines)
            row["old"] += old["tokens_in"] + old["tokens_out"]
            n = new["tokens_in"] + new["tokens_out"]
            row["new"] += n
            hb = row["by_harness"].setdefault(h, {"old": 0, "new": 0})
            hb["old"] += old["tokens_in"] + old["tokens_out"]
            hb["new"] += n
        rows.append(row)
    lines = ["| run | recorded | old count | new count | new / old | Claude old -> new | Codex old -> new |", "|---|---|---|---|---|---|---|"]
    for r in rows:
        hb = r["by_harness"]
        lines.append("| " + " | ".join(str(x) for x in [
            r["run"], r["recorded"] if r["recorded"] is not None else "-", r["old"], r["new"],
            f"{r['new'] / r['old']:.3f}" if r["old"] else "-",
            *(f"{hb[h]['old']} -> {hb[h]['new']}" if h in hb else "-" for h in ("claude", "codex"))]) + " |")
    cells = {}
    for r in rows:
        m = re.match(r"^(A2|A|B)-(\w+)-(disjoint|colliding)-", r["run"])
        if m:
            c = cells.setdefault((m.group(2), m.group(3)), {}).setdefault(m.group(1), {"old": 0, "new": 0, "n": 0})
            if r["old"]:
                c["old"] += r["old"]
                c["new"] += r["new"]
                c["n"] += 1
    for (repo, s), c in sorted(cells.items()):
        if c.get("A", {}).get("n") and c.get("B", {}).get("n"):
            ratio = {key: (c["B"][key] / c["B"]["n"]) / (c["A"][key] / c["A"]["n"]) for key in ("old", "new")}
            lines.append(f"{repo} {s}: B / A mean tokens per run ({c['B']['n']} B, {c['A']['n']} A runs): "
                         f"old {ratio['old']:.2f}, new {ratio['new']:.2f}")
    text = "\n".join(lines) + "\n"
    print(text, end="")
    return rows


# ===================================================================================== setup


def pilot_order():
    """The frozen pilot order (PREREGISTRATION.md section 9): requests, 3 repetitions x 2 sets x 2 arms."""
    out = []
    for rep in (1, 2, 3):
        arms = ("A", "B") if rep in (1, 3) else ("B", "A")
        for s in SETS:
            for arm in arms:
                out.append((arm, s, rep))
    return out


def setup(scratch):
    src = scratch / "t2" / "src"
    src.mkdir(parents=True, exist_ok=True)
    for repo, spec in REPOS.items():
        d = src / repo
        if not (d / ".git").exists():
            git("init", "-q", str(d), cwd=src)
            git("fetch", "-q", "--depth", "1", spec["url"], spec["rev"], cwd=d)
        git("checkout", "-q", "--detach", spec["rev"], cwd=d)
        print(f"{repo}: {git('rev-parse', 'HEAD', cwd=d).stdout.strip()}")
    if REPOS is DEV_REPOS:
        venv = scratch / "t2" / "env" / VENV
        if not venv.exists():
            sh(["uv", "venv", "-q", str(venv), "--python", "3.12"])
            pkgs = sorted({p for spec in REPOS.values() for p in spec.get("venv_packages", [])})
            sh(["uv", "pip", "install", "-q", "--python", str(venv / "bin" / "python"), *pkgs])
        print("dev venv:", venv)
        return
    venv = scratch / "t2" / "env" / "requests-venv"
    if not venv.exists():
        sh(["uv", "venv", "-q", str(venv), "--python", "3.12"])
        sh(["uv", "pip", "install", "-q", "--python", str(venv / "bin" / "python"), *REPOS["requests"]["venv_packages"]])
    print("requests venv:", venv)
    env = repo_env("ripgrep", scratch)
    r = subprocess.run([CARGO, "test", "-q", "--no-run", *sum((["-p", c] for c in REPOS["ripgrep"]["suite_crates"]), [])],
                       cwd=src / "ripgrep", env=env, capture_output=True, text=True)
    print("ripgrep warm build:", "ok" if r.returncode == 0 else r.stderr[-500:])


# ===================================================================================== main


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for n in ("setup", "generate", "validate", "run", "rehearse", "pilot"):
        p = sub.add_parser(n)
        p.add_argument("--scratch", required=True, type=Path)
        if n in ("setup", "generate") and DEV_REPOS:
            p.add_argument("--repo", action="append", choices=list(DEV_REPOS),
                           help="the development set (dev/README.md); without it, the T2 repositories")
        if n in ("validate", "rehearse"):
            p.add_argument("--repo", action="append", choices=list(REPOS) + list(DEV_REPOS))
        if n == "pilot":
            p.add_argument("--yes-real", action="store_true")
        if n == "run":
            p.add_argument("--arm", required=True, choices=ARMS)
            p.add_argument("--repo", required=True, choices=list(REPOS) + list(DEV_REPOS))
            p.add_argument("--set", required=True, choices=SETS)
            p.add_argument("--rep", required=True, type=int)
            p.add_argument("--harness", required=True, choices=["oracle", "noop", "stub", "real"])
            p.add_argument("--yes-real", action="store_true", help="needed for --harness real (owner's yes and budget)")
            p.add_argument("--dry-run", action="store_true", help="set up the arm and print the agent commands; start no agent")
            p.add_argument("--block", type=Path, help="arm B, development set only: this instruction block instead of block-b.md")
    p = sub.add_parser("collect")
    p.add_argument("--results", type=Path, default=RESULTS)
    p.add_argument("--out", type=Path)
    p.add_argument("--summary", type=Path)
    p.add_argument("--only", action="append")
    p = sub.add_parser("budget")
    p.add_argument("--pilot", required=True, type=Path)
    p = sub.add_parser("recount", help="tokens of old runs from their transcripts: old count against new")
    p.add_argument("--runs", required=True, type=Path, help="a directory of run directories (agent-N.jsonl)")
    p.add_argument("--results", action="append", type=Path, help="run files to show the recorded total from")
    for n in ("agent-oracle", "agent-noop", "agent-stub"):
        p = sub.add_parser(n)
        p.add_argument("--ctx", required=True)
        p.add_argument("--k", required=True, type=int)
        p.add_argument("--resume", action="store_true")
    a = ap.parse_args(argv)
    if a.cmd == "agent-oracle":
        return agent_oracle(a.ctx, a.k, a.resume)
    if a.cmd == "agent-noop":
        return agent_noop(a.ctx, a.k)
    if a.cmd == "agent-stub":
        return agent_stub(a.ctx, a.k, a.resume)
    if a.cmd == "collect":
        collect(a.results, a.out or a.results / "metrics.json", a.summary or a.results / "summary.txt", a.only)
        return
    if a.cmd == "budget":
        budget(a.pilot)
        return
    if a.cmd == "recount":
        recount(a.runs, a.results or ())
        return
    scratch = a.scratch.resolve()
    dev = select_kit([a.repo] if isinstance(getattr(a, "repo", None), str) else getattr(a, "repo", None))
    if getattr(a, "block", None):
        if a.arm != "B" or not dev:
            raise SystemExit("--block is for arm B on the development set only (dev/README.md); T2 runs use block-b.md")
        if not a.block.is_file():
            raise SystemExit(f"--block: no file {a.block}")
        a.block = a.block.resolve()
    if a.cmd == "setup":
        return setup(scratch)
    if a.cmd == "generate":
        return generate(scratch)
    if a.cmd == "validate":
        sys.exit(0 if validate(scratch, a.repo or list(REPOS)) else 1)
    if a.cmd == "run":
        if a.dry_run:
            for line in Run(scratch, a.arm, a.repo, a.set, a.rep, a.harness, block_file=a.block).dry_run():
                print(line)
            return
        if a.harness == "real" and not a.yes_real:
            raise SystemExit("--harness real starts paid or quota-using agents: it needs --yes-real (the owner's yes and a budget)")
        truth = pair_table(scratch, a.repo, a.set) if a.arm == "B" else None
        r = Run(scratch, a.arm, a.repo, a.set, a.rep, a.harness, block_file=a.block).execute(truth)
        print(json.dumps({k: r.get(k) for k in ("run", "status", "stop_reason", "tasks_on_main", "suite_green", "merge_conflicts",
                                                 "warned_before_land", "interventions", "wall_s")}))
        sys.exit(0 if r["status"] == "done" else 3)
    if a.cmd == "pilot":
        order = pilot_order()
        if not a.yes_real:
            for arm, s, rep in order:
                print(f"run --arm {arm} --repo requests --set {s} --rep {rep} --harness real")
            print("(order only: --yes-real starts the 12 pilot runs; it needs the owner's yes and a budget)")
            return
        bad = 0
        for arm, s, rep in order:
            r = Run(scratch, arm, "requests", s, rep, "real").execute(pair_table(scratch, "requests", s) if arm == "B" else None)
            bad += r["status"] != "done"
            print(json.dumps({k: r.get(k) for k in ("run", "status", "stop_reason", "tasks_on_main", "interventions", "wall_s")}), flush=True)
        sys.exit(1 if bad else 0)
    if a.cmd == "rehearse":
        bad = 0
        for repo in a.repo or list(REPOS):
            truth = {s: pair_table(scratch, repo, s) for s in SETS}
            for harness in ("oracle", "noop"):
                for s in SETS:
                    for arm in ARMS:
                        r = Run(scratch, arm, repo, s, 1, harness).execute(truth[s] if arm == "B" else None)
                        bad += r["status"] != "done"
                        print(json.dumps({k: r.get(k) for k in ("run", "status", "stop_reason", "tasks_on_main", "suite_green",
                                                                 "merge_conflicts", "warned_before_land", "reacted", "interventions",
                                                                 "false_alarms", "wall_s")}), flush=True)
        sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
