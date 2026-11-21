"""Stop tests for the T2 runner: scripted stub agents, a tiny repository, no model, no network.

One test per stop: turn limit, token limit, wall-clock timeout (per agent), and the anomalies that
end the whole run: wrong agent count, server down, zero tool calls. `make bench-test` runs them.
"""

import http.server
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

from . import t2 as k


def mini_world(root):
    """A 5-line repository and 5 tasks (one line each), in the layout of bench/t2/tasks."""
    src = root / "src" / "mini"
    src.mkdir(parents=True)
    (src / "a.txt").write_text("".join(f"line {i}\n" for i in range(1, 6)))
    k.git("init", "-q", "-b", "main", cwd=src)
    k.git("add", "a.txt", cwd=src)
    k.git("commit", "-q", "-m", "mini", cwd=src)
    rev = k.git("rev-parse", "HEAD", cwd=src).stdout.strip()
    tasks = root / "tasks"
    for i in range(1, 6):
        d = tasks / "mini" / "colliding" / f"{i}-t{i}"
        (d / "hidden").mkdir(parents=True)
        (d / "prompt.md").write_text(f"# task {i}\n\nChange line {i}.\n")
        (d / "hidden" / f"test_{i}.txt").write_text("x\n")
        (d / "reference.patch").write_text("")
        (d / "meta.json").write_text(json.dumps({"repo": "mini", "set": "colliding", "k": i, "id": f"t{i}", "title": f"task {i}",
                                                 "files": ["a.txt"], "test": f"test_{i}.txt"}))
    spec = {"rev": rev, "local_src": str(src), "workdir": ".", "decide": [sys.executable, "-c", "pass"], "suite": [sys.executable, "-c", "pass"]}
    return spec, tasks


def oracle_world(root):
    """Like `mini_world`, with real reference patches: task i sets line i to `task i`. Neighbouring lines
    conflict in a git merge, so the colliding order needs the hand-back or `t2-integrate`."""
    spec, tasks = mini_world(root)
    base = {"a.txt": "".join(f"line {i}\n" for i in range(1, 6))}
    defs = [{"id": f"t{i}", "edits": [{"op": "replace", "file": "a.txt", "old": f"line {i}\n", "new": f"task {i}\n"}]} for i in range(1, 6)]
    for i in range(1, 6):
        d = tasks / "mini" / "colliding" / f"{i}-t{i}"
        (d / "reference.patch").write_text(k.unified(base, k.apply_edits(base, [defs[i - 1]])))
        (d / f"resolved-{i}.patch").write_text(k.unified(k.apply_edits(base, defs[: i - 1]), k.apply_edits(base, defs[:i])))
        (d / "hidden" / f"test_{i}.txt").write_text(f"{i}\n")
    spec["decide"] = [sys.executable, "-c", "import sys; n = int(open(sys.argv[1]).read()); "
                      "sys.exit(open('a.txt').read().splitlines()[n - 1] != 'task %d' % n)", "{test}"]
    return spec, tasks


class Health(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *a):
        pass


class StubBase(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="t2-stop-"))
        self.spec, self.tasks = mini_world(self.tmp)
        self.old_env = os.environ.get("T2_STUB_MODES")
        self.old_every = k.HEALTH_EVERY_S
        k.HEALTH_EVERY_S = 0.1

    def tearDown(self):
        k.HEALTH_EVERY_S = self.old_every
        os.environ.pop("T2_STUB_AS", None)
        if self.old_env is None:
            os.environ.pop("T2_STUB_MODES", None)
        else:
            os.environ["T2_STUB_MODES"] = self.old_env
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_stub(self, modes, arm="A", limits=None, agents=None, health_url=None, dialects=""):
        os.environ["T2_STUB_MODES"] = modes
        os.environ["T2_STUB_AS"] = dialects
        r = k.Run(self.tmp, arm, "mini", "colliding", 1, "stub", limits=limits or {"max_turns": 50, "max_tokens": 10**9, "timeout_s": 60},
                  agents=agents, tasks_dir=self.tasks, repo_spec=self.spec, run_dir=self.tmp / "run", health_url=health_url,
                  results=self.tmp / "results")
        t0 = time.time()
        res = r.execute()
        res["_elapsed"] = time.time() - t0
        for p in r.registry.values():
            self.assertIsNotNone(p.poll(), "an agent process is still alive after the run")
        return res


