# Free checks of the long set

Made with `S` a scratch set up with `setup --repo networkx`, `T2_TREEYARD` the TreeYard 0.2.0 binary.

- `validate-long.txt`: `validate --repo networkx --set long`.
- `rehearsal-summary.txt`: `rehearse --repo networkx --set long --arms A,A2,A3,BN,B`, collected with
  `collect --summary`.
- `solo-oracle.txt`, `solo-noop.txt`: `solo --repo networkx --set long --rep 1 --harness oracle|noop`.
- `dry-run.txt`: `run --arm A|A3 ... --harness real --dry-run` and `solo ... --harness real --dry-run`.
