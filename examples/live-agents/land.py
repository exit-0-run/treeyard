#!/usr/bin/env python3
"""Merge queue for the plain-git arm: one land at a time (a file lock), like a CI merge queue.

Run inside an agent's worktree:  python3 ../land.py
1. Refuses uncommitted work.  2. Rebases the branch on master.  3. Runs the tests.
4. Fast-forwards master.  Every attempt is logged as one JSON line in ../land.log.
"""
import fcntl, json, os, subprocess, sys, time

G = os.path.dirname(os.path.abspath(__file__))  # demo.py copies this file into <DEMO_DIR>/g
REPO = os.path.join(G, "repo")
PY = os.environ["DEMO_PY"]


def git(*a, cwd="."):
    return subprocess.run(["git", *a], cwd=cwd, capture_output=True, text=True)


def log(rec):
    rec["t"] = time.time()
    with open(os.path.join(G, "land.log"), "a") as f:
        f.write(json.dumps(rec) + "\n")


def main():
    branch = git("branch", "--show-current").stdout.strip()
    if git("status", "--porcelain").stdout.strip():
        print("REFUSED: commit your work first (git add -A && git commit -m ...)")
        log({"branch": branch, "result": "dirty"})
        return 1
    if os.path.isdir(git("rev-parse", "--git-path", "rebase-merge").stdout.strip()):
        print("REFUSED: a rebase is in progress; resolve the conflicts, git add, git rebase --continue")
        return 1
    t0 = time.time()
    with open(os.path.join(G, "land.lock"), "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        waited = time.time() - t0
        r = git("rebase", "master")
        if r.returncode != 0:
            files = git("diff", "--name-only", "--diff-filter=U").stdout.split()
            log({"branch": branch, "result": "conflict", "files": files, "waited_s": round(waited, 2)})
            print("CONFLICT while rebasing on master in: " + ", ".join(files))
            print("Resolve the conflict markers in these files (keep both sides' additions), then:")
            print("  git add <files> && git rebase --continue   # then run the tests, then: python3 ../land.py")
            return 1
        t_test = time.time()
        tests = subprocess.run([PY, "-m", "pytest", "-q", "-x", "tests"], capture_output=True, text=True)
        test_s = round(time.time() - t_test, 2)
        if tests.returncode != 0:
            log({"branch": branch, "result": "tests_failed", "waited_s": round(waited, 2), "test_s": test_s})
            print("TESTS FAILED after rebase on master:\n" + tests.stdout[-3000:])
            return 1
        m = git("merge", "--ff-only", branch, cwd=REPO)
        if m.returncode != 0:
            log({"branch": branch, "result": "ff_failed", "error": m.stderr[-500:]})
            print("could not fast-forward master: " + m.stderr)
            return 1
        log({"branch": branch, "result": "landed", "waited_s": round(waited, 2), "test_s": test_s,
             "seconds": round(time.time() - t0, 2)})
        print("LANDED on master")
        return 0


sys.exit(main())
