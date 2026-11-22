# Free checks of the heavy set

- `validate-heavy.txt`: `validate --set heavy` (every hidden test fails on the pin and passes with its
  reference; the suite stays green; all 10 resolved together; each integration test).
- `rehearsal-summary.txt`: `rehearse --set heavy` (oracle and noop in arms A, A2, B), collected with
  `collect --summary`. Arm B ran with the server build `treeyard-v4` of 2026-11-21.

Reference size per task (changed lines of `reference.patch`, in all and under `src/`):

```sh
cd bench/t2/dev/tasks/click/heavy
for d in $(ls -d [0-9]*-* | sort -n); do p=$d/reference.patch
  all=$(grep -E '^[+-]' $p | grep -vE '^(\+\+\+|---) ' | wc -l)
  src=$(awk '/^diff --git/{f=$3} /^[+-]/ && !/^(\+\+\+|---) / && f ~ /^a\/src\// {n++} END{print n+0}' $p)
  echo "$d files=$(grep -c '^diff --git' $p) all=$all src=$src"; done
```
