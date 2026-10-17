"""Tests for bench.py on a tiny synthetic repo (no network, a few seconds). Run: make bench-test."""

import argparse
import contextlib
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent


def git(repo, *args):
    env = {**bench.DETERMINISTIC, "PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"}
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True, env=env).stdout


def tiny_history(root):
    """Ten commits. Commits 2 and 3 edit the same line from the same file, so an agent that
    starts commit 3 on its true parent conflicts once commit 2 is on main."""
    src = root / "tiny"
    src.mkdir()
    git(src, "init", "-q", "-b", "main")
    (src / "a.txt").write_text("".join(f"line {i}\n" for i in range(20)))
    (src / "README.md").write_text("tiny\n")
    git(src, "add", ".")
    git(src, "commit", "-q", "-m", "c0")
    for c in range(1, 10):
        if c in (2, 3):
            lines = (src / "a.txt").read_text().splitlines()
            lines[5] = f"line 5 edited by c{c}"
            (src / "a.txt").write_text("\n".join(lines) + "\n")
        else:
            (src / f"f{c}.txt").write_text(f"{c}\n")
        git(src, "add", ".")
        git(src, "commit", "-q", "-m", f"c{c}")
    return src, git(src, "rev-parse", "HEAD").strip()


class BenchTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        src, rev = tiny_history(self.root)
        bench.REPOS["tiny"] = {
            "url": str(src), "rev": rev, "branch": "main",
            "seed": ["mkdir -p deps && echo dep > deps/lib.txt"],
            "setups": {"checkout": [], "clone-deps": ["@clone deps"]},
            "ci": "test -f a.txt", "ci_setup": "clone-deps",
            "cow": {"layer": "deps=a.txt", "setup": "test -f deps/lib.txt || (mkdir -p deps && echo dep > deps/lib.txt)"},
        }
        self.base = ["--dir", str(self.root / "work"), "--oss", str(self.root / "oss")]

    def tearDown(self):
        bench.REPOS.pop("tiny", None)
        self.tmp.cleanup()

    def run_bench(self, *argv):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            bench.main([*self.base, *argv])

    def result(self, name):
        return json.loads((self.root / "work" / "results" / f"{name}.json").read_text())

    def test_summary_and_percentile(self):
        self.assertEqual(bench.pct([3, 1, 2], 0.5), 2)
        s = bench.summary([1.0, 2.0, 3.0, 4.0])
        self.assertEqual((s["n"], s["max"], s["mean"]), (4, 4.0, 2.5))
        self.assertEqual(bench.summary([]), {})

    def test_one_agent_has_no_late_conflicts(self):
        src, _ = bench.source(argparse.Namespace(dir=str(self.root / "work"), oss=str(self.root / "oss")), "tiny")
        g = bench.Git(src)
        commits = bench.history(g, "bench-base", 9)
        r = bench.replay_git(g, commits, 1, 1)
        self.assertEqual(r["late_conflicts"], 0)
        self.assertEqual(r["faithful_waves"], r["waves"])
        self.assertTrue(r["final_tree_equal"])

    def test_wave_sees_the_conflict_late_but_overlap_is_visible_early(self):
        seeds = "1,2,3,4,5,6"
        self.run_bench("conflicts", "--repo", "tiny", "--commits", "9", "--agents", "1,9", "--seeds", seeds)
        runs = self.result("conflicts-tiny")["runs"]
        self.assertTrue(all(r["late_conflicts"] == 0 for r in runs if r["agents"] == 1))
        wave = [r for r in runs if r["agents"] == 9]
        # Commit 3 rewrites the line commit 2 wrote. When 3 lands before 2, git sees the conflict
        # only at merge time, though the two changes shared a path from the start.
        hit = [r for r in wave if r["late_conflicts"]]
        self.assertTrue(hit and len(hit) < len(wave), wave)
        for r in hit:
            self.assertEqual((r["late_conflicts"], r["visible_before_merge"], r["redone"]), (1, 1, 1), r)
            self.assertEqual(r["top_conflicted_paths"], [["a.txt", 1]], r)
        for r in wave:
            self.assertEqual((r["changes"], r["landed"]), (9, 9), r)
            self.assertTrue(r["final_tree_equal"], r)

    def test_history_times_log_and_blame(self):
        self.run_bench("history", "--repo", "tiny", "--commits", "9", "--files", "3", "--n", "1,4")
        r = self.result("history-tiny")
        self.assertEqual(r["files"][0], "a.txt")  # touched by two commits
        self.assertEqual((r["blame_s"]["n"], [b["n"] for b in r["bursts"]]), (3, [1, 4]))

    def test_workspaces_measure_time_and_disk_and_clean_up(self):
        self.run_bench("workspaces", "--repo", "tiny", "--n", "1,3", "--jobs", "2")
        runs = self.result("workspaces-tiny")["runs"]
        self.assertEqual([(r["setup"], r["n"]) for r in runs],
                         [("checkout", 1), ("checkout", 3), ("clone-deps", 1), ("clone-deps", 3)])
        for r in runs:
            self.assertEqual(r["per_workspace_s"]["n"], r["n"])
            self.assertGreater(r["du_total_kb"], 0)
        self.assertFalse((self.root / "work" / "ws" / "tiny-checkout-3").exists())
        listed = git(self.root / "work" / "src" / "tiny", "worktree", "list").splitlines()
        self.assertEqual(len(listed), 2, listed)  # the bare-ish clone itself and the seed

    def test_cow_workspaces_reuse_the_layer_and_sync(self):
        built = [p for p in (ROOT / "target/release/rootstock", ROOT / "target/debug/rootstock") if p.exists()]
        binary = max(built, key=lambda p: p.stat().st_mtime, default=None)
        if binary is None:
            self.skipTest("no rootstock binary built yet")
        self.run_bench("cow", "--repo", "tiny", "--bin", str(binary), "--n", "1,3", "--jobs", "2")
        r = self.result("cow-tiny")
        self.assertEqual(r["first"]["layers"][0]["restored"], False)
        runs = r["runs"]
        self.assertEqual([(x["mode"], x["n"]) for x in runs],
                         [(m, n) for m in ("files", "build", "spare-files", "spare-build") for n in (1, 3)])
        for x in runs:
            self.assertEqual(x["layer_restored"], x["n"])
            self.assertEqual(x["sync_s"]["n"], x["n"])
            self.assertEqual(x["from_spare"], x["n"] if x["mode"].startswith("spare") else 0)

    def test_serial_queue_waits_grow_with_position(self):
        self.run_bench("queue", "--repo", "tiny", "--n", "1,5")
        runs = {r["n"]: r for r in self.result("queue-tiny")["runs"]}
        self.assertEqual(runs[1]["wait_s"]["max"], 0.0)
        self.assertEqual(runs[5]["ci_s"]["n"], 5)
        self.assertEqual(runs[5]["ci_failed"], 0)
        self.assertGreater(runs[5]["wait_s"]["max"], runs[5]["ci_s"]["p50"] * 3)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            bench.main([*self.base, "report"])
        self.assertIn("### Serial merge queue", out.getvalue())
        self.assertIn("| tiny | 5 |", out.getvalue())


if __name__ == "__main__":
    unittest.main()
