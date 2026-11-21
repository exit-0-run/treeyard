#!/usr/bin/env python3
"""Task 022 end-to-end user path, run once from a clean clone.

Usage: T1_FORBIDDEN=w1,w2 python3 e2e.py CLONE OUT_DIR
Steps: make repo -> connect (x2) -> status -> one real claude -p agent -> status -> audit ->
git pull from the server -> tests -> disconnect (x2) -> tree check -> user config check.
"""

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path

CLONE = Path(sys.argv[1]).resolve()
OUT = Path(sys.argv[2]).resolve()
spec = importlib.util.spec_from_file_location("t1run", CLONE / "docs/trials/t1/run.py")
t1 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(t1)
BIN = CLONE / "target" / "release" / "treeyard"
t1.BIN = BIN

PROMPT = t1.PROMPT
assert "treeyard" not in PROMPT.lower()
MAX_TURNS = 30
TIMEOUT_S = 600

OUT.mkdir(parents=True, exist_ok=False)
REPO = OUT / "repo"
LOG = open(OUT / "transcript.txt", "w")
ENV = dict(os.environ, PATH=f"{BIN.parent}:{os.environ['PATH']}", PYTHONDONTWRITEBYTECODE="1")
os.environ["T1_SCRATCH"] = str(OUT)


def log(s):
    LOG.write(s + "\n")
    LOG.flush()
    print(s)


def step(title, args, cwd=REPO, check=True, timeout=120):
    log(f"\n$ {' '.join(str(a) for a in args)}    # {title}")
    t0 = time.time()
    r = subprocess.run([str(a) for a in args], cwd=cwd, env=ENV, capture_output=True, text=True, timeout=timeout)
    dt = time.time() - t0
    body = (r.stdout + r.stderr).rstrip()
    if body:
        log(body)
    log(f"[exit {r.returncode}, {dt:.2f} s]")
    if check and r.returncode != 0:
        log("STEP FAILED")
    return r, dt


def tree_hash(root):
    """sha256 over (path, mode, bytes) of every file outside .git."""
    h = hashlib.sha256()
    files = []
    for p in sorted(root.rglob("*")):
        rel = p.relative_to(root)
        if rel.parts[0] == ".git" or not (p.is_file() or p.is_symlink()):
            continue
        files.append(str(rel))
        h.update(str(rel).encode() + b"\0" + oct(p.lstat().st_mode).encode() + b"\0" + p.read_bytes() + b"\0")
    return h.hexdigest(), files


results = {"prompt": PROMPT, "limits": {"max_turns": MAX_TURNS, "timeout_s": TIMEOUT_S}}

# 0. user config before
t1.snapshot(OUT / "userconfig-before.txt")

# 1. the repository: the T1 slugify repo plus a pre-existing AGENTS.md
t1.make_repo(REPO)
(REPO / "AGENTS.md").write_text("# Notes for agents\n\nKeep functions small.\n")
subprocess.run(["git", "add", "AGENTS.md"], cwd=REPO, check=True)
subprocess.run(["git", "commit", "-q", "-m", "agent notes"], cwd=REPO, check=True)
before_head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip()
before_hash, before_files = tree_hash(REPO)
log(f"# repo created: files {before_files}, tree hash {before_hash}, HEAD {before_head[:12]}")
step("tool versions", ["treeyard", "--version"])

# 2. connect, twice
r, dt = step("connect", ["treeyard", "connect", "--harness", "all", "--agents", "2", "--no-mirror"])
results["connect_s"] = round(dt, 2)
results["connect_exit"] = r.returncode
h1, f1 = tree_hash(REPO)
r2, dt2 = step("second connect (idempotent)", ["treeyard", "connect", "--harness", "all", "--agents", "2", "--no-mirror"])
h2, f2 = tree_hash(REPO)
results["second_connect_same_tree"] = h1 == h2
results["second_connect_output"] = (r2.stdout + r2.stderr).strip()
log(f"# tree after connect {h1}; after second connect {h2}; equal: {h1 == h2}; files {f1}")
step("git status after connect", ["git", "status", "--short"])
st = json.loads(step("status json", ["treeyard", "connect", "status", "--json"])[0].stdout)
url = st["url"]
assert not url.endswith(":7420"), "port 7420 refused"
results["url_loopback"] = url.startswith("http://127.0.0.1:")
step("connect status before the agent", ["treeyard", "connect", "status"])
(OUT / "block.md").write_text((REPO / "CLAUDE.md").read_text())

# 3. one real agent: claude -p, project .mcp.json, config dir in scratch, turn limit and timeout
cfg = OUT / "claudecfg"
cfg.mkdir()
tok, err = t1.claude_token()
if tok is None:
    log(f"# BLOCKED: claude token: {err}")
    results["agent"] = {"blocked": err}
