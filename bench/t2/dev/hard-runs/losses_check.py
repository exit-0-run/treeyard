#!/usr/bin/env python3
"""Runs `losses.classify` on stored histories of main of five `heavy` runs whose losses were traced
by hand (table "Every loss that is not h7" in ../heavy-runs/README.md) and prints both readings.
`losses-check.json` holds, per run, the pass/fail of every test at every state of main, the agent
whose work entered at each state, and whether each task passed on its agent's own work."""
import json, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from losses import classify  # noqa: E402

BY_HAND = {
    "A-click-heavy-r3-real": [("h1", "broken by", "h8"), ("i1", "do not fit", None)],
    "A-click-heavy-r4-real": [("i1", "do not fit", None), ("i7", "do not fit", None)],
    "A-click-heavy-r12-real": [("h1", "broken by", "h3")],
    "A2-click-heavy-r3-real": [("h1", "broken by", "h8"), ("h7", "own miss", "h7")],
    "BN-click-heavy-r13-real": [("i6", "do not fit", None)],
}
d = json.loads((HERE / "losses-check.json").read_text())
same = total = 0
for run, f in d["runs"].items():
    got = classify([{"passed": r} for r in f["rows"]], f["agent_at"], d["task_k"], d["pairs"], lambda t: f["own"][t])
    got = [(x["test"], x["cause"], x["by"]) for x in got]
    for want in BY_HAND[run]:
        total += 1
        hit = next((g for g in got if g[0] == want[0]), None)
        ok = hit is not None and hit[1] == want[1] and (want[2] is None or hit[2] == want[2])
        same += ok
        print(f"{run}: {want[0]} by hand {want[1]} {want[2] or ''} | script {hit[1] if hit else 'not lost'} {hit[2] if hit else ''} | {'same' if ok else 'DIFFERENT'}")
    extra = [g for g in got if g[0] not in [w[0] for w in BY_HAND[run]]]
    if extra:
        print(f"{run}: script also reports {extra}")
print(f"{same} of {total} agree")
