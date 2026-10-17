#!/usr/bin/env python3
"""Probe: how long agents wait to land while the first grep builds the trigram index.

Stdlib only. Makes a fresh data dir from REPO, starts the server, runs one grep in a thread
and lands one-line changes in a loop until the grep returns. Prints one JSON line.

  python3 bench/grep_stall.py BINARY REPO DATA_DIR [BRANCH]
"""

import json
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import scale  # noqa: E402


def main(argv):
    binary, repo, data = argv[1], argv[2], argv[3]
    branch = argv[4] if len(argv) > 4 else "master"
    shutil.rmtree(data, ignore_errors=True)
    subprocess.run([binary, "init", "--data", data, "--from", repo, "--branch", branch], check=True, capture_output=True)
    srv = scale.Server(binary, data)
    paths, _ = scale.sample(repo, 200, 1)
    res = {}

    def grep():
        t = time.perf_counter()
        scale.post(srv.url, "grep", {"pattern": r"EXPORT_SYMBOL_GPL\(", "max": 100})
        res["grep_s"] = round(time.perf_counter() - t, 2)

    th = threading.Thread(target=grep)
    th.start()
    lands = []
    while th.is_alive():
        i = len(lands)
        t0 = time.perf_counter()
        scale.land_one(srv.url, f"p{i}", paths[i % len(paths)], f"/* p {i} */\n")
        lands.append(time.perf_counter() - t0)
    th.join()
    srv.stop()
    lands.sort()
    res["lands_during_grep"] = len(lands)
    res["land_round_trip_max_s"] = round(lands[-1], 3) if lands else None
    res["land_round_trip_p50_ms"] = round(lands[len(lands) // 2] * 1000, 1) if lands else None
    print(json.dumps(res))


if __name__ == "__main__":
    main(sys.argv)
