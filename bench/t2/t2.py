#!/usr/bin/env python3
"""T2 experiment kit: plain git worktrees (arm A), worktrees plus `t2-integrate` (arm A2) and `treeyard
connect` (arm B), 5 agents at once (10 on the development set `mixed`).

Standard library only. Subcommands (see bench/t2/README.md and PREREGISTRATION.md):

  setup     --scratch S                 pinned clones, the requests venv, a warm cargo build
  generate                              tasks/ from taskdefs.py (prompts, hidden tests, patches)
  validate  --scratch S [--repo R]      each test fails on the pin and passes with its reference
  run       --scratch S --arm A|A2|B --repo R --set disjoint|colliding|mixed --rep N --harness H [--results D] [--agent-root D]
  rehearse  --scratch S [--repo R]      oracle and noop runs of every cell (no model, no cost)
  solo      --scratch S --repo R --set S --rep N --harness H   each task by one agent alone on the pin
  collect   [--results DIR]             metrics.json and a summary table from the run files
  pilot     --scratch S [--yes-real]    the 12 pilot runs in the frozen order (without --yes-real: the order only)
  budget    --pilot DIR                 full budget from the pilot run files (formula only)
  recount   --runs DIR [--results DIR]  tokens of old runs from their transcripts, old count against new

Development set (dev/README.md, for tuning the arm B block): `--repo click` on setup, generate,
validate, rehearse and run uses dev/ instead; `run --arm B --block FILE` swaps the instruction block.

Harnesses: oracle (applies the reference patches), noop (changes nothing, one tool call),
stub (scripted misbehaviour for the stop tests), real (3 Claude Code, 2 Codex; a set.json can set
other slots: the `mixed` set has 6 Claude Code, 4 Codex).
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
import tempfile
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
FROZEN_BLOCK_BN = HERE / "block-bn.md"  # the block of `treeyard connect --native`; arm BN refuses any other
REPOS = json.loads((HERE / "repos.json").read_text())
# The development set (dev/README.md): its own repos.json, taskdefs.py, tasks/ and results/, for tuning
# the arm B block. `use_dev` points the kit at it; without a dev repo name nothing here changes.
DEV = HERE / "dev"
DEV_REPOS = json.loads((DEV / "repos.json").read_text()) if (DEV / "repos.json").is_file() else {}
TASKDEFS = HERE
VENV = "requests-venv"
TASK_TOTAL = 30
SETS = ("disjoint", "colliding")
# Sets beyond the two of preregistration v3, defined in sibling modules `taskdefs_<name>.py` (only the
# development set has them: dev/taskdefs_mixed.py, dev/taskdefs_heavy.py, dev/taskdefs_hard.py,
# dev/taskdefs_long.py). Their
# tasks/<repo>/<set>/set.json holds the set-level settings: agent count, harness slots, limits, integration tests
# (`set_spec`).
EXTRA_SETS = ("mixed", "heavy", "hard", "long")
ARMS = ("A", "A2", "A3", "B", "BN")  # A2: worktrees plus t2-integrate (git-native coordinated baseline); not in preregistration v3
# A3: arm A2 with a block that tells the agent to merge main into its branch often while it works ("git, merge
# often"); not in preregistration v3. `rehearse` runs it only when asked (`--arms`).
A2_ARMS = ("A2", "A3")
GIT_ARMS = ("A", "A2", "A3")
REHEARSE_ARMS = ("A", "A2", "B", "BN")
# BN: arm B connected in native mode (`connect --native`): each agent edits and tests a directory of its own
# with its own tools and sends the result with the bridge's `sync` and `land`. Not in preregistration v3.
B_ARMS = ("B", "BN")
AGENTS = 5
# Agent slots per repetition: 3 Claude Code, 2 Codex, rotated by one slot per repetition (preregistration v2).
# v1 had an OpenCode slot; its free tier ran out of quota during the v1 pilot (results/v1-exploratory).
MIX = ["claude", "claude", "claude", "codex", "codex"]

# Frozen per-agent limits (PREREGISTRATION.md). Tests pass smaller ones.
LIMITS = {"max_turns": 80, "max_tokens": 4_000_000, "timeout_s": 1800, "claude_budget_usd": 3.0}
HEALTH_EVERY_S = 2.0

# The hand-back texts of preregistration v3 (section 6). Kept for reference: the runner now gives the
# same file-editing hand-back in every arm (`HANDBACK`, `handback_text`); a run file records its sha256.
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


def binary_id(path):
    """The treeyard binary a run used: its sha256 and `--version` (empty when it does not exist)."""
    path = Path(path)
    if not path.is_file():
        return {"treeyard_sha256": None, "treeyard_version": None}
    r = subprocess.run([str(path), "--version"], capture_output=True, text=True)
    return {"treeyard_sha256": sha256(path), "treeyard_version": r.stdout.strip() or None}


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
    TASK_TOTAL = 10  # the two 5-task sets of dev/taskdefs.py (click only; the other dev repositories have only extra sets)


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


def _apply_op(text, e):
    """One edit op on `text`, or None when its `old` or anchor does not occur exactly once. `create`
    makes a file that is not there yet (`text` is None for a path absent from the tree)."""
    if e["op"] == "create":
        return e["text"] if text is None else None
    if text is None:
        return None
    key = e["old"] if e["op"] == "replace" else e["anchor"]
    if text.count(key) != 1:
        return None
    if e["op"] == "replace":
        return text.replace(key, e["new"])
    return text.replace(key, e["text"] + key if e["op"] == "insert_before" else key + e["text"])


def apply_seq(files, tasks, combined=True):
    """Applies the edits of `tasks` one task after the other (sets with `"sequential": true`). An op may
    carry `alts`, the same edit written for a tree that already holds another task's lines: a combined
    tree (`combined`) tries the alts first, then the op; a task alone uses only the op. A task may also
    carry `combined_edits`, its whole step on a tree that holds all the tasks before it (the heavy set:
    refactors move code that later tasks edit, so the step differs from the task alone); a combined tree
    applies those instead of `edits`. A path that is not in `files` is absent (a `create` op makes it)."""
    out = dict(files)
    for t in tasks:
        for e in (t["combined_edits"] if combined and "combined_edits" in t else t["edits"]):
            for o in ([*e.get("alts", []), e] if combined else [e]):
                new = _apply_op(out.get(o["file"]), o)
                if new is not None:
                    out[o["file"]] = new
                    break
            else:
                raise ValueError(f"{t['id']}: an edit does not apply in {e['file']}: {(e.get('old') or e.get('anchor') or '')[:60]!r}")
    return out


def merge3(ours, base, theirs, union=False):
    """`git merge-file` of three texts (None = absent file); None when the merge conflicts. `union`
    keeps the lines of both sides of a conflict (ours first)."""
    with tempfile.TemporaryDirectory() as d:
        paths = []
        for name, text in (("ours", ours), ("base", base), ("theirs", theirs)):
            p = Path(d) / name
            p.write_text(text or "")
            paths.append(str(p))
        r = subprocess.run(["git", "merge-file", "-p", "--diff-algorithm=histogram", *(["--union"] if union else []), *paths],
                           capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def replay_without(files, tasks, k):
    """The tree of all `tasks` but task k (1-based), for sets whose steps (`combined_edits`) do not apply
    on a tree that lacks an earlier task. Each other task j is merged into the tree file by file, as
    `git cherry-pick` would: its step (the chain 1..j-1 to 1..j), or, where the step conflicts because it
    was written on task k's lines, the task alone (pin to its reference), and when that conflicts too
    (lines inserted next to another task's) the task alone with both sides kept (`--union`). A file
    that exists only through task k (a module that k creates) is left out. These trees serve only to
    show that an integration test fails without one partner; `validate` checks that the other
    partner's hidden test passes on them."""
    chain = [dict(files)]
    for j in range(1, len(tasks) + 1):
        chain.append(apply_seq(chain[-1], [tasks[j - 1]]))
    out = dict(files)
    for j in range(1, len(tasks) + 1):
        if j == k:
            continue
        before, after = chain[j - 1], chain[j]
        alone = None
        for f in sorted(set(before) | set(after)):
            if before.get(f) == after.get(f):
                continue
            if out.get(f) is None and before.get(f) is not None:
                continue
            merged = merge3(out.get(f), before.get(f), after.get(f))
            if merged is None:
                alone = alone or apply_seq(files, [tasks[j - 1]], False)
                if alone.get(f) != files.get(f):
                    merged = merge3(out.get(f), files.get(f), alone.get(f))
                    if merged is None:
                        merged = merge3(out.get(f), files.get(f), alone.get(f), union=True)
                else:
                    merged = out.get(f)
            if merged is None:
                raise ValueError(f"{tasks[j - 1]['id']}: its step conflicts in {f} without task {k}")
            out[f] = merged
    return out


def unified(before, after):
    """A git-style patch for the files that differ. A path missing from `before` (or None there) is a
    new file."""
    parts = []
    for f in sorted(set(before) | set(after)):
        a, b = before.get(f), after.get(f)
        if (a or "") == (b or "") and (a is None) == (b is None):
            continue
        parts.append(f"diff --git a/{f} b/{f}\n")
        if a is None:
            parts.append("new file mode 100644\n")
        parts.extend(difflib.unified_diff((a or "").splitlines(True), (b or "").splitlines(True),
                                          "/dev/null" if a is None else f"a/{f}", f"b/{f}", n=3))
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
    """{path: text at the pin}; a path that is not at the pin is left out (a task creates it)."""
    have = set(git("ls-tree", "-r", "--name-only", rev, "--", *paths, cwd=src).stdout.splitlines()) if paths else set()
    return {p: git("show", f"{rev}:{p}", cwd=src).stdout for p in paths if p in have}


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
    for mod in extra_taskdefs():
        generate_extra(scratch, mod)


def extra_taskdefs():
    """The sibling modules `taskdefs_<name>.py` of the kit's taskdefs.py (sets beyond `SETS`)."""
    import importlib.util

    mods = []
    for f in sorted(Path(TASKDEFS).glob("taskdefs_*.py")):
        spec = importlib.util.spec_from_file_location(f"t2_{Path(TASKDEFS).name}_{f.stem}", f)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        mods.append(mod)
    return mods


def generate_extra(scratch, mod):
    """Writes the sets of one sibling module: tasks/<repo>/<set>/ as `generate` does, plus resolved-<k>.patch
    for every k, set.json (agent count, slots, limits, integration tests) and integration/<id>/ (hidden
    test, and `without-<k>.patch`: all tasks but partner k, for `validate`). Its own manifest is
    tasks/MANIFEST-<set>.json, so tasks/MANIFEST.json stays that of the sets in `SETS`."""
    td = mod
    for s in td.SETS:
        manifest = {}
        settings = td.SET_SETTINGS[s]
        missing = [r for r in td.TASKS if not (scratch / "t2" / "src" / r / ".git").exists()]
        if missing:  # the files of this set stay as they are
            print(f"skipped {s}: no clone of {', '.join(missing)} in {scratch} (run `setup --repo {missing[0]}`)")
            continue
        for repo, sets in td.TASKS.items():
            tasks = sets[s]
            assert len(tasks) == settings["agents"], (repo, s, len(tasks))
            assert len(settings["mix"]) == settings["agents"], (repo, s)
            src = scratch / "t2" / "src" / repo
            rev = REPOS[repo]["rev"]
            paths = sorted({o["file"] for t in tasks for e in task_ops(t) for o in [e, *e.get("alts", [])]})
            base = pinned_files(src, rev, paths)
            seq = settings.get("sequential")
            apply = (lambda b, ts, combined=True: apply_seq(b, ts, combined)) if seq else (lambda b, ts, combined=True: apply_edits(b, ts))
            for t in tasks:  # an alt is for a tree with other tasks in: it may not fit the pin
                for e in t["edits"]:
                    for alt in e.get("alts", []):
                        if _apply_op(base.get(alt["file"]), alt) is not None:
                            raise SystemExit(f"{t['id']}: an alt applies to the pin: {alt}")
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
                (d / "reference.patch").write_text(unified(base, apply(base, [t], False)))
                hidden = d / "hidden" / test_path(repo, t)
                hidden.parent.mkdir(parents=True)
                hidden.write_text(t["test"])
                (d / f"resolved-{k}.patch").write_text(unified(apply(base, tasks[: k - 1]), apply(base, tasks[:k])))
                files = sorted({o["file"] for e in task_ops(t) for o in [e, *e.get("alts", [])]})
                meta_ = {"repo": repo, "set": s, "k": k, "id": t["id"], "title": t["title"], "files": files,
                         "test": test_path(repo, t)}
                (d / "meta.json").write_text(json.dumps(meta_, indent=2) + "\n")
            integ = []
            for it in td.INTEGRATIONS.get(repo, {}).get(s, []):
                d = set_dir / "integration" / it["id"]
                test = test_path(repo, it)
                (d / "hidden" / test).parent.mkdir(parents=True)
                (d / "hidden" / test).write_text(it["test"])
                for k in it["pair"]:
                    rest = [t for j, t in enumerate(tasks, 1) if j != k]
                    given = getattr(td, "WITHOUT", {}).get(repo, {}).get(s, {}).get(k)
                    try:
                        if given is not None:  # the set's own tree of all tasks but k
                            without = apply_seq(base, [{"id": f"without-{k}", "edits": given}], False)
                        else:
                            without = apply(base, rest)
                    except ValueError:  # steps written for a tree with task k: merge them in without it
                        try:
                            without = replay_without(base, tasks, k)
                        except ValueError as e:
                            print(f"{it['id']}: no tree without task {k}: {e}")
                            continue
                    (d / f"without-{k}.patch").write_text(unified(base, without))
                integ.append({"id": it["id"], "pair": it["pair"], "test": test, "why": it.get("why", "")})
            spec = {"set": s, "agents": settings["agents"], "mix": settings["mix"], "limits": settings.get("limits", {}),
                    "sequential": bool(seq), "tasks": [t["id"] for t in tasks], "integration": integ}
            (set_dir / "set.json").write_text(json.dumps(spec, indent=2) + "\n")
            for f in sorted(p for p in set_dir.rglob("*") if p.is_file()):
                manifest[str(f.relative_to(HERE))] = sha256(f)
        (TASKS_DIR / f"MANIFEST-{s}.json").write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n")
        print(f"generated {s}: {sum(1 for k in manifest if k.endswith('prompt.md'))} tasks, {len(manifest)} files")


def task_ops(t):
    """Every edit op of a task: its own and those of its `combined_edits`."""
    return [*t["edits"], *t.get("combined_edits", [])]


def set_spec(tasks_dir, repo, s):
    """The set-level settings of a set: its set.json, or those of the sets of preregistration v3 (5 agents,
    `MIX`, the frozen `LIMITS`, no integration tests)."""
    f = Path(tasks_dir) / repo / s / "set.json"
    spec = json.loads(f.read_text()) if f.is_file() else {}
    return {"agents": spec.get("agents", AGENTS), "mix": spec.get("mix", MIX), "limits": spec.get("limits", {}),
            "integration": spec.get("integration", []), "from_file": f.is_file()}


def sets_of(repo):
    """The extra sets that this repository has tasks for (in `EXTRA_SETS` order)."""
    return [s for s in EXTRA_SETS if (TASKS_DIR / repo / s / "set.json").is_file()]


def task_dirs(repo, s):
    d = TASKS_DIR / repo / s
    out = sorted((p for p in d.iterdir() if p.is_dir() and p.name.split("-")[0].isdigit()), key=lambda p: int(p.name.split("-")[0]))
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


def validate(scratch, repos, out_dir=None, only_sets=None):
    lines, ok_all = [], True
    old_sets = only_sets is None or any(x in only_sets for x in SETS)
    for repo in repos if old_sets else []:
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
        for s in (SETS if only_sets is None else [x for x in SETS if x in only_sets]):
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
    out_dir = Path(out_dir or RESULTS)
    out_dir.mkdir(parents=True, exist_ok=True)
    if old_sets:
        n = sum(1 for x in lines if x.endswith("-> ok") and " all 5 " not in x)
        lines.append(f"tasks that fail on the pin and pass with the reference: {n}/{TASK_TOTAL}")
        lines.append("validate: " + ("ok" if ok_all else "FAILED"))
        text = "\n".join(lines) + "\n"
        if set(repos) == set(REPOS) and (only_sets is None or set(SETS) <= set(only_sets)):
            (out_dir / "validate.txt").write_text(text)
        print(text, end="")
    # The extra sets (set.json) of these repositories: their own report, validate-<set>.txt.
    for s in EXTRA_SETS:
        have = [r for r in repos if s in sets_of(r)]
        if have and (only_sets is None or s in only_sets):
            ok_set, extra = validate_extra(scratch, have, s)
            ok_all &= ok_set
            if set(have) == {r for r in REPOS if s in sets_of(r)}:
                (out_dir / f"validate-{s}.txt").write_text(extra)
            print(extra, end="")
    return ok_all


def validate_extra(scratch, repos, s):
    """Validates one extra set: each hidden test fails on the pin and passes with its reference (and the
    suite stays green with the reference alone); resolved-1..n together pass the suite, every hidden
    test and every integration test; an integration test fails on the pin, with either partner alone,
    and with all tasks but either partner (`without-<k>.patch`). Returns (ok, report text)."""
    lines, ok_all = [], True
    for repo in repos:
        rev = REPOS[repo]["rev"]
        tree = scratch / "t2" / "validate" / repo
        if not tree.exists():
            tree.parent.mkdir(parents=True, exist_ok=True)
            git("clone", "-q", "--no-checkout", str(scratch / "t2" / "src" / repo), str(tree), cwd=scratch)
            git("checkout", "-q", "--detach", rev, cwd=tree)
        spec = set_spec(TASKS_DIR, repo, s)
        dirs = task_dirs(repo, s)
        n = len(dirs)
        good_n = 0
        for d in dirs:
            m = meta(d)
            reset_tree(tree, rev)
            fail_ok, out1, s1 = decide(repo, d, tree, scratch)
            reset_tree(tree, rev)
            apply_patch(tree, d / "reference.patch")
            sok, sout, _ = suite(repo, tree, scratch)
            pass_ok, out2, s2 = decide(repo, d, tree, scratch)
            good = (not fail_ok) and pass_ok and sok
            good_n += good
            ok_all &= good
            size = sum(1 for x in (d / "reference.patch").read_text().splitlines() if x[:1] in "+-" and x[:3] not in ("+++", "---"))
            lines.append(f"{repo} {s} {m['k']} {m['id']}: pinned {'pass' if fail_ok else 'fail'} ({s1} s), reference "
                         f"{'pass' if pass_ok else 'fail'} ({s2} s), suite with the reference alone {'pass' if sok else 'FAIL'}, "
                         f"{len(m['files'])} files, {size} diff lines -> {'ok' if good else 'BAD'}")
            if not good:
                lines.append("    " + (sout if not sok else out2 if not pass_ok else out1)[-600:].replace("\n", "\n    "))
        reset_tree(tree, rev)
        for d in dirs:
            apply_patch(tree, d / f"resolved-{meta(d)['k']}.patch")
        sok, _, ssecs = suite(repo, tree, scratch)
        done = [decide(repo, d, tree, scratch)[0] for d in dirs]
        integ = {it["id"]: decide_integration(repo, s, it, tree, scratch)[0] for it in spec["integration"]}
        good = sok and all(done) and all(integ.values())
        ok_all &= good
        lines.append(f"{repo} {s} all {n} (resolved-1..{n}): suite {'pass' if sok else 'FAIL'} ({ssecs} s), tests {sum(done)}/{n} pass, "
                     f"integration {sum(integ.values())}/{len(integ)} pass -> {'ok' if good else 'BAD'}")
        by_k = {meta(d)["k"]: d for d in dirs}
        for it in spec["integration"]:
            idir = TASKS_DIR / repo / s / "integration" / it["id"]
            checks, sane = {}, {}
            reset_tree(tree, rev)
            checks["pin"] = decide_integration(repo, s, it, tree, scratch)[0]
            for k in it["pair"]:
                reset_tree(tree, rev)
                apply_patch(tree, by_k[k] / "reference.patch")
                checks[f"only {k}"] = decide_integration(repo, s, it, tree, scratch)[0]
                reset_tree(tree, rev)
                apply_patch(tree, idir / f"without-{k}.patch")
                checks[f"all but {k}"] = decide_integration(repo, s, it, tree, scratch)[0]
                # The tree without k is a real tree: the hidden tests of the other partners pass on it.
                others = [by_k[j] for j in it["pair"] if j != k]
                sane[f"all but {k}"] = all(decide(repo, o, tree, scratch)[0] for o in others)
            good = not any(checks.values()) and all(sane.values())
            ok_all &= good
            lines.append(f"{repo} {s} integration {it['id']} (tasks {it['pair'][0]} and {it['pair'][1]}): "
                         + ", ".join(f"{c} {'PASSES' if v else 'fails'}" for c, v in checks.items())
                         + "; the partner's own test " + ", ".join(f"{c} {'passes' if v else 'FAILS'}" for c, v in sane.items())
                         + f" -> {'ok' if good else 'BAD'}")
        table = pair_table(scratch, repo, s)
        hits = sorted(p for p, v in table.items() if v)
        lines.append(f"{repo} {s}: {len(hits)} of {len(table)} pairs of reference patches conflict in git: {', '.join(hits)}")
        lines.append(f"{repo} {s}: tasks that fail on the pin and pass with the reference: {good_n}/{n}; agents {spec['agents']}, "
                     f"slots {','.join(spec['mix'])}, limits {json.dumps(spec['limits'], sort_keys=True)}")
    lines.append(f"validate {s}: " + ("ok" if ok_all else "FAILED"))
    return ok_all, "\n".join(lines) + "\n"


def decide_integration(repo, s, it, tree, scratch):
    """Copies one integration test of set `s` into `tree` and runs the deciding command. True = passes."""
    m = {"repo": repo, "test": it["test"]}
    src = TASKS_DIR / repo / s / "integration" / it["id"] / "hidden" / it["test"]
    dst = tree / it["test"]
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    rc, out, secs = run_cmd(repo, fill(REPOS[repo]["decide"], scratch, m), tree, scratch)
    return rc == 0, out, secs


def pair_conflicts(scratch, repo, dirs):
    """How many of the task pairs (10 for 5 tasks) conflict when both reference patches merge on the pinned commit."""
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
    for i in range(len(commits)):
        for j in range(i + 1, len(commits)):
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
            with open(f, errors="replace") as fh:
                for line in fh:
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
    """One tool call of a scripted agent: `tool_use` and `tool_result` events (the reply in full for a
    treeyard command, so that the warning metrics read it as they read a real agent's MCP replies)."""
    is_ty = Path(cmd[0]).name.startswith("treeyard")
    emit({"type": "tool_use", "name": "treeyard " + cmd[1] if is_ty else Path(cmd[0]).name, "cmd": [Path(cmd[0]).name, *cmd[1:]]})
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env=env)
    ev = {"type": "tool_result", "exit": r.returncode, "out": (r.stdout + r.stderr)[-400:]}
    if is_ty:
        ev["reply"] = r.stdout
    emit(ev)
    return r