class StopTests(StubBase):
    def test_turn_limit_kills_the_agent(self):
        res = self.run_stub("loop-turns,ok,ok,ok,ok", limits={"max_turns": 5, "max_tokens": 10**9, "timeout_s": 60})
        self.assertEqual(res["status"], "done")
        self.assertEqual(res["limit_kills"], ["agent-1: turns > 5"])
        self.assertEqual(res["agent_stats"][0]["turns"], 6)

    def test_token_limit_kills_the_agent(self):
        res = self.run_stub("tokens,ok,ok,ok,ok", limits={"max_turns": 10**6, "max_tokens": 5000, "timeout_s": 60})
        self.assertEqual(res["limit_kills"], ["agent-1: tokens > 5000"])
        s = res["agent_stats"][0]
        self.assertGreater(s["tokens_in"] + s["tokens_out"], 5000)
        self.assertLessEqual(s["tokens_in"] + s["tokens_out"], 5000 + 1100)

    def test_timeout_kills_the_agent(self):
        res = self.run_stub("sleep,ok,ok,ok,ok", limits={"max_turns": 50, "max_tokens": 10**9, "timeout_s": 1})
        self.assertEqual(res["limit_kills"], ["agent-1: timeout 1 s"])
        self.assertLess(res["_elapsed"], 30)

    def test_wrong_agent_count_stops_the_run(self):
        res = self.run_stub("ok,ok,ok,ok,ok", agents=4)
        self.assertEqual(res["status"], "stopped")
        self.assertEqual(res["stop_reason"], "agent count 4, want 5")
        self.assertEqual(res["tool_calls"], 0, "no agent may start")

    def test_zero_tool_calls_stops_the_run(self):
        os.environ["T2_STUB_SLOW_S"] = "20"
        try:
            res = self.run_stub("no-tools,slow-ok,slow-ok,slow-ok,slow-ok")
        finally:
            os.environ.pop("T2_STUB_SLOW_S", None)
        self.assertEqual(res["status"], "stopped")
        self.assertEqual(res["stop_reason"], "zero tool calls: agent-1 (claude-1) made no tool call")
        self.assertLess(res["_elapsed"], 15, "the other agents must be killed, not waited for")

    def test_server_down_stops_the_run(self):
        srv = http.server.HTTPServer(("127.0.0.1", 0), Health)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{srv.server_address[1]}/health"

        def down():
            time.sleep(0.8)
            srv.shutdown()
            srv.server_close()

        threading.Thread(target=down, daemon=True).start()
        os.environ["T2_STUB_SLOW_S"] = "20"
        try:
            res = self.run_stub("slow-ok,slow-ok,slow-ok,slow-ok,slow-ok", health_url=url)
        finally:
            os.environ.pop("T2_STUB_SLOW_S", None)
        self.assertEqual(res["status"], "stopped")
        self.assertEqual(res["stop_reason"], f"server down: {url} does not answer")
        self.assertLess(res["_elapsed"], 15)

    def test_failed_hand_back_counts_an_intervention(self):
        res = self.run_stub("edit-conflict,edit-conflict,edit-conflict,edit-conflict,edit-conflict")
        self.assertEqual(res["status"], "done")
        self.assertEqual(res["merge_conflicts"], 4)
        self.assertEqual(res["interventions"], 4)
        self.assertEqual([m["handback"] for m in res["merges"]], [None, "failed", "failed", "failed", "failed"])
        self.assertEqual([m["needs_human"] for m in res["merges"]], [False, True, True, True, True])
        # nothing is resolved by taking a side: the 4 changes stay out of main
        self.assertEqual((self.tmp / "run" / "final" / "a.txt").read_text().splitlines()[0], "agent 1")

    @unittest.skipUnless(k.treeyard_bin().exists(), "needs target/debug/treeyard")
    def test_server_down_stops_the_run_arm_b(self):
        """The same stop with the real `treeyard connect` server of arm B: its serve process dies mid-run."""

        def kill_serve():
            state = self.tmp / "run" / "repo" / ".git" / "treeyard" / "connect.json"
            for _ in range(300):
                if (self.tmp / "run" / "agent-1.jsonl").exists() and state.exists():
                    break
                time.sleep(0.05)
            time.sleep(0.5)
            os.kill(json.loads(state.read_text())["pid"], signal.SIGKILL)

        threading.Thread(target=kill_serve, daemon=True).start()
        os.environ["T2_STUB_SLOW_S"] = "20"
        try:
            res = self.run_stub("slow-ok,slow-ok,slow-ok,slow-ok,slow-ok", arm="B")
        finally:
            os.environ.pop("T2_STUB_SLOW_S", None)
        self.assertEqual(res["status"], "stopped")
        self.assertTrue(res["stop_reason"].startswith("server down: http://127.0.0.1:"), res["stop_reason"])
        self.assertNotIn(":7420/", res["stop_reason"])
        self.assertLess(res["_elapsed"], 15)


HAS_TREEYARD = k.treeyard_bin().exists()


class HandbackTests(StubBase):
    """The hand-back in every arm with stub agents that settle the markers, leave them, or do nothing,
    in the transcript dialects of both real harnesses (`T2_STUB_AS`)."""

    def final(self):
        return (self.tmp / "run" / "final" / "a.txt").read_text()

    def assert_all_kept(self, res, key):
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["interventions"], 0)
        self.assertEqual([m["handback"] for m in res[key]], [None, "resolved", "resolved", "resolved", "resolved"])
        text = self.final()
        self.assertFalse(k.has_marker_text(text), text)
        for i in range(1, 6):
            self.assertIn(f"agent {i}\n", text)

    def assert_left_out(self, res, key):
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["interventions"], 4)
        self.assertEqual([m["needs_human"] for m in res[key]], [False, True, True, True, True])
        text = self.final()
        self.assertFalse(k.has_marker_text(text), text)
        self.assertTrue(text.startswith("agent 1\n"), text)
        self.assertNotIn("agent 2", text)

    def test_arm_a_claude_dialect_resolves(self):
        res = self.run_stub(",".join(["conflict-resolve"] * 5), dialects=",".join(["claude"] * 5))
        self.assertEqual(res["merge_conflicts"], 4)
        self.assert_all_kept(res, "merges")
        self.assertEqual(res["merges"][1]["handback_files"], ["a.txt"])
        self.assertEqual(res["agent_stats"][1]["invocations"], 2)
        self.assertEqual(res["agent_stats"][1]["turns"], 2)  # num_turns of both results

    def test_arm_a_codex_dialect_leaves_markers(self):
        res = self.run_stub(",".join(["conflict-markers"] * 5), dialects=",".join(["codex"] * 5))
        self.assert_left_out(res, "merges")
        self.assertIn("markers", res["merges"][1]["handback_failure"])

    def test_hand_back_files_carry_the_same_markers_in_both_arms(self):
        arms = ["A"] + (["B"] if HAS_TREEYARD else [])
        for arm in arms:
            res = self.run_stub("edit-conflict,conflict-markers,ok,ok,ok", arm=arm)
            self.assertEqual(res["interventions"], 1, arm)
            notes = [json.loads(x) for x in (self.tmp / "run" / "agent-2.jsonl").read_text().splitlines() if '"handback_file"' in x]
            self.assertEqual(len(notes), 1, arm)
            text = notes[0]["handback_file"]
            self.assertIn("<<<<<<< main\nagent 1\n=======\nagent 2\n>>>>>>> change\n", text, arm)

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_arm_b_claude_dialect_resolves(self):
        res = self.run_stub(",".join(["conflict-resolve"] * 5), arm="B", dialects=",".join(["claude"] * 5))
        self.assert_all_kept(res, "lands")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_arm_b_codex_dialect_leaves_markers(self):
        res = self.run_stub(",".join(["conflict-markers"] * 5), arm="B", dialects=",".join(["codex"] * 5))
        self.assert_left_out(res, "lands")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_arm_b_do_nothing_is_left_out(self):
        res = self.run_stub(",".join(["edit-conflict"] * 5), arm="B")
        self.assert_left_out(res, "lands")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_arm_b_mcp_change_resolves_on_disk(self):
        res = self.run_stub(",".join(["mcp-conflict"] * 5), arm="B", dialects="codex,claude,codex,claude,codex")
        self.assert_all_kept(res, "lands")
        # the workspace files went back after the hand-back of a change that is not the workspace one
        for a in range(2, 6):
            ws = [p for p in (self.tmp / "t2").rglob(f"ws/agent-{a}/a.txt")]
            for p in ws:
                self.assertFalse(k.has_marker_text(p.read_text()))