else:
    aenv = dict(ENV)
    aenv.pop("CLAUDECODE", None)
    aenv.pop("CLAUDE_CODE_ENTRYPOINT", None)
    aenv.update(CLAUDE_CONFIG_DIR=str(cfg), CLAUDE_CODE_OAUTH_TOKEN=tok, DISABLE_AUTOUPDATER="1", DISABLE_TELEMETRY="1")
    cmd = ["claude", "-p", PROMPT, "--mcp-config", ".mcp.json", "--strict-mcp-config", "--max-turns", str(MAX_TURNS),
           "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits",
           "--allowedTools", "mcp__treeyard__*,Bash,Read,Edit,Write"]
    log("\n$ " + " ".join(c if c != PROMPT else f"'{PROMPT}'" for c in cmd) + "    # real agent (CLAUDE_CONFIG_DIR in scratch)")
    t1.TIMEOUT_S = TIMEOUT_S
    info = t1.run_agent("claude", cmd, aenv, REPO, OUT / "agent-transcript.jsonl", OUT / "agent-stderr.txt")
    tools = t1.tool_calls("claude", OUT / "agent-transcript.jsonl")
    final = None
    for line in (OUT / "agent-transcript.jsonl").read_text(errors="replace").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if ev.get("type") == "result":
            final = {k: ev.get(k) for k in ("subtype", "num_turns", "duration_ms", "is_error", "total_cost_usd")}
    info.update(tool_calls=tools, result_event=final)
    results["agent"] = info
    log(json.dumps(info, indent=1))

# 4. status after the agent, audit, events
step("connect status after the agent", ["treeyard", "connect", "status"])
audit = json.loads(step("audit", ["treeyard", "call", "--url", url, "audit", '{"limit": 1000}'])[0].stdout)
(OUT / "audit.json").write_text(json.dumps(audit, indent=2))
rows = sorted(audit["entries"], key=lambda r: r["seq"])
seq = [(r.get("agent"), r.get("op"), r.get("outcome") or r.get("status")) for r in rows]
results["audit_sequence"] = seq
log("# audit rows: " + json.dumps(seq))

# 5. take the landed work back into the checkout, run the tests
step("pull landed main", ["git", "pull", "-q", f"{url}/git", "main"])
step("git log", ["git", "log", "--oneline", "-n", "5"])
step("diff before..HEAD", ["git", "diff", "--stat", before_head, "HEAD"])
t, _ = step("tests in the checkout", [sys.executable, "-m", "unittest"], check=False)
results["tests_exit"] = t.returncode

# 6. disconnect, twice
d, ddt = step("disconnect", ["treeyard", "disconnect"])
results["disconnect_s"] = round(ddt, 2)
d2, _ = step("second disconnect (idempotent)", ["treeyard", "disconnect"])
results["second_disconnect_output"] = (d2.stdout + d2.stderr).strip()
pid = st.get("pid")
alive = subprocess.run(["kill", "-0", str(pid)], capture_output=True).returncode == 0 if pid else None
log(f"# server pid {pid} alive after disconnect: {alive}")
results["server_alive_after"] = alive
c = subprocess.run(["curl", "-s", "-m", "2", "-o", "/dev/null", "-w", "%{http_code}", url + "/"], capture_output=True, text=True)
log(f"# curl {url}/ after disconnect: http_code {c.stdout} exit {c.returncode}")
results["curl_after"] = [c.stdout, c.returncode]
step("git status after disconnect", ["git", "status", "--short", "--untracked-files=all"])

# 7. tree check: before tree + the files of the landed change, nothing else
after_hash, after_files = tree_hash(REPO)
changed = subprocess.run(["git", "diff", "--name-only", before_head, "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.split()
exp = OUT / "expected"
subprocess.run(["git", "clone", "-q", "--no-checkout", str(REPO), str(exp)], check=True)
subprocess.run(["git", "-C", str(exp), "checkout", "-q", before_head], check=True)
for f in changed:
    data = subprocess.run(["git", "-C", str(REPO), "show", f"HEAD:{f}"], capture_output=True).stdout
    (exp / f).write_bytes(data)
exp_hash, exp_files = tree_hash(exp)
results["tree"] = {"before": before_hash, "after": after_hash, "expected": exp_hash, "changed_by_land": changed,
                   "equal": after_hash == exp_hash, "extra_files": sorted(set(after_files) - set(exp_files)),
                   "missing_files": sorted(set(exp_files) - set(after_files))}
log("# tree check: " + json.dumps(results["tree"]))

# 8. user config after
t1.snapshot(OUT / "userconfig-after.txt")
b = (OUT / "userconfig-before.txt").read_text().splitlines()
a = (OUT / "userconfig-after.txt").read_text().splitlines()
results["userconfig_changed_lines"] = [x for x in a if x not in b]
log("# user config lines that changed: " + json.dumps(results["userconfig_changed_lines"]))
(OUT / "results.json").write_text(json.dumps(results, indent=2))
log("# done")
