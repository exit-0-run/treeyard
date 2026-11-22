"""Stop tests for the T2 runner: scripted stub agents, a tiny repository, no model, no network.

One test per stop: turn limit, token limit, wall-clock timeout (per agent), and the anomalies that
end the whole run: wrong agent count, server down, zero tool calls. `make bench-test` runs them.
"""

import contextlib
import http.server
import io
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
        arms = ["A"] + (["B", "BN"] if HAS_TREEYARD else [])
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


class ArmBNTests(StubBase):
    """Arm BN: arm B in native mode. The stub agents write files in their `start` directory and use the
    bridge's `sync` and `land`; the runner lands through the bridge with the agent's environment."""

    final = HandbackTests.final
    assert_all_kept = HandbackTests.assert_all_kept
    assert_left_out = HandbackTests.assert_left_out

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_claude_dialect_resolves_in_the_start_directory(self):
        res = self.run_stub(",".join(["conflict-resolve"] * 5), arm="BN", dialects=",".join(["claude"] * 5))
        self.assert_all_kept(res, "lands")
        l2 = res["lands"][1]
        self.assertTrue(all(x.get("native") for x in l2["runner_landed"] if not x["workspace"]), l2)
        self.assertEqual(l2["handback_files"], ["a.txt"])
        ev = k.tool_events("claude", (self.tmp / "run" / "agent-2.jsonl").read_text().splitlines())
        self.assertEqual([e["op"] for e in ev if e["op"]], ["start", "sync"])
        self.assertTrue(res["warned"] >= 1, "sync replies carry the collisions")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_codex_dialect_leaves_markers_and_the_land_is_refused(self):
        res = self.run_stub(",".join(["conflict-markers"] * 5), arm="BN", dialects=",".join(["codex"] * 5))
        self.assert_left_out(res, "lands")
        self.assertIn("markers left", res["lands"][1]["handback_failure"][0])
        self.assertIn("refused", [x["status"] for x in res["lands"][1]["runner_landed"]])

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_do_nothing_is_left_out(self):
        res = self.run_stub(",".join(["edit-conflict"] * 5), arm="BN")
        self.assert_left_out(res, "lands")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_a_held_land_from_the_start_directory(self):
        res = self.run_stub("land-line,undo-line,ok,ok,ok", arm="BN", dialects="codex,claude,,,")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["interventions"], res["undoes_held"]), (1, 1))
        l2 = res["lands"][1]
        held = l2["undoes_held"][0]
        self.assertEqual([e["text"] for e in held["undoes"]], ["kept by agent 1\n"])
        self.assertIn({"change": held["change"], "workspace": False, "native": True, "status": "undoes", "undoes": held["undoes"]},
                      l2["runner_landed"])
        self.assertIn("kept by agent 1\n", (self.tmp / "run" / "final" / "a.txt").read_text())
        # agent 2's own sync reply named the undone line
        ev = k.tool_events("claude", (self.tmp / "run" / "agent-2.jsonl").read_text().splitlines())
        sync = [e for e in ev if e["op"] == "sync"]
        self.assertEqual(len(sync), 1)
        self.assertIn("kept by agent 1", sync[0]["reply"])

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_unsynced_edits_reach_the_runners_land(self):
        """An agent that edits its directory and stops without sync or land: the runner's native land sends the
        files (the commit rule first removes env and lock files)."""
        os.environ["T2_STUB_MODES"] = "ok,ok,ok,ok,ok"
        r = k.Run(self.tmp, "BN", "mini", "colliding", 1, "noop", limits={"max_turns": 50, "max_tokens": 10**9, "timeout_s": 60},
                  tasks_dir=self.tasks, repo_spec=self.spec, run_dir=self.tmp / "run", results=self.tmp / "results")
        r.setup()
        try:
            a = r.agents[0]
            br = k.NativeBridge(r.ty, r.url, a["agent"], env=a["env"], cwd=a["cwd"])
            st = json.loads(br.call("start", {"intent": "line 3"})[1])
            br.close()
            d = Path(st["dir"])
            self.assertEqual(r.native_dir(a, st["id"]), d)
            self.assertTrue(str(d).startswith(str(Path(a["box"]) / "tmp")), "inside the box (Codex writable root)")
            (d / "a.txt").write_text((d / "a.txt").read_text().replace("line 3\n", "task 3\n"))
            (d / "uv.lock").write_text("lock\n")
            lands, interventions = r.finalize_b()
            self.assertEqual(interventions, 0)
            self.assertIn(f"{st['id']}:uv.lock", lands[0]["skipped_paths"])
            self.assertIn({"change": st["id"], "workspace": False, "native": True, "status": "landed"}, lands[0]["runner_landed"])
            self.assertIn("task 3\n", r.call("read", {"path": "a.txt"})["content"])
            self.assertIsNone(r.call("read", {"path": "uv.lock"}).get("content"))
        finally:
            k.sh([str(r.ty), "disconnect", "--purge"], cwd=r.repo_dir, check=False)
            r.archive()

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_prompt_block_and_commands(self):
        os.environ["T2_DRY"] = "1"
        r = k.Run(self.tmp, "BN", "mini", "colliding", 1, "stub", tasks_dir=self.tasks, repo_spec=self.spec,
                  run_dir=self.tmp / "run", results=self.tmp / "results")
        try:
            r.setup()
            want = k.FROZEN_BLOCK_BN.read_text().rstrip("\n")
            self.assertEqual(r.block, want)
            self.assertTrue(r.prompt(r.agents[0]).startswith(want + "\n\n" + k.preamble("BN") + "\n\n# task 1"))
            self.assertIn("from `dir`", k.preamble("BN"))
            self.assertIn("--tools", r.ctx_data["mcp_args"])
            self.assertEqual(r.ctx_data["mcp_args"][r.ctx_data["mcp_args"].index("--tools") + 1], "native")
            a = r.agents[0]
            for h in ("claude", "codex"):
                cmd, env = k.real_command(h, "p", a, r.ctx_data, r.limits)
                if h == "claude":
                    self.assertEqual(cmd[cmd.index("--add-dir") + 1], str(Path(a["box"]) / "tmp"))
                    self.assertTrue(cmd[cmd.index("--allowedTools") + 1].endswith(",mcp__treeyard__*"))
                else:
                    roots = [x for x in cmd if x.startswith("sandbox_workspace_write.writable_roots=")][0]
                    self.assertIn(json.dumps(str(Path(a["box"]))), roots)
                    self.assertIn('"native"', " ".join(cmd))
        finally:
            os.environ.pop("T2_DRY", None)
            k.sh([str(r.ty), "disconnect", "--purge"], cwd=r.repo_dir, check=False)
            r.archive()


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

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_oracle_arm_bn(self):
        r, res = self.run_world("oracle", arm="BN")
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["tasks_on_main"], res["interventions"]), (5, 0))
        self.assertEqual((self.tmp / "run" / "final" / "a.txt").read_text(), "".join(f"task {i}\n" for i in range(1, 6)))
        self.assert_guard(res)
        ev = k.tool_events("script", (self.tmp / "run" / "agent-5.jsonl").read_text().splitlines())
        ops = [e["op"] for e in ev if e["op"]]
        self.assertEqual(ops[:2], ["start", "sync"])
        self.assertIn("land", ops)

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
        # arm BN: `start` opens the change, `sync` replies warn, and a later sync is the reaction
        native = [ev("start", json.dumps({"id": "ch_me", "dir": "/d"}), intent="x"),
                  ev("sync", json.dumps({"id": "ch_me", "collisions": [file_hit]}), change="ch_me"),
                  ev("sync", "{}", change="ch_me"), ev("land", "{}", change="ch_me")]
        w = k.warning_of(native, "claude-1", native=True)
        self.assertEqual((w["warned"], w["reacted"], w["others"]), (True, True, ["ch_b"]))
        self.assertFalse(k.warning_of(native, "claude-1")["reacted"], "a sync is no reaction in arm B")

    @staticmethod
    def bn_transcripts():
        """Arm BN agents in both dialects: codex-1 is held, then lands with `undo` (an override); claude-1 gets a
        conflict, a refusal (markers left), a hold, then lands with the line back (a reaction)."""
        hold = {"status": "undoes", "change": "{me}", "advice": "Not landed: your change removes what another agent landed",
                "undoes": [{"path": "f.py", "lines": [3, 3], "agent": "x", "change": "ch_other", "text": "a\n"}]}
        conflict = {"status": "conflict", "change": "{me}", "conflicted": ["f.py"], "advice": "Not landed: main moved"}
        refused = {"status": "refused", "change": "{me}", "markers_left": ["f.py"]}

        def codex(calls):
            out = [json.dumps({"type": "thread.started", "thread_id": "t"})]
            for name, args, reply in calls:
                out.append(json.dumps({"type": "item.completed", "item": {
                    "type": "mcp_tool_call", "server": "treeyard", "tool": name, "arguments": args, "status": "completed",
                    "result": {"content": [{"type": "text", "text": json.dumps(reply)}]}}}))
            return out

        def claude(calls):
            out = [json.dumps({"type": "system", "subtype": "init", "session_id": "s"})]
            for i, (name, args, reply) in enumerate(calls):
                out.append(json.dumps({"type": "assistant", "message": {"id": f"m{i}", "content": [
                    {"type": "tool_use", "id": f"u{i}", "name": f"mcp__treeyard__{name}", "input": args}]}}))
                out.append(json.dumps({"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": f"u{i}", "content": [{"type": "text", "text": json.dumps(reply)}]}]}}))
            return out

        def fill(v, me):
            return json.loads(json.dumps(v).replace("{me}", me))

        c1 = codex([("start", {"intent": "x"}, {"id": "ch_c1", "dir": "/d"}),
                    ("land", {"change": "ch_c1"}, fill(hold, "ch_c1")),
                    ("land", {"change": "ch_c1", "undo": True, "reason": "the task says so"},
                     {"status": "landed", "change": "ch_c1", "undid": {"by": "codex-1", "reason": "the task says so"}})])
        c2 = claude([("start", {"intent": "y"}, {"id": "ch_c2", "dir": "/e"}),
                     ("land", {"change": "ch_c2"}, fill(conflict, "ch_c2")),
                     ("land", {"change": "ch_c2"}, fill(refused, "ch_c2")),
                     ("land", {"change": "ch_c2"}, fill(hold, "ch_c2")),
                     ("land", {"change": "ch_c2"}, {"status": "landed", "change": "ch_c2"})])
        return c1, c2

    def test_bn_land_replies_count(self):
        c1, c2 = self.bn_transcripts()
        w1 = k.warning_of(k.tool_events("codex", c1), "codex-1", native=True)
        self.assertEqual((w1["warned"], w1["reacted"], w1["land_holds"], w1["undoes_overridden"], w1["kinds"], w1["others"]),
                         (True, False, 1, 1, ["undoes"], ["ch_other"]))
        w2 = k.warning_of(k.tool_events("claude", c2), "claude-1", native=True)
        self.assertEqual((w2["warned"], w2["reacted"], w2["hold_reacted"], w2["land_conflicts"], w2["land_refused"], w2["held_changes"]),
                         (True, True, True, 1, 1, ["ch_c2"]))
        self.assertNotIn("land_holds", k.warning_of(k.tool_events("claude", c2), "claude-1"), "arm B unchanged")
        self.assertFalse(k.warning_of(k.tool_events("claude", c2), "claude-1")["warned"])

    def test_recount_metrics_of_a_bn_run(self):
        tmp = Path(tempfile.mkdtemp(prefix="t2-recount-"))
        try:
            c1, c2 = self.bn_transcripts()
            run = tmp / "runs" / "BN-x-heavy-r1-real"
            run.mkdir(parents=True)
            (run / "agent-1.jsonl").write_text("\n".join(c1) + "\n")
            (run / "agent-2.jsonl").write_text("\n".join(c2) + "\n")
            (run / "ctx.json").write_text(json.dumps({"arm": "BN", "agents": [{"k": 1, "harness": "codex"}, {"k": 2, "harness": "claude"}]}))
            res = tmp / "results"
            res.mkdir()
            old = {"run": "BN-x-heavy-r1-real", "arm": "BN", "kit": 2, "warned": 0, "reacted": 0, "undoes_held": 0, "false_alarms": 0,
                   "agents": [{"k": 1, "agent": "codex-1"}, {"k": 2, "agent": "claude-1"}],
                   "lands": [{"k": 1, "undoes_held": []}, {"k": 2, "undoes_held": [{"change": "ch_c2", "undoes": []}]}]}
            (res / "BN-x-heavy-r1-real.json").write_text(json.dumps(old))
            out = tmp / "out"
            with contextlib.redirect_stdout(io.StringIO()) as buf:
                k.main(["recount", "--metrics", "--runs", str(tmp / "runs"), "--results", str(res), "--out", str(out)])
            self.assertIn("| BN-x-heavy-r1-real | 2 | 1 | 2 | 1 | 1 | 1 | 1 | 1 | 2 | 0 / 0 / 0 |", buf.getvalue())
            r = json.loads((out / "BN-x-heavy-r1-real.json").read_text())
            self.assertEqual((r["warned"], r["reacted"], r["land_holds"], r["undoes_overridden"], r["undoes_held"], r["undoes_held_runner"]),
                             (2, 1, 2, 1, 2, 1))
            self.assertEqual([x["undoes_held_changes"] for x in r["lands"]], [["ch_c1"], ["ch_c2"]], "one change counted once")
            self.assertEqual(r["false_alarms"], 0)
            self.assertEqual(json.loads((res / "BN-x-heavy-r1-real.json").read_text()), old, "the run file stays")
        finally:
            shutil.rmtree(tmp)

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