def ctx_bin(c):
    return c.get("treeyard") or str(treeyard_bin())


class NativeBridge:
    """`treeyard mcp --tools native` on stdio, driven with JSON-RPC as a harness drives it (arm BN: the
    scripted agents and the runner's own lands). The process gets the agent's environment, so `start`
    puts the directory in the agent's TMPDIR, where the runner finds it again."""

    def __init__(self, ty, url, agent, env=None, cwd=None):
        self.p = subprocess.Popen([str(ty), "mcp", "--url", url, "--agent", agent, "--tools", "native"], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env, cwd=cwd)
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t2", "version": "1"}})

    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise RuntimeError("the native bridge exited")
        return json.loads(line)

    def call(self, name, args):
        """(ok, reply text) of one tool call."""
        r = self.rpc("tools/call", {"name": name, "arguments": args})
        res = r.get("result") or {}
        text = "".join(c.get("text", "") for c in res.get("content") or [])
        return not res.get("isError") and "error" not in r, text or json.dumps(r.get("error"))

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=30)
        except (OSError, subprocess.TimeoutExpired):
            self.p.kill()
            self.p.wait()
        self.p.stdout.close()


def ntool(br, name, args, talk=None):
    """One native tool call of a scripted agent, in its transcript dialect (`talk`, else the stub events);
    returns the reply as a dict ({} when it is not JSON, {"error": text} on a tool error)."""
    if talk is not None and talk.d != "stub":
        ok, text = talk.mcp(br, name, args)
    else:
        emit({"type": "tool_use", "name": "treeyard " + name, "cmd": ["treeyard", "mcp", name], "args": args})
        ok, text = br.call(name, args)
        emit({"type": "tool_result", "exit": 0 if ok else 1, "out": text[-400:], "reply": text})
    if not ok:
        return {"error": text}
    try:
        v = json.loads(text)
    except ValueError:
        return {}
    return v if isinstance(v, dict) else {}


def native_dirs():
    """The `start` directories of this agent (its TMPDIR), for a scripted agent's hand-back."""
    tmp = Path(os.environ.get("TMPDIR") or tempfile.gettempdir())
    return sorted(d for d in tmp.glob("treeyard-checkouts-*/ch_*") if d.is_dir())


def oracle_files_from_main(c, td, k, into, cwd):
    """Main's content of the task files plus this task's resolved patch, written into `into` (arm B: the
    workspace, arm BN: the `start` directory)."""
    ty = ctx_bin(c)
    resolved = td / f"resolved-{k}.patch"
    tmp = Path(tempfile.mkdtemp(prefix=f"oracle-{k}-", dir=c["run_dir"]))
    subprocess.run(["git", "init", "-q"], cwd=tmp)
    for f in meta(td)["files"]:
        rd = tool([ty, "call", "--url", c["url"], "read", json.dumps({"path": f})], cwd)
        content = json.loads(rd.stdout or "{}").get("content") if rd.returncode == 0 else None
        if content is None:  # not on main yet (a file the task creates): the patch makes it
            continue
        p = tmp / f
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)
    tool(["git", "apply", str(resolved if resolved.exists() else td / "reference.patch")], tmp)
    for f in meta(td)["files"]:
        if (tmp / f).is_file():
            (Path(into) / f).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(tmp / f, Path(into) / f)


def agent_oracle_native(c, a, k, td, resume):
    """Arm BN: `start`, `git apply` of reference.patch in the start directory, `sync`, then in the fixed
    oracle order `land`; on a land conflict it writes main + resolved-k.patch into the directory (over the
    markers) and lands again. On a hand-back it writes the same files and stops (the runner lands)."""
    cwd = a["cwd"]
    landed_dir = Path(c["run_dir"]) / "landed"
    if resume:
        for d in native_dirs():
            oracle_files_from_main(c, td, k, d, cwd)
        return
    br = NativeBridge(ctx_bin(c), c["url"], a["agent"], cwd=cwd)
    try:
        st = ntool(br, "start", {"intent": a["title"], "paths": meta(td)["files"]})
        cid, d = st.get("id"), st.get("dir")
        if not cid or not d:
            emit({"type": "note", "start": "failed"})
            return
        tool(["git", "apply", str(td / "reference.patch")], d)
        r = ntool(br, "sync", {"change": cid})
        warned = any(f.get("kind") in ("overlap", "conflict") for x in r.get("collisions", []) for f in x.get("files") or [])
        emit({"type": "note", "warned": warned})
        while not all((landed_dir / str(j)).exists() for j in range(1, k)):
            time.sleep(0.2)
        for _ in range(3):
            r = ntool(br, "land", {"change": cid})
            status = r.get("status")
            if status == "landed":
                landed_dir.mkdir(exist_ok=True)
                (landed_dir / str(k)).write_text("")
                return
            if status != "conflict":
                emit({"type": "note", "land_status": status or r.get("error", "")[:200]})
                break
            oracle_files_from_main(c, td, k, d, cwd)
        else:
            emit({"type": "note", "land_status": "gave up"})
    finally:
        br.close()
    landed_dir.mkdir(exist_ok=True)
    (landed_dir / str(k)).write_text("not landed")


def agent_noop(ctx, k):
    c = json.loads(Path(ctx).read_text())
    emit({"type": "turn"})
    if c["arm"] in GIT_ARMS:
        tool(["git", "status", "--short"], c["agents"][k - 1]["cwd"])
    elif c["arm"] == "BN":
        a = c["agents"][k - 1]
        br = NativeBridge(ctx_bin(c), c["url"], a["agent"], cwd=a["cwd"])
        try:
            ntool(br, "start", {"intent": a["title"]})
        finally:
            br.close()
    else:
        tool([ctx_bin(c), "workspace", "status", c["agents"][k - 1]["workspace"], "--url", c["url"], "--home", c["home"]],
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
    if c["arm"] in GIT_ARMS:
        if not resume:
            tool(["git", "apply", str(td / "reference.patch")], cwd)
            if c["arm"] in A2_ARMS:
                return oracle_integrate(c, k, td, cwd)
            tool(["git", "add", "-A"], cwd)
            tool(["git", "commit", "-q", "-m", a["title"]], cwd)
            return
        # The hand-back: the runner left main's work and this task's in the files, with markers.
        # The oracle writes main's content of its files plus its resolved patch, and stops.
        oracle_resolve(td, k, cwd)
        return
    if c["arm"] == "BN":
        return agent_oracle_native(c, a, k, td, resume)
    ty = ctx_bin(c)
    base = ["--url", c["url"], "--home", c["home"]]
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
            break
        # Resolve: main's content of the task files plus this task's resolved patch.
        tmp = Path(c["run_dir"]) / f"oracle-{k}-{attempt}"
        tmp.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=tmp)
        for f in meta(td)["files"]:
            rd = tool([ty, "call", "--url", c["url"], "read", json.dumps({"path": f})], cwd)
            content = json.loads(rd.stdout or "{}").get("content") if rd.returncode == 0 else None
            if content is None:  # not on main yet (a file the task creates): the patch makes it
                continue
            p = tmp / f
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(content)
        tool(["git", "apply", str(resolved if resolved.exists() else td / "reference.patch")], tmp)
        for f in meta(td)["files"]:
            if (tmp / f).is_file():
                (Path(cwd) / f).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(tmp / f, Path(cwd) / f)
    else:
        emit({"type": "note", "land_status": "gave up"})
    # Not landed (held, an error, gave up): the next oracle in the fixed order goes on all the same, so the
    # run ends and the runner's own land of this change records the status.
    landed_dir.mkdir(exist_ok=True)
    (landed_dir / str(k)).write_text("not landed")


def oracle_resolve(td, k, cwd):
    resolved = td / f"resolved-{k}.patch"
    for f in meta(td)["files"]:
        show = subprocess.run(["git", "show", f"main:{f}"], cwd=cwd, capture_output=True, text=True)
        if show.returncode != 0:  # not on main yet (a file the task creates): the patch makes it
            Path(cwd, f).unlink(missing_ok=True)
            continue
        Path(cwd, f).write_text(show.stdout)
    tool(["git", "apply", str(resolved if resolved.exists() else td / "reference.patch")], cwd)


def oracle_integrate(c, k, td, cwd):
    """Arm A2: integrates in the fixed oracle order 1 to 5 with `t2-integrate`; on its conflict writes
    main + resolved-k.patch into the files and integrates again."""
    landed_dir = Path(c["run_dir"]) / "landed"
    while not all((landed_dir / str(j)).exists() for j in range(1, k)):
        time.sleep(0.2)
    for _ in range(3):
        r = tool(["t2-integrate"], cwd)
        if r.returncode == 0:
            landed_dir.mkdir(exist_ok=True)
            (landed_dir / str(k)).write_text("")
            return
        oracle_resolve(td, k, cwd)
    emit({"type": "note", "integrate": "gave up"})
    landed_dir.mkdir(exist_ok=True)  # the next oracle in the order goes on; the runner merges what is left
    (landed_dir / str(k)).write_text("not integrated")


class StubTalk:
    """The transcript dialect of a stub agent (T2_STUB_AS per slot: stub, claude or codex), so that the
    runner's parser and resume path of each real harness are exercised without a model."""

    def __init__(self, dialect, k, resume):
        self.d, self.k, self.n = dialect, k, 0
        sid = f"stub-session-{k}"
        if self.d == "claude":
            emit({"type": "system", "subtype": "init", "session_id": sid, "model": "stub-model"})
        elif self.d == "codex":
            emit({"type": "thread.started", "thread_id": sid})
            emit({"type": "turn.started"})

    def call(self, cmd, cwd):
        self.n += 1
        if self.d == "stub":
            return tool(cmd, cwd)
        r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
        out = r.stdout + r.stderr
        if self.d == "claude":
            tid = f"tu-{self.k}-{self.n}"
            emit({"type": "assistant", "message": {"id": f"msg-{self.k}-{self.n}", "model": "stub-model", "content": [
                {"type": "tool_use", "id": tid, "name": "Bash", "input": {"command": " ".join(cmd)}}],
                "usage": {"input_tokens": 10, "output_tokens": 1}}})
            emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": out, "is_error": r.returncode != 0}]}})
        else:
            emit({"type": "item.started", "item": {"type": "command_execution", "command": " ".join(cmd)}})
            emit({"type": "item.completed", "item": {"type": "command_execution", "command": " ".join(cmd), "exit_code": r.returncode,
                                                      "aggregated_output": out, "status": "completed"}})
        return r

    def mcp(self, br, name, args):
        """A native bridge call in the dialect: an MCP tool call as each real harness records it."""
        self.n += 1
        ok, text = br.call(name, args)
        if self.d == "claude":
            tid = f"tu-{self.k}-{self.n}"
            emit({"type": "assistant", "message": {"id": f"msg-{self.k}-{self.n}", "model": "stub-model", "content": [
                {"type": "tool_use", "id": tid, "name": f"mcp__treeyard__{name}", "input": args}],
                "usage": {"input_tokens": 10, "output_tokens": 1}}})
            emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid,
                                                            "content": [{"type": "text", "text": text}], "is_error": not ok}]}})
        else:
            item = {"type": "mcp_tool_call", "server": "treeyard", "tool": name, "arguments": args, "status": "completed"}
            if ok:
                item["result"] = {"content": [{"type": "text", "text": text}]}
            else:
                item["error"] = {"message": text}
            emit({"type": "item.started", "item": dict(item, status="in_progress")})
            emit({"type": "item.completed", "item": item})
        return ok, text

    def noop(self):
        if self.d == "stub":
            emit({"type": "turn"})
            emit({"type": "tool_use", "name": "noop"})
        else:
            self.call(["true"], ".")

    def end(self):
        if self.d == "claude":
            emit({"type": "result", "session_id": f"stub-session-{self.k}", "num_turns": self.n, "total_cost_usd": 0.0,
                  "usage": {"input_tokens": 10 * self.n, "output_tokens": self.n}})
        elif self.d == "codex":
            emit({"type": "turn.completed", "usage": {"input_tokens": 10 * self.n, "cached_input_tokens": 0, "output_tokens": self.n}})


def stub_resolve(cwd, keep_markers=False):
    """Settles every marked block under `cwd`: main's lines, then the change's lines."""
    for f in sorted(Path(cwd).rglob("*.txt")):
        text = f.read_text()
        if not has_marker_text(text):
            continue
        if keep_markers:
            f.write_text(text + "edited\n")
            continue
        f.write_text(re.sub(r"^<<<<<<< [^\n]*\n(.*?)^=======\n(.*?)^>>>>>>> [^\n]*\n", lambda m: m.group(1) + m.group(2),
                            text, flags=re.S | re.M))


def agent_stub(ctx, k, resume=False):
    """Scripted misbehaviour for the stop tests; the mode comes from T2_STUB_MODES (comma list per slot).

    Hand-back modes (both arms): `edit-conflict` (rewrites line 1 of a.txt; arm A commits, arm B syncs
    its workspace; on the hand-back it does nothing), `conflict-resolve` (on the hand-back it settles the
    markers), `conflict-markers` (on the hand-back it edits the file but leaves the markers),
    `mcp-conflict` (arm B: the edit goes into its own `open_change` + `apply` change; on the hand-back it
    settles the markers on disk). `integrate` (arm A2): edits line 1, runs `t2-integrate`, settles a
    conflict and runs it again. Undo modes (arm B): `land-line` sets line 1 in its own change and lands
    it; `undo-line` waits until that line is on main, then puts line 1 back in its own change and stops
    (the runner's land of it is held with status `undoes`)."""
    modes = os.environ.get("T2_STUB_MODES", "ok,ok,ok,ok,ok").split(",")
    mode = modes[k - 1] if k - 1 < len(modes) else "ok"
    dialects = os.environ.get("T2_STUB_AS", "").split(",")
    dialect = dialects[k - 1] if k - 1 < len(dialects) and dialects[k - 1] else "stub"
    if mode in ("edit-conflict", "conflict-resolve", "conflict-markers", "mcp-conflict", "integrate"):
        c = json.loads(Path(ctx).read_text())
        a = c["agents"][k - 1]
        cwd = a["cwd"]
        talk = StubTalk(dialect, k, resume)
        if c["arm"] == "BN":
            return stub_native_edit(c, a, k, mode, talk, resume)
        if resume:
            for f in sorted(Path(cwd).glob("*.txt")):
                if has_marker_text(f.read_text()):
                    emit({"type": "note", "handback_file": f.read_text()})
            if mode in ("conflict-resolve", "mcp-conflict", "integrate"):
                stub_resolve(cwd)
            elif mode == "conflict-markers":
                stub_resolve(cwd, keep_markers=True)
            talk.noop()
            talk.end()
            return
        f = Path(cwd) / "a.txt"
        if mode == "mcp-conflict":
            ty = ctx_bin(c)
            ch = json.loads(talk.call([ty, "call", "--url", c["url"], "open_change", json.dumps({"intent": f"stub {k}", "agent": a["agent"]})], cwd).stdout)
            talk.call([ty, "call", "--url", c["url"], "apply", json.dumps({"change": ch["id"], "edits": [
                {"path": "a.txt", "replace": {"old": "line 1\n", "new": f"agent {k}\n"}}]})], cwd)
            talk.end()
            return
        lines = f.read_text().splitlines(True)
        lines[0] = f"agent {k}\n"
        f.write_text("".join(lines))
        if c["arm"] == "A":
            talk.call(["git", "commit", "-q", "-am", f"stub {k}"], cwd)
        elif c["arm"] in A2_ARMS:
            for _ in range(10):  # main can move again between a resolve and the next integrate
                if talk.call(["t2-integrate"], cwd).returncode == 0:
                    break
                stub_resolve(cwd)
        else:
            talk.call([ctx_bin(c), "workspace", "sync", a["workspace"], "--url", c["url"], "--home", c["home"]], cwd)
        talk.end()
        return
    if mode in ("land-line", "undo-line") and not resume:
        c = json.loads(Path(ctx).read_text())
        a = c["agents"][k - 1]
        talk = StubTalk(dialect, k, resume)
        ty = ctx_bin(c)
        if c["arm"] == "BN":
            return stub_native_line(c, a, k, mode, talk)

        def call(op, args):
            return json.loads(talk.call([ty, "call", "--url", c["url"], op, json.dumps(args)], a["cwd"]).stdout or "{}")

        old, new = ("line 1\n", "kept by agent 1\n") if mode == "land-line" else ("kept by agent 1\n", "line 1\n")
        if mode == "undo-line":
            for _ in range(200):
                if old in (call("read", {"path": "a.txt"}).get("content") or ""):
                    break
                time.sleep(0.1)
        ch = call("open_change", {"intent": f"stub {k}", "agent": a["agent"]})
        call("apply", {"change": ch["id"], "edits": [{"path": "a.txt", "replace": {"old": old, "new": new}}]})
        if mode == "land-line":
            call("land", {"change": ch["id"]})
        talk.end()
        return
    if mode == "ok" or resume or mode in ("land-line", "undo-line"):
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
    elif mode == "slow-ok":
        emit({"type": "turn"})
        emit({"type": "tool_use", "name": "noop"})
        time.sleep(float(os.environ.get("T2_STUB_SLOW_S", "3")))


