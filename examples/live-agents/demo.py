#!/usr/bin/env python3
"""Live demo: 8 real Claude Code agents (claude -p) work on more-itertools at once (DEMO_SET=bugs: 8 real bugs;
DEMO_SET=hard: 4 pairs of feature requests, two agents per function).

    sh setup.sh                # once: clone more-itertools, make a venv with pytest
    python3 demo.py git        # arm 1: git worktrees + a serial merge queue (land.py)
    python3 demo.py treeyard   # arm 2: TreeYard workspaces + daemon + optimistic landing

Env: DEMO_DIR (work dir, default ~/.cache/treeyard-live-agents), TREEYARD_BIN, DEMO_MODEL (sonnet),
DEMO_N (use the first N tasks), DEMO_POOL (at most this many claude -p at once; default all),
DEMO_LANDING (TreeYard landing mode: optimistic, the default, or queue: batches of 8 tested before they land),
DEMO_LAND_AND_GO=1 (TreeYard agents do not wait for the tests; a rejected or reverted change resumes its agent).
Needs the `claude` CLI (Claude Code); each run starts 8 `claude -p` sessions.

Same model, tasks and task text in both arms; only the way work reaches master differs.
Results: results/<set>-<arm>.json and agents/<arm>-<n>.json (the claude -p JSON output).
"""
import json, os, shutil, subprocess, sys, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
D = os.environ.get("DEMO_DIR", os.path.expanduser("~/.cache/treeyard-live-agents"))  # made by setup.sh
SRC = os.path.join(D, "src")
PY = os.path.join(D, "venv", "bin", "python")
TY = os.environ.get("TREEYARD_BIN") or shutil.which("treeyard") or os.path.join(HERE, "..", "..", "target", "release", "treeyard")
SET = os.environ.get("DEMO_SET", "bugs")  # bugs: 8 real bugs; hard: 4 pairs on one function; wide: 50 tasks
BASE = {"bugs": "2fe1b2e", "hard": "4aeb433", "wide": "4aeb433"}[SET]
PROBES = {"bugs": "probes.py", "hard": "probes_hard.py", "wide": "probes_wide.py"}[SET]
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

TASKS_HARD = [
    ("windowed_strict", "windowed: add strict",
     "Add a keyword-only parameter strict=False to windowed(seq, n, fillvalue=None, step=1). With strict=True, "
     "raise ValueError instead of yielding a window that would need fillvalue."),
    ("windowed_skip_partial", "windowed: add skip_partial",
     "Add a keyword-only parameter skip_partial=False to windowed(seq, n, fillvalue=None, step=1). With "
     "skip_partial=True, do not yield a last window that would need fillvalue (and do not raise)."),
    ("first_pred", "first: add pred",
     "Add a keyword-only parameter pred=None to first(iterable, default=...). When pred is given, return the "
     "first item for which pred(item) is true; if there is none, return default or raise as today."),
    ("first_too_short", "first: add too_short",
     "Add a keyword-only parameter too_short=None to first(iterable, default=...), like one(): when no item is "
     "found and there is no default, raise too_short (an exception class or instance) instead of ValueError."),
    ("split_at_keep_empty", "split_at: add keep_empty",
     "Add a keyword-only parameter keep_empty=True to split_at(iterable, pred, maxsplit=-1, keep_separator=False). "
     "With keep_empty=False, do not yield empty groups: split_at('a,,b', lambda c: c == ',', keep_empty=False) "
     "gives [['a'], ['b']]."),
    ("split_at_separator_value", "split_at: accept a separator value",
     "Let split_at(iterable, pred, ...) take a separator value instead of a predicate: when pred is not callable, "
     "split where item == pred. split_at('a,b', ',') gives [['a'], ['b']]."),
    ("padded_strict", "padded: add strict",
     "Add a keyword-only parameter strict=False to padded(iterable, fillvalue=None, n=None, next_multiple=False). "
     "With strict=True and n given (not next_multiple), raise ValueError if the iterable has more than n items."),
    ("padded_left", "padded: add left",
     "Add a keyword-only parameter left=False to padded(iterable, fillvalue=None, n=None, next_multiple=False). "
     "With left=True, put the fill values before the items: padded([1, 2], 0, n=4, left=True) gives [0, 0, 1, 2]."),
]
if SET == "hard":
    TASKS = TASKS_HARD
if SET == "wide":  # 10 pairs on one function each and 30 single tasks (tasks_wide.py)
    sys.path.insert(0, HERE)
    from tasks_wide import TASKS