MIXED_SLOTS = ["codex", "claude", "claude", "codex", "claude", "claude", "codex", "claude", "codex", "claude"]
MIXED_LIMITS = {"max_turns": 150, "max_tokens": 8_000_000, "timeout_s": 2400, "claude_budget_usd": 5.0}


def mixed_world(root, n=10, tasks_made=None):
    """An extra set like dev `mixed`: n agents and tasks (task i sets line i to `task i`, so neighbours
    conflict in git), a set.json with the slots and limits, and one integration test (lines 1 and 2)."""
    src = root / "src" / "mini"
    src.mkdir(parents=True)
    base = {"a.txt": "".join(f"line {i}\n" for i in range(1, n + 1))}
    (src / "a.txt").write_text(base["a.txt"])
    k.git("init", "-q", "-b", "main", cwd=src)
    k.git("add", "a.txt", cwd=src)
    k.git("commit", "-q", "-m", "mini", cwd=src)
    rev = k.git("rev-parse", "HEAD", cwd=src).stdout.strip()
    tasks = root / "tasks"
    defs = [{"id": f"t{i}", "edits": [{"op": "replace", "file": "a.txt", "old": f"line {i}\n", "new": f"task {i}\n"}]}
            for i in range(1, n + 1)]
    set_dir = tasks / "mini" / "mixed"
    for i in range(1, (tasks_made or n) + 1):
        d = set_dir / f"{i}-t{i}"
        (d / "hidden").mkdir(parents=True)
        (d / "prompt.md").write_text(f"# task {i}\n\nChange line {i}.\n")
        (d / "reference.patch").write_text(k.unified(base, k.apply_seq(base, [defs[i - 1]], False)))
        (d / f"resolved-{i}.patch").write_text(k.unified(k.apply_seq(base, defs[: i - 1]), k.apply_seq(base, defs[:i])))
        (d / "hidden" / f"test_{i}.txt").write_text(f"{i}\n")
        (d / "meta.json").write_text(json.dumps({"repo": "mini", "set": "mixed", "k": i, "id": f"t{i}", "title": f"task {i}",
                                                 "files": ["a.txt"], "test": f"test_{i}.txt"}))
    it = set_dir / "integration" / "i1-one-and-two" / "hidden"
    it.mkdir(parents=True)
    (it / "test_i1.txt").write_text("1 2\n")
    (set_dir / "set.json").write_text(json.dumps({"set": "mixed", "agents": n, "mix": MIXED_SLOTS[:n], "limits": MIXED_LIMITS,
                                                  "integration": [{"id": "i1-one-and-two", "pair": [1, 2], "test": "test_i1.txt"}]}))
    spec = {"rev": rev, "local_src": str(src), "workdir": ".", "suite": [sys.executable, "-c", "pass"],
            "decide": [sys.executable, "-c", "import sys; lines = open('a.txt').read().splitlines(); "
                       "sys.exit(any(lines[n - 1] != 'task %d' % n for n in map(int, open(sys.argv[1]).read().split())))", "{test}"]}
    return spec, tasks