def stub_native_edit(c, a, k, mode, talk, resume):
    """Arm BN hand-back modes: `start`, line 1 of a.txt set in the start directory with plain file
    writes, `sync`, stop (the runner lands). On the hand-back the marked files are in that directory."""
    if resume:
        for d in native_dirs():
            for f in sorted(d.glob("*.txt")):
                if has_marker_text(f.read_text()):
                    emit({"type": "note", "handback_file": f.read_text()})
            if mode != "edit-conflict":
                stub_resolve(d, keep_markers=mode == "conflict-markers")
        talk.noop()
        talk.end()
        return
    br = NativeBridge(ctx_bin(c), c["url"], a["agent"], cwd=a["cwd"])
    try:
        st = ntool(br, "start", {"intent": f"stub {k}", "paths": ["a.txt"]}, talk)
        f = Path(st["dir"]) / "a.txt"
        lines = f.read_text().splitlines(True)
        lines[0] = f"agent {k}\n"
        f.write_text("".join(lines))
        ntool(br, "sync", {"change": st["id"]}, talk)
    finally:
        br.close()
    talk.end()


def stub_native_line(c, a, k, mode, talk):
    """Arm BN undo modes: `land-line` sets line 1 in its start directory and lands; `undo-line` waits until
    that line is on main, starts (its directory then has the line), puts line 1 back and syncs, and stops
    (the runner's land of it is held with status `undoes`)."""
    ty = ctx_bin(c)
    old, new = ("line 1\n", "kept by agent 1\n") if mode == "land-line" else ("kept by agent 1\n", "line 1\n")
    if mode == "undo-line":
        for _ in range(200):
            r = subprocess.run([ty, "call", "--url", c["url"], "read", json.dumps({"path": "a.txt"})], capture_output=True, text=True)
            if old in (json.loads(r.stdout or "{}").get("content") or ""):
                break
            time.sleep(0.1)
    br = NativeBridge(ty, c["url"], a["agent"], cwd=a["cwd"])
    try:
        st = ntool(br, "start", {"intent": f"stub {k}", "paths": ["a.txt"]}, talk)
        f = Path(st["dir"]) / "a.txt"
        f.write_text(f.read_text().replace(old, new, 1))
        if mode == "land-line":
            ntool(br, "land", {"change": st["id"]}, talk)
        else:
            ntool(br, "sync", {"change": st["id"]}, talk)
    finally:
        br.close()
    talk.end()


# ----------------------------------------------------------------------- real harness wiring


def real_command(harness, prompt, a, c, limits, resume_msg=None, session=None, prior=None):
    """The command line of one real agent. Config homes are in the agent's box (lesson of T1): the runner
    never runs a CLI against the user's real config directory. On a hand-back (`prior` = the counters so
    far) the CLI's own limits get what is left of the agent's limits."""
    env = dict(a["env"])
    env.pop("CLAUDECODE", None)
    env.pop("CLAUDE_CODE_ENTRYPOINT", None)
    box = Path(a["box"])
    home = box / "cfg"
    home.mkdir(parents=True, exist_ok=True)
    text = resume_msg or prompt
    prior = prior or {}
    if harness == "claude":
        env["CLAUDE_CONFIG_DIR"] = str(home)
        env["DISABLE_AUTOUPDATER"] = "1"
        env["DISABLE_TELEMETRY"] = "1"
        tok = "<dry-run>" if os.environ.get("T2_DRY") else claude_token()
        if tok is None:
            raise Stop("claude: no OAuth token in the Keychain item of the installed Claude Code")
        env["CLAUDE_CODE_OAUTH_TOKEN"] = tok
        servers = {}
        if c["arm"] in B_ARMS:
            servers["treeyard"] = {"command": c["mcp_command"], "args": c["mcp_args"] + ["--agent", a["agent"]],
                                   "env": {"TREEYARD_URL": c["url"]}}
        (home / "mcp.json").write_text(json.dumps({"mcpServers": servers}, indent=2))
        mcp = str(home / "mcp.json")
        turns_left = max(1, limits["max_turns"] - int(prior.get("turns", 0)))
        budget_left = max(0.01, round(limits["claude_budget_usd"] - float(prior.get("cost_usd", 0)), 4))
        cmd = ["claude", "-p", text, "--mcp-config", mcp, "--strict-mcp-config",
               "--max-turns", str(turns_left), "--max-budget-usd", str(budget_left),
               "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits",
               "--allowedTools", "Bash,Read,Edit,Write,Glob,Grep" + (",mcp__treeyard__*" if c["arm"] in B_ARMS else "")]
        if c["arm"] == "BN":
            # The `start` directory is in the box's TMPDIR, outside the cwd: the agent edits it with Edit/Write.
            cmd += ["--add-dir", str(box / "tmp")]
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
        # `codex exec resume` has no -s or --add-dir: the sandbox goes in as config values, which both forms
        # take. The box (HOME, TMPDIR, the agent's venv and cargo target) is writable besides the cwd.
        cmd += ["--json", "--skip-git-repo-check", "-c", "sandbox_mode=\"workspace-write\"", "-c", "approval_policy=\"never\"",
                "-c", f"sandbox_workspace_write.writable_roots={json.dumps([str(box)])}"]
        if c["arm"] in B_ARMS:
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
        if c["arm"] in B_ARMS:
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
            text = p.read_text(errors="replace")
            out["~/" + rel] = "run-path entries: %d" % sum(text.count(m) for m in ("/t2/runs/", "/t2/runner/", "/t2-agents-"))
        else:
            out["~/" + rel] = sha256(p)
    return out


# ===================================================================================== metrics

KIT_VERSION = 2  # run files with "kit": 2 have the fields of this version; older ones have none
DEFAULT_TEST_GLOBS = ["tests/**", "test/**"]


def sha_text(text):
    return hashlib.sha256((text or "").encode()).hexdigest()


def glob_match(path, pattern):
    """`**` crosses directories, `*` and `?` stay in one path segment."""
    rx = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            rx += ".*"
            i += 2
        elif pattern[i] == "*":
            rx += "[^/]*"
            i += 1
        elif pattern[i] == "?":
            rx += "[^/]"
            i += 1
        else:
            rx += re.escape(pattern[i])
            i += 1
    return re.fullmatch(rx, path) is not None


def diff_stats(diff, test_globs):
    """Guard metrics of one agent's own diff: files, lines added and removed, lines added in test files."""
    files, added, removed, test_added, path = set(), 0, 0, 0, None
    for line in (diff or "").splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
            if path:
                files.add(path)
            continue
        if line.startswith("--- ") or line.startswith("diff --git") or not path:
            continue
        if line.startswith("+"):
            added += 1
            test_added += any(glob_match(path, g) for g in test_globs)
        elif line.startswith("-"):
            removed += 1
    return {"files": len(files), "added": added, "removed": removed, "test_added": test_added, "paths": sorted(files)}


def by_harness(agent_stats):
    """Sums per harness (Claude Code and Codex apart): tokens with the price-free split, cost, turns."""
    out = {}
    for s in agent_stats:
        h = out.setdefault(s.get("harness") or "?", {"agents": 0, **{k: 0 for k in TOKEN_KEYS}, "cost_usd": 0.0, "turns": 0, "tool_calls": 0})
        h["agents"] += 1
        for key in (*TOKEN_KEYS, "turns", "tool_calls"):
            h[key] += s.get(key, 0) or 0
        h["cost_usd"] = round(h["cost_usd"] + (s.get("cost_usd") or 0), 6)
    return out


def _reply_text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(c.get("text", "") for c in content if isinstance(c, dict))
    if isinstance(content, dict):
        return _reply_text(content.get("content"))
    return ""


def tool_events(harness, lines, box=None):
    """The tool calls of one transcript, in order: {name, op (the treeyard operation or None), args,
    command (shell), reply (text), ok}. Claude: tool_use + tool_result; a reply too large for the
    transcript is read from the file Claude saved it to, when that file is in the agent's box. Codex:
    mcp_tool_call and command_execution items. Scripted agents: tool_use + tool_result events."""
    out, pending = [], {}
    for line in lines:
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if not isinstance(ev, dict):
            continue
        t = ev.get("type")
        if harness == "claude":
            msg = ev.get("message") or {}
            for c in msg.get("content") or [] if isinstance(msg.get("content"), list) else []:
                if not isinstance(c, dict):
                    continue
                if t == "assistant" and c.get("type") == "tool_use" and c.get("id") not in pending:
                    name = c.get("name") or ""
                    inp = c.get("input") or {}
                    rec = {"name": name, "op": name.split("__", 2)[2] if name.startswith("mcp__treeyard__") else None,
                           "args": inp, "command": inp.get("command") if name == "Bash" else None, "reply": None, "ok": None}
                    pending[c.get("id")] = rec
                    out.append(rec)
                elif t == "user" and c.get("type") == "tool_result" and c.get("tool_use_id") in pending:
                    rec = pending[c["tool_use_id"]]
                    text = _reply_text(c.get("content"))
                    m = re.search(r"saved to (\S+)", text) if text.startswith("Error: result (") else None
                    saved = Path(m.group(1).rstrip(".,;:)")) if m else None
                    if saved and box and saved.is_file() and Path(box).resolve() in saved.resolve().parents:
                        text = saved.read_text(errors="replace")
                        rec["reply_from_file"] = True
                    rec["reply"], rec["ok"] = text, not c.get("is_error")
        elif harness == "codex":
            item = ev.get("item") or {}
            if t != "item.completed":
                continue
            if item.get("type") == "mcp_tool_call":
                res = item.get("result") or {}
                err = item.get("error")
                out.append({"name": item.get("tool"), "op": item.get("tool") if item.get("server") == "treeyard" else None,
                            "args": item.get("arguments") or {}, "command": None,
                            "reply": _reply_text(res) if res else str((err or {}).get("message", err or "")),
                            "ok": not err and item.get("status") in (None, "completed")})
            elif item.get("type") == "command_execution":
                out.append({"name": "shell", "op": None, "args": {}, "command": item.get("command") or "",
                            "reply": item.get("aggregated_output") or "", "ok": item.get("exit_code") == 0})
        else:
            if t == "tool_use":
                cmd = ev.get("cmd") or []
                op = None
                if cmd[:1] and cmd[0].startswith("treeyard") and len(cmd) > 2 and cmd[1] in ("workspace", "call", "mcp"):
                    op = cmd[2] if cmd[1] in ("workspace", "mcp") else next((x for x in cmd[2:] if not x.startswith("-") and not x.startswith("http")), None)
                args = ev.get("args") if isinstance(ev.get("args"), dict) else {}
                rec = {"name": ev.get("name"), "op": op, "args": args, "command": " ".join(cmd), "reply": None, "ok": None}
                out.append(rec)
                pending["last"] = rec
            elif t == "tool_result" and pending.get("last"):
                rec = pending.pop("last")
                rec["reply"] = ev.get("reply") if ev.get("reply") is not None else ev.get("out", "")
                rec["ok"] = ev.get("exit") == 0
    return out


def warning_of(events, agent, native=False):
    """`warned` and `reacted` of one agent from its tool events (see Run.transcript_metrics). `native` (arm
    BN): `start` opens the agent's change, and a `sync` after the warning (edits sent again) is a reaction.
    Arm BN also reads the agent's own `land` replies: a hold (status `undoes`) is a warning of kind `undoes`
    (`land_holds`, `held_changes`; the other changes in its entries count in `others`); a later land that
    lands without `undid` is a reaction (`hold_reacted`), one with `undid` (or `undo: true`) an override
    (`undoes_overridden`). `land_conflicts` and `land_refused` (markers left) count those replies."""
    own, warned_at, reacted, landed_after = set(), None, False, False
    holds, held, overridden, hold_reacted, conflicts, refused = 0, set(), 0, False, 0, 0
    tests_failed, gate_rounds, gate_s = 0, 0, 0.0
    react_ops = ("change", "rebase", "apply", "resolve") + (("sync",) if native else ())
    kinds, others, with_main = set(), set(), False
    for i, e in enumerate(events):
        op = e.get("op")
        if not op:
            continue
        try:
            reply = json.loads(e.get("reply") or "")
        except ValueError:
            reply = None
        reply = reply if isinstance(reply, dict) else {}
        args = e.get("args") if isinstance(e.get("args"), dict) else {}
        if op in ("open_change", "start") and reply.get("id"):
            own.add(reply["id"])
        if op in ("apply", "declare", "rebase", "resolve", "land", "sync") and args.get("change"):
            own.add(args["change"])
        if warned_at is not None and i > warned_at and not landed_after:
            if op == "land":
                landed_after = True
            elif op in react_ops and not reacted:
                reacted = True
        mine = reply.get("agent") == agent or reply.get("id") in own or op in ("apply", "open_change", "declare", "rebase", "sync", "status", "start")
        if op == "change" and args.get("change") not in own:
            mine = False
        hits = [(c.get("change"), f) for c in reply.get("collisions") or [] if isinstance(c, dict)
                for f in c.get("files") or [] if isinstance(f, dict) and f.get("path") and f.get("kind") in ("overlap", "conflict")]
        if mine and hits:
            if warned_at is None:
                warned_at = i
            for other, f in hits:
                kinds.add(f["kind"])
                if other == "main":
                    with_main = True
                elif other:
                    others.add(other)
        if native and op == "land":
            status = reply.get("status")
            cid = reply.get("change") if isinstance(reply.get("change"), str) else args.get("change")
            if status == "undoes":
                holds += 1
                held.add(cid)
                kinds.add("undoes")
                if warned_at is None:
                    warned_at = i
                for u in reply.get("undoes") or []:
                    if isinstance(u, dict) and u.get("change") and u["change"] not in own:
                        others.add(u["change"])
            elif status == "conflict":
                conflicts += 1
            elif status == "refused":
                refused += 1
            elif status in ("landed", "already_on_main"):
                if reply.get("undid") or args.get("undo") is True:
                    overridden += 1
                elif held:
                    hold_reacted = True
            if status == "tests_failed":
                tests_failed += 1
            lt = reply.get("land_test") if isinstance(reply.get("land_test"), dict) else {}
            gate_rounds += int(lt.get("rounds") or 0)
            gate_s += float(lt.get("seconds") or 0)
    out = {"warned": warned_at is not None, "reacted": warned_at is not None and (reacted or hold_reacted),
           "first_warning_call": warned_at, "kinds": sorted(kinds), "with_main": with_main, "others": sorted(others)}
    if native:
        out.update(land_holds=holds, held_changes=sorted(c for c in held if c), undoes_overridden=overridden,
                   hold_reacted=hold_reacted, land_conflicts=conflicts, land_refused=refused, changes=sorted(own),
                   land_tests_failed=tests_failed, land_test_rounds=gate_rounds, land_test_seconds=round(gate_s, 2))
    return out


def metrics_of(arm, agents, k_of, pairs_truth=None):
    """The transcript metrics of a run (Run.transcript_metrics, `recount --metrics`): `agents` = [(k, agent
    name, tool events)], `k_of` = change id -> k. Without `pairs_truth` there is no `false_alarms`."""
    native = arm == "BN"
    per, pairs, tests = {}, set(), {}
    for k, agent, ev in agents:
        w = warning_of(ev, agent, native=native)
        per[str(k)] = w
        for other in w["others"]:
            if other in k_of and k_of[other] != k:
                pairs.add(tuple(sorted((k, k_of[other]))))
        runs = [e for e in ev if TEST_CMD in (e.get("command") or "")]
        tests[str(k)] = {"runs": len(runs), "passed": any(e.get("ok") and f"{TEST_CMD}: passed" in (e.get("reply") or "") for e in runs)}
    out = {"warned": sum(w["warned"] for w in per.values()), "reacted": sum(w["reacted"] for w in per.values()),
           "warnings": per, "warned_pairs": sorted(f"{i}-{j}" for i, j in pairs),
           "test_command": tests, "test_passed_agents": sum(t["passed"] for t in tests.values())}
    if pairs_truth is not None:
        out["false_alarms"] = sum(1 for i, j in pairs if not pairs_truth.get(f"{i}-{j}", False))
    if native:
        # `land_conflicts` of a run counts the server's land conflict events; this is the agents' own replies.
        out["agent_land_conflicts"] = sum(w["land_conflicts"] for w in per.values())
        for key in ("land_holds", "undoes_overridden", "land_refused", "hold_reacted", "land_tests_failed", "land_test_rounds"):
            out[key] = sum(w[key] for w in per.values())
        out["land_test_seconds"] = round(sum(w["land_test_seconds"] for w in per.values()), 2)
    return out


def bn_undoes_held(result):
    """Arm BN: `undoes_held` per agent and per run = the distinct changes of the agent held with status
    `undoes`, by its own `land` (transcript) or by the runner's (`lands[].undoes_held`); the runner's alone
    stay in `undoes_held_runner` (what arm B's `undoes_held` counts). Changes `result` in place."""
    total = 0
    for rec in result.get("lands") or []:
        runner = {x.get("change") for x in rec.get("undoes_held") or []}
        own = set((result.get("warnings") or {}).get(str(rec.get("k")), {}).get("held_changes") or [])
        rec["undoes_held_changes"] = sorted(c for c in runner | own if c)
        total += len(rec["undoes_held_changes"])
    result["undoes_held_runner"] = sum(len(x.get("undoes_held") or []) for x in result.get("lands") or [])
    result["undoes_held"] = total


# ===================================================================================== agent environment

TEST_CMD = "t2-test"
# The land test of `run --land-test` (arm BN): the bridge runs it in the agent's start directory, on PATH
# from the agent's box like the agent's own runs.
LAND_TEST_CMD = TEST_CMD
INTEGRATE_CMD = "t2-integrate"

# The common preamble of every arm (item: same deliverable in both arms). Arm-independent text, outside
# the frozen task files; a run file records its sha256 (`preamble_sha256`).
PREAMBLE = (
    "## What to deliver\n\n"
    "- The code change that the task below asks for, and one regression test for it, added to the repository's "
    "existing test file for that module.\n"
    "- No changelog entry, no release notes, and no change to files that the task does not need.\n"
    f"- The test command is `{TEST_CMD}`, run from this directory. It runs the repository's tests with no install and "
    f"no network; `{TEST_CMD} ARGS` passes the arguments to the repository's test runner (for example one test file). "
    f"The task is finished only when `{TEST_CMD}` passes."
)

# Arm A2: plain worktrees plus a git-native way to integrate early (`t2-integrate`). Its sha256 goes in
# the run file (`arm_block_sha256`).
A2_BLOCK = (
    "## Other agents work on this repository at the same time\n\n"
    "Each agent works in its own git worktree and branch of this repository.\n\n"
    f"1. When your change passes `{TEST_CMD}`, integrate it yourself: run `{INTEGRATE_CMD}` in this directory. It "
    "merges main into your branch and, when that merges cleanly, moves main to your work. It runs no tests.\n"
    f"2. When `{INTEGRATE_CMD}` reports a conflict, the files it names hold your work and the work on main with "
    "conflict markers. Resolve them in your files so that your work and the work on main are both kept, run "
    f"`{TEST_CMD}`, then run `{INTEGRATE_CMD}` again."
)