class ArmBUndoesTests(StubBase):
    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_a_held_land_needs_a_person_and_the_runner_never_overrides(self):
        res = self.run_stub("land-line,undo-line,ok,ok,ok", arm="B")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["interventions"], 1)
        self.assertEqual(res["undoes_held"], 1)
        l2 = res["lands"][1]
        self.assertEqual((l2["needs_human"], l2["intervention"], l2["handback"]), (True, True, None))
        self.assertEqual(len(l2["left_out"]), 1)
        held = l2["undoes_held"][0]
        self.assertEqual(held["change"], l2["left_out"][0])
        self.assertEqual([e["text"] for e in held["undoes"]], ["kept by agent 1\n"])
        self.assertTrue(held["undoes"][0]["agent"] and held["undoes"][0]["change"].startswith("ch_"), held)
        self.assertIn({"change": held["change"], "workspace": False, "status": "undoes", "undoes": held["undoes"]}, l2["runner_landed"])
        self.assertEqual([x["needs_human"] for x in res["lands"]], [False, True, False, False, False])
        # agent 1's line stays on main; nothing was landed with an override
        self.assertIn("kept by agent 1\n", (self.tmp / "run" / "final" / "a.txt").read_text())
        run = json.loads(next((self.tmp / "results").glob("*.json")).read_text())
        self.assertEqual((run["undoes_held"], run["lands"][1]["undoes_held"]), (1, l2["undoes_held"]), "in the run file")