class MixedSetTests(StubBase):
    """Extra sets with a set.json: agent count, harness slots and limits from the set, integration tests."""

    def mixed_run(self, harness, arm="A", agents=None, limits=None, n=10, tasks_made=None, rep=1, modes=""):
        os.environ["T2_STUB_MODES"] = modes
        spec, tasks = mixed_world(Path(tempfile.mkdtemp(prefix="w-", dir=self.tmp)), n, tasks_made)
        r = k.Run(self.tmp, arm, "mini", "mixed", rep, harness, limits=limits, agents=agents, tasks_dir=tasks, repo_spec=spec,
                  run_dir=self.tmp / "run", results=self.tmp / "results")
        return r

    def test_old_sets_keep_five_agents_mix_and_frozen_limits(self):
        spec = k.set_spec(self.tasks, "mini", "colliding")
        self.assertEqual((spec["agents"], spec["mix"], spec["limits"], spec["integration"], spec["from_file"]),
                         (5, k.MIX, {}, [], False))
        r = k.Run(self.tmp, "A", "mini", "colliding", 1, "stub", tasks_dir=self.tasks, repo_spec=self.spec, run_dir=self.tmp / "run")
        self.assertEqual((r.expected, r.n_agents, r.limits), (5, 5, k.LIMITS))
        self.assertEqual(k.LIMITS, {"max_turns": 80, "max_tokens": 4_000_000, "timeout_s": 1800, "claude_budget_usd": 3.0})
        self.assertEqual(k.MIX, ["claude", "claude", "claude", "codex", "codex"])

    def test_set_settings_drive_count_slots_and_limits(self):
        r = self.mixed_run("stub")
        self.assertEqual((r.expected, r.n_agents), (10, 10))
        self.assertEqual(r.limits, MIXED_LIMITS)
        self.assertEqual([r.harness_of(i) for i in range(1, 11)], ["stub"] * 10)
        r.harness = "real"
        self.assertEqual([r.harness_of(i) for i in range(1, 11)], MIXED_SLOTS)
        self.assertEqual(MIXED_SLOTS.count("claude"), 6)
        r.rep = 2  # rotated by one slot per repetition, as the 5-agent sets
        self.assertEqual([r.harness_of(i) for i in range(1, 11)], MIXED_SLOTS[1:] + MIXED_SLOTS[:1])
        small = self.mixed_run("stub", limits={"max_turns": 7})
        self.assertEqual(small.limits, dict(MIXED_LIMITS, max_turns=7), "test limits override the set's")

    def test_ten_stub_agents_run_in_every_dialect(self):
        os.environ["T2_STUB_AS"] = ",".join(MIXED_SLOTS)
        r = self.mixed_run("stub", modes=",".join(["integrate"] * 10), arm="A2")
        res = r.execute()
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["set_agents"], res["n_tasks"], len(res["agent_stats"])), (10, 10, 10))
        self.assertEqual(res["interventions"], 0)
        self.assertEqual(sum(1 for s in res["agent_stats"] if s["tool_calls"] > 0), 10)
        text = (self.tmp / "run" / "final" / "a.txt").read_text()
        self.assertFalse(k.has_marker_text(text))
        self.assertEqual([f"agent {i}" in text for i in range(1, 11)], [True] * 10)
        self.assertEqual(res["limits"], MIXED_LIMITS)

    def test_wrong_agent_count_stops_a_ten_agent_set(self):
        res = self.mixed_run("stub", agents=9).execute()
        self.assertEqual((res["status"], res["stop_reason"], res["tool_calls"]), ("stopped", "agent count 9, want 10", 0))
        res = self.mixed_run("stub", tasks_made=9).execute()
        self.assertEqual((res["status"], res["stop_reason"]), ("stopped", "agent count 9 tasks, want 10"))

    def oracle_and_noop(self, arm):
        res = self.mixed_run("oracle", arm=arm).execute()
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["tasks_on_main"], res["n_tasks"], res["interventions"]), (10, 10, 0))
        self.assertEqual((res["integration"], res["integration_passed"], res["integration_total"]), ({"i1-one-and-two": True}, 1, 1))
        self.assertEqual((self.tmp / "run" / "final" / "a.txt").read_text(), "".join(f"task {i}\n" for i in range(1, 11)))
        res = self.mixed_run("noop", arm=arm).execute()
        self.assertEqual(res["status"], "done", res.get("stop_reason"))
        self.assertEqual((res["tasks_on_main"], res["integration_passed"], res["integration_total"]), (0, 0, 1))
        return res

    def test_oracle_and_noop_arm_a(self):
        self.oracle_and_noop("A")

    def test_oracle_and_noop_arm_a2(self):
        self.oracle_and_noop("A2")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_oracle_and_noop_arm_b_ten_workspaces(self):
        self.oracle_and_noop("B")

    @unittest.skipUnless(HAS_TREEYARD, "needs target/debug/treeyard (or T2_TREEYARD)")
    def test_oracle_and_noop_arm_bn_ten_directories(self):
        res = self.oracle_and_noop("BN")
        self.assertEqual({x["status"] for m in res["lands"] for x in m["runner_landed"]}, {"empty"})

    def test_collect_shows_the_task_count_and_integration(self):
        tmp = self.tmp / "collect"
        tmp.mkdir()
        (tmp / "old.json").write_text(json.dumps({"run": "A-x-colliding-r1-real", "kit": 2, "status": "done", "tasks_on_main": 5}))
        (tmp / "new.json").write_text(json.dumps({"run": "A-x-mixed-r1-oracle", "kit": 2, "status": "done", "tasks_on_main": 9,
                                                  "n_tasks": 10, "integration_passed": 5, "integration_total": 6}))
        rows = {r["run"]: r for r in k.collect(tmp, tmp / "m.json", tmp / "s.txt")}
        self.assertEqual(rows["A-x-mixed-r1-oracle"]["integration_passed"], 5)
        text = (tmp / "s.txt").read_text()
        self.assertIn("| A-x-mixed-r1-oracle | done | - | - | 9/10 | 5/6 |", text)
        self.assertIn("| A-x-colliding-r1-real | done | - | - | 5/5 | - |", text)


