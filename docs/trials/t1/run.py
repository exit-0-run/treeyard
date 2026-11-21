#!/usr/bin/env python3
"""T1 smoke trial: one real agent, one small task, one fresh repository.

Usage:
  python3 docs/trials/t1/run.py snapshot OUT_FILE
  python3 docs/trials/t1/run.py run HARNESS ATTEMPT SCRATCH [--model M] [--block vN]

`run` makes SCRATCH/t1/runs/HARNESS-ATTEMPT/repo, runs `treeyard connect` in it, gives the
agent a task prompt that does not name TreeYard, then saves the server audit log and events,
tests main from a clone, runs `disconnect --purge` and copies sanitised files to
docs/trials/t1/HARNESS-ATTEMPT/. Standard library only.
"""

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
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BIN = ROOT / "target" / "debug" / "treeyard"
HOME = Path.home()

PROMPT = "Fix the bug in slugify in text.py so that `python3 -m unittest` passes. Finish the work completely."
TIMEOUT_S = 600
CLAUDE_MAX_TURNS = 30
CODEX_MAX_TOOL_EVENTS = 40
OPENCODE_STEPS = 30

TEXT_PY = '''"""Small text helpers."""

import re


def slugify(title):
    """Return a URL slug: lower-case words of letters and digits, joined by single hyphens.

    slugify("Hello, World!") == "hello-world"
    """
    words = re.split(r"\\s+", title.strip())
    return "-".join(words)
'''

TEST_PY = '''import unittest

from text import slugify


class SlugifyTest(unittest.TestCase):
    def test_punctuation_and_case(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_many_spaces(self):
        self.assertEqual(slugify("  Many   spaces here "), "many-spaces-here")

    def test_symbols_between_words(self):
        self.assertEqual(slugify("C++ & Rust: 2 tips"), "c-rust-2-tips")

    def test_already_a_slug(self):
        self.assertEqual(slugify("already-a-slug"), "already-a-slug")


if __name__ == "__main__":
    unittest.main()
'''

# Words that must never reach the public repository, and token-like patterns.
FORBIDDEN = [w for w in os.environ.get("T1_FORBIDDEN", "").split(",") if w]
TOKEN_PATTERNS = [
    re.compile(r"sk-ant-[A-Za-z0-9_\-]{10,}"),
    re.compile(r"sk-[A-Za-z0-9]{20,}"),
    re.compile(r"eyJ[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]{20,}"),
    re.compile(r"gh[pousr]_[A-Za-z0-9]{20,}"),
]


def sh(args, cwd=None, env=None, check=True, timeout=120):
    r = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    if check and r.returncode != 0:
        raise SystemExit(f"command failed ({r.returncode}): {args[0]} {args[1] if len(args) > 1 else ''}\n{r.stderr}")
    return r


# ---------------------------------------------------------------- user config proof


def user_config_files():
    files = [HOME / ".claude.json", HOME / ".claude" / "CLAUDE.md", HOME / ".codex" / "config.toml"]
    files += sorted((HOME / ".claude").glob("settings*.json"))
    oc = HOME / ".config" / "opencode"
    if oc.is_dir():
        files += sorted(p for p in oc.rglob("*") if p.is_file())
    return files


def snapshot(out):
    lines = []
    for f in user_config_files():
        rel = "~/" + str(f.relative_to(HOME))
        if f.is_file():
            lines.append(f"{hashlib.sha256(f.read_bytes()).hexdigest()}  {rel}")
        else:
            lines.append(f"missing  {rel}")
    cj = HOME / ".claude.json"
    if cj.is_file():
        try:
            d = json.loads(cj.read_text())
            lines.append("# ~/.claude.json top-level keys: " + ",".join(sorted(d)))
            projects = sorted(d.get("projects", {}))
            lines.append(f"# ~/.claude.json projects: {len(projects)}")
            scratch = os.environ.get("T1_SCRATCH", "")
            under = [p for p in projects if scratch and p.startswith(scratch)]
            lines.append(f"# ~/.claude.json projects under the scratch dir: {len(under)}")
        except (ValueError, OSError) as e:
            lines.append(f"# ~/.claude.json unreadable: {e}")
    Path(out).write_text("\n".join(lines) + "\n")


# ---------------------------------------------------------------- the throwaway repo


