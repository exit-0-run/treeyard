#!/usr/bin/env python3
"""Live demo v2: 8 real Claude Code agents (claude -p) fix 8 real bugs from more-itertools history at once.

    sh setup.sh                # once: clone more-itertools, make a venv with pytest
    python3 demo.py git        # arm 1: git worktrees + a serial merge queue (land.py)
    python3 demo.py treeyard   # arm 2: TreeYard workspaces + daemon + optimistic landing

Env: DEMO_DIR (work dir, default ~/.cache/treeyard-live-agents), TREEYARD_BIN, DEMO_MODEL (sonnet).
Needs the `claude` CLI (Claude Code); each run starts 8 `claude -p` sessions.

Same model, tasks and task text in both arms; only the way work reaches master differs.
Results: results/<arm>.json and agents/<arm>-<n>.json (the claude -p JSON output).
"""
import json, os, shutil, subprocess, sys, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
D = os.environ.get("DEMO_DIR", os.path.expanduser("~/.cache/treeyard-live-agents"))  # made by setup.sh
SRC = os.path.join(D, "src")
PY = os.path.join(D, "venv", "bin", "python")
TY = os.environ.get("TREEYARD_BIN") or shutil.which("treeyard") or os.path.join(HERE, "..", "..", "target", "release", "treeyard")
BASE = "2fe1b2e"
MODEL = os.environ.get("DEMO_MODEL", "sonnet")
URL = "http://127.0.0.1:7431"

TASKS = [
    ("numeric_range_consistent", "numeric_range: len(), in and index() disagree with the items",
     "numeric_range produces its items by repeated multiplication, but derives len(), __contains__ and index() "
     "from division by the step. With floats the two disagree, so a range can report a length that does not match "
     "what it yields, and can deny membership to its own items (try numeric_range(0.0, 1.0, 0.1))."),
    ("numeric_range_reversed", "numeric_range: reversed() yields different values",
     "reversed(numeric_range(start, stop, step)) can yield values that differ from the forward iteration when the "
     "step is a float: reversed(numeric_range(0.0, 1.0, 0.1)) is not the forward list reversed. Reverse iteration "
     "must yield exactly the forward items in reverse order."),
    ("seekable_zero_cache_peek", "seekable(maxlen=0): peek() and bool() lose items",
     "With seekable(iterable, maxlen=0), calling peek() or bool() consumes the next item, and because a zero-size "
     "cache cannot keep it, the item is lost: the next next() skips it. peek() and bool() must not lose items when "
     "maxlen is 0."),
    ("bucket_no_phantom_key", "bucket: a lookup invents a key",
     "In bucket, a membership test or lookup for a key that has no items (for example 'c' in b) leaves a "
     "permanent empty key behind, so iterating the bucket afterwards lists keys that never had items, and "
     "'c' in b and 'c' in list(b) disagree. A failed lookup must not change the bucket."),
    ("zip_broadcast_iter_once", "zip_broadcast: iter() is called twice",
     "zip_broadcast calls iter() twice on every non-scalar argument: once to check whether it is iterable and "
     "again to iterate it. An object whose __iter__ is expensive or has side effects runs it twice. Call iter() "
     "once per argument."),
    ("constrained_batches_max_count", "constrained_batches: max_count <= 0 is accepted",
     "constrained_batches accepts max_count=0 or a negative max_count and then misbehaves. It must raise "
     "ValueError for max_count <= 0, as it does for max_size <= 0, and the docstring must say so."),
    ("split_maxsplit_zero_empty", "split_before/after/when: maxsplit=0 yields [[]] for empty input",
     "split_before, split_after and split_when yield [[]] for an empty iterable when maxsplit is 0, while every "
     "other maxsplit yields []. With maxsplit=0 an empty iterable must yield nothing. Leave split_at as it is."),
    ("value_chain_keeps_type_error", "value_chain: a TypeError from inside an argument is swallowed",
     "value_chain swallows a TypeError raised while iterating an argument and then emits the partly consumed "
     "object itself: list(value_chain(map(len, ['ab', 'cde', 5]))) gives [2, 3, <map object>]. A TypeError "
     "raised during iteration must propagate; arguments that cannot be iterated at all are still emitted as is."),
]

COMMON = """You are one of 8 coding agents that fix bugs in the more-itertools library at the same time, in the
same repository. Your bug report: {title}.
{desc}
Fix it in more_itertools/more.py with the smallest clean change, and add regression tests in
tests/test_more.py next to the existing tests of that function. Follow the existing style. Run the tests
with `{py} -m pytest -q -x tests`. Do not change anything else.
"""

GIT_FINISH = """When the tests pass, commit (`git add -A && git commit -m "Fix {name}"`) and run `python3 ../land.py`.
land.py rebases your branch on master, runs the tests and merges. If it prints CONFLICT, resolve the
conflict markers in the listed files (keep both sides' additions), `git add` them, run
`git rebase --continue`, run the tests, then run `python3 ../land.py` again. Repeat until it prints
LANDED. You are done when it prints LANDED.
"""