# Arm A3 ("git, merge often"): arm A2's block plus a concrete rule to merge main into the branch while working,
# so that conflicts come early and in small pieces. Its sha256 goes in the run file (`arm_block_sha256`).
A3_BLOCK = (
    "## Other agents work on this repository at the same time\n\n"
    "Each agent works in its own git worktree and branch of this repository, and other agents put their work on "
    "the branch `main` while you work. Meet their changes early and in small pieces:\n\n"
    "1. Merge main into your branch often: commit your work (`git add` the files you changed, `git commit`), then "
    "run `git merge main` in this directory. Do it before you start, after each logical step of your work (a "
    "function written, a test passing), at least every 10 minutes of work, and right before you integrate.\n"
    "2. When `git merge main` reports a conflict, resolve it at once, before you go on: keep your work and the work "
    f"on main, `git add` the files and `git commit`. Then run `{TEST_CMD}` (or the tests of the files you touched) "
    "before you continue.\n"
    f"3. When your change passes `{TEST_CMD}`, integrate it: run `{INTEGRATE_CMD}` in this directory. It merges main "
    "into your branch and, when that merges cleanly, moves main to your work. It runs no tests.\n"
    f"4. When `{INTEGRATE_CMD}` reports a conflict, the files it names hold your work and the work on main with "
    "conflict markers. Resolve them in your files so that your work and the work on main are both kept, run "
    f"`{TEST_CMD}`, then run `{INTEGRATE_CMD}` again."
)


def arm_b_test_note():
    """HOOK (arm B tests): extra preamble text for arm B on how the agent's edits reach the files that
    `t2-test` runs on. In arm B an agent edits through `apply`, so the files on disk do not hold its
    edits; a product feature that writes them is being built elsewhere. Until it exists this returns ""
    The `checkout` tool of the bridge writes the change into a directory of its own; `t2-test` runs on the
    tree it is called from."""
    return (f"Your edits go through `apply`, so they are not in this directory: run `{TEST_CMD}` from the directory "
            "`checkout.dir` of your `apply` reply.")


def arm_bn_test_note():
    """Arm BN: the agent edits and tests the directory `dir` of its `start` reply; `t2-test` runs on the tree it
    is called from."""
    return f"You work in the directory `dir` of your `start` reply, not in this directory: run `{TEST_CMD}` from `dir`."


def preamble(arm):
    note = {"B": arm_b_test_note, "BN": arm_bn_test_note}.get(arm, lambda: "")()
    return PREAMBLE + ("\n" + note if note else "")


def reach_check(adir, forbidden):
    """Problems with an agent directory `adir`: an ancestor of it (below the user's home and below `/`)
    that holds kit material (`forbidden`: the kit, the tasks, the runner's scratch, the results), or an
    agent root that holds other runs. Empty = fine."""
    out = []
    adir = Path(adir).resolve()
    home = Path.home().resolve()
    stop = {home, Path("/"), Path(adir.anchor)}
    p = adir
    while p not in stop and p != p.parent:
        for f in forbidden:
            f = Path(f).resolve()
            if f == p or p in f.parents:
                out.append(f"{p} holds {f}")
        p = p.parent
    root = adir.parent
    if root.is_dir():
        others = sorted(x.name for x in root.iterdir() if x.name != adir.name and not x.name.startswith("."))
        if others:
            out.append(f"{root} holds other entries: {', '.join(others[:5])}")
    return out


def clone_file(src, dst):
    """Copy-on-write copy where the file system has it (APFS clonefile, reflink), else a copy."""
    flag = ["-c"] if sys.platform == "darwin" else ["--reflink=auto"]
    if subprocess.run(["cp", *flag, str(src), str(dst)], capture_output=True).returncode != 0:
        shutil.copy2(src, dst)


def clone_tree(src, dst):
    flag = ["-c"] if sys.platform == "darwin" else ["--reflink=auto"]
    if subprocess.run(["cp", *flag, "-R", str(src), str(dst)], capture_output=True).returncode != 0:
        shutil.copytree(src, dst, symlinks=True, dirs_exist_ok=True)


AGENT_ENV_KEEP = ("PATH", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "USER", "LOGNAME", "SHELL", "TZ")


def base_agent_env(box, keep_t2=False, hide=()):
    """A new environment for one agent: only the keys in AGENT_ENV_KEEP from the runner's, PATH without
    entries inside `hide` (the kit, the scratch), HOME and TMPDIR in its box, a neutral git identity,
    and offline package tools. No variable names a kit or scratch path."""
    env = {k: os.environ[k] for k in AGENT_ENV_KEEP if k in os.environ}
    hidden = [Path(h).resolve() for h in hide]

    def visible(entry):
        try:
            e = Path(entry).resolve()
        except OSError:
            return True
        return not any(e == h or h in e.parents for h in hidden)

    env["PATH"] = os.pathsep.join(e for e in os.environ.get("PATH", "").split(os.pathsep) if e and visible(e))
    if keep_t2:  # the scripted agents read their modes from these
        env.update({k: v for k, v in os.environ.items() if k.startswith("T2_STUB")})
    box = Path(box)
    env.update(HOME=str(box / "home"), TMPDIR=str(box / "tmp"), XDG_CACHE_HOME=str(box / "home" / ".cache"),
               XDG_CONFIG_HOME=str(box / "home" / ".config"), XDG_DATA_HOME=str(box / "home" / ".local" / "share"),
               GIT_AUTHOR_NAME="t2 agent", GIT_AUTHOR_EMAIL="agent@example.com", GIT_COMMITTER_NAME="t2 agent",
               GIT_COMMITTER_EMAIL="agent@example.com", GIT_CONFIG_NOSYSTEM="1", UV_OFFLINE="1", PIP_NO_INDEX="1",
               npm_config_offline="true")
    return env


def make_venv(src, dst, packages):
    """A venv of the agent's own (a venv holds absolute paths, so it is made, not copied): the same Python
    as the kit's venv `src`, the same packages from uv's cache with no network."""
    home = None
    cfg = Path(src) / "pyvenv.cfg"
    if cfg.is_file():
        for line in cfg.read_text().splitlines():
            if line.split("=")[0].strip() == "home":
                home = line.split("=", 1)[1].strip()
    py = str(Path(home) / "python3") if home else "3.12"
    r = subprocess.run(["uv", "venv", "-q", str(dst), "--python", py], capture_output=True, text=True)
    if r.returncode == 0:
        r = subprocess.run(["uv", "pip", "install", "-q", "--offline", "--python", str(Path(dst) / "bin" / "python"), *packages],
                           capture_output=True, text=True)
    if r.returncode != 0:
        raise Stop(f"agent env: venv from the uv cache failed (run `t2.py setup` again): {tail(r.stderr, 300)}")


def cargo_home(d):
    """A CARGO_HOME of the agent's own (lock and cache files are written there) that reads the operator's
    downloaded registry and git checkouts through symlinks; builds run offline."""
    d = Path(d)
    d.mkdir(parents=True, exist_ok=True)
    real = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    for sub in ("registry", "git"):
        if (real / sub).exists() and not (d / sub).exists():
            (d / sub).symlink_to(real / sub)
    (d / "config.toml").write_text("[net]\noffline = true\n")
    return d


def neutral_hook():
    """astro_hook.mjs without its comment header (it names the kit's hidden tests)."""
    lines = HOOK.read_text().splitlines(True)
    while lines and lines[0].startswith("//"):
        lines.pop(0)
    return "// Resolve hook: imports of ./x.js and ../dist/x.js load the .ts source.\n" + "".join(lines)


def write_script(path, text):
    Path(path).write_text(text)
    os.chmod(path, 0o755)


def test_script(root, workdir, suite, prefix, env):
    """`t2-test`: the repository's tests (the `suite` command of repos.json) or `test_prefix` + ARGS.
    It runs in the agent's directory (`root`, plus the repo's `workdir`); called from a directory
    outside it (a copy of the tree, as block B point 6 describes), it runs there. Absolute paths only,
    so it works under any shell profile and sandbox."""
    return f'''#!{sys.executable} -I
"""t2-test: runs the repository's tests. `t2-test ARGS` passes ARGS to the test runner."""
import os
import subprocess
import sys

ROOT = {str(root)!r}
WORKDIR = {str(workdir)!r}
SUITE = {suite!r}
PREFIX = {prefix!r}
ENV = {env!r}
here = os.getcwd()
base = ROOT if here == ROOT or here.startswith(ROOT + os.sep) else here
wd = os.path.join(base, WORKDIR) if os.path.isdir(os.path.join(base, WORKDIR)) else base
args = [os.path.abspath(x) if os.path.exists(x) else x for x in sys.argv[1:]]
cmd = PREFIX + args if args else SUITE
rc = subprocess.call(cmd, cwd=wd, env=dict(os.environ, **ENV))
print("t2-test: passed" if rc == 0 else f"t2-test: failed (exit {{rc}})", flush=True)
sys.exit(rc)
'''


def integrate_script(spool):
    """`t2-integrate` of arm A2: a request file in the agent's TMPDIR, answered by the runner (`Run.integrate`),
    which does the git work outside any sandbox. A Codex sandbox lets a command write its TMPDIR."""
    return f'''#!{sys.executable} -I
"""t2-integrate: merges main into this branch and, when that merges cleanly, moves main to it."""
import json
import os
import sys
import time
import uuid

SPOOL = {str(spool)!r}
os.makedirs(SPOOL, exist_ok=True)
rid = uuid.uuid4().hex
tmp = os.path.join(SPOOL, ".req-" + rid)
with open(tmp, "w") as f:
    json.dump({{"id": rid, "cwd": os.getcwd()}}, f)
os.rename(tmp, os.path.join(SPOOL, "req-" + rid + ".json"))
res = os.path.join(SPOOL, "res-" + rid + ".json")
t0 = time.time()
while not os.path.exists(res):
    if time.time() - t0 > 900:
        print("t2-integrate: no answer within 900 s")
        sys.exit(2)
    time.sleep(0.1)
with open(res) as f:
    d = json.load(f)
os.unlink(res)
print(d["out"], end="")
sys.exit(d["exit"])
'''


# ===================================================================================== one run


