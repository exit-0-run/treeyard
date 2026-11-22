#!/usr/bin/env python3
"""Traffic without a model: scripted agents send small ready changes at a set pace.

Two arms on the same repository and the same stream of changes:
  git       one clone per agent, a shared bare repository; an agent with a ready change commits,
            then loops fetch, rebase onto main, push; a rejected push is a retry.
  treeyard_queue  as treeyard, but the server runs in landing mode `queue` (needs --test-s): `land`
            answers `queued`, the server tests up to --batch changes together and lands the batch;
            the agent polls `land_status`.
  treeyard  native mode: one stdio bridge per agent (`treeyard mcp --tools native`); `start` gives
            the directory, the agent edits it, `land` sends and lands; a reply that is not
            `landed` is a retry.

Each agent, until the time is up: take the current main, wait (exponential, mean THINK seconds),
write one change on that now stale base (a new 2-line function before a top-level `def` or
`class` of one file), then bring it in. With probability HOT the file is one of the hot files.
Two inserts at the same place conflict; both arms settle that the same mechanical way (keep both
functions: drop the marker lines). With --test-s N both arms must run a command of N seconds on
the rebased tree before it may enter (git: before each push; TreeYard: the land test).

Usage: traffic.py --seed-repo DIR --rev REV --treeyard BIN --work DIR --out FILE.json
                  [--arms git,treeyard] [--levels 10,20,50,100,200] [--duration 180] [--think 30]
                  [--hot 0.5] [--test-s 0] [--seed 1]
"""
import argparse, json, os, random, shutil, statistics, subprocess, sys, threading, time
from pathlib import Path

HOT_FILES = ["src/click/core.py", "src/click/termui.py", "src/click/types.py"]
MARKS = ("<<<<<<< ", "=======", ">>>>>>> ", "||||||| ")
GENV = dict(os.environ, GIT_AUTHOR_NAME="sim", GIT_AUTHOR_EMAIL="sim@example.invalid", GIT_COMMITTER_NAME="sim",
            GIT_COMMITTER_EMAIL="sim@example.invalid", GIT_TERMINAL_PROMPT="0", GIT_EDITOR="true")


def git(*a, cwd, check=True):
    r = subprocess.run(["git", *a], cwd=cwd, capture_output=True, text=True, env=GENV)
    if check and r.returncode:
        raise RuntimeError(f"git {' '.join(a)}: {r.stderr.strip()[:300]}")
    return r


def make_seed(src, rev, dst):
    """One commit with the tree of `rev`, branch main."""
    dst.mkdir(parents=True)
    tar = subprocess.run(["git", "archive", rev], cwd=src, capture_output=True, check=True).stdout
    subprocess.run(["tar", "-x", "-C", str(dst)], input=tar, check=True)
    git("init", "-q", "-b", "main", cwd=dst)
    git("add", "-A", cwd=dst)
    git("commit", "-q", "-m", "seed", cwd=dst)


def other_files(seed):
    out = []
    for p in sorted((seed / "src/click").glob("*.py")):
        rel = str(p.relative_to(seed))
        if rel not in HOT_FILES and any(l.startswith(("def ", "class ")) for l in p.read_text().splitlines()):
            out.append(rel)
    return out


def edit(root, rel, rng, name):
    p = Path(root) / rel
    lines = p.read_text().splitlines(keepends=True)
    slots = [i for i, l in enumerate(lines) if l.startswith(("def ", "class "))]
    i = rng.choice(slots)
    while i > 0 and lines[i - 1].startswith("@"):
        i -= 1
    lines[i:i] = [f"def {name}():\n", f"    return {name!r}\n", "\n", "\n"]
    p.write_text("".join(lines))


def strip_markers(path):
    p = Path(path)
    p.write_text("".join(l for l in p.read_text().splitlines(keepends=True) if not l.startswith(MARKS)))


