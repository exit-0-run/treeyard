# Free checks of the long set

- `validate-long.txt`: `validate --repo networkx --set long` (scratch with `setup --repo networkx`).
- `rehearsal-summary.txt`: `rehearse --repo networkx --set long --arms A,A2,A3`, collected with `collect --summary`.
- `solo-oracle.txt`, `solo-noop.txt`: `solo --repo networkx --set long --rep 1 --harness oracle|noop`, when present.