class Run:
    def __init__(self, scratch, arm, repo, s, rep, harness, limits=None, agents=None, tasks_dir=None, repo_spec=None,
                 run_dir=None, health_url=None, results=None, block_file=None, agent_root=None, land_test=False):
        self.scratch, self.arm, self.repo, self.set, self.rep, self.harness = scratch, arm, repo, s, rep, harness
        self.tasks_dir = tasks_dir or TASKS_DIR
        # Set-level settings (set.json of an extra set; else 5 agents, MIX, the frozen LIMITS): the agent
        # count that the stop rule wants, the harness slots, and limits that the `limits` argument overrides.
        self.set_spec = set_spec(self.tasks_dir, repo, s)
        self.expected = self.set_spec["agents"]
        self.mix = self.set_spec["mix"]
        self.limits = {**LIMITS, **self.set_spec["limits"], **(limits or {})}
        self.n_agents = self.expected if agents is None else agents
        self.spec = repo_spec or REPOS[repo]
        self.name = f"{arm}-{repo}-{s}-r{rep}-{harness}"
        # --block: another instruction block for arm B (dev set only); the run name carries its file stem.
        self.block_file = Path(block_file) if block_file else None
        if self.block_file:
            self.name += f"-{self.block_file.stem}"
        # --land-test (arm BN): the bridge's land test (`connect --land-test`) runs the set's suite, the
        # command `t2-test` runs, on the rebased change before each land. Off by default.
        self.land_test = bool(land_test)
        if self.land_test:
            if arm != "BN":
                raise ValueError("--land-test is for arm BN")
            self.name += "-gate"
        # The runner's directory (ctx.json, transcripts, final trees, the archive of the agent area):
        # never within an agent's reach. The agents' directory is `agent_area()`.
        self.run_dir = run_dir or scratch / "t2" / "runner" / self.name
        self.made_root = None
        self.results = Path(results) if results else RESULTS
        self.agent_root_arg = Path(agent_root) if agent_root else None
        self.health_url = health_url
        self.ty = treeyard_bin()
        self.registry = {}
        self.stop_reason = None
        self.log = []

    # -------------------------------------------------------------- setup

    def note(self, msg):
        self.log.append(f"{time.strftime('%H:%M:%S')} {msg}")

    def tasks(self):
        d = self.tasks_dir / self.repo / self.set
        return sorted((p for p in d.iterdir() if p.is_dir() and p.name.split("-")[0].isdigit()), key=lambda p: int(p.name.split("-")[0]))

    def make_repo(self, where):
        repo = where / "repo"
        src = Path(self.spec.get("local_src") or self.scratch / "t2" / "src" / self.repo)
        git("clone", "-q", "--no-checkout", str(src), str(repo), cwd=where)
        git("checkout", "-q", "-B", "main", self.spec["rev"], cwd=repo)
        git("remote", "remove", "origin", cwd=repo)
        git("config", "user.name", "t2", cwd=repo)
        git("config", "user.email", "t2@example.com", cwd=repo)
        return repo

    def harness_of(self, k):
        if self.harness != "real":
            return self.harness
        return self.mix[(k - 1 + self.rep - 1) % len(self.mix)]

    def agent_area(self):
        """The directory that holds everything an agent can reach in this run (README, "Layout").
        Real runs: a new directory in the system temp dir (or --agent-root), checked by `reach_check`.
        Scripted harnesses: <scratch>/agents."""
        if self.agent_root_arg:
            root = self.agent_root_arg
        elif self.harness == "real":
            root = Path(tempfile.mkdtemp(prefix="t2-agents-"))
            self.made_root = root
        else:
            root = self.scratch / "agents"
        root.mkdir(parents=True, exist_ok=True)
        adir = root.resolve() / self.name
        if adir.exists():
            shutil.rmtree(adir)
        if self.harness == "real":
            bad = reach_check(adir, self.reach_forbidden())
            if bad:
                raise Stop("agent directories within reach of kit material: " + "; ".join(bad))
        adir.mkdir(parents=True)
        return adir

    def reach_forbidden(self):
        out = [HERE, ROOT, self.tasks_dir, self.scratch / "t2", self.run_dir, self.results]
        if self.spec.get("local_src"):
            out.append(Path(self.spec["local_src"]))
        return [Path(p) for p in out]

    def setup(self):
        if self.run_dir.exists():
            shutil.rmtree(self.run_dir)
        self.run_dir.mkdir(parents=True)
        self.adir = self.agent_area()
        # The runner's own copy of the binary inside the agent area: `ps` shows this path, not the kit's.
        if self.arm in B_ARMS and treeyard_bin().exists():
            (self.adir / "bin").mkdir()
            self.ty = self.adir / "bin" / "treeyard"
            clone_file(treeyard_bin(), self.ty)
        # Arm A, A2: the repository is the worktrees' common git dir, so it lives with them. Arm B: the
        # server's repository (and its data in .git/treeyard) stays with the runner.
        self.repo_dir = self.make_repo(self.adir if self.arm in GIT_ARMS else self.run_dir)
        self.base = git("rev-parse", "HEAD", cwd=self.repo_dir).stdout.strip()
        tasks = self.tasks()
        if len(tasks) != self.expected:
            raise Stop(f"agent count {len(tasks)} tasks, want {self.expected}")
        tasks = tasks[: self.n_agents]
        self.agents = []
        counts = {}
        ctx = {"arm": self.arm, "run_dir": str(self.run_dir), "agents": [], "treeyard": str(self.ty)}
        if self.arm in B_ARMS:
            ty = self.ty
            # The mode explicitly: `connect` is native by default, arm B is the API mode.
            native = ["--native"] if self.arm == "BN" else ["--api"]
            if self.land_test:
                native += ["--land-test", LAND_TEST_CMD]
            r = sh([str(ty), "connect", "--harness", "claude,codex,opencode", "--agents", str(self.expected), "--no-mirror", *native],
                   cwd=self.repo_dir)
            self.note("connect: " + r.stdout.strip().splitlines()[0])
            st = json.loads(sh([str(ty), "connect", "status", "--json"], cwd=self.repo_dir).stdout)
            self.url = st["url"]
            if self.url.endswith(":7420"):
                raise Stop("connect picked port 7420 (the live server); refused")
            self.health_url = self.url + "/health"
            self.home = self.adir / "node"
            ctx.update(url=self.url, home=str(self.home))
            if self.block_file:
                put_block(self.repo_dir, block_text(self.block_file))
            block = (self.repo_dir / "AGENTS.md").read_text()
            m = re.search(r"<!-- treeyard:connect:start -->.*?<!-- treeyard:connect:end -->", block, re.S)
            self.block = m.group(0) if m else block
            if self.block_file:
                if self.block != block_text(self.block_file):
                    raise Stop(f"instruction block differs from {self.block_file.name} (--block); refused")
            elif self.arm == "BN":
                if self.block != FROZEN_BLOCK_BN.read_text().rstrip("\n"):
                    raise Stop("instruction block differs from bench/t2/block-bn.md (frozen); refused")
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
            slot = self.mix[(k - 1 + self.rep - 1) % len(self.mix)]
            counts[slot] = counts.get(slot, 0) + 1
            agent = f"{slot}-{counts[slot]}"
            a = {"k": k, "agent": agent, "harness": h, "task_dir": str(td), "title": m["title"], "id": m["id"]}
            if self.arm in GIT_ARMS:
                wt = self.adir / "wt" / f"agent-{k}"
                git("worktree", "add", "-q", "-b", f"agent-{k}", str(wt), "main", cwd=self.repo_dir)
                a["cwd"] = str(wt)
            else:
                env = dict(os.environ, TREEYARD_HOME=str(self.home))
                r = sh([str(self.ty), "workspace-hook", "create", "--name", f"agent-{k}", "--agent", agent,
                        "--intent", m["title"], "--url", self.url, "--home", str(self.home)], cwd=self.repo_dir, env=env)
                a["cwd"] = r.stdout.strip().splitlines()[-1]
                a["workspace"] = f"agent-{k}"
            a["box"] = str(self.adir / "box" / f"agent-{k}")
            self.make_box(a)
            self.agents.append(a)
        ctx["agents"] = [{x: v for x, v in a.items() if x != "env"} for a in self.agents]
        self.ctx = self.run_dir / "ctx.json"
        self.ctx.write_text(json.dumps(ctx, indent=2))
        self.ctx_data = ctx
        if len(self.agents) != self.expected:
            raise Stop(f"agent count {len(self.agents)}, want {self.expected}")
        if self.arm in B_ARMS:
            wss = json.loads(sh([str(self.ty), "call", "--url", self.url, "changes", "{}"]).stdout)
            n_open = len(wss.get("changes", wss) if isinstance(wss, dict) else wss)
            if n_open != self.expected:
                raise Stop(f"agent count: {n_open} open changes after setup, want {self.expected}")

    # -------------------------------------------------------------- the agent's box: home, tmp, test env

    def make_box(self, a):
        """box/ of one agent: home/ (HOME), tmp/ (TMPDIR), bin/ (t2-test, t2-integrate in A2), cfg/ (CLI
        config: CLAUDE_CONFIG_DIR or CODEX_HOME) and, in real runs, its own venv/, cargo target/ and cargo/.
        home and tmp and the env copies go after the run; cfg goes to the run's archive."""
        box = Path(a["box"])
        for d in ("home", "tmp", "bin", "cfg"):
            (box / d).mkdir(parents=True, exist_ok=True)
        own = self.harness == "real"  # real agents get their own copies; scripted ones never run the tests
        env = base_agent_env(box, keep_t2=self.harness != "real", hide=[HERE, ROOT, self.scratch])
        fill_vals = {"python": str(self.scratch / "t2" / "env" / VENV / "bin" / "python"), "cargo": CARGO, "hook": str(box / "bin" / "ts-hook.mjs")}
        path = [str(box / "bin")]
        if self.spec.get("venv_packages"):
            src = self.scratch / "t2" / "env" / VENV
            venv = box / "venv" if own else src
            if own:
                make_venv(src, venv, self.spec["venv_packages"])
            fill_vals["python"] = str(venv / "bin" / "python")
            path.append(str(venv / "bin"))
            env["VIRTUAL_ENV"] = str(venv)
        if self.repo == "requests":
            env.update(PYTHONPATH="src", PYTHONDONTWRITEBYTECODE="1")
        if self.repo == "ripgrep":
            target = box / "target"
            warm = self.scratch / "t2" / "env" / "rg-target"
            if own and warm.is_dir():
                clone_tree(warm, target)
            else:
                target.mkdir(exist_ok=True)
            env.update(CARGO_TARGET_DIR=str(target), CARGO_NET_OFFLINE="true", CARGO_HOME=str(cargo_home(box / "cargo")),
                       RUSTUP_HOME=os.environ.get("RUSTUP_HOME") or str(Path.home() / ".rustup"))
            path.append(str(Path(CARGO).parent))
        if self.repo == "astro":
            (box / "bin" / "ts-hook.mjs").write_text(neutral_hook())
        env.update(self.spec.get("env", {}))
        env["PATH"] = os.pathsep.join(path + [env.get("PATH", "")])
        wd = Path(a["cwd"]) / self.spec.get("workdir", ".")
        rel = self.spec.get("workdir", ".")
        suite = self.fill_spec(self.spec.get("suite") or [sys.executable, "-c", "pass"], fill_vals)
        prefix = self.fill_spec(self.spec.get("test_prefix") or self.spec.get("suite") or [sys.executable, "-c", "pass"], fill_vals)
        test_env = {x: env[x] for x in ("PYTHONPATH", "PYTHONDONTWRITEBYTECODE", "CARGO_TARGET_DIR", "CARGO_HOME", "CARGO_NET_OFFLINE",
                                         "RUSTUP_HOME", "VIRTUAL_ENV", *self.spec.get("env", {})) if x in env}
        write_script(box / "bin" / TEST_CMD, test_script(a["cwd"], rel, suite, prefix, test_env))
        if self.arm in A2_ARMS:
            write_script(box / "bin" / INTEGRATE_CMD, integrate_script(box / "tmp" / ".t2-integrate"))
        a["env"] = env
        a["test_command"] = {"suite": suite, "prefix": prefix, "workdir": str(wd)}

    def fill_spec(self, cmd, vals):
        out = []
        for x in cmd:
            try:
                out.append(x.format(**vals))
            except (KeyError, IndexError):
                out.append(x)
        return out

    def arm_block(self):
        """The arm's instruction block: B and BN the connect block, A2 `A2_BLOCK`, A3 `A3_BLOCK`, A none."""
        return {"B": getattr(self, "block", None), "BN": getattr(self, "block", None), "A2": A2_BLOCK,
                "A3": A3_BLOCK}.get(self.arm)

    def prompt(self, a):
        """[arm block, empty line,] the common preamble, an empty line, the frozen task prompt."""
        text = Path(a["task_dir"], "prompt.md").read_text()
        parts = [x for x in (self.arm_block(), preamble(self.arm), text) if x]
        return "\n\n".join(parts)

    def command(self, a, resume_msg=None, session=None):
        h = a["harness"]
        if h in ("oracle", "noop", "stub"):
            extra = ["--resume"] if resume_msg else []
            parser = "script"
            if h == "stub":
                dialects = os.environ.get("T2_STUB_AS", "").split(",")
                d = dialects[a["k"] - 1] if a["k"] - 1 < len(dialects) else ""
                parser = d if d in ("claude", "codex") else "stub"
            return script_cmd(h, self.ctx, a["k"], extra), dict(a["env"]), parser
        cmd, env = real_command(h, self.prompt(a), a, self.ctx_data, self.limits, resume_msg, session, a.get("stats"))
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
        if resume is None and len(which) != self.expected:
            raise Stop(f"agent count {len(which)}, want {self.expected}")
        if not self.healthy():
            raise Stop(f"server down: {self.health_url} does not answer")
        results = {}
        done = threading.Event()
        mon = threading.Thread(target=self.monitor, args=(done,), daemon=True)
        mon.start()

        def one(a):
            msg = (resume or {}).get(a["k"])
            cmd, env, parser = self.command(a, resume_msg=msg, session=(a.get("stats") or {}).get("session"))
            a["parser"] = parser
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

    def own_diff_a(self, a, rec, onto):
        """The agent's own work, the same way in every arm: what each of its integrations (A2) and the
        runner's merge put on main (`onto` to the new main), like the diff of a landed change in arm B.
        Work that stays out of main counts from the merge base (three dots: not the reverse of main's).
        A diff against main taken after the agent integrated is empty (guard 0 for every A2 agent)."""
        parts = list(a.get("own_diffs") or [])
        if rec.get("status") == "merged":
            parts.append(git("diff", onto, "main", cwd=self.repo_dir).stdout)
        elif rec.get("status") == "left out":
            parts.append(git("diff", f"{onto}...agent-{a['k']}", cwd=self.repo_dir).stdout)
        return "".join(parts)

    def merging(self, wt):
        return git("rev-parse", "-q", "--verify", "MERGE_HEAD", cwd=wt, check=False).returncode == 0

    def finalize_a(self):
        """Arm A and A2: merges agent-1..5 into main in slot order (`git merge --no-ff`). On a conflict
        the hand-back (`handback_a`): the agent's worktree gets the conflict as files with markers, the
        agent edits them, the runner checks and commits. When that fails the agent's work stays out of
        main (`needs_human`); nothing is resolved by taking one side."""
        merges, interventions = [], 0
        for a in self.agents:
            k, wt = a["k"], a["cwd"]
            rec = {"k": k, "conflict": False, "conflict_files": [], "handback": None, "needs_human": False,
                   "intervention": False, "left_out": False}
            if self.merging(wt):  # an A2 agent stopped inside an integration conflict
                git("merge", "--abort", cwd=wt, check=False)
                rec["aborted_open_merge"] = True
            rec["skipped_paths"] = self.commit_work(wt, f"agent-{k}: work left in the worktree")
            onto = git("rev-parse", "main", cwd=self.repo_dir).stdout.strip()
            if git("merge-base", "--is-ancestor", f"agent-{k}", "main", cwd=self.repo_dir, check=False).returncode == 0:
                rec["status"] = "on main" if git("rev-parse", f"agent-{k}", cwd=self.repo_dir).stdout.strip() != self.base else "no work"
            else:
                r = git("merge", "-q", "--no-ff", "--no-edit", f"agent-{k}", cwd=self.repo_dir, check=False)
                if r.returncode == 0:
                    rec["status"] = "merged"
                else:
                    rec["conflict"] = True
                    rec["conflict_files"] = conflict_files(self.repo_dir)
                    rec["merge_output"] = tail(r.stdout + r.stderr)
                    git("merge", "--abort", cwd=self.repo_dir, check=False)
                    ok, why = self.handback_a(a, rec)
                    if ok:
                        r2 = git("merge", "-q", "--no-ff", "--no-edit", f"agent-{k}", cwd=self.repo_dir, check=False)
                        if r2.returncode != 0:
                            ok, why = False, "merge after the hand-back failed: " + tail(r2.stdout + r2.stderr)
                            git("merge", "--abort", cwd=self.repo_dir, check=False)
                    rec["handback"] = "resolved" if ok else "failed"
                    rec["status"] = "merged" if ok else "left out"
                    if not ok:
                        rec["handback_failure"] = why
                        rec.update(needs_human=True, intervention=True, left_out=True)
                        interventions += 1
            rec["own_diff"] = self.own_diff_a(a, rec, onto)
            merges.append(rec)
        return merges, interventions

    def prepare_conflict_a(self, wt):
        """Merges main into the agent's branch in its worktree (`--no-commit`) and writes each conflicted
        file with the markers of arm B: `<<<<<<< main`, `=======`, `>>>>>>> change` (main first, no base
        section; `git merge-file` on the index stages). Returns {path: prepared text} ({} = merged clean)."""
        r = git("merge", "--no-ff", "--no-commit", "main", cwd=wt, check=False)
        if r.returncode == 0:
            return {}
        stages = {}
        for line in git("ls-files", "-u", "-z", cwd=wt).stdout.split("\0"):
            if not line.strip():
                continue
            meta_, path = line.split("\t", 1)
            _, blob, stage = meta_.split()
            stages.setdefault(path, {})[int(stage)] = blob
        out = {}
        tmp = self.run_dir / "tmp"
        tmp.mkdir(exist_ok=True)
        for path, st in sorted(stages.items()):
            f = Path(wt) / path
            if 2 in st and 3 in st:
                files = []
                for n, stage in (("main", 3), ("base", 1), ("change", 2)):
                    p = tmp / f"merge-{n}"
                    p.write_bytes(git_bytes(["cat-file", "blob", st[stage]], wt) if stage in st else b"")
                    files.append(str(p))
                m = subprocess.run(["git", "merge-file", "-p", "--diff-algorithm=histogram", "-L", "main", "-L", "base", "-L", "change", *files],
                                   capture_output=True)
                f.write_bytes(m.stdout)
            out[path] = f.read_text(errors="replace") if f.is_file() else None
        return out

    def handback_a(self, a, rec):
        """One hand-back (section 6 of the preregistration, file mechanics). Returns (ok, why)."""
        k, wt = a["k"], a["cwd"]
        prepared = self.prepare_conflict_a(wt)
        rec["handback_files"] = sorted(prepared)
        if prepared:
            self.run_agents([a], resume={k: handback_text(sorted(prepared))})
        if self.merging(wt):
            left = [p for p in prepared if (Path(wt) / p).is_file() and has_marker_text((Path(wt) / p).read_text(errors="replace"))]
            if left:
                git("merge", "--abort", cwd=wt, check=False)
                return False, "conflict markers left in " + ", ".join(left)
            self.commit_work(wt, f"agent-{k}: merge main (hand-back)", extra=list(prepared))
            if self.merging(wt):
                why = "the merge commit failed"
                git("merge", "--abort", cwd=wt, check=False)
                return False, why
        else:
            self.commit_work(wt, f"agent-{k}: work left after the hand-back")
        if git("merge-base", "--is-ancestor", "main", f"agent-{k}", cwd=self.repo_dir, check=False).returncode != 0:
            return False, "main is not merged into the branch after the hand-back"
        added = git("diff", "main", f"agent-{k}", cwd=self.repo_dir).stdout
        if re.search(r"^\+(<<<<<<<|>>>>>>>)( |$)", added, re.M):
            return False, "the branch holds conflict markers"
        return True, None

    def commit_work(self, wt, message, extra=()):
        """Commits what the agent changed in `wt` under the commit rule (`commit_paths`); returns the
        paths that the rule kept out. `extra` paths (conflicted ones) are always staged."""
        paths, skipped = commit_paths(wt, self.tracked_at_pin())
        paths = sorted(set(paths) | set(extra))
        for i in range(0, len(paths), 200):
            git("add", "-A", "--", *paths[i:i + 200], cwd=wt)
        for i in range(0, len(skipped), 200):  # staged by the agent itself: out of the index again
            git("rm", "-q", "--cached", "--ignore-unmatch", "--", *skipped[i:i + 200], cwd=wt, check=False)
        if paths or self.merging(wt):
            git("commit", "-q", "--no-verify", "-m", message, cwd=wt, check=False)
        return skipped

    def tracked_at_pin(self):
        if getattr(self, "_tracked", None) is None:
            self._tracked = set(git("ls-tree", "-r", "-z", "--name-only", self.base, cwd=self.repo_dir).stdout.split("\0")) - {""}
        return self._tracked

    # -------------------------------------------------------------- finalize, arm B

    def call(self, op, args):
        r = sh([str(self.ty), "call", "--url", self.url, op, json.dumps(args)], check=False)
        try:
            return json.loads(r.stdout)
        except ValueError:
            return {"error": tail(r.stdout + r.stderr, 400)}

    def changes_of(self, agent):
        out = []
        for state in ("open", "landed"):
            d = self.call("changes", {"state": state})
            for ch in d.get("changes", []) if isinstance(d, dict) else d:
                if ch.get("agent") == agent:
                    out.append(ch)
        return out

    def native_dir(self, a, cid):
        """Arm BN: the `start` directory of change `cid` (in the agent's TMPDIR, its manifest beside it), or None."""
        if self.arm != "BN":
            return None
        root = Path(a["box"]) / "tmp" / f"treeyard-checkouts-{os.getuid()}"
        return root / cid if (root / f"{cid}.native.json").is_file() else None

    def land_native(self, a, cid):
        """`land` through a native bridge with the agent's environment (its TMPDIR, so the same directory): edits
        in the directory that the agent did not sync yet go in first. On a conflict it writes the markers there."""
        br = NativeBridge(self.ty, self.url, a["agent"], env=a["env"], cwd=a["cwd"])
        try:
            ok, text = br.call("land", {"change": cid})
        finally:
            br.close()
        try:
            land = json.loads(text) if ok else None
        except ValueError:
            land = None
        return land if isinstance(land, dict) else {"status": "error", "error": tail(text, 600)}

    def land_b(self, a, cid, is_ws):
        """One land by the runner: the workspace change through `workspace land` (it syncs first and, on a
        conflict, writes the markers into the workspace), an arm BN change with a `start` directory through
        the native bridge's `land` (the same, for that directory), any other change through `land`."""
        native = self.native_dir(a, cid)
        if native is not None:
            land = self.land_native(a, cid)
        elif is_ws:
            r = sh([str(self.ty), "workspace", "land", a["workspace"], "--wait-s", "0", "--url", self.url, "--home", str(self.home)], check=False)
            try:
                d = json.loads(r.stdout)
                land = d.get("land") or {"status": "error", "error": tail(r.stdout + r.stderr, 600)}
            except ValueError:
                land = {"status": "error", "error": tail(r.stdout + r.stderr, 600)}
        else:
            land = self.call("land", {"change": cid})
        status = land.get("status") or "error"
        if status == "error" and re.search(r"\bis empty\b", str(land.get("error") or "")):
            status = "empty"  # an idle change (the agent's work went through another change)
        rec = {"change": cid, "workspace": is_ws, "status": status}
        if native is not None:
            rec["native"] = True
        if isinstance(land.get("land_test"), dict):
            rec["land_test"] = land["land_test"]
        if status in ("tests_failed", "untested"):
            rec["output"] = tail(land.get("output") or land.get("advice") or "", 600)
        if status == "refused":  # arm BN: conflict markers left in the directory; the hand-back settles them
            rec["markers_left"] = land.get("markers_left") or []
        if status == "undoes":
            # Held: the change removes lines another agent landed. The runner never overrides.
            rec["undoes"] = [{x: e.get(x) for x in ("path", "lines", "agent", "change", "text")} for e in land.get("undoes") or []]
        elif status not in ("landed", "conflict", "already_on_main", "refused", "tests_failed", "untested"):
            rec["error"] = land.get("error") or land.get("detail") or tail(json.dumps(land), 600)
        return rec

    def finalize_b(self):
        """Arm B: lands every open change of each agent in slot order. On a conflict the hand-back
        (`handback_b`) with the same file mechanics as arm A; when it fails the change stays open and
        out of main (`needs_human`). A land held with status `undoes` (the change removes lines another
        agent landed) counts like a conflict the hand-back could not settle: not landed, `needs_human`,
        no hand-back and no override by the runner (`undoes_held` lists the change and its entries)."""
        lands, interventions = [], 0
        for a in self.agents:
            k = a["k"]
            rec = {"k": k, "conflict": False, "handback": None, "needs_human": False, "intervention": False,
                   "runner_landed": [], "left_out": [], "undoes_held": []}
            rec["skipped_paths"] = self.clean_workspace(a)
            for ch in [c for c in self.changes_of(a["agent"]) if c.get("state") == "open"]:
                cid = ch["id"]
                is_ws = ch.get("session") == f"workspace:{a['workspace']}"
                nd = self.native_dir(a, cid)
                if nd is not None:  # the same commit rule for the agent's `start` directory
                    rec["skipped_paths"] += [f"{cid}:{p}" for p in self.clean_tree(nd)]
                lr = self.land_b(a, cid, is_ws)
                rec["runner_landed"].append(lr)
                if lr["status"] == "undoes":
                    rec["undoes_held"].append({"change": cid, "undoes": lr.get("undoes") or []})
                    rec["left_out"].append(cid)
                    if not rec["needs_human"]:
                        interventions += 1
                    rec.update(needs_human=True, intervention=True)
                    continue
                if lr["status"] in ("tests_failed", "untested"):
                    # The land test refused the merged result: out of main, a person decides.
                    rec.setdefault("land_test_refused", []).append(cid)
                    rec["left_out"].append(cid)
                    if not rec["needs_human"]:
                        interventions += 1
                    rec.update(needs_human=True, intervention=True)
                    continue
                if lr["status"] not in ("conflict", "refused"):
                    continue
                rec["conflict"] = True
                if nd is not None:
                    ok, why = self.handback_native(a, cid, nd, rec)
                else:
                    ok, why = self.handback_b(a, cid, is_ws, rec)
                rec["handback"] = "resolved" if ok and rec["handback"] != "failed" else "failed"
                if not ok:
                    rec.setdefault("handback_failure", []).append(f"{cid}: {why}")
                    rec["left_out"].append(cid)
                    if not rec["needs_human"]:
                        interventions += 1
                    rec.update(needs_human=True, intervention=True)
            lands.append(rec)
        return lands, interventions

    def handback_b(self, a, cid, is_ws, rec):
        """The arm B hand-back: the conflicted paths of the change, as the server holds them (text with
        `<<<<<<< main` / `>>>>>>> change` markers), are files in the agent's workspace directory; the agent
        edits them; the runner writes them into the change with `apply` and lands. For a change other
        than the workspace change the runner puts the workspace files back afterwards. A path that the
        agent left as prepared but settled through its own tools is taken from the server."""
        k, cwd = a["k"], Path(a["cwd"])
        info = self.call("change", {"change": cid})
        paths = sorted({c["path"] for c in info.get("conflicts") or [] if c.get("path")})
        prepared, saved = {}, {}
        for p in paths:
            text = self.call("read", {"path": p, "at": cid}).get("content")
            f = cwd / p
            if not is_ws:
                saved[p] = f.read_bytes() if f.is_file() else None
            if text is not None:
                f.parent.mkdir(parents=True, exist_ok=True)
                f.write_text(text)
            prepared[p] = text
        rec.setdefault("handback_files", []).extend(paths)
        if paths:
            self.run_agents([a], resume={k: handback_text(paths)})
        final = {}
        for p in paths:
            f = cwd / p
            disk = f.read_text(errors="replace") if f.is_file() else None
            server = self.call("read", {"path": p, "at": cid}).get("content")
            if disk != prepared[p]:
                final[p] = disk
            elif server is not None and not has_marker_text(server):
                final[p] = server
            else:
                final[p] = disk
        for p, b in saved.items():
            f = cwd / p
            if b is None:
                if f.exists():
                    f.unlink()
            else:
                f.write_bytes(b)
        left = [p for p, t in final.items() if t is None or has_marker_text(t)]
        if left:
            return False, "conflict markers left in " + ", ".join(left)
        if final:
            r = self.call("apply", {"change": cid, "edits": [{"path": p, "content": t} for p, t in final.items()]})
            if "error" in r and not r.get("id") and not r.get("change"):
                return False, "apply failed: " + str(r["error"])
        land = self.call("land", {"change": cid})
        rec["runner_landed"].append({"change": cid, "workspace": is_ws, "status": land.get("status") or "error",
                                     **({} if land.get("status") == "landed" else {"error": tail(json.dumps(land), 600)})})
        if land.get("status") == "undoes":
            # The hand-back dropped lines another agent landed: held, a person decides (no override).
            rec.setdefault("undoes_held", []).append({"change": cid, "undoes": [
                {x: e.get(x) for x in ("path", "lines", "agent", "change", "text")} for e in land.get("undoes") or []]})
        if land.get("status") != "landed":
            return False, f"land after the hand-back: {land.get('status') or land.get('error')}"
        return True, None

    def handback_native(self, a, cid, d, rec):
        """The arm BN hand-back: the runner's native `land` left the conflicted files in the agent's `start`
        directory `d` with markers; the agent edits them there; the runner lands through the bridge again
        (its `land` sends the files and refuses while markers are left)."""
        info = self.call("change", {"change": cid})
        paths = sorted({c["path"] for c in info.get("conflicts") or [] if c.get("path")})
        rec.setdefault("handback_files", []).extend(paths)
        if paths:
            self.run_agents([a], resume={a["k"]: handback_native_text(d, paths)})
        if self.call("change", {"change": cid}).get("state") == "landed":
            return True, None  # the agent landed it itself
        land = self.land_native(a, cid)
        status = land.get("status") or "error"
        rec["runner_landed"].append({"change": cid, "workspace": False, "native": True, "status": status,
                                     **({} if status == "landed" else {"error": tail(json.dumps(land), 600)})})
        if status == "refused":
            return False, "conflict markers left in " + ", ".join(land.get("markers_left") or paths)
        if status == "undoes":
            rec.setdefault("undoes_held", []).append({"change": cid, "undoes": [
                {x: e.get(x) for x in ("path", "lines", "agent", "change", "text")} for e in land.get("undoes") or []]})
        if status != "landed":
            return False, f"land after the hand-back: {status if status != 'error' else land.get('error')}"
        return True, None

    def clean_workspace(self, a):
        """Arm B side of the commit rule: files that `commit_paths` keeps out of a commit in arm A (env,
        lock and cache files that the pin does not track) are removed from the workspace before the runner
        syncs it. Returns their paths."""
        return self.clean_tree(Path(a["cwd"]))

    def clean_tree(self, cwd):
        """`clean_workspace` of one directory (arm BN: also the agent's `start` directory)."""
        cwd = Path(cwd)
        tracked = self.tracked_at_pin()
        tracked_dirs = {str(Path(t).parent) for t in tracked}
        gone = []
        for root, dirs, files in os.walk(cwd):
            rel = Path(root).relative_to(cwd)
            if rel.parts[:1] in ((".treeyard",), (".git",)):
                dirs[:] = []
                continue
            for d in list(dirs):
                p = str(rel / d) if str(rel) != "." else d
                if kit_ignored(p, is_dir=True) and p not in tracked_dirs and not any(t.startswith(p + "/") for t in tracked):
                    shutil.rmtree(Path(root) / d, ignore_errors=True)
                    dirs.remove(d)
                    gone.append(p + "/")
            for fn in files:
                p = str(rel / fn) if str(rel) != "." else fn
                if p not in tracked and kit_ignored(p):
                    (Path(root) / fn).unlink(missing_ok=True)
                    gone.append(p)
        return sorted(gone)

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

    # -------------------------------------------------------------- arm A2: t2-integrate

    def start_integrator(self):
        """Answers the agents' `t2-integrate` requests (files in each box's TMPDIR), one at a time."""
        self.integrate_open = True
        self.integrate_lock = threading.Lock()
        stop = threading.Event()
        t0 = time.time()

        def loop():
            while not stop.wait(0.1):
                for a in self.agents:
                    spool = Path(a["box"]) / "tmp" / ".t2-integrate"
                    for req in sorted(spool.glob("req-*.json")) if spool.is_dir() else []:
                        try:
                            rid = json.loads(req.read_text())["id"]
                        except (ValueError, KeyError, OSError):
                            continue
                        req.unlink(missing_ok=True)
                        if self.integrate_open:
                            with self.integrate_lock:
                                try:
                                    rc, out, status = self.integrate(a)
                                except Exception as e:  # noqa: BLE001 - the agent gets the text, the run goes on
                                    rc, out, status = 2, f"{INTEGRATE_CMD}: runner error: {e}\n", "error"
                        else:
                            rc, out, status = 1, f"{INTEGRATE_CMD}: closed, the runner merges now; edit the files only\n", "closed"
                        a.setdefault("integrations", []).append({"t_s": round(time.time() - t0, 2), "exit": rc, "status": status})
                        tmp = spool / f".res-{rid}"
                        tmp.write_text(json.dumps({"exit": rc, "out": out}))
                        tmp.rename(spool / f"res-{rid}.json")

        threading.Thread(target=loop, daemon=True).start()
        return stop

    def integrate(self, a):
        """One `t2-integrate`: commit the agent's work (commit rule), merge main into its branch, and when
        that is clean fast-forward main to it. On a conflict the files keep the markers (`<<<<<<< main`,
        `>>>>>>> change`) and the merge stays open; the next call checks the markers and concludes it.
        Runs no tests. Returns (exit, text, status)."""
        k, wt = a["k"], a["cwd"]
        br = f"agent-{k}"
        if self.merging(wt):
            files = a.get("integrate_files") or conflict_files(wt)
            left = [p for p in files if (Path(wt) / p).is_file() and has_marker_text((Path(wt) / p).read_text(errors="replace"))]
            if left:
                return 1, (f"{INTEGRATE_CMD}: conflict markers are still in {', '.join(left)}. Resolve them so that your work and "
                           f"the work on main are both kept, run {TEST_CMD}, then {INTEGRATE_CMD} again.\n"), "markers left"
            self.commit_work(wt, f"{br}: merge main", extra=files)
            if self.merging(wt):
                return 1, f"{INTEGRATE_CMD}: the merge commit failed\n", "error"
        else:
            self.commit_work(wt, f"{br}: work")
        anc = lambda x, y: git("merge-base", "--is-ancestor", x, y, cwd=self.repo_dir, check=False).returncode == 0  # noqa: E731
        if anc(br, "main"):
            return 0, f"{INTEGRATE_CMD}: nothing to integrate, main has all of your work\n", "nothing"
        if not anc("main", br):
            prepared = self.prepare_conflict_a(wt)
            if prepared:
                a["integrate_files"] = sorted(prepared)
                return 1, (f"{INTEGRATE_CMD}: conflict with the work on main in {', '.join(sorted(prepared))}. These files now hold both "
                           "versions with conflict markers: the lines between `<<<<<<< main` and `=======` are on main, the lines "
                           "between `=======` and `>>>>>>> change` are yours. Resolve them so that your work and the work on main are "
                           f"both kept, run {TEST_CMD}, then {INTEGRATE_CMD} again.\n"), "conflict"
            self.commit_work(wt, f"{br}: merge main")
        before = git("rev-parse", "main", cwd=self.repo_dir).stdout.strip()
        r = git("merge", "-q", "--ff-only", br, cwd=self.repo_dir, check=False)
        if r.returncode != 0:
            return 1, f"{INTEGRATE_CMD}: main did not move: {tail(r.stdout + r.stderr, 300)}\n", "error"
        # The agent's own work in this integration: what moving main put on it (the guard metrics).
        a.setdefault("own_diffs", []).append(git("diff", before, "main", cwd=self.repo_dir).stdout)
        a.pop("integrate_files", None)
        head = git("rev-parse", "--short", "main", cwd=self.repo_dir).stdout.strip()
        return 0, f"{INTEGRATE_CMD}: integrated, main is now your work ({head})\n", "integrated"

    # -------------------------------------------------------------- metrics from the transcripts

    def transcript_metrics(self, pairs_truth):
        """Warnings, reactions and test runs per agent, read from each agent's own tool replies (not from
        the server's event stream). `warned`: a reply of the agent's own tool call held a file-level
        collision (a `path` with kind overlap or conflict; a conflict with main counts; a symbol or
        declared hit does not). `reacted`: after that reply and before its next land, the agent called
        change, rebase, apply or resolve. Descriptive only."""
        k_of = {}
        if self.arm in B_ARMS:
            for a in self.agents:
                for ch in self.changes_of(a["agent"]):
                    k_of[ch["id"]] = a["k"]
        agents = []
        for a in self.agents:
            path = self.run_dir / f"agent-{a['k']}.jsonl"
            lines = path.read_text(errors="replace").splitlines() if path.exists() else []
            agents.append((a["k"], a["agent"], tool_events(a.get("parser") or a["harness"], lines, Path(a["box"]))))
        return metrics_of(self.arm, agents, k_of, pairs_truth or {})

    # -------------------------------------------------------------- score

    def final_tree(self):
        tree = self.run_dir / "final"
        if self.arm in GIT_ARMS:
            git("clone", "-q", "--branch", "main", str(self.repo_dir), str(tree), cwd=self.run_dir)
        else:
            git("clone", "-q", f"{self.url}/git", str(tree), cwd=self.run_dir)
        return tree

    def pinned_tree(self, tree):
        """A copy of the final tree whose test files are those of the pin: agent-written test files go,
        changed ones are restored (`test_globs` of repos.json). Returns (dir, changed paths)."""
        globs = self.spec.get("test_globs") or DEFAULT_TEST_GLOBS
        dst = self.run_dir / "final-pinned"
        if dst.exists():
            shutil.rmtree(dst)
        shutil.copytree(tree, dst, symlinks=True, ignore=shutil.ignore_patterns(".git"))
        def snap(f):
            return ("link", os.readlink(f)) if f.is_symlink() else f.read_bytes() if f.is_file() else None

        def tests_in(d):
            return {str(f.relative_to(d)): f for f in d.rglob("*") if (f.is_symlink() or f.is_file())
                    and any(glob_match(str(f.relative_to(d)), g) for g in globs)}

        before = {rel: snap(f) for rel, f in tests_in(dst).items()}
        for f in tests_in(dst).values():
            f.unlink()
        idx = self.run_dir / "tmp" / "pinned.index"
        idx.parent.mkdir(exist_ok=True)
        env = dict(os.environ, GIT_INDEX_FILE=str(idx))
        pin_tests = sorted(p for p in self.tracked_at_pin() if any(glob_match(p, g) for g in globs))
        for i in range(0, len(pin_tests), 200):
            sh(["git", f"--git-dir={self.repo_dir / '.git'}", f"--work-tree={dst}", "checkout", self.base, "--",
                *pin_tests[i:i + 200]], env=env)
        after = {rel: snap(f) for rel, f in tests_in(dst).items()}
        changed = sorted(p for p in set(before) | set(after) if before.get(p) != after.get(p))
        return dst, changed

    def score(self, tree):
        """`suite_green_all`: the suite on final main as it is (agent-written tests included; `suite_green`
        is the same value under its old name). `suite_green_pinned`: the suite with the test files of the
        pin. Then the hidden deciding tests go into the final tree."""
        out = {}
        if self.spec.get("suite"):
            ok, text, secs = run_cmd_spec(self.spec, self.spec["suite"], tree, self.scratch)
            out.update(suite_green=ok, suite_green_all=ok, suite_s=secs, suite_tail_all=tail(text, 1500))
            pinned, changed = self.pinned_tree(tree)
            ok2, text2, secs2 = run_cmd_spec(self.spec, self.spec["suite"], pinned, self.scratch)
            out.update(suite_green_pinned=ok2, suite_pinned_s=secs2, suite_tail_pinned=tail(text2, 1500), test_files_restored=changed)
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
        out["n_tasks"] = len(done)
        # Integration tests of an extra set: both behaviours of a designed pair at once, a metric of its own.
        if self.set_spec["integration"]:
            integ = {}
            for it in self.set_spec["integration"]:
                src = self.tasks_dir / self.repo / self.set / "integration" / it["id"] / "hidden" / it["test"]
                dst = tree / it["test"]
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(src, dst)
                ok, text, secs = run_cmd_spec(self.spec, self.spec["decide"], tree, self.scratch, {"repo": self.repo, "test": it["test"]})
                integ[it["id"]] = ok
            out["integration"] = integ
            out["integration_passed"] = sum(integ.values())
            out["integration_total"] = len(integ)
        return out

    # -------------------------------------------------------------- legacy warnings (arm B, server events)

    def warnings(self, pairs_truth):
        """The warning metrics of preregistration v3 (section 5), from the server's event stream matched to
        audit rows by the second. Kept under `legacy_*` keys; `transcript_metrics` has the new ones."""
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
        """Arm setup, the final prompts and the real-agent command lines, with no agent started (wiring
        check, free). The box environments are made as in a real run (venvs from the uv cache)."""
        os.environ["T2_DRY"] = "1"
        lines = []
        try:
            self.setup()
            lines.append(json.dumps({"run": self.name, "agent_area": "system temp dir" if self.made_root else "--agent-root or scratch",
                                     "preamble_sha256": sha_text(preamble(self.arm)),
                                     "arm_block_sha256": sha_text(self.arm_block()) if self.arm_block() else None,
                                     "handback_sha256": sha_text(HANDBACK_BN if self.arm == "BN" else HANDBACK)}))
            for a in self.agents:
                cmd, env, parser = self.command(a)
                p = self.prompt(a)
                shown = ["<prompt>" if x == p else x for x in cmd]
                homes = {k: v for k, v in env.items() if k in ("CLAUDE_CONFIG_DIR", "CODEX_HOME", "XDG_CONFIG_HOME", "HOME", "TMPDIR")}
                lines.append(json.dumps({"k": a["k"], "agent": a["agent"], "harness": a["harness"], "cwd": a["cwd"],
                                         "command": shown, "config_homes": homes, "path_head": env["PATH"].split(os.pathsep)[:3],
                                         "test_command": a["test_command"], "prompt_sha256": sha_text(p), "prompt": p}))
        finally:
            os.environ.pop("T2_DRY", None)
            if self.arm in B_ARMS and getattr(self, "repo_dir", None) and (self.repo_dir / ".git" / "treeyard").exists():
                sh([str(self.ty), "disconnect", "--purge"], cwd=self.repo_dir, check=False)
            self.archive()
        return lines

    def archive(self):
        """After the run: HOME, TMPDIR and the env copies of every box go, the logins are deleted, and the
        agent area moves into the runner's directory (`agents/`), so no later agent can reach it."""
        adir = getattr(self, "adir", None)
        if not adir or not adir.exists():
            return
        for box in (adir / "box").glob("*") if (adir / "box").exists() else []:
            if self.arm == "BN":
                self.keep_start_dirs(box)
            for sub in ("home", "tmp", "venv", "target", "cargo"):
                p = box / sub
                if p.is_symlink() or p.is_file():
                    p.unlink()
                elif p.exists():
                    shutil.rmtree(p, ignore_errors=True)
            for p in (box / "cfg").glob("auth.json"):
                p.unlink()
        dst = self.run_dir / "agents"
        if dst.exists():
            shutil.rmtree(dst, ignore_errors=True)
        shutil.move(str(adir), str(dst))
        if self.made_root and self.made_root.exists() and not any(self.made_root.iterdir()):
            self.made_root.rmdir()

    def keep_start_dirs(self, box):
        """Arm BN: each `start` directory of the agent (and its manifest, follow-up and stale copies) goes to
        `box/start/` before TMPDIR is deleted, without build caches, so unsent edits can be audited."""
        root = box / "tmp" / f"treeyard-checkouts-{os.getuid()}"
        if not root.is_dir():
            return
        dst = box / "start"
        skip = shutil.ignore_patterns(*KIT_IGNORE_DIRS, "*.pyc", "*.pyo", ".DS_Store")
        for p in sorted(root.iterdir()):
            try:
                if p.is_dir() and not p.is_symlink():
                    shutil.copytree(p, dst / p.name, symlinks=True, ignore=skip)
                elif p.is_file() and p.name.endswith(".native.json"):
                    dst.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(p, dst / p.name)
            except OSError as e:
                self.note(f"start dir not archived: {p.name}: {e}")

    def scrub(self, text):
        """Run files are published: local paths become placeholders."""
        pairs = [(getattr(self, "adir", None), "<agents>"), (self.run_dir, "<run>"), (self.scratch, "<scratch>"),
                 (HERE, "<kit>"), (ROOT, "<root>"), (Path.home(), "~")]
        for p, name in sorted(((str(p), n) for p, n in pairs if p), key=lambda x: -len(x[0])):
            text = text.replace(p, name)
            if p.startswith("/private/"):
                text = text.replace(p[len("/private"):], name)
        return text

    # -------------------------------------------------------------- the whole run

    def execute(self, pairs_truth=None):
        t0 = time.time()
        result = {"run": self.name, "arm": self.arm, "repo": self.repo, "set": self.set, "rep": self.rep, "kit": KIT_VERSION,
                  "harness": self.harness, "limits": self.limits, "started": time.strftime("%Y-%m-%dT%H:%M:%S"),
                  "preamble_sha256": sha_text(preamble(self.arm)),
                  "handback_sha256": sha_text(HANDBACK_BN if self.arm == "BN" else HANDBACK)}
        if self.set_spec["from_file"]:
            result["set_agents"] = self.expected
            result["set_mix"] = self.mix
        if self.harness == "real":
            result["user_config_before"] = user_config_hashes()
        service = None
        try:
            self.setup()
            result.update(binary_id(self.ty if Path(self.ty).exists() else treeyard_bin()))
            result["agent_area"] = "system temp dir" if self.made_root else ("--agent-root" if self.agent_root_arg else "scratch")
            result["agents"] = [{"k": a["k"], "agent": a["agent"], "harness": a["harness"], "id": a["id"],
                                 "prompt_sha256": sha_text(self.prompt(a))} for a in self.agents]
            if self.arm_block():
                result["arm_block_sha256"] = sha_text(self.arm_block())
            if self.arm == "BN":
                result["land_test_gate"] = {"on": self.land_test, "command": LAND_TEST_CMD if self.land_test else None}
            if self.arm in B_ARMS:
                result["block_sha256"] = hashlib.sha256(self.block.encode()).hexdigest()
                if self.block_file:
                    result["block_file"] = self.block_file.name
            if self.arm in A2_ARMS:
                service = self.start_integrator()
            t_agents = time.time()
            self.run_agents()
            if service:
                self.integrate_open = False
            if self.arm in GIT_ARMS:
                merges, interventions = self.finalize_a()
                own = {m["k"]: m.pop("own_diff") for m in merges}
                result["merges"] = merges
                result["merge_conflicts"] = sum(m["conflict"] for m in merges)
                result.update(land_conflicts=0)
                if self.arm in A2_ARMS:
                    result["integrations"] = {str(a["k"]): a.get("integrations", []) for a in self.agents}
                    result["integrate_conflicts"] = sum(1 for a in self.agents for x in a.get("integrations", []) if x["status"] == "conflict")
                    result["integrated_by_agent"] = sum(1 for m in merges if m.get("status") == "on main")
            else:
                lands, interventions = self.finalize_b()
                result["lands"] = lands
                result["undoes_held"] = sum(len(x.get("undoes_held") or []) for x in lands)
                legacy = self.warnings(pairs_truth or {})
                result["land_conflicts"] = legacy.pop("land_conflicts")
                result.update({"legacy_" + x: v for x, v in legacy.items()})
                result["merge_conflicts"] = result["land_conflicts"]
                own = self.own_diffs_b()
            result["interventions"] = interventions
            result["needs_human"] = interventions
            result["wall_s"] = round(time.time() - t_agents, 2)
            result["setup_s"] = round(t_agents - t0, 2)
            result.update(self.transcript_metrics(pairs_truth or {}))
            if self.arm in GIT_ARMS:
                result["conflict_work"] = conflict_work(self.arm, self.agents, self.run_dir)
                result["flow"] = integration_flow(self.arm, self.agents, result.get("merges"), result.get("wall_s"))
            if self.arm == "BN":
                bn_undoes_held(result)
                gated = [x for rec in result.get("lands") or [] for x in rec["runner_landed"] if x.get("land_test")]
                result["land_tests_failed_runner"] = sum(1 for rec in result.get("lands") or [] for _ in rec.get("land_test_refused") or [])
                result["land_test_rounds"] = result.get("land_test_rounds", 0) + sum(int(x["land_test"].get("rounds") or 0) for x in gated)
                result["land_test_seconds"] = round(result.get("land_test_seconds", 0) + sum(float(x["land_test"].get("seconds") or 0) for x in gated), 2)
            result["guard"] = {str(k): diff_stats(d, self.spec.get("test_globs") or DEFAULT_TEST_GLOBS) for k, d in own.items()}
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
            if service:
                service.set()
            if self.arm in B_ARMS and getattr(self, "repo_dir", None) and (self.repo_dir / ".git" / "treeyard").exists():
                sh([str(self.ty), "disconnect", "--purge"], cwd=self.repo_dir, check=False)
            result["models"] = self.models()
            self.archive()
        result["agent_stats"] = [{"k": a["k"], "harness": a["harness"], "invocations": a.get("invocations", 0),
                                  **{k: v for k, v in (a.get("stats") or {}).items() if k != "session" and not k.startswith("_")}}
                                 for a in getattr(self, "agents", [])]
        for key in TOKEN_KEYS:
            result[key] = sum(s.get(key, 0) for s in result["agent_stats"])
        result["by_harness"] = by_harness(result["agent_stats"])
        result["cost_usd_claude_only"] = round(sum(s.get("cost_usd", 0) for s in result["agent_stats"]), 6)
        result["cost_usd"] = result["cost_usd_claude_only"]  # legacy name; Codex reports no cost
        result["tool_calls"] = sum(s.get("tool_calls", 0) for s in result["agent_stats"])
        result["limit_kills"] = [f"agent-{s['k']}: {s['killed']}" for s in result["agent_stats"] if s.get("killed")]
        if self.harness == "real":
            result["user_config_after"] = user_config_hashes()
            result["user_config_unchanged"] = result["user_config_before"] == result["user_config_after"]
        result["log"] = self.log
        self.results.mkdir(parents=True, exist_ok=True)
        text = self.scrub(json.dumps(result, indent=1, sort_keys=True) + "\n")
        (self.results / f"{self.name}.json").write_text(text)
        return json.loads(text)

    def models(self):
        """The model names of each harness, from the transcripts (Claude) and the rollout files (Codex)."""
        out = {}
        for a in getattr(self, "agents", []):
            names = list((a.get("stats") or {}).get("models") or [])
            if a["harness"] == "codex":
                names += codex_models(Path(a["box"]) / "cfg")
            if names:
                out.setdefault(a["harness"], [])
                out[a["harness"]] = list(dict.fromkeys(out[a["harness"]] + names))
        return out


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