TASKS = TASKS[:int(os.environ.get("DEMO_N", len(TASKS)))]
POOL = int(os.environ.get("DEMO_POOL", len(TASKS)))
LANDING = os.environ.get("DEMO_LANDING", "optimistic")
LIMIT_S = 2400 if len(TASKS) <= 16 else 5400  # one agent's wall-clock limit
# With many agents on few cores, each agent runs only the tests of its function; the merge queue and
# TreeYard's landing still run the whole suite.
WHERE = ("more_itertools/more.py (and its type stub in more_itertools/more.pyi when the\nsignature changes)",
         "tests/test_more.py") if SET != "wide" else (
         "the module that defines the function, more_itertools/more.py or recipes.py (and its type\nstub, more.pyi or "
         "recipes.pyi, when the signature changes)", "tests/test_more.py or tests/test_recipes.py")
ONLY = (" -k NAME` (NAME: the function you changed without underscores, for example mapif for map_if, since the\n"
        "test classes are named like MapIfTests; landing runs the whole suite)") if len(TASKS) > 16 else "`"

COMMON = """You are one of {n} coding agents that work on the more-itertools library at the same time, in the
same repository. Your task: {title}.
{desc}
Make the change in {where[0]} with the smallest clean change, update the docstring, and add tests in
{where[1]} next to the existing tests of that function. Follow the existing style. Run the tests
with `{py} -m pytest -q -x tests{only}. Do not change anything else.
"""

GIT_FINISH = """When the tests pass, commit (`git add -A && git commit -m "Fix {name}"`) and run `python3 ../land.py`.
land.py rebases your branch on master, runs the tests and merges. If it prints CONFLICT, resolve the
conflict markers in the listed files (keep both sides' additions), `git add` them, run
`git rebase --continue`, run the tests, then run `python3 ../land.py` again. Repeat until it prints
LANDED. You are done when it prints LANDED.
"""

TY_FINISH = """This directory is a TreeYard workspace: every file you save is sent to the TreeYard server
automatically, and other agents' work is visible to the server. If the file `.treeyard/overlaps.json`
exists, it lists other agents' open changes that touch the same code as you, and the file
`.treeyard/collisions.json` (rewritten on every save) lists real conflicts with them and the reason.
`treeyard workspace status {ws}` asks the server again. If another agent's change conflicts with yours,
read it with `treeyard call diff '{{"change": "<their change id>"}}'` and make your edit fit with theirs
(keep their change working too); check `treeyard workspace status {ws}` again before you land.
When the tests pass, run `treeyard workspace land {ws}`. If the reply has "status": "conflict", the
conflicted files in this directory now hold conflict markers plus the work other agents already landed:
resolve the markers (keep both sides' intent), run the tests, and run `treeyard workspace land {ws}`
again. After a land the reply waits for the server's tests and has a "verdict": if it is "reverted",
"rejected" or "broke_main", read its "output", fix your work here, run the tests, and run `treeyard workspace
land {ws}` again (the workspace carries on by itself). You are done when the land status is "landed" and the
verdict is "passed", or "main_red" (your change is on main, and main's tests fail because of another
agent's change, "red_change", which that agent is fixing). If the verdict is "open", run
`treeyard workspace land {ws}` again: it shows a conflict found while your change waited to land.
"""

# Land and go (DEMO_LAND_AND_GO=1, TreeYard arm): the agent lands without waiting for the server's tests;
# when they reject or revert the change later, the runner resumes that agent's session with the output
# (like a CI notification). The run ends when every change has passed.
LAND_AND_GO = os.environ.get("DEMO_LAND_AND_GO") == "1"
TY_GO = """This directory is a TreeYard workspace: every file you save is sent to the TreeYard server
automatically, and other agents' work is visible to the server. If the file `.treeyard/collisions.json`
exists, it lists real conflicts with other agents' open changes and the reason; `treeyard workspace status
{ws}` asks the server again. If another agent's change conflicts with yours, read it with
`treeyard call diff '{{"change": "<their change id>"}}'` and make your edit fit with theirs.
When the tests pass, run `treeyard workspace land {ws} --wait-s 0`. If the reply's land status is
"conflict", the conflicted files in this directory now hold conflict markers plus the work other agents
already landed: resolve the markers (keep both sides' intent), run the tests, and land again. You are done
when the land status is "landed", "queued" or "already_on_main": the server runs the full test suite on its
own, and if your change fails it you will be told, with the output.
"""
FIXUP = """The TreeYard server's full test suite did not pass your change: the verdict is "{verdict}".
Test output (the end):
{output}
Fix your work in this directory (the workspace carries on by itself), run the tests, and run
`treeyard workspace land {ws} --wait-s 0` again; handle a "conflict" as before. You are done when the
land status is "landed", "queued" or "already_on_main".
"""
NOT_LANDED = """Your change is not landed yet. Finish it, run the tests, and run `treeyard workspace land {ws}
--wait-s 0`; handle a "conflict" as before. You are done when the land status is "landed", "queued" or
"already_on_main".
"""