class SeqEditTests(unittest.TestCase):
    def test_tasks_apply_in_order_and_alts_fit_a_tree_with_the_other_task(self):
        files = {"f": "a = 1\n"}
        t1 = {"id": "1", "edits": [{"op": "replace", "file": "f", "old": "a = 1\n", "new": "a = 2\n"}]}
        t2 = {"id": "2", "edits": [{"op": "replace", "file": "f", "old": "a = 1\n", "new": "a = 1\nb = 1\n",
                                    "alts": [{"op": "insert_after", "file": "f", "anchor": "a = 2\n", "text": "b = 1\n"}]}]}
        self.assertEqual(k.apply_seq(files, [t2], False)["f"], "a = 1\nb = 1\n")
        self.assertEqual(k.apply_seq(files, [t2])["f"], "a = 1\nb = 1\n", "the alt does not fit the pin: the op itself")
        self.assertEqual(k.apply_seq(files, [t1, t2])["f"], "a = 2\nb = 1\n")
        with self.assertRaises(ValueError):
            k.apply_seq(files, [t1, t2], False)
        ins = [{"id": str(i), "edits": [{"op": "insert_before", "file": "f", "anchor": "a = 1\n", "text": f"x{i}\n"}]} for i in (1, 2)]
        self.assertEqual(k.apply_seq(files, ins)["f"], "x1\nx2\na = 1\n", "insertions at one anchor keep the task order")

    def test_create_and_new_file_patches(self):
        t = {"id": "1", "edits": [{"op": "create", "file": "new.py", "text": "x = 1\n"},
                                  {"op": "replace", "file": "f", "old": "a = 1\n", "new": "from new import x\n"}]}
        out = k.apply_seq({"f": "a = 1\n"}, [t], False)
        self.assertEqual(out, {"f": "from new import x\n", "new.py": "x = 1\n"})
        with self.assertRaises(ValueError):  # a create on a file that is there
            k.apply_seq(out, [t], False)
        patch = k.unified({"f": "a = 1\n"}, out)
        self.assertIn("diff --git a/new.py b/new.py\nnew file mode 100644\n--- /dev/null\n+++ b/new.py\n", patch)
        tmp = Path(tempfile.mkdtemp(prefix="t2-new-"))
        try:
            subprocess.run(["git", "init", "-q"], cwd=tmp, check=True)
            (tmp / "f").write_text("a = 1\n")
            (tmp / "p.patch").write_text(patch)
            subprocess.run(["git", "apply", "p.patch"], cwd=tmp, check=True)
            self.assertEqual((tmp / "new.py").read_text(), "x = 1\n")
            self.assertEqual((tmp / "f").read_text(), "from new import x\n")
        finally:
            shutil.rmtree(tmp)

    def test_combined_edits_are_the_step_on_the_earlier_tasks(self):
        files = {"f": "def g():\n    return 1\n"}
        move = {"id": "1", "edits": [{"op": "create", "file": "m.py", "text": "def g():\n    return 1\n"},
                                     {"op": "replace", "file": "f", "old": "def g():\n    return 1\n", "new": "from m import g\n"}]}
        change = {"id": "2", "edits": [{"op": "replace", "file": "f", "old": "return 1", "new": "return 2"}],
                  "combined_edits": [{"op": "replace", "file": "m.py", "old": "return 1", "new": "return 2"}]}
        self.assertEqual(k.apply_seq(files, [change], False)["f"], "def g():\n    return 2\n", "alone: its own edits")
        both = k.apply_seq(files, [move, change])
        self.assertEqual(both, {"f": "from m import g\n", "m.py": "def g():\n    return 2\n"})
        with self.assertRaises(ValueError):  # combined on a tree without the move: no silent fallback
            k.apply_seq(files, [change])
        self.assertEqual(k.task_ops(change)[-1]["file"], "m.py")

    def test_replay_without_a_task(self):
        files = {"f": "a\nb\nc\nd\ne\nf\ng\n"}
        t1 = {"id": "1", "edits": [{"op": "replace", "file": "f", "old": "a\n", "new": "A\n"}]}
        t2 = {"id": "2", "edits": [{"op": "replace", "file": "f", "old": "g\n", "new": "G\n"}],
              "combined_edits": [{"op": "replace", "file": "f", "old": "f\ng\n", "new": "f\nG\n"}]}
        t3 = {"id": "3", "edits": [{"op": "replace", "file": "f", "old": "d\n", "new": "D\n"}],
              "combined_edits": [{"op": "replace", "file": "f", "old": "A\nb\nc\nd\n", "new": "A\nb\nc\nD\n"}]}
        self.assertEqual(k.replay_without(files, [t1, t2, t3], 1)["f"], "a\nb\nc\nD\ne\nf\nG\n")
        self.assertEqual(k.replay_without(files, [t1, t2, t3], 2)["f"], "A\nb\nc\nD\ne\nf\ng\n")
        self.assertEqual(k.merge3("x\n", "x\n", "y\n"), "y\n")
        self.assertIsNone(k.merge3("z\n", "x\n", "y\n"))
        self.assertEqual(k.merge3("z\n", "x\n", "y\n", union=True), "z\ny\n")