def tail(text, n=2000):
    return (text or "")[-n:]


def git_bytes(args, cwd):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, check=True).stdout


MARKER_RE = re.compile(r"^(<<<<<<<|>>>>>>>)( |$)", re.M)


def has_marker_text(text):
    """A conflict start or end marker at the start of a line (`=======` alone is a heading underline)."""
    return bool(MARKER_RE.search(text or ""))


# Env, lock and cache files that agents' tools create. The commit rule (`commit_paths`) keeps them out
# of the agent's commit when the pin does not track the path: a path the pinned commit tracks is always
# committed, an untracked one only when the repository's ignore rules and this list both let it in.
KIT_IGNORE_FILES = ("uv.lock", "poetry.lock", "Pipfile.lock", "pdm.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
                    "bun.lockb", "Cargo.lock", ".coverage", ".DS_Store", "*.pyc", "*.pyo", "*.orig", "*.rej")
KIT_IGNORE_DIRS = (".venv", "venv", ".tox", "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".hypothesis",
                   "node_modules", "target", ".cache", "*.egg-info", ".eggs", "build", "dist", ".t2")


def kit_ignored(path, is_dir=False):
    import fnmatch

    parts = Path(path).parts
    dirs = parts if is_dir else parts[:-1]
    if any(fnmatch.fnmatch(d, pat) for d in dirs for pat in KIT_IGNORE_DIRS):
        return True
    return not is_dir and any(fnmatch.fnmatch(parts[-1], pat) for pat in KIT_IGNORE_FILES)


