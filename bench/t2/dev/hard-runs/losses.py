#!/usr/bin/env python3
"""Why a task or an integration test is lost on the final main of an arm A run, from the history of main.

Usage: python3 losses.py --scratch S --run RUNNER_DIR [--set hard] [--out FILE.json]

For every hidden test and integration test that fails on the final main:
  own miss             the task's hidden test already fails on the agent's own work, before any merge
  own merge            it passes on the agent's own work and fails from the merge of that agent on
                       (the agent lost its own task while it settled its conflict)
  broken by <agent>    it passed on main and fails from a later merge on: that agent's merge or conflict
                       resolution removed or changed what the test needs
  do not fit           (integration tests) it never passed on main although both tasks pass at the end
  follows <task>       (integration tests) one of its tasks is lost
Reads the run directory only.
"""
import argparse, contextlib, io, json, re, shutil, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[2]))
import t2  # noqa: E402


def classify(rows, agent_at, task_k, pairs, own_passes):
    """rows: states of main in order, each {"passed": {test id: bool}}; agent_at: the agent number whose
    work entered at each state (None for the pin); task_k: task id -> agent number; pairs: integration
    id -> the two agent numbers; own_passes(task id) -> True/False for the agent's own work before any
    merge, None when it never reached main. Returns one entry per test that fails at the last state."""
    k_to_id = {k: t for t, k in task_k.items()}
    last = rows[-1]["passed"]
    out = []
    for tid in list(task_k) + list(pairs):
        if last[tid]:
            continue
        seq = [r["passed"][tid] for r in rows]
        if any(seq):
            i = max(j for j in range(1, len(seq)) if seq[j - 1] and not seq[j])
            cause, by = "broken by", k_to_id.get(agent_at[i], agent_at[i])
        elif tid in task_k:
            own = own_passes(tid)
            cause = "not merged" if own is None else "own merge" if own else "own miss"
            by = tid if own is not None else None
        else:
            lost = [k_to_id[k] for k in pairs[tid] if not last[k_to_id[k]]]
            cause, by = ("follows", lost) if lost else ("do not fit", None)
        out.append({"test": tid, "kind": "task" if tid in task_k else "integration", "cause": cause, "by": by})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", required=True, type=Path)
    ap.add_argument("--run", required=True, type=Path)
    ap.add_argument("--set", default="hard")
    ap.add_argument("--out")
    a = ap.parse_args()
    repo, s = "click", a.set
    t2.select_kit([repo])
    tasks_dir, spec = t2.TASKS_DIR, t2.REPOS[repo]
    final = a.run.resolve() / "final"
    with contextlib.redirect_stdout(io.StringIO()):
        h = t2.main_health(a.scratch.resolve(), a.run.resolve(), repo, s, tasks_dir=tasks_dir, spec=spec)
    rows = h["rows"]
    full = t2.git("rev-list", "--first-parent", "--reverse", f"{spec['rev']}..HEAD", cwd=final).stdout.split()
    agent_at = []                       # agent number merged at each state of main
    for c in full:
        subj = t2.git("log", "-1", "--format=%s", c, cwd=final).stdout
        m = re.search(r"agent-(\d+)", subj)
        agent_at.append(int(m.group(1)) if m else None)
    metas = {}
    for td in (tasks_dir / repo / s).iterdir():
        if td.is_dir() and td.name.split("-")[0].isdigit():
            m = json.loads((td / "meta.json").read_text())
            metas[m["id"]] = (int(td.name.split("-")[0]), td, m)
    integ = {it["id"]: it for it in t2.set_spec(tasks_dir, repo, s)["integration"]}
    k_to_id = {k: tid for tid, (k, _, _) in metas.items()}

    def own_commit(k):
        if k not in agent_at:
            return None
        c = full[agent_at.index(k)]
        parents = t2.git("log", "-1", "--format=%P", c, cwd=final).stdout.split()
        if len(parents) < 2:
            return c
        tip = parents[1]
        if "hand-back" in t2.git("log", "-1", "--format=%s", tip, cwd=final).stdout:
            tip = t2.git("log", "-1", "--format=%P", tip, cwd=final).stdout.split()[0]
        return tip

    def passes_at(commit, tid):
        _, td, m = metas[tid]
        work = Path(tempfile.mkdtemp(prefix="t2-losses-"))
        try:
            wt = work / "tree"
            t2.git("clone", "-q", "--no-checkout", str(final), str(wt), cwd=work)
            t2.git("checkout", "-q", "-f", "--detach", commit, cwd=wt)
            dst = wt / m["test"]
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(td / "hidden" / m["test"], dst)
            ok, _, _ = t2.run_cmd_spec(spec, spec["decide"], wt, a.scratch.resolve(), m)
            return ok
        finally:
            shutil.rmtree(work, ignore_errors=True)

    if not rows:
        sys.exit("no commit on main after the pin")
    out = classify(rows, agent_at, {tid: k for tid, (k, _, _) in metas.items()},
                   {i: it["pair"] for i, it in integ.items()},
                   lambda tid: (lambda oc: None if oc is None else passes_at(oc, tid))(own_commit(metas[tid][0])))
    last = rows[-1]["passed"]
    res = {"run": a.run.name, "tasks_on_main": sum(last[t] for t in metas), "tasks": len(metas),
           "integration_passed": sum(last[t] for t in integ), "integration": len(integ), "losses": out,
           "breaks": h["breaks"]}
    print(json.dumps(res, indent=1))
    if a.out:
        Path(a.out).write_text(json.dumps(res, indent=1) + "\n")


if __name__ == "__main__":
    main()
