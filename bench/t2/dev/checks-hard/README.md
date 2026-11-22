# Free checks of the hard set

- `validate-hard.txt`: `validate --set hard` (every hidden test fails on the pin and passes with its
  reference; the suite stays green; all 13 resolved together; each integration test fails on the pin,
  with either partner alone and with all tasks but either partner).
- `rehearsal-summary.txt`: `rehearse --set hard` (oracle and noop in arms A, A2, B, BN), collected with
  `collect --summary`. Arms B and BN ran with the server build `treeyard-v8` (version 0.2.0).

Commands, with `S` the scratch directory and `D` a results directory:

```sh
python3 bench/t2/t2.py setup --scratch S --repo click
python3 bench/t2/t2.py generate --scratch S --repo click
python3 bench/t2/t2.py validate --scratch S --repo click --set hard --results bench/t2/dev/checks-hard
T2_TREEYARD=BIN python3 bench/t2/t2.py rehearse --scratch S --repo click --set hard --results D
python3 bench/t2/t2.py collect --results D --out D/metrics.json --summary bench/t2/dev/checks-hard/rehearsal-summary.txt
```

Reference size per task: the `awk` line of [`../checks-heavy/README.md`](../checks-heavy/README.md), run in
`tasks/click/hard`.