def make_repo(repo):
    repo.mkdir(parents=True)
    (repo / "text.py").write_text(TEXT_PY)
    (repo / "test_text.py").write_text(TEST_PY)
    (repo / "README.md").write_text("# text helpers\n\nRun the tests with `python3 -m unittest`.\n")
    sh(["git", "init", "-q", "-b", "main"], cwd=repo)
    sh(["git", "config", "user.name", "trial"], cwd=repo)
    sh(["git", "config", "user.email", "trial@example.com"], cwd=repo)
    sh(["git", "add", "text.py", "test_text.py", "README.md"], cwd=repo)
    sh(["git", "commit", "-q", "-m", "text helpers with a slugify bug"], cwd=repo)


# ---------------------------------------------------------------- harness commands


def parse_codex_config(repo):
    """Reads the [mcp_servers.treeyard] entry that connect wrote (a small subset of TOML)."""
    text = (repo / ".codex" / "config.toml").read_text()
    cmd = re.search(r'^command = "(.*)"$', text, re.M).group(1)
    args = json.loads(re.search(r"^args = (\[.*\])$", text, re.M).group(1))
    url = re.search(r'^TREEYARD_URL = "(.*)"$', text, re.M).group(1)
    return cmd, args, url


def claude_token():
    """The OAuth access token of the installed Claude Code, read from the Keychain into memory only."""
    r = subprocess.run(
        ["security", "find-generic-password", "-s", "Claude Code-credentials", "-w"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        return None, f"security find-generic-password exit {r.returncode}"
    try:
        tok = json.loads(r.stdout)["claudeAiOauth"]["accessToken"]
    except (ValueError, KeyError, TypeError):
        return None, "keychain item has no claudeAiOauth.accessToken"
    return tok, None


def build(harness, run_dir, repo, model):
    env = dict(os.environ)
    env.pop("CLAUDECODE", None)
    env.pop("CLAUDE_CODE_ENTRYPOINT", None)
    if harness == "claude":
        cfg = run_dir / "claudecfg"
        cfg.mkdir()
        tok, err = claude_token()
        if tok is None:
            return None, None, err
        env["CLAUDE_CONFIG_DIR"] = str(cfg)
        env["CLAUDE_CODE_OAUTH_TOKEN"] = tok
        env["DISABLE_AUTOUPDATER"] = "1"
        env["DISABLE_TELEMETRY"] = "1"
        cmd = [
            "claude", "-p", PROMPT,
            "--mcp-config", ".mcp.json", "--strict-mcp-config",
            "--max-turns", str(CLAUDE_MAX_TURNS),
            "--output-format", "stream-json", "--verbose",
            "--permission-mode", "acceptEdits",
            "--allowedTools", "mcp__treeyard__*,Bash,Read,Edit,Write",
        ]
        if model:
            cmd += ["--model", model]
        shown = [c if c != PROMPT else "<prompt>" for c in cmd]
        return cmd, env, shown
    if harness == "codex":
        mcmd, margs, url = parse_codex_config(repo)
        cmd = [
            "codex", "exec", "--json", "--ephemeral", "--skip-git-repo-check", "--ignore-user-config",
            "-s", "workspace-write", "-C", str(repo),
            "-c", "approval_policy=\"never\"",
            "-c", f"mcp_servers.treeyard.command={json.dumps(mcmd)}",
            "-c", f"mcp_servers.treeyard.args={json.dumps(margs)}",
            "-c", f"mcp_servers.treeyard.env={{TREEYARD_URL={json.dumps(url)}}}",
            # exec has no one to ask: without this, codex refuses each MCP call ("requires approval").
            "-c", "mcp_servers.treeyard.default_tools_approval_mode=\"approve\"",
        ]
        if model:
            cmd += ["-m", model]
        cmd.append(PROMPT)
        shown = [c if c != PROMPT else "<prompt>" for c in cmd]
        return cmd, env, shown
    if harness == "opencode":
        x = run_dir / "xdg"
        for d in ("config", "data", "cache", "state"):
            (x / d).mkdir(parents=True, exist_ok=True)
        (x / "config" / "opencode").mkdir(exist_ok=True)
        (x / "config" / "opencode" / "opencode.json").write_text(
            json.dumps({"$schema": "https://opencode.ai/config.json", "agent": {"build": {"steps": OPENCODE_STEPS}}}, indent=2)
        )
        env.update(
            XDG_CONFIG_HOME=str(x / "config"),
            XDG_DATA_HOME=str(x / "data"),
            XDG_CACHE_HOME=str(x / "cache"),
            XDG_STATE_HOME=str(x / "state"),
        )
        cmd = ["opencode", "run", "--standalone", "--format", "json", "--auto", "--model", model or "opencode/glm-5.3-flash-free", PROMPT]
        shown = [c if c != PROMPT else "<prompt>" for c in cmd]
        return cmd, env, shown
    if harness == "stub":
        # Free plumbing check: a script makes the protocol calls that a real agent should make.
        fixed = TEXT_PY.replace('words = re.split(r"\\s+", title.strip())', 'words = re.findall(r"[a-z0-9]+", title.lower())')
        url = json.loads(sh([str(BIN), "connect", "status", "--json"], cwd=repo).stdout)["url"]
        script = (
            "import json, subprocess, sys\n"
            "def call(op, a):\n"
            f"    return json.loads(subprocess.run([{str(BIN)!r}, 'call', '--url', {url!r}, op, json.dumps(a)], capture_output=True, text=True, check=True).stdout)\n"
            "ch = call('open_change', {'agent': 'stub-1', 'intent': 'fix slugify', 'paths': ['text.py']})['id']\n"
            f"print(json.dumps(call('apply', {{'change': ch, 'edits': [{{'path': 'text.py', 'content': {fixed!r}}}]}})))\n"
            "print(json.dumps(call('land', {'change': ch})))\n"
        )
        return [sys.executable, "-c", script], env, ["python3", "-c", "<stub protocol script>"]
    raise SystemExit(f"unknown harness {harness}")


def run_agent(harness, cmd, env, repo, out_path, err_path):
    """Runs the agent in its own process group. Kills the group on timeout or too many tool events."""
    t0 = time.time()
    with open(out_path, "w") as out, open(err_path, "w") as err:
        # OpenCode takes its project from $PWD, not from the process cwd: set both.
        env = dict(env, PWD=str(repo))
        p = subprocess.Popen(cmd, cwd=repo, env=env, stdout=subprocess.PIPE, stderr=err, stdin=subprocess.DEVNULL, text=True, start_new_session=True)
        state = {"tool_events": 0, "killed": None}

        def kill(why):
            if state["killed"] is None:
                state["killed"] = why
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

        timer = threading.Timer(TIMEOUT_S, kill, args=(f"timeout {TIMEOUT_S} s",))
        timer.start()
        for line in p.stdout:
            out.write(line)
            out.flush()
            if harness == "codex":
                try:
                    ev = json.loads(line)
                except ValueError:
                    continue
                item = ev.get("item") or {}
                if ev.get("type") == "item.started" and item.get("type") in ("mcp_tool_call", "command_execution", "file_change"):
                    state["tool_events"] += 1
                    if state["tool_events"] > CODEX_MAX_TOOL_EVENTS:
                        kill(f"more than {CODEX_MAX_TOOL_EVENTS} tool-call events")
        rc = p.wait()
        timer.cancel()
    return {"exit": rc, "killed": state["killed"], "wall_s": round(time.time() - t0, 1), "codex_tool_events": state["tool_events"] if harness == "codex" else None}


# ---------------------------------------------------------------- evidence


def tool_calls(harness, transcript):
    """Counts tool names that the agent called, from its own transcript."""
    names = {}
    for line in transcript.read_text(errors="replace").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        found = []
        if harness == "claude" and ev.get("type") == "assistant":
            for c in ev.get("message", {}).get("content", []):
                if c.get("type") == "tool_use":
                    found.append(c.get("name"))
        elif harness == "codex" and ev.get("type") == "item.started":
            item = ev.get("item") or {}
            t = item.get("type")
            if t == "mcp_tool_call":
                found.append(f"mcp:{item.get('server')}:{item.get('tool')}")
            elif t:
                found.append(t)
        elif harness == "opencode" and ev.get("type") == "tool_use":
            part = ev.get("part") or {}
            found.append(part.get("tool"))
            # OpenCode can wrap MCP calls in code ("execute"): count each tools.treeyard.<op>(...) in it.
            code = str(((part.get("state") or {}).get("input") or {}).get("code", ""))
            found += [f"mcp:treeyard:{op}" for op in re.findall(r"tools\.treeyard\.(\w+)\(", code)]
            if part.get("tool", "").startswith("treeyard_"):
                found.append("mcp:treeyard:" + part["tool"][len("treeyard_"):])
        for n in found:
            names[n] = names.get(n, 0) + 1
    return names


def verdict(audit_rows, test_ok, disk):
    rows = sorted(audit_rows, key=lambda r: r["seq"])
    seq = [(r.get("op"), r.get("status")) for r in rows]
    i_open = next((i for i, r in enumerate(rows) if r.get("op") == "open_change" and r.get("status") == "ok"), None)
    applies = [i for i, r in enumerate(rows) if r.get("op") == "apply" and r.get("status") == "ok" and (i_open is not None and i > i_open)]
    i_land = next((i for i, r in enumerate(rows) if r.get("op") == "land" and r.get("status") == "landed" and applies and i > applies[0]), None)
    protocol = i_open is not None and bool(applies) and i_land is not None
    bypass = disk["text_py"] == "edited, differs from main" or disk["text_py"] == "edited, equals main, not pulled"
    passed = protocol and test_ok and not bypass
    return {"pass": passed, "protocol": protocol, "test_on_main": test_ok, "bypass_on_disk": bypass, "audit_sequence": seq}


def disk_state(repo, main_clone):
    st = sh(["git", "status", "--porcelain", "--untracked-files=all"], cwd=repo, check=False).stdout
    head = sh(["git", "log", "--oneline", "-n", "5"], cwd=repo, check=False).stdout
    disk = (repo / "text.py").read_text() if (repo / "text.py").exists() else None
    main = (main_clone / "text.py").read_text() if (main_clone / "text.py").exists() else None
    if disk == TEXT_PY:
        kind = "unchanged"
    elif disk == main and sh(["git", "diff", "--quiet", "HEAD", "--", "text.py"], cwd=repo, check=False).returncode == 0:
        kind = "equals main, pulled into HEAD"
    elif disk == main:
        kind = "edited, equals main, not pulled"
    else:
        kind = "edited, differs from main"
    return {"git_status": st, "git_log": head, "text_py": kind}


PS = re.compile(r"(^|[;&|(]\s*)ps\s")
DROPPED = "<output removed: process list of the host>"


def drop_process_lists(text):
    """Removes the output of `ps` calls from a JSONL transcript: it lists other jobs on the host."""
    out, ids = [], set()
    for line in text.splitlines(keepends=True):
        try:
            ev = json.loads(line)
        except ValueError:
            ev = None
        if not isinstance(ev, dict):
            out.append(line)
            continue
        hit = False
        item = ev.get("item") or {}
        if PS.search(str(item.get("command", ""))) and "aggregated_output" in item:
            item["aggregated_output"] = DROPPED
            hit = True
        state = (ev.get("part") or {}).get("state") or {}
        if PS.search(str((state.get("input") or {}).get("command", ""))):
            state["output"] = DROPPED
            if isinstance(state.get("metadata"), dict):
                state["metadata"].pop("output", None)
            hit = True
        for c in (ev.get("message") or {}).get("content") or []:
            if not isinstance(c, dict):
                continue
            if c.get("type") == "tool_use" and PS.search(str((c.get("input") or {}).get("command", ""))):
                ids.add(c.get("id"))
            if c.get("type") == "tool_result" and c.get("tool_use_id") in ids:
                c["content"] = DROPPED
                hit = True
        if hit and isinstance(ev.get("tool_use_result"), (dict, str)):
            ev["tool_use_result"] = DROPPED
        out.append(json.dumps(ev) + "\n" if hit else line)
    return "".join(out)


def sanitize(text, scratch):
    text = drop_process_lists(text)
    text = text.replace(str(scratch), "<scratch>").replace(str(ROOT), "<worktree>").replace(str(HOME), "<home>")
    for w in FORBIDDEN:
        text = re.sub(re.escape(w), "<redacted>", text, flags=re.I)
    for pat in TOKEN_PATTERNS:
        text = pat.sub("<token>", text)
    return text


def leaks(text):
    bad = [w for w in FORBIDDEN if w.lower() in text.lower()]
    bad += [p.pattern for p in TOKEN_PATTERNS if p.search(text)]
    if re.search(r"/Users/[A-Za-z0-9]", text):
        bad.append("a /Users/<name> path")
    return bad


# ---------------------------------------------------------------- one run


def run(harness, attempt, scratch, model, block):
    if "treeyard" in PROMPT.lower():
        raise SystemExit("the prompt names TreeYard; refused")
    if not FORBIDDEN:
        raise SystemExit("set T1_FORBIDDEN (comma list of words that must not reach the repo)")
    scratch = Path(scratch).resolve()
    os.environ["T1_SCRATCH"] = str(scratch)
    name = f"{harness}-{attempt}"
    run_dir = scratch / "t1" / "runs" / name
    if run_dir.exists():
        raise SystemExit(f"{run_dir} exists; pick a new attempt id")
    repo = run_dir / "repo"
    make_repo(repo)
    snapshot(run_dir / "userconfig-before.txt")

    c = sh([str(BIN), "connect", "--harness", "claude" if harness == "stub" else harness, "--agents", "1", "--no-mirror"], cwd=repo)
    (run_dir / "connect.txt").write_text(c.stdout + c.stderr)
    status = json.loads(sh([str(BIN), "connect", "status", "--json"], cwd=repo).stdout)
    url = status["url"]
    if url.endswith(":7420"):
        raise SystemExit("connect picked port 7420; refused")
    block_now = "".join((repo / f).read_text() for f in ("AGENTS.md", "CLAUDE.md") if (repo / f).exists())

    result = {"harness": harness, "attempt": attempt, "block": block, "model": model, "prompt": PROMPT}
    cmd, env, shown = build(harness, run_dir, repo, model)
    if cmd is None:
        result.update(blocked=True, error=shown)
    else:
        result["command"] = shown
        result["limits"] = {"timeout_s": TIMEOUT_S, "claude_max_turns": CLAUDE_MAX_TURNS, "codex_max_tool_events": CODEX_MAX_TOOL_EVENTS, "opencode_steps": OPENCODE_STEPS}
        result["run"] = run_agent(harness, cmd, env, repo, run_dir / "transcript.jsonl", run_dir / "stderr.txt")
        result["tool_calls"] = tool_calls(harness, run_dir / "transcript.jsonl")

    audit = json.loads(sh([str(BIN), "call", "--url", url, "audit", '{"limit": 1000}']).stdout)
    events = json.loads(sh([str(BIN), "call", "--url", url, "events", '{"since": 0, "limit": 5000}']).stdout)
    (run_dir / "audit.json").write_text(json.dumps(audit, indent=2))
    (run_dir / "events.json").write_text(json.dumps(events, indent=2))
    rows = audit["entries"]

    clone = run_dir / "main-clone"
    sh(["git", "clone", "-q", f"{url}/git", str(clone)])
    t = sh([sys.executable, "-m", "unittest"], cwd=clone, check=False)
    (run_dir / "test-main.txt").write_text(f"exit {t.returncode}\n{t.stdout}{t.stderr}")
    disk = disk_state(repo, clone)
    (run_dir / "repo-status.txt").write_text(f"text.py on disk: {disk['text_py']}\n--- git status --porcelain\n{disk['git_status']}--- git log\n{disk['git_log']}")
    result["agents_in_audit"] = sorted({r.get("agent") or "" for r in rows})
    result["verdict"] = verdict(rows, t.returncode == 0, disk)
    result["disk"] = disk["text_py"]
    (run_dir / "result.json").write_text(json.dumps(result, indent=2))

    d = sh([str(BIN), "disconnect", "--purge"], cwd=repo, check=False)
    (run_dir / "disconnect.txt").write_text(d.stdout + d.stderr)
    snapshot(run_dir / "userconfig-after.txt")
    (run_dir / "block.md").write_text(block_now)

    copy_out(scratch, name)
    v = result["verdict"]
    print(json.dumps({"run": name, "pass": v["pass"], "protocol": v["protocol"], "test_on_main": v["test_on_main"], "disk": result["disk"], "audit": v["audit_sequence"], "run_info": result.get("run"), "tools": result.get("tool_calls"), "blocked": result.get("blocked")}, indent=1))


def copy_out(scratch, name):
    """Copies the files of one run, sanitised and leak-checked, to docs/trials/t1/NAME/."""
    run_dir = scratch / "t1" / "runs" / name
    dest = HERE / name
    dest.mkdir(exist_ok=True)
    problems = {}
    for f in ["result.json", "transcript.jsonl", "stderr.txt", "audit.json", "events.json", "test-main.txt", "repo-status.txt", "connect.txt", "disconnect.txt", "userconfig-before.txt", "userconfig-after.txt", "block.md"]:
        src = run_dir / f
        if not src.exists():
            continue
        text = sanitize(src.read_text(errors="replace"), scratch)
        bad = leaks(text)
        if bad:
            problems[f] = bad
            continue
        (dest / f).write_text(text)
    if problems:
        print(f"NOT COPIED (leak check): {problems}", file=sys.stderr)


def mcp_ops(harness, transcript):
    """The TreeYard operations that the agent called through MCP tools (not through a shell)."""
    if not transcript.exists():
        return []
    names = tool_calls(harness, transcript)
    ops = set()
    for n in names:
        if n.startswith("mcp__treeyard__"):
            ops.add(n[len("mcp__treeyard__"):])
        elif n.startswith("mcp:treeyard:"):
            ops.add(n[len("mcp:treeyard:"):])
    return sorted(ops)


def steps(harness, transcript):
    """One short line per tool call, in order: TreeYard MCP calls as ty:<op>, shell calls as sh:<command>."""
    out = []
    for line in transcript.read_text(errors="replace").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if not isinstance(ev, dict):
            continue
        if harness == "claude" and ev.get("type") == "assistant":
            for c in ev.get("message", {}).get("content", []):
                if c.get("type") == "tool_use":
                    n = c["name"].replace("mcp__treeyard__", "ty:")
                    out.append("sh:" + c["input"].get("command", "") if n == "Bash" else n)
        elif harness == "codex" and ev.get("type") == "item.completed":
            it = ev.get("item") or {}
            if it.get("type") == "mcp_tool_call":
                out.append(f"ty:{it.get('tool')} ({it.get('status')})")
            elif it.get("type") == "command_execution":
                out.append("sh:" + it.get("command", ""))
        elif harness == "opencode" and ev.get("type") == "tool_use":
            part = ev.get("part") or {}
            inp = (part.get("state") or {}).get("input") or {}
            if part.get("tool") == "execute":
                out.append("ty:" + ",".join(re.findall(r"tools\.treeyard\.(\w+)\(", str(inp.get("code", "")))) + " (in execute)")
            elif part.get("tool") == "shell":
                out.append("sh:" + inp.get("command", ""))
            else:
                out.append(str(part.get("tool")))
    return [" ".join(x.split())[:110] for x in out]


def sequences():
    for d in sorted(HERE.glob("*/transcript.jsonl")):
        h = d.parent.name.split("-")[0]
        if h == "stub":
            continue
        print(f"== {d.parent.name}")
        for x in steps(h, d):
            print("   " + x)


def summary():
    """Prints one table row per saved run, from its result.json (the numbers in README.md)."""
    print("| run | block | model | exit | killed | wall s | audit ops | via MCP tools | test on main | text.py on disk | pass |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for d in sorted(HERE.glob("*/result.json")):
        r = json.loads(d.read_text())
        if r["harness"] == "stub":
            continue
        run_info = r.get("run") or {}
        v = r["verdict"]
        ops = " -> ".join(f"{o}:{s}" for o, s in v["audit_sequence"]) or "(none)"
        via = mcp_ops(r["harness"], d.parent / "transcript.jsonl")
        mcp = all(op in via for op in ("open_change", "apply", "land"))
        ok = v["pass"] and mcp
        print(
            f"| {d.parent.name} | {r.get('block')} | {r.get('model') or 'default'} | {run_info.get('exit')} | {run_info.get('killed') or 'no'} "
            f"| {run_info.get('wall_s')} | {ops} | {'yes' if mcp else 'no'} | {'green' if v['test_on_main'] else 'red'} | {r['disk']} | {'yes' if ok else 'no'} |"
        )


def main():
    a = sys.argv[1:]
    if a == ["summary"]:
        return summary()
    if a == ["sequences"]:
        return sequences()
    if a[:1] == ["snapshot"] and len(a) == 2:
        return snapshot(a[1])
    if a[:1] == ["rescrub"] and len(a) == 3:
        return copy_out(Path(a[2]).resolve(), a[1])
    if a[:1] == ["run"] and len(a) >= 4:
        model = block = None
        rest = a[4:]
        while rest:
            k = rest.pop(0)
            if k == "--model":
                model = rest.pop(0)
            elif k == "--block":
                block = rest.pop(0)
            else:
                raise SystemExit(f"unknown flag {k}")
        return run(a[1], a[2], a[3], model, block)
    raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
