#!/usr/bin/env python3
"""The repository's suite at every state of main of an arm A run, and on every agent's own work.

Usage: python3 suite_at.py --scratch S --run RUNNER_DIR [--out FILE.json]

A queue or a test before entry would test main with the change in it: that is each state of main.
Reads the run directory only.
"""
import argparse, json, re, shutil, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[2]))
import t2  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", required=True, type=Path)
    ap.add_argument("--run", required=True, type=Path)
    ap.add_argument("--out")
    a = ap.parse_args()
    repo = "click"
    t2.select_kit([repo])
    spec = t2.REPOS[repo]
    final = a.run.resolve() / "final"
    full = t2.git("rev-list", "--first-parent", "--reverse", f"{spec['rev']}..HEAD", cwd=final).stdout.split()

    def suite(commit):
        work = Path(tempfile.mkdtemp(prefix="t2-suite-"))
        try:
            wt = work / "tree"
            t2.git("clone", "-q", "--no-checkout", str(final), str(wt), cwd=work)
            t2.git("checkout", "-q", "-f", "--detach", commit, cwd=wt)
            ok, out, secs = t2.run_cmd_spec(spec, spec["suite"], wt, a.scratch.resolve())
            return ok, secs, out.strip().splitlines()[-1][:200] if out.strip() else ""
        finally:
            shutil.rmtree(work, ignore_errors=True)

    rows = []
    for c in full:
        subj = t2.git("log", "-1", "--format=%s", c, cwd=final).stdout.strip()
        m = re.search(r"agent-(\d+)", subj)
        parents = t2.git("log", "-1", "--format=%P", c, cwd=final).stdout.split()
        ok, secs, tail = suite(c)
        row = {"commit": c[:10], "subject": subj[:80], "agent": int(m.group(1)) if m else None, "main_suite": ok, "s": secs, "tail": tail}
        if len(parents) > 1:
            tip = parents[1]
            handback = "hand-back" in t2.git("log", "-1", "--format=%s", tip, cwd=final).stdout
            if handback:
                tip = t2.git("log", "-1", "--format=%P", tip, cwd=final).stdout.split()[0]
            row["handback"] = handback
            row["own_suite"], _, row["own_tail"] = suite(tip)
        rows.append(row)
        print(json.dumps(row), flush=True)
    res = {"run": a.run.name, "states": len(rows), "main_suite_failed": sum(not r["main_suite"] for r in rows),
           "own_work": sum("own_suite" in r for r in rows), "own_suite_failed": sum(r.get("own_suite") is False for r in rows), "rows": rows}
    if a.out:
        Path(a.out).write_text(json.dumps(res, indent=1))
    print(json.dumps({k: v for k, v in res.items() if k != "rows"}))


if __name__ == "__main__":
    main()