class Bridge:
    def __init__(self, ty, url, agent, env):
        self.p = subprocess.Popen([str(ty), "mcp", "--url", url, "--agent", agent, "--tools", "native"], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "traffic", "version": "1"}})

    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise RuntimeError("the bridge exited")
        return json.loads(line)

    def call(self, name, args):
        r = self.rpc("tools/call", {"name": name, "arguments": args})
        res = r.get("result") or {}
        text = "".join(c.get("text", "") for c in res.get("content") or [])
        try:
            v = json.loads(text)
        except ValueError:
            v = {"error": text or json.dumps(r.get("error"))}
        if res.get("isError") and "error" not in v:
            v["error"] = text[:300]
        return v if isinstance(v, dict) else {"error": text[:300]}

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=10)
        except Exception:
            self.p.kill()


class Level:
    def __init__(self, a, arm, n, work):
        self.a, self.arm, self.n, self.work = a, arm, n, work
        self.rows, self.errors, self.lock = [], [], threading.Lock()
        self.files = other_files(a.seed)
        self.queue = arm == "treeyard_queue"
        self.verdicts = {}
        self.stop_new = 0.0
        self.deadline = 0.0

    def pick(self, rng):
        return rng.choice(HOT_FILES) if rng.random() < self.a.hot else rng.choice(self.files)

    # ---- git
    def setup_git(self):
        self.origin = self.work / "origin.git"
        git("clone", "-q", "--bare", str(self.a.seed), str(self.origin), cwd=self.work)
        self.dirs = []
        for i in range(self.n):
            d = self.work / "agents" / f"a{i}"
            d.parent.mkdir(exist_ok=True)
            git("clone", "-q", str(self.origin), str(d), cwd=self.work)
            self.dirs.append(d)

    def agent_git(self, i):
        rng = random.Random(self.a.seed_n * 100003 + i)
        d, k = self.dirs[i], 0
        while time.time() < self.stop_new:
            if git("fetch", "-q", "origin", cwd=d, check=False).returncode:
                self.err("fetch failed before a new change")
                time.sleep(1)
                continue
            git("reset", "-q", "--hard", "origin/main", cwd=d)
            time.sleep(rng.expovariate(1 / self.a.think))
            k += 1
            name = f"_sim_{i}_{k}"
            rel = self.pick(rng)
            edit(d, rel, rng, name)
            git("commit", "-q", "-am", name, cwd=d)
            ready, retries, conflicts, landed = time.time(), 0, 0, None
            while time.time() < self.deadline:
                try:
                    git("fetch", "-q", "origin", cwd=d)
                    r = git("rebase", "origin/main", cwd=d, check=False)
                    if r.returncode:
                        conflicts += 1
                        for f in git("diff", "--name-only", "--diff-filter=U", cwd=d).stdout.split():
                            strip_markers(d / f)
                            git("add", f, cwd=d)
                        git("rebase", "--continue", cwd=d)
                except RuntimeError as e:  # a git command failed under load: note it, clean up, try again
                    self.err(str(e)[:300])
                    git("rebase", "--abort", cwd=d, check=False)
                    retries += 1
                    time.sleep(0.2)
                    continue
                if self.a.test_s:
                    time.sleep(self.a.test_s)
                if git("push", "-q", "origin", "HEAD:main", cwd=d, check=False).returncode == 0:
                    landed = time.time()
                    break
                retries += 1
            self.row(i, name, rel, ready, landed, retries, conflicts)

    def names_on_main_git(self):
        out = git("grep", "-h", "-o", "-E", "^def _sim_[0-9]+_[0-9]+", "main", "--", "src", cwd=self.origin, check=False).stdout
        return {l.split()[1] for l in out.splitlines()}

    # ---- treeyard
    def setup_treeyard(self):
        data = self.work / "data"
        subprocess.run([self.a.treeyard, "init", "--data", str(data), "--from", str(self.a.seed), "--branch", "main"],
                       check=True, capture_output=True)
        if self.a.test_s:
            with open(data / "treeyard.toml", "a") as f:
                if self.queue:
                    f.write(f'\n[landing]\nmode = "queue"\ntest = "sleep {self.a.test_s}"\nbatch = {self.a.batch}\n')
                else:
                    f.write(f'\n[coordination]\nland_test = "sleep {self.a.test_s}"\n')
        pf = self.work / "port"
        self.server = subprocess.Popen([self.a.treeyard, "serve", "--data", str(data), "--addr", "127.0.0.1:0", "--port-file", str(pf),
                                        "--max-inflight", "1024"], stdout=subprocess.DEVNULL, stderr=open(self.work / "server.log", "w"))
        for _ in range(100):
            if pf.exists() and pf.read_text().strip():
                break
            time.sleep(0.1)
        self.url = f"http://127.0.0.1:{pf.read_text().strip()}"
        self.bridges = []
        for i in range(self.n):
            tmp = self.work / "tmp" / f"a{i}"
            tmp.mkdir(parents=True)
            self.bridges.append(Bridge(self.a.treeyard, self.url, f"sim-{i}", dict(os.environ, TMPDIR=str(tmp))))

    def agent_treeyard(self, i):
        rng = random.Random(self.a.seed_n * 100003 + i)
        br, k = self.bridges[i], 0
        while time.time() < self.stop_new:
            k += 1
            name = f"_sim_{i}_{k}"
            st = br.call("start", {"intent": name})
            cid, d = st.get("id"), st.get("dir")
            if not cid or not d:
                self.err(f"start: {json.dumps(st)[:200]}")
                time.sleep(1)
                continue
            time.sleep(rng.expovariate(1 / self.a.think))
            rel = self.pick(rng)
            edit(d, rel, rng, name)
            ready, retries, conflicts, landed = time.time(), 0, 0, None
            while time.time() < self.deadline:
                r = br.call("land", {"change": cid})
                retries += max(0, int((r.get("land_test") or {}).get("rounds", 1)) - 1)
                s = r.get("status")
                if s == "queued":
                    v = self.wait_queue(cid)
                    if v == "passed":
                        landed = time.time()
                        break
                    retries += 1
                    with self.lock:
                        self.verdicts[v] = self.verdicts.get(v, 0) + 1
                    continue
                if s == "landed":
                    landed = time.time()
                    break
                retries += 1
                if s == "conflict" or r.get("conflicted"):
                    conflicts += 1
                    for f in r.get("conflicted") or []:
                        strip_markers(Path(d) / (f["path"] if isinstance(f, dict) else f))
                elif s not in ("main_moved", "untested"):
                    self.err(f"land: {json.dumps(r)[:300]}")
                    if retries > 20:
                        break
                    time.sleep(0.2)
            self.row(i, name, rel, ready, landed, retries, conflicts)
            shutil.rmtree(d, ignore_errors=True)

    def wait_queue(self, cid):
        """Queue mode: poll `land_status` until the change left the queue; the verdict."""
        import urllib.request
        body = json.dumps({"change": cid}).encode()
        while time.time() < self.deadline:
            time.sleep(0.25)
            try:
                req = urllib.request.Request(self.url + "/api/land_status", data=body, headers={"Content-Type": "application/json"})
                v = json.loads(urllib.request.urlopen(req, timeout=30).read()).get("verdict")
            except Exception as e:
                self.err(f"land_status: {e}"[:200])
                continue
            if v not in ("queued", "pending"):
                return v
        return "timeout"

    setup_treeyard_queue = lambda self: self.setup_treeyard()
    agent_treeyard_queue = lambda self, i: self.agent_treeyard(i)
    names_on_main_treeyard_queue = lambda self: self.names_on_main_treeyard()

    def names_on_main_treeyard(self):
        tmp = self.work / "tmp" / "check"
        tmp.mkdir(parents=True)
        br = Bridge(self.a.treeyard, self.url, "sim-check", dict(os.environ, TMPDIR=str(tmp)))
        d = br.call("start", {"intent": "check"}).get("dir")
        br.close()
        out = subprocess.run(["grep", "-r", "-h", "-o", "-E", "^def _sim_[0-9]+_[0-9]+", str(Path(d) / "src")], capture_output=True, text=True).stdout
        return {l.split()[1] for l in out.splitlines()}

    # ---- common
    def row(self, i, name, rel, ready, landed, retries, conflicts):
        with self.lock:
            self.rows.append(dict(agent=i, name=name, file=rel, ready=ready, landed=landed, retries=retries, conflicts=conflicts))

    def err(self, text):
        with self.lock:
            self.errors.append(text)

    def run(self):
        t = time.time()
        getattr(self, "setup_" + self.arm)()
        setup_s = time.time() - t
        t0 = time.time()
        self.stop_new = t0 + self.a.duration
        self.deadline = self.stop_new + self.a.grace
        fn = getattr(self, "agent_" + self.arm)

        def guard(i):
            try:
                fn(i)
            except Exception as e:  # an agent that dies is a result, not a crash of the run
                self.err(f"agent {i}: {e}"[:300])

        th = [threading.Thread(target=guard, args=(i,), daemon=True) for i in range(self.n)]
        for x in th:
            x.start()
        for x in th:
            x.join(timeout=max(1, self.deadline + 60 - time.time()))
        on_main = getattr(self, "names_on_main_" + self.arm)()
        if self.arm.startswith("treeyard"):
            for b in self.bridges:
                b.close()
            self.server.terminate()
            self.server.wait(timeout=20)
        ready = [r for r in self.rows if r["ready"] <= self.stop_new]   # made within the window
        done = [r for r in ready if r["landed"]]
        waits = sorted(r["landed"] - r["ready"] for r in done)
        in_window = [r for r in done if r["landed"] <= self.stop_new]
        q = lambda p: round(waits[min(len(waits) - 1, int(p * len(waits)))], 3) if waits else None
        res = dict(arm=self.arm, agents=self.n, test_s=self.a.test_s, duration_s=self.a.duration, setup_s=round(setup_s, 1),
                   ready=len(ready), landed=len(done), not_landed=len(ready) - len(done),
                   landed_per_hour=round(len(in_window) * 3600 / self.a.duration),
                   wait_median_s=q(0.5), wait_p95_s=q(0.95), wait_max_s=round(waits[-1], 3) if waits else None,
                   retries=sum(r["retries"] for r in ready), conflicts=sum(r["conflicts"] for r in ready),
                   hot_share=round(sum(r["file"] in HOT_FILES for r in ready) / max(1, len(ready)), 3),
                   landed_missing_on_main=len({r["name"] for r in done} - on_main), on_main=len(on_main),
                   queue_verdicts=self.verdicts, errors=len(self.errors), error_samples=self.errors[:5], load_avg=os.getloadavg()[0])
        return res, self.rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed-repo", required=True)
    ap.add_argument("--rev", required=True)
    ap.add_argument("--treeyard", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--arms", default="git,treeyard")
    ap.add_argument("--levels", default="10,20,50,100,200")
    ap.add_argument("--duration", type=float, default=180)
    ap.add_argument("--grace", type=float, default=120)
    ap.add_argument("--think", type=float, default=30)
    ap.add_argument("--hot", type=float, default=0.5)
    ap.add_argument("--test-s", type=float, default=0)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--batch", type=int, default=8)
    a = ap.parse_args()
    a.seed_n = a.seed
    work = Path(a.work).resolve()
    shutil.rmtree(work, ignore_errors=True)
    a.seed = work / "seed"
    make_seed(a.seed_repo, a.rev, a.seed)
    results = []
    for n in [int(x) for x in a.levels.split(",")]:
        for arm in a.arms.split(","):
            w = work / f"{arm}-{n}"
            w.mkdir()
            res, rows = Level(a, arm, n, w).run()
            results.append(res)
            print(json.dumps(res), flush=True)
            Path(a.out).write_text(json.dumps(dict(params=dict(levels=a.levels, duration=a.duration, grace=a.grace, think=a.think, hot=a.hot,
                                                               test_s=a.test_s, seed=a.seed_n, rev=a.rev, cpus=os.cpu_count()), results=results), indent=1))
            (Path(a.out).parent / f"rows-{arm}-{n}-t{a.test_s:g}.json").write_text(json.dumps(rows))
            shutil.rmtree(w, ignore_errors=True)


if __name__ == "__main__":
    main()