TY_FINISH = """This directory is a TreeYard workspace: every file you save is sent to the TreeYard server
automatically, and other agents' work is visible to the server. If the file `.treeyard/overlaps.json`
exists, it lists other agents' open changes that touch the same code as you: read it before you land.
When the tests pass, run `treeyard workspace land {ws}`. If the reply has "status": "conflict", the
conflicted files in this directory now hold conflict markers plus the work other agents already landed:
resolve the markers (keep both sides' intent), run the tests, and run `treeyard workspace land {ws}`
again. After a land the reply waits for the server's tests and has a "verdict": if it is "reverted" or
"rejected", read its "output", fix your work here, run the tests, and run `treeyard workspace land {ws}`
again (the workspace carries on by itself). You are done when the land status is "landed" and the
verdict is "passed".
"""

ALLOWED = ["Read", "Edit", "Write", "Glob", "Grep", "Bash(git:*)", "Bash(python3:*)", f"Bash({PY}:*)",
           "Bash(treeyard:*)", "Bash(cat:*)", "Bash(ls:*)", "Bash(grep:*)", "Bash(sed:*)", "Bash(head:*)",
           "Bash(tail:*)", "Bash(wc:*)", "Bash(diff:*)"]


def sh(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw)


def run_agents(arm, dirs, prompts, env):
    os.makedirs(os.path.join(D, "agents"), exist_ok=True)
    procs = []
    t0 = time.time()
    for i, (cwd, prompt) in enumerate(zip(dirs, prompts)):
        out = open(os.path.join(D, "agents", f"{arm}-{i}.json"), "w")
        err = open(os.path.join(D, "agents", f"{arm}-{i}.err"), "w")
        cmd = ["claude", "-p", prompt, "--output-format", "json", "--model", MODEL, "--max-turns", "80",
               "--permission-mode", "acceptEdits", "--allowedTools", *ALLOWED,
               "--mcp-config", '{"mcpServers":{}}', "--strict-mcp-config"]
        procs.append((i, subprocess.Popen(cmd, cwd=cwd, env=env[i], stdout=out, stderr=err), time.time()))
    ends = {}
    while len(ends) < len(procs):
        for i, p, _ in procs:
            if i not in ends and p.poll() is not None:
                ends[i] = time.time() - t0
                print(f"[{arm}] agent {i} ({TASKS[i][0]}) exited {p.returncode} after {ends[i]:.0f} s", flush=True)
            elif i not in ends and time.time() - t0 > 2400:
                p.kill()
        time.sleep(2)
    agents = []
    for i, p, _ in procs:
        try:
            r = json.load(open(os.path.join(D, "agents", f"{arm}-{i}.json")))
        except Exception:
            r = {}
        agents.append({"task": TASKS[i][0], "exit": p.returncode, "wall_s": round(ends[i], 1),
                       "turns": r.get("num_turns"), "cost_usd": r.get("total_cost_usd"),
                       "duration_ms": r.get("duration_ms"), "is_error": r.get("is_error"),
                       "last": (r.get("result") or "")[-400:]})
    return agents, round(time.time() - t0, 1)


def final_tests(repo):
    r = subprocess.run([PY, "-m", "pytest", "-q", "tests"], cwd=repo, capture_output=True, text=True)
    p = subprocess.run([PY, os.path.join(HERE, "probes.py"), repo], capture_output=True, text=True)
    try:
        probes = json.loads(p.stdout)
    except ValueError:
        probes = {"error": p.stderr[-500:]}
    return {"pytest_exit": r.returncode, "pytest_tail": r.stdout.strip().splitlines()[-1:],
            "hidden_checks_passed": sum(v is True for v in probes.values()), "hidden_checks": probes}


def arm_git():
    G = os.path.join(D, "g")
    shutil.rmtree(G, ignore_errors=True)
    os.makedirs(G)
    shutil.copy(os.path.join(HERE, "land.py"), G)
    t0 = time.time()
    sh(["git", "clone", "-q", SRC, os.path.join(G, "repo")])
    sh(["git", "checkout", "-q", "-B", "master", BASE], cwd=os.path.join(G, "repo"))
    dirs = []
    for i, (name, _, _) in enumerate(TASKS):
        wt = os.path.join(G, f"wt-{i}")
        sh(["git", "worktree", "add", "-q", "-b", f"task-{name}", wt, "master"], cwd=os.path.join(G, "repo"))
        dirs.append(wt)
    setup_s = round(time.time() - t0, 2)
    base_env = {**os.environ, "DEMO_PY": PY}
    prompts = [COMMON.format(title=t, desc=d, py=PY) + GIT_FINISH.format(name=n) for n, t, d in TASKS]
    agents, wall = run_agents("git", dirs, prompts, [base_env] * len(TASKS))
    log = [json.loads(l) for l in open(os.path.join(G, "land.log"))] if os.path.exists(os.path.join(G, "land.log")) else []
    t_landed = [r["t"] for r in log if r["result"] == "landed"]
    return {"arm": "git worktrees + merge queue", "setup_s": setup_s, "agents_wall_s": wall, "agents": agents,
            "land_attempts": len(log), "landed": len(t_landed),
            "conflicts": [r for r in log if r["result"] == "conflict"],
            "tests_failed_in_queue": sum(r["result"] == "tests_failed" for r in log),
            "final": final_tests(os.path.join(G, "repo"))}