def commit_paths(wt, tracked):
    """(paths to stage, paths kept out) for the changes in worktree `wt` under the commit rule."""
    out = git("status", "--porcelain=v1", "-z", "--untracked-files=all", cwd=wt).stdout.split("\0")
    keep, skip = [], []
    i = 0
    while i < len(out):
        e = out[i]
        i += 1
        if len(e) < 4:
            continue
        code, path = e[:2], e[3:]
        if code[0] in "RC":  # a staged rename or copy: the source path follows
            if i < len(out) and out[i]:
                keep.append(out[i])
            i += 1
        if path in tracked or not kit_ignored(path):
            keep.append(path)
        else:
            skip.append(path)
    return keep, skip


HANDBACK = (
    "Your work conflicts with work that is already on main. The files listed below, in this directory, now hold "
    "both versions with conflict markers: the lines between `<<<<<<< main` and `=======` are on main, the lines "
    "between `=======` and `>>>>>>> change` are yours. Edit these files so that your work and the work on main are "
    "both kept, and remove every marker. Edit the files only: do not merge, commit or land anything, the runner "
    "does that when you stop.\n\nFiles: {files}"
)


# Arm BN: the files are in the agent's `start` directory, not in its working directory.
HANDBACK_BN = HANDBACK.replace("The files listed below, in this directory,", "The files listed below, in your directory `{dir}`,")


def handback_text(paths):
    return HANDBACK.format(files=", ".join(f"`{p}`" for p in paths))


def handback_native_text(d, paths):
    return HANDBACK_BN.format(dir=d, files=", ".join(f"`{p}`" for p in paths))


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


METRICS = ["suite_green", "suite_green_all", "suite_green_pinned", "tasks_on_main", "merge_conflicts", "warned", "reacted",
           "legacy_warned_before_land", "legacy_reacted", "interventions", "wall_s", "lost_lines", "tokens_in", "tokens_out",
           "tokens_fresh_in", "tokens_cached_in", "cost_usd_claude_only", "false_alarms", "tool_calls", "test_passed_agents",
           "integrate_conflicts", "n_tasks", "integration_passed", "integration_total"]


def normalize(r):
    """One run file in the fields of this kit version. Old files (no `kit`): `warned_before_land` and
    `reacted` are the legacy event-stream metrics, `suite_green` is the suite on final main as is
    (= suite_green_all), `cost_usd` is Claude's cost; the new transcript metrics are None."""
    r = dict(r)
    if (r.get("kit") or 1) < 2:
        r["legacy_warned_before_land"] = r.pop("warned_before_land", None)
        r["legacy_reacted"] = r.pop("reacted", None)
        r["legacy_false_alarms"] = r.get("false_alarms")
        r["warned"] = r["reacted"] = None
        r.setdefault("suite_green_all", r.get("suite_green"))
        r.setdefault("suite_green_pinned", None)
        r.setdefault("cost_usd_claude_only", r.get("cost_usd"))
        if "by_harness" not in r:
            harness_of = {a.get("k"): a.get("harness") for a in r.get("agents") or []}
            r["by_harness"] = by_harness([dict(s, harness=harness_of.get(s.get("k"))) for s in r.get("agent_stats") or []])
    return r


def collect(results_dir, out_json, out_txt, only=None):
    rows = []
    for f in sorted(Path(results_dir).glob("*.json")):
        if f.name in ("metrics.json",):
            continue
        try:
            r = json.loads(f.read_text())
        except ValueError:
            continue
        if not isinstance(r, dict) or "run" not in r or (only and r.get("harness") not in only):
            continue
        rows.append(normalize(r))
    table = []
    for r in rows:
        row = {"run": r["run"], "status": r.get("status"), "stop_reason": r.get("stop_reason"), "kit": r.get("kit") or 1}
        for m in METRICS:
            v = r.get(m)
            row[m] = v["total"] if isinstance(v, dict) and "total" in v else v
        row["by_harness"] = {h: {k: v.get(k, 0) for k in (*TOKEN_KEYS, "cost_usd")} for h, v in (r.get("by_harness") or {}).items()}
        table.append(row)
    Path(out_json).write_text(json.dumps({"runs": table}, indent=1) + "\n")
    fmt = lambda v: {True: "green", False: "red", None: "-"}.get(v, v) if isinstance(v, bool) or v is None else v  # noqa: E731
    hdr = ["run", "status", "suite all", "suite pinned", "done", "integration", "merge/land conflicts", "warned", "reacted", "legacy warned",
           "legacy reacted", "interv", "lost", "tests passed", "tool calls", "tokens", "Claude tokens", "Codex tokens",
           "cost (Claude only)", "wall s"]
    lines = ["| " + " | ".join(hdr) + " |", "|" + "---|" * len(hdr)]
    for r in table:
        bh = r["by_harness"]
        lines.append("| " + " | ".join(str(x) for x in [
            r["run"], r["status"] if r["status"] == "done" else f"stopped: {r['stop_reason']}",
            fmt(r["suite_green_all"]), fmt(r["suite_green_pinned"]),
            f"{r['tasks_on_main']}/{r['n_tasks'] or 5}" if r["tasks_on_main"] is not None else "-",
            f"{r['integration_passed']}/{r['integration_total']}" if r["integration_total"] else "-",
            fmt(r["merge_conflicts"]), fmt(r["warned"]), fmt(r["reacted"]), fmt(r["legacy_warned_before_land"]), fmt(r["legacy_reacted"]),
            fmt(r["interventions"]), fmt(r["lost_lines"]), fmt(r["test_passed_agents"]), fmt(r["tool_calls"]),
            (r["tokens_in"] or 0) + (r["tokens_out"] or 0),
            *((bh[h]["tokens_in"] + bh[h]["tokens_out"]) if h in bh else "-" for h in ("claude", "codex")),
            fmt(r["cost_usd_claude_only"]), fmt(r["wall_s"])]) + " |")
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


def own_changes(events):
    """The change ids an agent opened or named in its own treeyard calls."""
    out = set()
    for e in events:
        op = e.get("op")
        args = e.get("args") if isinstance(e.get("args"), dict) else {}
        try:
            reply = json.loads(e.get("reply") or "")
        except ValueError:
            reply = {}
        if op in ("open_change", "start") and isinstance(reply, dict) and reply.get("id"):
            out.add(reply["id"])
        if op and isinstance(args.get("change"), str):
            out.add(args["change"])
    return out


def recount_metrics(runs_dir, results_dirs, out_dir=None):
    """The transcript metrics (`metrics_of`, and `bn_undoes_held` for arm BN) of finished arm B and BN runs,
    again from their run files and transcripts (`<runs_dir>/<run>/agent-<k>.jsonl`, `ctx.json`). Prints one
    row per run; with `out_dir` writes each recounted run file there (the run files stay as they are).
    `false_alarms` keeps its recorded value (it needs the pair table)."""
    rows = []
    for d in results_dirs or ():
        for f in sorted(Path(d).glob("*.json")):
            try:
                r = json.loads(f.read_text())
            except ValueError:
                continue
            if not isinstance(r, dict) or r.get("arm") not in B_ARMS or not r.get("run"):
                continue
            run = Path(runs_dir) / r["run"]
            if not list(run.glob("agent-*.jsonl")):
                continue
            harness_of = {}
            if (run / "ctx.json").is_file():
                try:
                    harness_of = {a["k"]: a.get("harness") for a in json.loads((run / "ctx.json").read_text()).get("agents", [])}
                except (ValueError, KeyError):
                    pass
            names = {a["k"]: a.get("agent") for a in r.get("agents") or []}
            agents, k_of = [], {}
            for t in sorted(run.glob("agent-*.jsonl"), key=lambda p: int(p.stem.split("-")[1])):
                k = int(t.stem.split("-")[1])
                h = harness_of.get(k)
                h = h if h in ("claude", "codex") else transcript_harness(t) or "script"
                ev = tool_events(h, t.read_text(errors="replace").splitlines())
                agents.append((k, names.get(k), ev))
                for c in own_changes(ev):
                    k_of.setdefault(c, k)
            m = metrics_of(r["arm"], agents, k_of)
            new = dict(r, **m)
            if r["arm"] == "BN":
                bn_undoes_held(new)
            new["recounted"] = {"by": "recount --metrics", "time": time.strftime("%Y-%m-%dT%H:%M:%S"),
                                "was": {x: r.get(x) for x in ("warned", "reacted", "undoes_held")}}
            rows.append(new)
            if out_dir:
                Path(out_dir).mkdir(parents=True, exist_ok=True)
                (Path(out_dir) / f.name).write_text(json.dumps(new, indent=1, sort_keys=True) + "\n")
    cols = ("warned", "reacted", "land_holds", "hold_reacted", "undoes_overridden", "agent_land_conflicts", "land_refused",
            "undoes_held_runner", "undoes_held")
    lines = ["| run | " + " | ".join(cols) + " | was warned / reacted / undoes_held |", "|---" * (len(cols) + 2) + "|"]
    for r in rows:
        was = r["recounted"]["was"]
        lines.append("| " + " | ".join([r["run"], *(str(r.get(c, "-")) for c in cols),
                                        f"{was['warned']} / {was['reacted']} / {was['undoes_held']}"]) + " |")
    print("\n".join(lines))
    return rows


def main_health(scratch, run, repo, s, tasks_dir=None, spec=None):
    """Every state of main in a finished run (`<run>/final`, first-parent commits after the pin): the hidden
    tests of the tasks and the set's integration tests at each commit. A test that passed at one commit and
    fails at a later one is a break (`breaks`: test, from, to; `transient` when it passes again later).
    Reads the run directory only; works in a clone in the system temp directory."""
    tasks_dir = tasks_dir or TASKS_DIR
    spec = spec or REPOS[repo]
    final = Path(run) / "final"
    commits = git("rev-list", "--first-parent", "--reverse", f"{spec['rev']}..HEAD", cwd=final).stdout.split()
    tests = []
    for td in sorted((p for p in (tasks_dir / repo / s).iterdir() if p.is_dir() and p.name.split("-")[0].isdigit()),
                     key=lambda p: int(p.name.split("-")[0])):
        m = json.loads((td / "meta.json").read_text())
        tests.append((m["id"], td / "hidden" / m["test"], m["test"], m))
    for it in set_spec(tasks_dir, repo, s)["integration"]:
        src = tasks_dir / repo / s / "integration" / it["id"] / "hidden" / it["test"]
        tests.append((it["id"], src, it["test"], {"repo": repo, "test": it["test"]}))
    work = Path(tempfile.mkdtemp(prefix="t2-health-"))
    wt = work / "tree"
    rows = []
    try:
        git("clone", "-q", "--no-checkout", str(final), str(wt), cwd=work)
        for c in commits:
            git("checkout", "-q", "-f", "--detach", c, cwd=wt)
            git("clean", "-fdxq", cwd=wt)
            passed = {}
            for tid, src, rel, m in tests:
                dst = wt / rel
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(src, dst)
                ok, _, _ = run_cmd_spec(spec, spec["decide"], wt, scratch, m)
                passed[tid] = ok
                git("checkout", "-q", "-f", c, "--", ".", cwd=wt)
                git("clean", "-fdxq", cwd=wt)
            subject = git("log", "-1", "--format=%s", c, cwd=wt).stdout.strip()
            rows.append({"commit": c[:12], "subject": subject, "passed": passed})
    finally:
        shutil.rmtree(work, ignore_errors=True)
    breaks = []
    for tid, *_ in tests:
        seq = [r["passed"][tid] for r in rows]
        for i in range(1, len(seq)):
            if seq[i - 1] and not seq[i]:
                back = next((j for j in range(i + 1, len(seq)) if seq[j]), None)
                breaks.append({"test": tid, "from": rows[i]["commit"], "subject": rows[i]["subject"],
                               "to": rows[back]["commit"] if back is not None else None, "transient": back is not None})
    ids = [t[0] for t in tests]
    print("| commit | subject | " + " | ".join(ids) + " |")
    print("|---" * (len(ids) + 2) + "|")
    for r in rows:
        print(f"| {r['commit']} | {r['subject'][:50]} | " + " | ".join("ok" if r["passed"][i] else "FAIL" for i in ids) + " |")
    for b in breaks:
        print(f"break: {b['test']} at {b['from']} ({b['subject'][:60]})" + (f", passes again at {b['to']}" if b["transient"] else ", never again"))
    return {"rows": rows, "breaks": breaks}


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
        m = re.match(r"^(A3|A2|A|B)-(\w+)-(disjoint|colliding|mixed)-", r["run"])
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


# ===================================================================================== conflict work (git arms)

# Relative price weights of Claude token kinds (input 1, cache write 2 for the 1 h cache, cache read 0.1, output 5):
# the cost of a part of a session is the session's cost times the part's share of price-weighted tokens.
PRICE_WEIGHTS = {"input_tokens": 1.0, "cache_creation_input_tokens": 2.0, "cache_read_input_tokens": 0.1, "output_tokens": 5.0}
CONFLICT_SIGNS = ("CONFLICT (", "Automatic merge failed", "conflict with the work on main", "conflict markers are still in")
CLEAN_SIGNS = ("integrated, main is now", "nothing to integrate", "Already up to date", "Merge made by", "Fast-forward")