class ArmA2Tests(StubBase):
    """Arm A2: `t2-integrate` on PATH, answered by the runner; the runner merges only what is left."""

    def run_world(self, harness, arm="A2", modes="ok,ok,ok,ok,ok", dialects=""):
        os.environ["T2_STUB_MODES"] = modes
        os.environ["T2_STUB_AS"] = dialects
        spec, tasks = oracle_world(self.tmp / "w")
        r = k.Run(self.tmp, arm, "mini", "colliding", 1, harness, limits={"max_turns": 50, "max_tokens": 10**9, "timeout_s": 60},
                  tasks_dir=tasks, repo_spec=spec, run_dir=self.tmp / "run", results=self.tmp / "results")
        return r, r.execute()

    def test_oracle_integrates_in_order_with_its_conflicts(self):
        r, res = self.run_world("oracle")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["tasks_on_main"], 5)
        self.assertEqual(res["integrated_by_agent"], 5)
        self.assertEqual(res["merge_conflicts"], 0)  # nothing left for the runner
        self.assertGreaterEqual(res["integrate_conflicts"], 1)
        self.assertEqual(res["interventions"], 0)
        self.assertEqual((self.tmp / "run" / "final" / "a.txt").read_text(), "".join(f"task {i}\n" for i in range(1, 6)))
        # Every agent integrated itself, so the diff against main is empty at the end; the guard
        # counts what each integration put on main.
        self.assert_guard(res)

    def assert_guard(self, res):
        one = {"files": 1, "added": 1, "removed": 1, "test_added": 0, "paths": ["a.txt"]}
        self.assertEqual(res["guard"], {str(i): one for i in range(1, 6)})
        self.assertEqual(res["lost_lines"]["total"], 0)

    def test_oracle_arm_a_uses_the_hand_back(self):
        r, res = self.run_world("oracle", arm="A")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["tasks_on_main"], res["interventions"]), (5, 0))
        self.assertEqual(res["merge_conflicts"], 4)
        self.assertEqual([m["handback"] for m in res["merges"]], [None, "resolved", "resolved", "resolved", "resolved"])
        self.assert_guard(res)

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_oracle_arm_b(self):
        r, res = self.run_world("oracle", arm="B")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["tasks_on_main"], res["interventions"]), (5, 0))
        self.assert_guard(res)

    def test_stub_agents_integrate_and_settle_conflicts(self):
        r, res = self.run_world("stub", modes=",".join(["integrate"] * 5), dialects="claude,codex,claude,codex,stub")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["interventions"], 0)
        text = (self.tmp / "run" / "final" / "a.txt").read_text()
        self.assertFalse(k.has_marker_text(text))
        for i in range(1, 6):
            self.assertIn(f"agent {i}\n", text)
        self.assertTrue(all(x["exit"] in (0, 1) for xs in res["integrations"].values() for x in xs))

    def test_noop_and_a_stopped_merge(self):
        r, res = self.run_world("noop")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["tasks_on_main"], 0)
        self.assertEqual([m["status"] for m in res["merges"]], ["no work"] * 5)

    def test_integrate_reply_and_prompt(self):
        spec, tasks = oracle_world(self.tmp / "w")
        r = k.Run(self.tmp, "A2", "mini", "colliding", 1, "noop", tasks_dir=tasks, repo_spec=spec, run_dir=self.tmp / "run",
                  results=self.tmp / "results")
        r.setup()
        a1, a2 = r.agents[0], r.agents[1]
        self.assertTrue(r.prompt(a1).startswith(k.A2_BLOCK + "\n\n" + k.PREAMBLE + "\n\n# task 1"))
        for a, n in ((a1, 1), (a2, 2)):
            f = Path(a["cwd"]) / "a.txt"
            f.write_text(f.read_text().replace(f"line {n}\n", f"task {n}\n"))
        self.assertEqual(r.integrate(a1)[2], "integrated")
        rc, out, status = r.integrate(a2)
        self.assertEqual((rc, status), (1, "conflict"))
        self.assertIn("<<<<<<< main", (Path(a2["cwd"]) / "a.txt").read_text())
        self.assertEqual(r.integrate(a2)[2], "markers left")
        (Path(a2["cwd"]) / "a.txt").write_text("".join(f"task {i}\n" if i <= 2 else f"line {i}\n" for i in range(1, 6)))
        self.assertEqual(r.integrate(a2)[2], "integrated")
        self.assertEqual(k.git("show", "main:a.txt", cwd=r.repo_dir).stdout.splitlines()[:2], ["task 1", "task 2"])
        # the script on PATH goes through the spool to the runner
        stop = r.start_integrator()
        try:
            env = dict(a1["env"])
            p = subprocess.run([k.INTEGRATE_CMD], cwd=a1["cwd"], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
            self.assertIn("nothing to integrate", p.stdout)
        finally:
            stop.set()
            r.archive()


class LayoutTests(StubBase):
    """Agent area, environment, commit rule, scoring with the pin's test files."""

    def test_reach_check(self):
        kit = self.tmp / "kit"
        (kit / "tasks").mkdir(parents=True)
        bad = k.reach_check(self.tmp / "agents" / "run1", [kit / "tasks"])
        self.assertTrue(any("holds" in b for b in bad), bad)
        ok_root = Path(tempfile.mkdtemp(prefix="t2-agents-"))
        elsewhere = Path.home() / ".t2-test-no-such-kit"  # below the home directory, which the check does not walk
        try:
            self.assertEqual(k.reach_check(ok_root / "run1", [elsewhere]), [])
            (ok_root / "old-run").mkdir()
            self.assertTrue(any("other entries" in b for b in k.reach_check(ok_root / "run1", [elsewhere])))
        finally:
            shutil.rmtree(ok_root)

    def test_box_env_test_command_and_archive(self):
        os.environ["T2_STUB_MODES"] = "ok,ok,ok,ok,ok"
        r = k.Run(self.tmp, "A", "mini", "colliding", 1, "stub", tasks_dir=self.tasks, repo_spec=self.spec, run_dir=self.tmp / "run",
                  results=self.tmp / "results")
        r.setup()
        a = r.agents[0]
        env = a["env"]
        box = Path(a["box"])
        self.assertEqual((env["HOME"], env["TMPDIR"]), (str(box / "home"), str(box / "tmp")))
        self.assertNotIn(str(k.HERE), json.dumps(env))
        self.assertTrue(env["PATH"].startswith(str(box / "bin")))
        self.assertFalse(str(r.run_dir) in a["cwd"], "the agent's directory must not be inside the runner's")
        p = subprocess.run([k.TEST_CMD], cwd=a["cwd"], env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("t2-test: passed", p.stdout)
        r.archive()
        self.assertFalse(r.adir.exists())
        self.assertTrue((r.run_dir / "agents" / "wt" / "agent-1").is_dir())
        self.assertFalse((r.run_dir / "agents" / "box" / "agent-1" / "home").exists())

    def test_commit_rule_keeps_env_and_lock_files_out(self):
        src = self.tmp / "r"
        src.mkdir()
        k.git("init", "-q", "-b", "main", cwd=src)
        (src / "Cargo.lock").write_text("pinned\n")
        (src / "a.py").write_text("x = 1\n")
        k.git("add", ".", cwd=src)
        k.git("commit", "-q", "-m", "pin", cwd=src)
        (src / "Cargo.lock").write_text("changed\n")
        (src / "a.py").write_text("x = 2\n")
        (src / "uv.lock").write_text("junk\n")
        (src / ".venv").mkdir()
        (src / ".venv" / "x").write_text("junk\n")
        (src / "tests").mkdir()
        (src / "tests" / "test_new.py").write_text("def test(): pass\n")
        k.git("add", "uv.lock", cwd=src)  # staged by the agent: still kept out
        keep, skip = k.commit_paths(src, {"Cargo.lock", "a.py"})
        self.assertEqual(sorted(keep), ["Cargo.lock", "a.py", "tests/test_new.py"])
        self.assertEqual(sorted(skip), [".venv/x", "uv.lock"])

    def test_pinned_suite_restores_the_pins_test_files(self):
        src = self.tmp / "src" / "mini"
        (src / "tests").mkdir()
        (src / "tests" / "t.txt").write_text("pin\n")
        k.git("add", ".", cwd=src)
        k.git("commit", "-q", "-m", "tests", cwd=src)
        spec = dict(self.spec, rev=k.git("rev-parse", "HEAD", cwd=src).stdout.strip(), test_globs=["tests/**"],
                    suite=[sys.executable, "-c", "import sys; sys.exit(open('tests/t.txt').read() != 'pin\\n')"])
        r = k.Run(self.tmp, "A", "mini", "colliding", 1, "noop", tasks_dir=self.tasks, repo_spec=spec, run_dir=self.tmp / "run",
                  results=self.tmp / "results")
        r.setup()
        tree = self.tmp / "tree"
        k.git("clone", "-q", str(r.repo_dir), str(tree), cwd=self.tmp)
        (tree / "tests" / "t.txt").write_text("agent\n")
        (tree / "tests" / "new.txt").write_text("agent\n")
        out = r.score(tree)
        self.assertFalse(out["suite_green_all"])
        self.assertTrue(out["suite_green_pinned"])
        self.assertEqual(out["test_files_restored"], ["tests/new.txt", "tests/t.txt"])
        r.archive()


class MetricTests(unittest.TestCase):
    """Warnings and test runs from tool replies; guard stats; collect of old and new run files."""

    @staticmethod
    def reply(agent, colls):
        return json.dumps({"agent": agent, "id": "ch_me", "collisions": colls})

    def test_warning_from_own_replies_only(self):
        file_hit = {"change": "ch_b", "agent": "codex-1", "kind": "conflict", "files": [{"path": "f.py", "kind": "conflict"}]}
        symbol = {"change": "ch_c", "agent": "codex-2", "kind": "overlap", "files": [{"kind": "symbol", "symbol": "f"}]}
        main = {"change": "main", "agent": None, "kind": "conflict", "files": [{"path": "f.py", "kind": "conflict", "reason": "you_conflict_with_main"}]}
        ev = lambda op, reply, **args: {"op": op, "args": args, "reply": reply, "ok": True}  # noqa: E731
        # another agent's change, read with `change`: not this agent's warning
        w = k.warning_of([ev("open_change", json.dumps({"id": "ch_me", "agent": "claude-1"})),
                          ev("change", json.dumps({"agent": "codex-1", "id": "ch_b", "collisions": [file_hit]}), change="ch_b"),
                          ev("apply", self.reply("claude-1", [symbol]), change="ch_me"), ev("land", "{}", change="ch_me")], "claude-1")
        self.assertFalse(w["warned"])
        # a conflict with main counts; a rebase after it and before the land is a reaction
        w = k.warning_of([ev("open_change", json.dumps({"id": "ch_me", "agent": "claude-1"})),
                          ev("apply", self.reply("claude-1", [main]), change="ch_me"), ev("rebase", "{}", change="ch_me"),
                          ev("land", "{}", change="ch_me")], "claude-1")
        self.assertEqual((w["warned"], w["reacted"], w["with_main"]), (True, True, True))
        # warned, then land first: no reaction
        w = k.warning_of([ev("apply", self.reply("claude-1", [file_hit]), change="ch_me"), ev("land", "{}", change="ch_me"),
                          ev("apply", "{}", change="ch_me")], "claude-1")
        self.assertEqual((w["warned"], w["reacted"], w["others"]), (True, False, ["ch_b"]))

    def test_tool_events_of_both_harnesses(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-events-"))
        try:
            saved = tmp / "cfg" / "tool-results" / "land.txt"
            saved.parent.mkdir(parents=True)
            saved.write_text(self.reply("claude-1", [{"change": "ch_b", "files": [{"path": "f", "kind": "overlap"}]}]))
            claude = [
                json.dumps({"type": "assistant", "message": {"id": "m1", "content": [
                    {"type": "tool_use", "id": "u1", "name": "mcp__treeyard__apply", "input": {"change": "ch_me"}}]}}),
                json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "u1", "content": [
                    {"type": "text", "text": f"Error: result (115,540 characters across 69 lines) exceeds maximum allowed tokens. Output has been saved to {saved}"}]}]}}),
                json.dumps({"type": "assistant", "message": {"id": "m2", "content": [
                    {"type": "tool_use", "id": "u2", "name": "Bash", "input": {"command": "t2-test tests/test_x.py"}}]}}),
                json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "u2", "content": "3 passed\nt2-test: passed", "is_error": False}]}}),
            ]
            ev = k.tool_events("claude", claude, tmp)
            self.assertEqual([e["op"] for e in ev], ["apply", None])
            self.assertTrue(ev[0].get("reply_from_file"))
            self.assertTrue(k.warning_of(ev, "claude-1")["warned"])
            codex = [json.dumps({"type": "item.completed", "item": {"type": "mcp_tool_call", "server": "treeyard", "tool": "apply",
                                                                     "arguments": {"change": "ch_me"}, "result": {"content": [{"type": "text", "text": "{}"}]}, "status": "completed"}}),
                     json.dumps({"type": "item.completed", "item": {"type": "command_execution", "command": "/bin/zsh -lc t2-test",
                                                                     "aggregated_output": "t2-test: failed (exit 1)", "exit_code": 1}})]
            ev = k.tool_events("codex", codex)
            self.assertEqual([(e["op"], e["ok"]) for e in ev], [("apply", True), (None, False)])
        finally:
            shutil.rmtree(tmp)

    def test_diff_stats(self):
        d = "+++ b/src/x.py\n+a = 1\n-b = 2\n+++ b/tests/test_x.py\n+def test(): pass\n+    assert 1\n"
        self.assertEqual(k.diff_stats(d, ["tests/**"]), {"files": 2, "added": 3, "removed": 1, "test_added": 2,
                                                          "paths": ["src/x.py", "tests/test_x.py"]})

    def test_collect_reads_old_and_new_run_files(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-collect-"))
        try:
            old = {"run": "A-x-colliding-r1-real", "harness": "real", "status": "done", "suite_green": True, "tasks_on_main": 5,
                   "warned_before_land": 3, "reacted": 1, "tokens_in": 100, "tokens_out": 10, "cost_usd": 0.5,
                   "agents": [{"k": 1, "harness": "claude"}, {"k": 2, "harness": "codex"}],
                   "agent_stats": [{"k": 1, "tokens_in": 60, "tokens_out": 5}, {"k": 2, "tokens_in": 40, "tokens_out": 5}]}
            new = {"run": "B-x-colliding-r1-real", "kit": 2, "harness": "real", "status": "done", "suite_green": True,
                   "suite_green_all": True, "suite_green_pinned": False, "tasks_on_main": 4, "warned": 2, "reacted": 1,
                   "legacy_warned_before_land": 3, "legacy_reacted": 0, "tokens_in": 10, "tokens_out": 1,
                   "by_harness": {"claude": {"tokens_in": 6, "tokens_out": 1}}, "agent_stats": []}
            (tmp / "a.json").write_text(json.dumps(old))
            (tmp / "b.json").write_text(json.dumps(new))
            rows = {r["run"]: r for r in k.collect(tmp, tmp / "m.json", tmp / "s.txt")}
            a, b = rows["A-x-colliding-r1-real"], rows["B-x-colliding-r1-real"]
            self.assertEqual((a["warned"], a["legacy_warned_before_land"], a["legacy_reacted"], a["suite_green_all"]), (None, 3, 1, True))
            self.assertEqual(a["by_harness"]["codex"]["tokens_in"], 40)
            self.assertEqual((b["warned"], b["reacted"], b["legacy_reacted"], b["suite_green_pinned"]), (2, 1, 0, False))
            self.assertIn("Claude only", (tmp / "s.txt").read_text())
        finally:
            shutil.rmtree(tmp)


class KitTests(unittest.TestCase):
    def test_prompts_do_not_name_the_tool_and_match_the_manifest(self):
        manifest = json.loads((k.TASKS_DIR / "MANIFEST.json").read_text())
        prompts = [p for p in manifest if p.endswith("prompt.md")]
        self.assertEqual(len(prompts), 30)
        for rel, digest in manifest.items():
            f = k.HERE / rel
            self.assertEqual(k.sha256(f), digest, rel)
            if rel.endswith("prompt.md"):
                self.assertNotIn("treeyard", f.read_text().lower(), rel)

    def test_edits_compose(self):
        files = {"f": "a\nANCHOR\nb\n"}
        t1 = {"id": "1", "edits": [{"op": "insert_after", "file": "f", "anchor": "ANCHOR\n", "text": "one\n"}]}
        t2 = {"id": "2", "edits": [{"op": "insert_after", "file": "f", "anchor": "ANCHOR\n", "text": "two\n"}]}
        self.assertEqual(k.apply_edits(files, [t1, t2])["f"], "a\nANCHOR\none\ntwo\nb\n")
        self.assertEqual(k.apply_edits(files, [t2])["f"], "a\nANCHOR\ntwo\nb\n")

    def test_lost_lines(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-lost-"))
        try:
            (tmp / "f.py").write_text("x = 1\nkept_line = 2\n")
            own = {1: "+++ b/f.py\n+kept_line = 2\n+lost_line = 3\n+}\n"}
            self.assertEqual(k.lost_lines(own, tmp), {"per_agent": {"1": 1}, "total": 1})
        finally:
            shutil.rmtree(tmp)

    def test_budget_formula(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-budget-"))
        try:
            for i, cost in enumerate((1.0, 3.0)):
                (tmp / f"r{i}.json").write_text(json.dumps({"run": f"r{i}", "harness": "real", "status": "done", "repo": "requests",
                                                            "cost_usd": cost, "tokens_in": 100, "tokens_out": 10}))
            (tmp / "stopped.json").write_text(json.dumps({"run": "s", "harness": "real", "status": "stopped", "repo": "requests", "cost_usd": 50}))
            out = k.budget(tmp)
            factors = {r: k.ref_size(r) / k.ref_size("requests") for r in k.REPOS}
            self.assertEqual(out["pilot_runs"], 2)
            self.assertEqual(out["repo_factors"]["requests"], 1.0)
            self.assertAlmostEqual(out["full_budget_usd"], round(2.0 * 12 * sum(factors.values()) * 1.25, 2))
        finally:
            shutil.rmtree(tmp)

    def test_parse_usage(self):
        st = {"turns": 0, "tool_calls": 0, "tokens_in": 0, "tokens_out": 0, "cost_usd": 0.0, "session": None}
        k.parse_line("claude", json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use"}], "usage": {
            "input_tokens": 3, "cache_read_input_tokens": 10, "cache_creation_input_tokens": 5, "output_tokens": 7}}}), st)
        k.parse_line("codex", json.dumps({"type": "turn.completed", "usage": {"input_tokens": 100, "output_tokens": 9}}), st)
        k.parse_line("opencode", json.dumps({"type": "step_finish", "part": {"tokens": {"input": 1, "output": 2, "reasoning": 3,
                                                                                    "cache": {"read": 4, "write": 5}}, "cost": 0.5}}), st)
        self.assertEqual((st["turns"], st["tool_calls"], st["tokens_in"], st["tokens_out"], st["cost_usd"]), (2, 1, 128, 21, 0.5))

    def test_claude_counts_once_per_message_and_takes_the_result_usage(self):
        """Excerpt of a real stream-json transcript (one agent, a first run and a resumed hand-back), trimmed."""
        u1 = {"input_tokens": 3, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 16}
        u2 = {"input_tokens": 1, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 1200, "output_tokens": 21}

        def msg(mid, kind, u, tid=None):
            block = {"type": kind, **({"id": tid, "name": "Bash", "input": {}} if tid else {})}
            return json.dumps({"type": "assistant", "message": {"id": mid, "model": "model-x", "content": [block], "usage": u}})

        lines = [
            json.dumps({"type": "system", "subtype": "init", "session_id": "s1", "model": "model-x"}),
            msg("m1", "thinking", u1), msg("m1", "tool_use", u1, "t1"), msg("m1", "tool_use", u1, "t2"),
            msg("m2", "text", u2),
        ]
        st = k.new_stats()
        for line in lines:
            k.parse_line("claude", line, st)
        # live: each message id once (the old count added m1 three times)
        self.assertEqual((st["tokens_in"], st["tokens_out"], st["turns"], st["tool_calls"]), (1103 + 1201, 37, 2, 2))
        self.assertEqual(k.legacy_count("claude", lines)["tokens_in"], 3 * 1103 + 1201)
        lines.append(json.dumps({"type": "result", "session_id": "s1", "num_turns": 3, "total_cost_usd": 0.5, "usage": {
            "input_tokens": 4, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 2300, "output_tokens": 40}}))
        # resumed hand-back: the session total cost goes on, the usage is this invocation's
        lines += [json.dumps({"type": "system", "subtype": "init", "session_id": "s1", "model": "model-x"}),
                  msg("m3", "tool_use", u2, "t3"),
                  json.dumps({"type": "result", "session_id": "s1", "num_turns": 2, "total_cost_usd": 0.75, "usage": u2})]
        c = k.count_transcript("claude", lines)
        self.assertEqual(c["tokens_in"], 2404 + 1201)
        self.assertEqual(c["tokens_out"], 40 + 21)
        self.assertEqual((c["tokens_fresh_in"], c["tokens_cached_in"], c["tokens_cache_write"]), (104 + 1, 2300 + 1200, 100))
        self.assertEqual((c["turns"], c["tool_calls"], c["cost_usd"], c["models"]), (5, 3, 0.75, ["model-x"]))

    def test_codex_resume_reports_the_thread_total(self):
        """Codex `turn.completed` usage is the thread total; the resumed turn adds only the difference."""
        lines = [json.dumps({"type": "thread.started", "thread_id": "th"}),
                 json.dumps({"type": "item.started", "item": {"type": "command_execution"}}),
                 json.dumps({"type": "turn.completed", "usage": {"input_tokens": 230757, "cached_input_tokens": 211456, "output_tokens": 1122}}),
                 json.dumps({"type": "thread.started", "thread_id": "th"}),
                 json.dumps({"type": "turn.completed", "usage": {"input_tokens": 426515, "cached_input_tokens": 397824, "output_tokens": 2064}})]
        c = k.count_transcript("codex", lines)
        self.assertEqual((c["tokens_in"], c["tokens_out"], c["tokens_cached_in"], c["tokens_fresh_in"]),
                         (426515, 2064, 397824, 426515 - 397824))
        self.assertEqual(k.legacy_count("codex", lines)["tokens_in"], 230757 + 426515)
        # a resume in a new process carries the thread total through `prior`
        first = k.count_transcript("codex", lines[:3])
        st = k.new_stats({"_codex_cum": {"th": {"in": 230757, "cached": 211456, "out": 1122}}})
        for line in lines[3:]:
            k.parse_line("codex", line, st)
        self.assertEqual(first["tokens_in"] + st["tokens_in"], 426515)

    def test_codex_live_usage_reads_the_rollout(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-rollout-"))
        try:
            d = tmp / "sessions" / "2026" / "01" / "01"
            d.mkdir(parents=True)
            ev = lambda n: json.dumps({"type": "event_msg", "payload": {"type": "token_count", "info": {
                "total_token_usage": {"input_tokens": n, "cached_input_tokens": n // 2, "output_tokens": 7}}}})
            (d / "rollout-2026-01-01T00-00-00-th.jsonl").write_text(
                json.dumps({"type": "turn_context", "payload": {"model": "model-y"}}) + "\n" + ev(10) + "\n" + ev(30) + "\n")
            self.assertEqual(k.codex_live_usage(tmp, "th"), {"in": 30, "cached": 15, "out": 7})
            self.assertIsNone(k.codex_live_usage(tmp, "other"))
            self.assertEqual(k.codex_models(tmp), ["model-y"])
        finally:
            shutil.rmtree(tmp)

    def test_recount_prints_old_and_new(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-recount-"))
        try:
            run = tmp / "A-mini-colliding-r1-real"
            run.mkdir()
            u = {"input_tokens": 1, "cache_read_input_tokens": 9, "output_tokens": 2}
            m = json.dumps({"type": "assistant", "message": {"id": "m", "content": [{"type": "text"}], "usage": u}})
            (run / "agent-1.jsonl").write_text("\n".join([json.dumps({"type": "system", "session_id": "s"}), m, m]) + "\n")
            rows = k.recount(tmp)
            self.assertEqual((rows[0]["old"], rows[0]["new"]), (24, 12))
        finally:
            shutil.rmtree(tmp)


KIT_GLOBALS = ("REPOS", "TASKS_DIR", "RESULTS", "TASKDEFS", "VENV", "TASK_TOTAL")


class DevTests(unittest.TestCase):
    """The development set (dev/) and `run --block`: neither may change a T2 run."""

    def setUp(self):
        self.saved = {n: getattr(k, n) for n in KIT_GLOBALS}
        self.tmp = Path(tempfile.mkdtemp(prefix="t2-dev-"))

    def tearDown_globals(self):
        for n, v in self.saved.items():
            setattr(k, n, v)

    def tearDown(self):
        self.tearDown_globals()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_dev_tasks_match_their_manifest_and_stay_apart_from_t2(self):
        manifest = json.loads((k.DEV / "tasks" / "MANIFEST.json").read_text())
        prompts = [p for p in manifest if p.endswith("prompt.md")]
        self.assertEqual(len(prompts), 10)
        t2_prompts = {(k.HERE / p).read_text() for p in json.loads((k.TASKS_DIR / "MANIFEST.json").read_text()) if p.endswith("prompt.md")}
        for rel, digest in manifest.items():
            self.assertTrue(rel.startswith("dev/tasks/"), rel)
            f = k.HERE / rel
            self.assertEqual(k.sha256(f), digest, rel)
            if rel.endswith("prompt.md"):
                self.assertNotIn("treeyard", f.read_text().lower(), rel)
                self.assertNotIn(f.read_text(), t2_prompts, rel)
        self.assertFalse(set(k.DEV_REPOS) & set(k.REPOS), "a dev repository may not be a T2 one")

    def test_a_dev_repo_name_switches_the_kit_and_t2_names_do_not(self):
        self.assertFalse(k.select_kit(None))
        self.assertFalse(k.select_kit(["requests"]))
        self.assertEqual(k.TASKS_DIR, k.HERE / "tasks")
        with self.assertRaises(SystemExit):
            k.select_kit(["requests", "click"])
        self.assertTrue(k.select_kit(["click"]))
        self.assertEqual((k.TASKS_DIR, k.RESULTS, k.TASK_TOTAL), (k.DEV / "tasks", k.DEV / "results", 10))
        self.assertEqual(list(k.REPOS), ["click"])
        self.assertEqual(sorted(k.load_taskdefs().TASKS), ["click"])
        self.assertEqual(k.test_path("click", {"id": "c1-strip-spaces"}), "tests/test_t2_c1_strip_spaces.py")
        self.assertEqual(k.fill(["{python}"], self.tmp), [str(self.tmp / "t2" / "env" / "dev-venv" / "bin" / "python")])
        self.assertEqual(k.repo_env("click", self.tmp)["PYTHONPATH"], "src")

    def test_block_is_refused_for_arm_a_and_for_t2(self):
        block = self.tmp / "b.md"
        block.write_text("## Block\n")
        common = ["run", "--scratch", str(self.tmp), "--set", "colliding", "--rep", "1", "--harness", "oracle", "--block", str(block)]
        for arm, repo in (("A", "click"), ("B", "requests")):
            self.tearDown_globals()
            with self.assertRaises(SystemExit) as e:
                k.main(common + ["--arm", arm, "--repo", repo])
            self.assertIn("--block is for arm B on the development set only", str(e.exception))

    def test_block_text_and_put_block(self):
        plain = self.tmp / "plain.md"
        plain.write_text("## Block\n\nline\n\n")
        want = "<!-- treeyard:connect:start -->\n## Block\n\nline\n<!-- treeyard:connect:end -->"
        self.assertEqual(k.block_text(plain), want)
        marked = self.tmp / "marked.md"
        marked.write_text(want + "\n")
        self.assertEqual(k.block_text(marked), want)
        self.assertEqual(k.block_text(k.FROZEN_BLOCK), k.FROZEN_BLOCK.read_text().rstrip("\n"))
        (self.tmp / "AGENTS.md").write_text("# Repo\n\n" + k.FROZEN_BLOCK.read_text() + "tail\n")
        k.put_block(self.tmp, want)
        self.assertEqual((self.tmp / "AGENTS.md").read_text(), "# Repo\n\n" + want + "\ntail\n")
        self.assertFalse((self.tmp / "CLAUDE.md").exists())

    @unittest.skipUnless(k.treeyard_bin().exists(), "needs target/debug/treeyard")
    def test_a_custom_block_reaches_the_agents_of_arm_b(self):
        spec, tasks = mini_world(self.tmp)
        block = self.tmp / "try-1.md"
        block.write_text("## Another block\n\nRead line ranges with `read`.\n")
        os.environ["T2_STUB_MODES"] = "ok,ok,ok,ok,ok"
        try:
            r = k.Run(self.tmp, "B", "mini", "colliding", 1, "stub", limits={"max_turns": 50, "max_tokens": 10**9, "timeout_s": 60},
                      tasks_dir=tasks, repo_spec=spec, run_dir=self.tmp / "run", results=self.tmp / "results", block_file=block)
            res = r.execute()
        finally:
            os.environ.pop("T2_STUB_MODES", None)
        want = k.block_text(block)
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual(res["run"], "B-mini-colliding-r1-stub-try-1")
        self.assertEqual(res["block_file"], "try-1.md")
        self.assertEqual(res["block_sha256"], k.hashlib.sha256(want.encode()).hexdigest())
        self.assertTrue(r.prompt(r.agents[0]).startswith(want + "\n\n" + k.preamble("B") + "\n\n# task 1"))
        self.assertTrue((self.tmp / "results" / "B-mini-colliding-r1-stub-try-1.json").exists())


if __name__ == "__main__":
    unittest.main()