def call(op, params):
    req = urllib.request.Request(f"{URL}/api/{op}", data=json.dumps(params).encode(),
                                 headers={"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=30))


def arm_treeyard():
    T = os.path.join(D, "t")
    shutil.rmtree(T, ignore_errors=True)
    os.makedirs(T)
    data, home = os.path.join(T, "data"), os.path.join(T, "node")
    t0 = time.time()
    tmp = os.path.join(T, "src-at-base")
    sh(["git", "clone", "-q", SRC, tmp])
    sh(["git", "checkout", "-q", "-B", "master", BASE], cwd=tmp)
    sh([TY, "init", "--data", data, "--from", tmp, "--branch", "master"])
    with open(os.path.join(data, "treeyard.toml"), "w") as f:
        f.write(f'[landing]\nmode = "optimistic"\ntest = "{PY} -m pytest -q -x tests"\ntimeout_s = 300\n')
    env = {**os.environ, "TREEYARD_URL": URL, "TREEYARD_HOME": home, "PATH": os.path.dirname(TY) + ":" + os.environ["PATH"]}
    serve = subprocess.Popen([TY, "serve", "--data", data, "--addr", URL.split("//")[1]],
                             stdout=open(os.path.join(T, "serve.log"), "w"), stderr=subprocess.STDOUT)
    for _ in range(100):
        try:
            call("status", {})
            break
        except Exception:
            time.sleep(0.1)
    daemon = subprocess.Popen([TY, "daemon"], env=env, stdout=open(os.path.join(T, "daemon.log"), "w"),
                              stderr=subprocess.STDOUT)
    dirs, envs, prompts = [], [], []
    for i, (name, title, desc) in enumerate(TASKS):
        ws = f"agent-{i}"
        out = json.loads(sh([TY, "workspace", "create", ws, "--agent", ws, "--intent", title], env=env).stdout)
        w = json.loads(open(os.path.join(home, "meta", f"{ws}.json")).read())
        dirs.append(w["dir"])
        envs.append({**env, "TREEYARD_WORKSPACE": ws, "TREEYARD_AGENT": ws})
        prompts.append(COMMON.format(title=title, desc=desc, py=PY) + TY_FINISH.format(ws=ws, change=w["change"]))
    setup_s = round(time.time() - t0, 2)
    agents, wall = run_agents("treeyard", dirs, prompts, envs)
    # Optimistic landing tests main after the lands: wait for the worker to settle.
    t_wait = time.time()
    while time.time() - t_wait < 600:
        ev = call("events", {"limit": 5000})["events"]
        kinds = [e["kind"] for e in ev]
        last_land = max([i for i, k in enumerate(kinds) if k == "landed"], default=-1)
        if any(k in ("test_passed", "reverted", "test_failed") for k in kinds[last_land + 1:]):
            break
        time.sleep(3)
    settle_s = round(time.time() - t_wait, 1)
    ev = call("events", {"limit": 5000})["events"]
    final_dir = os.path.join(T, "final")
    sh(["git", "clone", "-q", f"{URL}/git", final_dir])
    result = {"arm": "TreeYard workspaces + optimistic landing", "setup_s": setup_s, "agents_wall_s": wall,
              "settle_after_agents_s": settle_s, "agents": agents,
              "events": {k: sum(e["kind"] == k for e in ev) for k in sorted({e["kind"] for e in ev})},
              "collision_pairs_warned": len({tuple(sorted(e.get("changes", []))) for e in ev if e["kind"] == "collision"}),
              "final": final_tests(final_dir)}
    json.dump(ev, open(os.path.join(T, "events.json"), "w"), indent=1)
    daemon.terminate()
    serve.terminate()
    return result


def main():
    arm = sys.argv[1]
    os.makedirs(os.path.join(D, "results"), exist_ok=True)
    r = arm_git() if arm == "git" else arm_treeyard()
    r["model"] = MODEL
    json.dump(r, open(os.path.join(D, "results", f"{arm}.json"), "w"), indent=2)
    print(json.dumps({k: v for k, v in r.items() if k != "agents"}, indent=2))


main()