ALLOWED = ["Read", "Edit", "Write", "Glob", "Grep", "Bash(git:*)", "Bash(python3:*)", f"Bash({PY}:*)",
           "Bash(treeyard:*)", "Bash(cat:*)", "Bash(ls:*)", "Bash(grep:*)", "Bash(sed:*)", "Bash(head:*)",
           "Bash(tail:*)", "Bash(wc:*)", "Bash(diff:*)"]


def sh(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw)


def run_agents(arm, dirs, prompts, env, only=None, resume=None, tag=""):
    """Runs agents `only` (default all), POOL at a time; `resume` maps an agent to the claude session to
    continue. Output: agents/<set>-<arm><tag>-<n>.json."""
    os.makedirs(os.path.join(D, "agents"), exist_ok=True)
    procs = []
    t0 = time.time()
    only = list(range(len(dirs))) if only is None else only
    resume = resume or {}

    def start(i):
        out = open(os.path.join(D, "agents", f"{SET}-{arm}{tag}-{i}.json"), "w")
        err = open(os.path.join(D, "agents", f"{SET}-{arm}{tag}-{i}.err"), "w")
        cmd = ["claude", "-p", prompts[i], "--output-format", "json", "--model", MODEL, "--max-turns", "80",
               "--permission-mode", "acceptEdits", "--allowedTools", *ALLOWED,
               "--mcp-config", '{"mcpServers":{}}', "--strict-mcp-config"]
        if i in resume:
            cmd += ["--resume", resume[i]]
        # A land can wait minutes in a long queue: let the agent's shell wait for it (both arms).
        e = {**env[i], "BASH_DEFAULT_TIMEOUT_MS": "1800000", "BASH_MAX_TIMEOUT_MS": "3600000"}
        procs.append((i, subprocess.Popen(cmd, cwd=dirs[i], env=e, stdin=subprocess.DEVNULL, stdout=out, stderr=err), time.time()))

    ends, waiting = {}, list(only)
    while len(ends) < len(only):
        while waiting and len(procs) - len(ends) < POOL:  # a pool: start the next agent when one exits
            start(waiting.pop(0))
        for i, p, started in procs:
            if i not in ends and p.poll() is not None:
                ends[i] = time.time() - t0
                print(f"[{arm}] agent {i} ({TASKS[i][0]}) exited {p.returncode} after {ends[i]:.0f} s", flush=True)
            elif i not in ends and time.time() - started > LIMIT_S:
                p.kill()
        time.sleep(2)
    procs.sort()
    agents = []
    for i, p, _ in procs:
        try:
            r = json.load(open(os.path.join(D, "agents", f"{SET}-{arm}{tag}-{i}.json")))
        except Exception:
            r = {}
        agents.append({"task": TASKS[i][0], "index": i, "session": r.get("session_id"), "exit": p.returncode,
                       "wall_s": round(ends[i], 1),
                       "turns": r.get("num_turns"), "cost_usd": r.get("total_cost_usd"),
                       "duration_ms": r.get("duration_ms"), "is_error": r.get("is_error"),
                       "last": (r.get("result") or "")[-400:]})
    return agents, round(time.time() - t0, 1)


def cpu_s(pid):
    """CPU seconds (user + system) a process has used so far, from `ps` (Linux and macOS)."""
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    if not out:
        return None
    days, _, hms = out.rpartition("-")
    parts = [float(x) for x in hms.split(":")]
    while len(parts) < 3:
        parts.insert(0, 0.0)
    return round((int(days) if days else 0) * 86400 + parts[0] * 3600 + parts[1] * 60 + parts[2], 1)


