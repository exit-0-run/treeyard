# Access control and audit

Rootstock has API tokens: one per agent (or person), each with read and write path globs, plus
admin tokens. An audit log records every write and every denied call.

## Turn auth on

A new data directory is open: every caller acts as `open` with full rights, as in v0.1. The first
token turns auth on, and auth stays on, even after you revoke every token.

```sh
rootstock token create --data D --name root --admin          # prints the token once
rootstock token create --data D --name claude-1 --write 'src/**' --write 'tests/**'
rootstock token create --data D --name reviewer               # read everything, write nothing
rootstock token list --data D                                  # ids, names, globs; never secrets
rootstock token revoke --data D <id>
```

The server keeps only a SHA-256 hash of each token. Store a token where the agent reads it, for
example in the environment variable `ROOTSTOCK_TOKEN` or in a secret manager. Do not commit it.
`rootstock call`, `rootstock mcp` and the workspace commands send `ROOTSTOCK_TOKEN` as
`Authorization: Bearer <token>`. An admin can also call `token_create`, `token_list` and
`token_revoke` through the API.

## Rules

- No token, or an unknown or revoked token: HTTP 401. A valid token without the right: HTTP 403.
- `read` globs (default `**`) limit `read`, `blame`, `outline` and `blobs`. They filter the results of
  `list`, `tree`, `grep`, `symbols`, `diff`, `log`, `change` and `changes`.
- `write` globs limit `apply` and `resolve` (per path), and `land` and `revert` (every path of the
  change). A token without write globs is read-only and cannot open a change.
- A non-admin token always acts as the agent with its own name: `open_change` with another `agent` is
  denied. It may only change, land or abandon its own changes.
- Admin tokens may do everything, including `config`, `queue_run`, `test_main` and token calls.
- `audit` needs an admin token or a token that reads every path. `bisect` needs a token that reads
  every path and may write something (it runs a command on the server).
- Globs use the gitignore style of `rootstock.toml`: `Cargo.lock` matches at any depth, `docs/` covers
  a directory, `src/*.rs` stays in one directory, `**` is everything.
- Git over HTTP: git sends the token as the Basic password (or user name), for example
  `git clone http://x:$ROOTSTOCK_TOKEN@host:7420/git`. A clone needs a token that reads every path. A
  push needs a write token, and every path the pushed commits change must match its write globs.

## Audit log

One row per call of `open_change`, `declare`, `apply`, `resolve`, `rebase`, `land`, `abandon`,
`revert`, `export_change`, `review_close`, `queue_run`, `test_main`, `bisect`, `token_create`,
`token_revoke`, per landed commit of a git push, and per denied call of any operation. A row holds:

| Field | What |
|---|---|
| `seq`, `time_ms` | order and time (Unix milliseconds) |
| `actor`, `token` | the token name and id (`open` when auth is off, `cli` for the token CLI) |
| `agent`, `model`, `session`, `intent` | from the change (set at `open_change`) |
| `op`, `status`, `detail` | the operation; `ok`, a land status (`landed`, `conflict`, ...), `denied` or `error`; the error text |
| `change`, `changeset`, `commit` | what the call touched or produced |
| `paths` | the paths it changed (or tried to change) |

```sh
rootstock call audit '{"path": "src/parser", "status": "landed"}'   # who landed what under src/parser
rootstock call audit '{"agent": "claude-1", "limit": 50}'
rootstock call audit '{"status": "denied"}'                          # every refusal
```

Filters: `agent`, `actor`, `change`, `op`, `status`, `path` (a file or a directory), `since` and
`until` (Unix seconds); page with `before` (the `seq` the reply returns). The row is written in the
same lock hold as the operation. Reads are not audited. Token secrets never reach the log.

## Cost

HTTP land latency on ripgrep `3fce3b5bb023`, 300 sequential lands of one new file each, release
build, Apple M5 Pro, 2026-10-17: auth off p50 1.63 ms, p95 2.4 ms; auth on p50 1.75 ms, p95 2.67 ms.
Both write one audit row per land (300 rows). Command: `python3 bench/derived.py --repo <ripgrep
clone> --dir <scratch> --history 0 --only land [--auth]`.

## Tests

`cargo test --test iam`: 401 and 403 cases, read filtering of `list`, `tree`, `grep` and `blobs`,
write globs, ownership, revocation, the audit rows of a land and of each denial, git clone and push
with tokens, and MCP with `ROOTSTOCK_TOKEN`.

## Limits

- Events (`events`, `GET /api/events`) carry path names to any valid token.
- A partial reader cannot clone over git: git has no partial checkout of a single history.
- No token expiry and no scopes per operation yet. No TLS: put the server behind a TLS proxy.