class DevMixedTests(unittest.TestCase):
    """The `mixed` set of the development set (dev/taskdefs_mixed.py, dev/tasks/click/mixed)."""

    def setUp(self):
        self.saved = {n: getattr(k, n) for n in KIT_GLOBALS}
        k.select_kit(["click"])

    def tearDown(self):
        for n, v in self.saved.items():
            setattr(k, n, v)

    def test_files_match_their_manifest(self):
        manifest = json.loads((k.DEV / "tasks" / "MANIFEST-mixed.json").read_text())
        prompts = [p for p in manifest if p.endswith("prompt.md")]
        self.assertEqual(len(prompts), 10)
        old = json.loads((k.DEV / "tasks" / "MANIFEST.json").read_text())
        self.assertFalse(set(manifest) & set(old), "the 5-agent sets keep their own manifest")
        self.assertEqual(sum(1 for p in old if p.endswith("prompt.md")), 10)
        old_prompts = {(k.HERE / p).read_text() for p in old if p.endswith("prompt.md")}
        for rel, digest in manifest.items():
            self.assertTrue(rel.startswith("dev/tasks/click/mixed/"), rel)
            f = k.HERE / rel
            self.assertEqual(k.sha256(f), digest, rel)
            if rel.endswith("prompt.md"):
                self.assertNotIn("treeyard", f.read_text().lower(), rel)
                self.assertNotIn(f.read_text(), old_prompts, rel)

    def test_set_json_and_the_designed_interactions(self):
        spec = k.set_spec(k.TASKS_DIR, "click", "mixed")
        self.assertEqual((spec["agents"], len(spec["mix"]), spec["mix"].count("claude"), spec["mix"].count("codex")), (10, 10, 6, 4))
        self.assertEqual(spec["limits"], MIXED_LIMITS)
        self.assertEqual(len(k.task_dirs("click", "mixed")), 10)
        self.assertEqual(k.sets_of("click"), ["mixed", "heavy"])
        pairs = {it["id"]: it["pair"] for it in spec["integration"]}
        self.assertEqual(len(pairs), 6)
        for it in spec["integration"]:
            d = k.TASKS_DIR / "click" / "mixed" / "integration" / it["id"]
            self.assertTrue((d / "hidden" / it["test"]).is_file())
            for p in it["pair"]:
                self.assertTrue((d / f"without-{p}.patch").is_file())
        # the two tasks of each designed pair get different harnesses in the first repetition
        for a, b in pairs.values():
            self.assertNotEqual(spec["mix"][a - 1], spec["mix"][b - 1], (a, b))
        td = [m for m in k.extra_taskdefs() if "mixed" in m.SETS][0]
        for t in td.TASKS["click"]["mixed"]:
            hidden_tests = t["test"].count("\ndef test_")
            self.assertGreaterEqual(hidden_tests, 3, t["id"])
            self.assertGreaterEqual(len({e["file"] for e in t["edits"]}) + len(t["edits"]), 3, t["id"])

    def test_old_dev_sets_are_unchanged_by_the_extra_module(self):
        td = k.load_taskdefs()
        self.assertEqual(sorted(td.TASKS["click"]), ["colliding", "disjoint"])
        self.assertEqual(td.SETS, ("disjoint", "colliding"))
        self.assertEqual(k.set_spec(k.TASKS_DIR, "click", "colliding")["from_file"], False)
        self.assertEqual(k.set_spec(k.TASKS_DIR, "click", "disjoint")["agents"], 5)

    @unittest.skipUnless(os.environ.get("T2_SCRATCH"), "needs T2_SCRATCH (a scratch dir with the click clone)")
    def test_generate_reproduces_every_dev_task_file(self):
        """`generate --repo click` in a copy of the kit writes the committed dev task files byte for byte."""
        tmp = Path(tempfile.mkdtemp(prefix="t2-gen-"))
        try:
            kit = tmp / "kit"
            shutil.copytree(k.HERE, kit, ignore=shutil.ignore_patterns("__pycache__", "results", "tasks"))
            r = subprocess.run([sys.executable, str(kit / "t2.py"), "generate", "--scratch", os.environ["T2_SCRATCH"], "--repo", "click"],
                               capture_output=True, text=True, timeout=300)
            self.assertEqual(r.returncode, 0, r.stderr)
            for man in ("MANIFEST.json", "MANIFEST-mixed.json", "MANIFEST-heavy.json"):
                self.assertEqual((kit / "dev" / "tasks" / man).read_text(), (k.DEV / "tasks" / man).read_text(), man)
                for rel in json.loads((k.DEV / "tasks" / man).read_text()):
                    self.assertEqual((kit / rel).read_bytes(), (k.HERE / rel).read_bytes(), rel)
        finally:
            shutil.rmtree(tmp)