def final_tests(repo):
    r = subprocess.run([PY, "-m", "pytest", "-q", "tests"], cwd=repo, capture_output=True, text=True)
    p = subprocess.run([PY, os.path.join(HERE, PROBES), repo], capture_output=True, text=True)
    try:
        probes = json.loads(p.stdout)
    except ValueError:
        probes = {"error": p.stderr[-500:]}
    if SET == "bugs":  # one check per task: keep the checks of the tasks that ran (DEMO_N)
        probes = {k: v for k, v in probes.items() if k in {t[0] for t in TASKS} or k == "error"}
    return {"pytest_exit": r.returncode, "pytest_tail": r.stdout.strip().splitlines()[-1:],
            "pytest_failed": [l for l in r.stdout.splitlines() if l.startswith(("FAILED", "ERROR"))][:20],
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
    prompts = [COMMON.format(n=len(TASKS), title=t, desc=d, py=PY, only=ONLY, where=WHERE) + GIT_FINISH.format(name=n) for n, t, d in TASKS]
    agents, wall = run_agents("git", dirs, prompts, [base_env] * len(TASKS))
    log = [json.loads(l) for l in open(os.path.join(G, "land.log"))] if os.path.exists(os.path.join(G, "land.log")) else []
    json.dump(log, open(os.path.join(D, "results", f"{SET}-git-land-log.json"), "w"), indent=1)
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
        f.write(f'[landing]\nmode = "{LANDING}"\ntest = "{PY} -m pytest -q -x tests"\nbatch = 8\ntimeout_s = 300\n')
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
        finish = TY_GO.format(ws=ws) if LAND_AND_GO else TY_FINISH.format(ws=ws, change=w["change"])
        prompts.append(COMMON.format(n=len(TASKS), title=title, desc=desc, py=PY, only=ONLY, where=WHERE) + finish)
    setup_s = round(time.time() - t0, 2)
    t_agents = time.time()
    agents, wall = run_agents("treeyard", dirs, prompts, envs)
    rounds = []
    if LAND_AND_GO:
        def verdicts():
            out = {}
            for i in range(len(TASKS)):
                ch = json.loads(open(os.path.join(home, "meta", f"agent-{i}.json")).read())["change"]
                out[i] = call("land_status", {"change": ch})
            return out

        sessions = {a["index"]: a["session"] for a in agents if a.get("session")}
        for rnd in range(1, 4):
            t_s = time.time()
            while time.time() - t_s < 900:  # the server's tests of every landed or queued change
                v = verdicts()
                if not any(x["verdict"] in ("pending", "queued") for x in v.values()):
                    break
                time.sleep(3)
            bad = [i for i, x in v.items() if x["verdict"] not in ("passed", "main_red", "not_tested")]
            rounds.append({"round": rnd, "settle_s": round(time.time() - t_s, 1),
                           "fixups": {TASKS[i][0]: v[i]["verdict"] for i in bad}})
            if not bad:
                break
            fix = list(prompts)
            for i in bad:
                if v[i]["verdict"] == "open":
                    fix[i] = NOT_LANDED.format(ws=f"agent-{i}")
                else:
                    fix[i] = FIXUP.format(verdict=v[i]["verdict"], output=str(v[i].get("output") or "")[-3000:],
                                          ws=f"agent-{i}")
            more, _ = run_agents("treeyard", dirs, fix, envs, only=bad,
                                 resume={i: sessions[i] for i in bad if i in sessions}, tag=f"-fix{rnd}")
            for a in more:
                a["round"] = rnd
                sessions[a["index"]] = a.get("session") or sessions.get(a["index"])
            agents += more
        wall = round(time.time() - t_agents, 1)  # until every change passed: comparable with the git arm
    # Optimistic landing tests main after the lands: wait for the worker to settle.
    t_wait = time.time()
    while time.time() - t_wait < 600:
        if LANDING == "queue":  # changes land only after their tests: done when the queue is empty
            if not call("queue", {})["queue"]:
                break
            time.sleep(3)
            continue
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
    result = {"arm": f"TreeYard workspaces + {LANDING} landing" + (", land and go" if LAND_AND_GO else ""),
              "setup_s": setup_s, "agents_wall_s": wall, "fixup_rounds": rounds,
              "settle_after_agents_s": settle_s, "agents": agents,
              "events": {k: sum(e["kind"] == k for e in ev) for k in sorted({e["kind"] for e in ev})},
              "collision_pairs_warned": len({tuple(sorted(e.get("changes", []))) for e in ev if e["kind"] == "collision"}),
              "final": final_tests(final_dir)}
    result["cpu_s"] = {"serve": cpu_s(serve.pid), "daemon": cpu_s(daemon.pid)}
    runs = [e for e in ev if e["kind"] == "test_run"]
    result["test_runs"] = {"count": len(runs), "seconds": round(sum(float(e.get("seconds") or 0) for e in runs), 1)}
    json.dump(ev, open(os.path.join(D, "results", f"{SET}-treeyard-events.json"), "w"), indent=1)
    shutil.copy(os.path.join(T, "serve.log"), os.path.join(D, "results", f"{SET}-treeyard-serve.log"))
    daemon.terminate()
    serve.terminate()
    return result


def main():
    arm = sys.argv[1]
    os.makedirs(os.path.join(D, "results"), exist_ok=True)
    r = arm_git() if arm == "git" else arm_treeyard()
    r["model"] = MODEL
    json.dump(r, open(os.path.join(D, "results", f"{SET}-{arm}.json"), "w"), indent=2)
    print(json.dumps({k: v for k, v in r.items() if k != "agents"}, indent=2))


main()