def _iso_s(text):
    import datetime
    try:
        return datetime.datetime.fromisoformat(str(text).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def claude_sessions(path):
    """The invocations of one Claude agent transcript (the first run, then each hand-back): per invocation its
    cost (`total_cost_usd` is the running total of the session), seconds (`duration_ms`), messages
    [(time, price-weighted tokens)] and tool calls [(time of the result, input text, result text)]."""
    sessions, cur, prev_cost, pending = [], None, 0.0, {}
    for line in Path(path).read_text(errors="replace").splitlines():
        try:
            o = json.loads(line)
        except ValueError:
            continue
        t = o.get("type")
        if t == "system" and o.get("subtype") == "init" or (t in ("assistant", "user") and cur is None):
            cur = {"cost": 0.0, "seconds": 0.0, "msgs": [], "tools": []}
            sessions.append(cur)
            if t == "system":
                continue
        if t == "assistant":
            m = o.get("message") or {}
            u = m.get("usage") or {}
            w = sum(PRICE_WEIGHTS[x] * float(u.get(x) or 0) for x in PRICE_WEIGHTS)
            ts = _iso_s(o.get("timestamp"))
            cur["msgs"].append((ts, w))
            for c in m.get("content") or []:
                if isinstance(c, dict) and c.get("type") == "tool_use":
                    pending[c.get("id")] = json.dumps(c.get("input"))
        elif t == "user":
            content = (o.get("message") or {}).get("content")
            for c in content if isinstance(content, list) else []:
                if isinstance(c, dict) and c.get("type") == "tool_result" and c.get("tool_use_id") in pending:
                    r = c.get("content")
                    cur["tools"].append((_iso_s(o.get("timestamp")), pending.pop(c["tool_use_id"]),
                                         r if isinstance(r, str) else json.dumps(r)))
        elif t == "result" and cur is not None:
            total = float(o.get("total_cost_usd") or 0)
            cur["cost"] = max(0.0, total - prev_cost) if total >= prev_cost else total
            prev_cost = total
            cur["seconds"] = float(o.get("duration_ms") or 0) / 1000
            cur = None
    return sessions


def conflict_episodes(tools):
    """[(start, end)] times of the conflict work in one invocation of an arm A2/A3 agent: from a `t2-integrate`
    or `git merge`/`git pull` whose result reports a conflict until the next clean integrate or merge (or the
    `git commit` that concludes a merge); an episode that never ends runs to the last tool result."""
    eps, start, by_merge = [], None, False
    last = None
    for ts, inp, res in tools:
        if ts is None:
            continue
        last = ts
        is_int = "t2-integrate" in inp
        is_merge = "git merge" in inp or "git pull" in inp
        if start is None:
            if (is_int or is_merge) and any(x in (res or "") for x in CONFLICT_SIGNS):
                start, by_merge = ts, is_merge
            continue
        clean = (is_int or is_merge) and any(x in (res or "") for x in CLEAN_SIGNS) and not any(x in (res or "") for x in CONFLICT_SIGNS)
        concluded = by_merge and "git commit" in inp and "error" not in (res or "").lower() and "conflict" not in (res or "").lower()
        if clean or concluded:
            eps.append((start, ts))
            start = None
    if start is not None:
        eps.append((start, last))
    return eps


def conflict_work(arm, agents, run_dir):
    """Seconds and Claude USD that the Claude agents of a git arm spent on conflict work, with their share of the
    Claude agents' totals. Arm A: every invocation after the first (the hand-backs). Arms A2, A3: the episodes of
    `conflict_episodes`; an episode's cost is the invocation's cost times its share of price-weighted tokens."""
    per, tot_s, tot_usd, cw_s, cw_usd, n_eps = [], 0.0, 0.0, 0.0, 0.0, 0
    for a in agents:
        if a.get("harness") != "claude":
            continue
        f = Path(run_dir) / f"agent-{a['k']}.jsonl"
        if not f.is_file():
            continue
        ses = claude_sessions(f)
        s_all = sum(x["seconds"] for x in ses)
        usd_all = sum(x["cost"] for x in ses)
        s_cw = usd_cw = 0.0
        eps = 0
        if arm == "A":
            for x in ses[1:]:
                s_cw += x["seconds"]
                usd_cw += x["cost"]
                eps += 1
        else:
            for x in ses:
                weight = sum(w for _, w in x["msgs"]) or 0.0
                for t0, t1 in conflict_episodes(x["tools"]):
                    eps += 1
                    s_cw += max(0.0, t1 - t0)
                    part = sum(w for ts, w in x["msgs"] if ts is not None and t0 <= ts < t1)
                    usd_cw += x["cost"] * part / weight if weight else 0.0
        per.append({"k": a["k"], "seconds": round(s_cw, 1), "usd": round(usd_cw, 4), "episodes": eps})
        tot_s += s_all
        tot_usd += usd_all
        cw_s += s_cw
        cw_usd += usd_cw
        n_eps += eps
    return {"claude_seconds": round(cw_s, 1), "claude_usd": round(cw_usd, 4), "episodes": n_eps,
            "share_of_claude_seconds": round(cw_s / tot_s, 4) if tot_s else None,
            "share_of_claude_usd": round(cw_usd / tot_usd, 4) if tot_usd else None,
            "claude_seconds_total": round(tot_s, 1), "claude_usd_total": round(tot_usd, 4), "per_agent": per}


def _pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return None
    i = min(len(xs) - 1, max(0, int(round(q * (len(xs) - 1)))))
    return round(xs[i], 2)


def integration_flow(arm, agents, merges, wall_s):
    """How work entered main. `changes_per_hour`: moves of main by an agent's work (each successful
    `t2-integrate` in A2/A3, each merge of the runner) per hour of the run. `wait_s`: per piece of work, from the
    agent's first integrate attempt to its work being on main (A2/A3; median and 95th percentile).
    `retries`: integrate attempts that did not put the work on main because main had moved (a conflict, or
    markers still to resolve) and were followed by another attempt."""
    moves, waits, retries = 0, [], 0
    if arm in A2_ARMS:
        for a in agents:
            first = None
            calls = a.get("integrations") or []
            for i, x in enumerate(calls):
                if x["status"] in ("integrated", "nothing"):
                    if x["status"] == "integrated":
                        waits.append(x["t_s"] - (x["t_s"] if first is None else first))
                    if x["status"] == "integrated":
                        moves += 1
                    first = None
                    continue
                if first is None:
                    first = x["t_s"]
                if x["status"] in ("conflict", "markers left") and i + 1 < len(calls):
                    retries += 1
    moves += sum(1 for m in merges or [] if m.get("status") == "merged")
    hours = (wall_s or 0) / 3600
    return {"changes_on_main": moves, "changes_per_hour": round(moves / hours, 2) if hours else None,
            "wait_s_median": _pct(waits, 0.5), "wait_s_p95": _pct(waits, 0.95), "pieces_waited": len(waits),
            "retries": retries}


# ===================================================================================== solo


class SoloRun(Run):
    """One agent does one task alone on a clean clone of the pin; then the task's hidden test and the suite run
    on its result. It is an arm A run with one agent (same preamble, prompt, box, limits, model and token and
    cost accounting as an arm A agent before any merge); with one agent the runner's merge of its branch is
    its own work. The harness slot is the one the task has in the set (`mix`, rotated by `rep`)."""

    def __init__(self, scratch, repo, s, rep, harness, task_dir, results=None, agent_root=None, limits=None, tasks_dir=None,
                 repo_spec=None):
        super().__init__(scratch, "A", repo, s, rep, harness, limits=limits, results=results, agent_root=agent_root,
                         tasks_dir=tasks_dir, repo_spec=repo_spec)
        self.task_dir = Path(task_dir)
        m = json.loads((self.task_dir / "meta.json").read_text())
        self.task_id, self.task_k = m["id"], m["k"]
        self.mix = [self.set_spec["mix"][(self.task_k - 1 + rep - 1) % len(self.set_spec["mix"])]]
        self.set_spec = {**self.set_spec, "integration": []}
        self.expected = self.n_agents = 1
        self.name = f"solo-{repo}-{s}-{self.task_id}-r{rep}-{harness}"
        self.run_dir = scratch / "t2" / "runner" / self.name

    def tasks(self):
        return [self.task_dir]

    def harness_of(self, k):
        return self.mix[0] if self.harness == "real" else self.harness

    def score(self, tree):
        out = super().score(tree)
        m = json.loads((self.task_dir / "meta.json").read_text())
        ok, text, _ = run_cmd_spec(self.spec, self.spec["decide"], tree, self.scratch, m)
        out["hidden_passed"] = ok
        out["hidden_failed"] = [] if ok else failed_functions(text)
        return out

    def execute(self, pairs_truth=None):
        r = super().execute(pairs_truth)
        r.update(solo=True, task=self.task_id, task_k=self.task_k)
        r.setdefault("hidden_passed", False)
        r.setdefault("hidden_failed", [])
        text = self.scrub(json.dumps(r, indent=1, sort_keys=True) + "\n")
        (self.results / f"{self.name}.json").write_text(text)
        return json.loads(text)


def failed_functions(text):
    """The test functions that pytest's short summary lists as failed (`FAILED path::name - why`), or one
    entry for an error outside them (an import that fails at collection)."""
    names = re.findall(r"^(?:FAILED|ERROR) [^:\s]+::(.+?)(?: - .*)?$", text or "", re.M)
    return sorted(set(names)) or ["(collection or other error)"]


def solo_tasks(repo, s, only=None):
    """The task directories of a set, in order; `only` keeps those whose id or number is listed."""
    dirs = task_dirs(repo, s)
    if not only:
        return dirs
    want = {x for item in only for x in item.split(",") if x}
    picked = [d for d in dirs if meta(d)["id"] in want or str(meta(d)["k"]) in want]
    missing = want - {meta(d)["id"] for d in picked} - {str(meta(d)["k"]) for d in picked}
    if missing:
        raise SystemExit(f"solo: no such task in {repo}/{s}: {', '.join(sorted(missing))}")
    return picked


def solo_cost(r):
    return float(r.get("cost_usd_claude_only") or 0.0)


def solo(scratch, repo, s, rep, harness, only=None, parallel=4, max_usd=None, results=None, agent_root=None, limits=None,
         make=None, dirs=None):
    """Runs the solo cells of a set (one task per run), at most `parallel` at once. With `max_usd` no new agent
    starts once the Claude cost so far plus the dearest agent so far, for each agent still running and the next
    one, would pass it. Returns (run files, task ids not started)."""
    make = make or (lambda td: SoloRun(scratch, repo, s, rep, harness, td, results=results, agent_root=agent_root, limits=limits))
    queue = list(dirs if dirs is not None else solo_tasks(repo, s, only))
    done, skipped, running = [], [], {}
    lock = threading.Lock()

    def start(td):
        def go():
            r = make(td).execute()
            with lock:
                done.append(r)
                running.pop(meta(td)["id"], None)
        t = threading.Thread(target=go)
        running[meta(td)["id"]] = t
        t.start()

    while queue or running:
        with lock:
            spent = sum(solo_cost(r) for r in done)
            dearest = max((solo_cost(r) for r in done), default=0.0)
            free = len(running) < max(1, parallel)
            n_run = len(running)
        if queue and free:
            if max_usd is not None and spent + dearest * (n_run + 1) > max_usd:
                skipped.extend(meta(td)["id"] for td in queue)
                queue.clear()
                continue
            start(queue.pop(0))
            continue
        time.sleep(0.2)
    done.sort(key=lambda r: (r.get("task_k") or 0))
    return done, skipped


def solo_table(runs, skipped=()):
    """The summary of solo run files: one row per (task, rep)."""
    lines = ["| task | rep | harness | hidden test | failed functions | suite all / pinned | turns | tokens | Claude USD | wall s |",
             "|---|---|---|---|---|---|---|---|---|---|"]
    for r in sorted(runs, key=lambda r: (r.get("task_k") or 0, r.get("rep") or 0)):
        st = r.get("agent_stats") or [{}]
        turns = sum(int(a.get("turns") or 0) for a in st)
        tokens = int(r.get("tokens_in") or 0) + int(r.get("tokens_out") or 0)
        suite = "/".join("green" if r.get(x) else "-" if r.get(x) is None else "RED" for x in ("suite_green_all", "suite_green_pinned"))
        status = ("pass" if r.get("hidden_passed") else "fail") if r.get("status") == "done" else r.get("status", "?")
        lines.append(f"| {r.get('task')} | {r.get('rep')} | {r.get('harness')} | {status} | {', '.join(r.get('hidden_failed') or []) or '-'} "
                     f"| {suite} | {turns} | {tokens} | {round(solo_cost(r), 4)} | {r.get('wall_s')} |")
    for tid in skipped:
        lines.append(f"| {tid} | - | - | not started (spend guard) | - | - | - | - | - | - |")
    passed = sum(1 for r in runs if r.get("hidden_passed"))
    lines.append("")
    lines.append(f"hidden tests passed: {passed}/{len(runs)}; Claude USD {round(sum(solo_cost(r) for r in runs), 4)}"
                 + (f"; not started: {len(skipped)}" if skipped else ""))
    return "\n".join(lines) + "\n"


# ===================================================================================== main


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for n in ("setup", "generate", "validate", "run", "rehearse", "pilot"):
        p = sub.add_parser(n)
        p.add_argument("--scratch", required=True, type=Path)
        if n in ("validate", "run", "rehearse", "pilot"):
            p.add_argument("--results", type=Path, help="where run files go (default: the kit's results/ or dev/results/)")
        if n in ("run", "rehearse", "pilot"):
            p.add_argument("--agent-root", type=Path, help="where agent directories go (default: a new directory in the "
                           "system temp dir for real runs, <scratch>/agents for the scripted harnesses)")
        if n in ("setup", "generate") and DEV_REPOS:
            p.add_argument("--repo", action="append", choices=list(DEV_REPOS),
                           help="the development set (dev/README.md); without it, the T2 repositories")
        if n in ("validate", "rehearse"):
            p.add_argument("--repo", action="append", choices=list(REPOS) + list(DEV_REPOS))
            p.add_argument("--set", action="append", choices=list(SETS) + list(EXTRA_SETS),
                           help="only these sets (default: every set the repository has)")
            if n == "rehearse":
                p.add_argument("--arms", help="comma-separated arms (default: " + ",".join(REHEARSE_ARMS) + "; A3 only when named)")
        if n == "pilot":
            p.add_argument("--yes-real", action="store_true")
        if n == "run":
            p.add_argument("--arm", required=True, choices=ARMS)
            p.add_argument("--repo", required=True, choices=list(REPOS) + list(DEV_REPOS))
            p.add_argument("--set", required=True, choices=list(SETS) + list(EXTRA_SETS))
            p.add_argument("--rep", required=True, type=int)
            p.add_argument("--harness", required=True, choices=["oracle", "noop", "stub", "real"])
            p.add_argument("--yes-real", action="store_true", help="needed for --harness real (owner's yes and budget)")
            p.add_argument("--dry-run", action="store_true", help="set up the arm and print the agent commands; start no agent")
            p.add_argument("--block", type=Path, help="arm B, development set only: this instruction block instead of block-b.md")
            p.add_argument("--land-test", action="store_true", help="arm BN: land only what passed the suite on the rebased "
                           "change (connect --land-test t2-test); the run name ends in -gate")
    p = sub.add_parser("solo", help="one agent, one task, alone on the pin; its hidden test and the suite after")
    p.add_argument("--scratch", required=True, type=Path)
    p.add_argument("--repo", required=True, choices=list(REPOS) + list(DEV_REPOS))
    p.add_argument("--set", required=True, choices=list(SETS) + list(EXTRA_SETS))
    p.add_argument("--rep", required=True, type=int)
    p.add_argument("--tasks", action="append", help="task ids or numbers (comma-separated, repeatable); default: all")
    p.add_argument("--harness", required=True, choices=["oracle", "noop", "real"])
    p.add_argument("--parallel", type=int, default=4, help="solo runs at once, each in its own clone (default 4)")
    p.add_argument("--max-usd", type=float, help="start no new agent once the Claude cost so far plus the dearest agent "
                   "so far (for each running agent and the next) would pass this")
    p.add_argument("--dry-run", action="store_true", help="set up each run and print the agent command; start no agent")
    p.add_argument("--yes-real", action="store_true", help="needed for --harness real (owner's yes and a budget)")
    p.add_argument("--results", type=Path, help="where run files and solo-summary.txt go (default: results/solo)")
    p.add_argument("--agent-root", type=Path)
    p = sub.add_parser("collect")
    p.add_argument("--results", type=Path, default=RESULTS)
    p.add_argument("--out", type=Path)
    p.add_argument("--summary", type=Path)
    p.add_argument("--only", action="append")
    p = sub.add_parser("budget")
    p.add_argument("--pilot", required=True, type=Path)
    p = sub.add_parser("health", help="hidden and integration tests at every state of main of a finished run")
    p.add_argument("--scratch", required=True, type=Path)
    p.add_argument("--run", required=True, type=Path, help="the runner's directory of the run (it holds final/)")
    p.add_argument("--repo", required=True, choices=list(REPOS) + list(DEV_REPOS))
    p.add_argument("--set", required=True, choices=list(SETS) + list(EXTRA_SETS))
    p = sub.add_parser("recount", help="tokens of old runs from their transcripts: old count against new")
    p.add_argument("--runs", required=True, type=Path, help="a directory of run directories (agent-N.jsonl)")
    p.add_argument("--results", action="append", type=Path, help="run files to show the recorded total from")
    p.add_argument("--metrics", action="store_true", help="recount the warning and land metrics of arm B and BN run files "
                   "(in --results) from the transcripts in --runs, instead of the tokens")
    p.add_argument("--out", type=Path, help="with --metrics: write the recounted run files here")
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
    if a.cmd == "health":
        select_kit([a.repo])
        return main_health(a.scratch.resolve(), a.run.resolve(), a.repo, a.set)
    if a.cmd == "recount":
        if a.metrics:
            if not a.results:
                raise SystemExit("recount --metrics needs --results DIR (the run files)")
            recount_metrics(a.runs, a.results, a.out)
            return
        recount(a.runs, a.results or ())
        return
    if a.cmd == "solo":
        select_kit([a.repo])
        scratch = a.scratch.resolve()
        results = a.results or RESULTS / "solo"
        if a.dry_run:
            for td in solo_tasks(a.repo, a.set, a.tasks):
                for line in SoloRun(scratch, a.repo, a.set, a.rep, a.harness, td, results=results, agent_root=a.agent_root).dry_run():
                    print(line)
            return
        if a.harness == "real" and not a.yes_real:
            raise SystemExit("--harness real starts paid or quota-using agents: it needs --yes-real (the owner's yes and a budget)")
        runs, skipped = solo(scratch, a.repo, a.set, a.rep, a.harness, a.tasks, a.parallel, a.max_usd, results, a.agent_root)
        table = solo_table(runs, skipped)
        Path(results).mkdir(parents=True, exist_ok=True)
        (Path(results) / f"solo-summary-{a.set}-r{a.rep}-{a.harness}.txt").write_text(table)
        print(table, end="")
        sys.exit(0 if all(r["status"] == "done" for r in runs) else 3)
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
        sys.exit(0 if validate(scratch, a.repo or list(REPOS), a.results, a.set) else 1)
    kw = {"results": getattr(a, "results", None), "agent_root": getattr(a, "agent_root", None)}
    if a.cmd == "run":
        if a.dry_run:
            for line in Run(scratch, a.arm, a.repo, a.set, a.rep, a.harness, block_file=a.block, land_test=a.land_test, **kw).dry_run():
                print(line)
            return
        if a.harness == "real" and not a.yes_real:
            raise SystemExit("--harness real starts paid or quota-using agents: it needs --yes-real (the owner's yes and a budget)")
        truth = pair_table(scratch, a.repo, a.set) if a.arm in B_ARMS else None
        if a.land_test and a.arm != "BN":
            raise SystemExit("--land-test is for arm BN")
        r = Run(scratch, a.arm, a.repo, a.set, a.rep, a.harness, block_file=a.block, land_test=a.land_test, **kw).execute(truth)
        print(json.dumps({k: r.get(k) for k in ("run", "status", "stop_reason", "tasks_on_main", "suite_green_all", "suite_green_pinned",
                                                 "merge_conflicts", "warned", "reacted", "interventions", "wall_s")}
                         | ({"integration": f"{r['integration_passed']}/{r['integration_total']}"} if r.get("integration_total") else {})))
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
            r = Run(scratch, arm, "requests", s, rep, "real", **kw).execute(pair_table(scratch, "requests", s) if arm == "B" else None)
            bad += r["status"] != "done"
            print(json.dumps({k: r.get(k) for k in ("run", "status", "stop_reason", "tasks_on_main", "interventions", "wall_s")}), flush=True)
        sys.exit(1 if bad else 0)
    if a.cmd == "rehearse":
        bad = 0
        for repo in a.repo or list(REPOS):
            sets = [x for x in (*SETS, *sets_of(repo)) if a.set is None or x in a.set]
            truth = {s: pair_table(scratch, repo, s) for s in sets}
            for harness in ("oracle", "noop"):
                for s in sets:
                    for arm in (a.arms.split(",") if a.arms else REHEARSE_ARMS):
                        if arm not in ARMS:
                            raise SystemExit(f"rehearse: no arm {arm}")
                        r = Run(scratch, arm, repo, s, 1, harness, **kw).execute(truth[s] if arm in B_ARMS else None)
                        bad += r["status"] != "done"
                        print(json.dumps({k: r.get(k) for k in ("run", "status", "stop_reason", "tasks_on_main", "suite_green_all",
                                                                 "suite_green_pinned", "merge_conflicts", "warned", "reacted",
                                                                 "interventions", "false_alarms", "wall_s")}
                                         | ({"integration": f"{r['integration_passed']}/{r['integration_total']}",
                                             "setup_s": r.get("setup_s")} if r.get("integration_total") else {})), flush=True)
        sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
