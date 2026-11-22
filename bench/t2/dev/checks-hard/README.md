# Free checks of the hard set

- `validate-hard.txt`: `validate --set hard` (every hidden test fails on the pin and passes with its
  reference; the suite stays green; all 13 resolved together; each integration test fails on the pin,
  with either partner alone and with all tasks but either partner).
- `rehearsal-summary.txt`: `rehearse --set hard` (oracle and noop in arms A, A2, B, BN), collected with
  `collect --summary`. Arms B and BN ran with the server build `treeyard-v8` (version 0.2.0).
- `variants/` and `variants.txt`: other correct solutions of h12 and h14 with other import styles, and
  their hidden tests on them (kit test `DevHardVariantTests`, with `T2_SCRATCH=S`).
- `solo-oracle.txt`, `solo-noop.txt`: `solo --set hard --rep 1 --harness oracle|noop` (one agent per task,
  alone on the pin).
- `solo-dry-run.txt`: `solo --set hard --rep 1 --harness real --dry-run`, the 13 commands (prompts and
  local paths left out). They are the commands of arm A's 13 agents, with the same prompts.

Commands, with `S` the scratch directory and `D` a results directory:

```sh
python3 bench/t2/t2.py setup --scratch S --repo click
python3 bench/t2/t2.py generate --scratch S --repo click
python3 bench/t2/t2.py validate --scratch S --repo click --set hard --results bench/t2/dev/checks-hard
T2_TREEYARD=BIN python3 bench/t2/t2.py rehearse --scratch S --repo click --set hard --results D
python3 bench/t2/t2.py collect --results D --out D/metrics.json --summary bench/t2/dev/checks-hard/rehearsal-summary.txt
python3 bench/t2/t2.py solo --scratch S --repo click --set hard --rep 1 --harness oracle --results D > bench/t2/dev/checks-hard/solo-oracle.txt
python3 bench/t2/t2.py solo --scratch S --repo click --set hard --rep 1 --harness noop --results D > bench/t2/dev/checks-hard/solo-noop.txt
```

Reference size per task: the `awk` line of [`../checks-heavy/README.md`](../checks-heavy/README.md), run in
`tasks/click/hard`.
