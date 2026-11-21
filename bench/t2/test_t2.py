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
        self.assertTrue(r.prompt(r.agents[0]).startswith(want + "\n\n# task 1"))
        self.assertTrue((self.tmp / "results" / "B-mini-colliding-r1-stub-try-1.json").exists())


if __name__ == "__main__":
    unittest.main()