class DevHeavyTests(unittest.TestCase):
    """The `heavy` set of the development set (dev/taskdefs_heavy.py, dev/heavy/, dev/tasks/click/heavy)."""

    def setUp(self):
        self.saved = {n: getattr(k, n) for n in KIT_GLOBALS}
        k.select_kit(["click"])

    def tearDown(self):
        for n, v in self.saved.items():
            setattr(k, n, v)

    def td(self):
        return [m for m in k.extra_taskdefs() if "heavy" in m.SETS][0]

    def test_files_match_their_manifest(self):
        manifest = json.loads((k.DEV / "tasks" / "MANIFEST-heavy.json").read_text())
        self.assertEqual(sum(1 for p in manifest if p.endswith("prompt.md")), 10)
        for rel, digest in manifest.items():
            self.assertTrue(rel.startswith("dev/tasks/click/heavy/"), rel)
            self.assertEqual(k.sha256(k.HERE / rel), digest, rel)
            if rel.endswith("prompt.md"):
                self.assertNotIn("treeyard", (k.HERE / rel).read_text().lower(), rel)

    def test_set_json(self):
        spec = k.set_spec(k.TASKS_DIR, "click", "heavy")
        self.assertEqual((spec["agents"], spec["mix"].count("claude"), spec["mix"].count("codex")), (10, 6, 4))
        self.assertGreaterEqual(spec["limits"]["timeout_s"], 1800)
        self.assertEqual(len(spec["integration"]), 9)
        apart = sum(spec["mix"][a - 1] != spec["mix"][b - 1] for a, b in (it["pair"] for it in spec["integration"]))
        self.assertEqual(apart, 7, "two triangles of pairs: 7 of 9 pairs get different harnesses")
        for it in spec["integration"]:
            d = k.TASKS_DIR / "click" / "heavy" / "integration" / it["id"]
            self.assertTrue((d / "hidden" / it["test"]).is_file())
            for p in it["pair"]:
                self.assertTrue((d / f"without-{p}.patch").is_file(), (it["id"], p))

    def test_tasks_are_feature_sized(self):
        for t in self.td().TASKS["click"]["heavy"]:
            files = {e["file"] for e in t["edits"]}
            self.assertGreaterEqual(len(files), 3, t["id"])
            lines = sum(1 for e in t["edits"] for x in k.unified({e["file"]: e.get("old", "")} if e["op"] == "replace" else {},
                                                                  {e["file"]: e.get("new", e.get("text"))}).splitlines()
                        if x[:1] in "+-" and x[:3] not in ("+++", "---"))
            self.assertGreaterEqual(lines, 140, t["id"])
            self.assertGreaterEqual(t["test"].count("assert"), 8, t["id"])
            self.assertGreaterEqual(t["test"].count("\ndef test_"), 5, t["id"])

    def test_patches_parse_into_ops(self):
        ops = self.td().ops_from_patch(
            "diff --git a/n.py b/n.py\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/n.py\n@@ -0,0 +1,2 @@\n+a\n+b\n"
            "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n x\n-y\n+Y\n z\n@@ -9,2 +9,3 @@\n p\n+q\n r\n")
        self.assertEqual(ops, [{"op": "create", "file": "n.py", "text": "a\nb\n"},
                               {"op": "replace", "file": "f", "old": "x\ny\nz\n", "new": "x\nY\nz\n"},
                               {"op": "replace", "file": "f", "old": "p\nr\n", "new": "p\nq\nr\n"}])


if __name__ == "__main__":
    unittest.main()
