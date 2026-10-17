# rootstock — working notes

Version control server for agent swarms (Rust, single binary). Product docs, plan and state live in
`~/appnice-apps/projects/rootstock/`.

## Commands

- `make check`: `cargo fmt --check`, `clippy -D warnings`, `cargo test`. Run before every commit.
- `make test` · `make release` (binary at `target/release/rootstock`).
- Rust comes from Homebrew `rustup` (`/opt/homebrew/opt/rustup/bin`); the Makefile puts it on PATH.
- Run: `rootstock init --data D --from REPO`, `rootstock serve --data D`, `rootstock call <op> '<json>'`,
  `rootstock mcp --url URL --agent NAME`.

## Layout

| File | What |
|---|---|
| `src/model.rs` | Entry, Manifest (im::OrdMap), Changeset (content-addressed), Change, Conflict |
| `src/store.rs` | SQLite: blobs written by agents, changesets, changes, refs, changeset → git commit |
| `src/git.rs` | object ids, loose-object writes, tree/commit encoding, `git cat-file --batch` reader |
| `src/merge.rs` | per-path 3-way merge (entry level, then `diffy` line merge) |
| `src/repo.rs` | changes, rebase, land, revert, export, read queries |
| `src/rpc.rs` | the one operation table (HTTP, MCP and CLI share it) + MCP tool schemas |
| `src/server.rs`, `src/client.rs`, `src/mcp.rs`, `src/main.rs` | axum API, ureq client, stdio MCP, CLI |
| `tests/swarm.rs` | phase-1 gate: 50 agents, every merge checked against `git merge-tree` |
| `tests/git_bridge.rs` | tree bytes vs git, fsck, clone, trailers, revert, MCP over stdio |

## Conventions and gotchas

- All state goes through `Repo::with` (one mutex). Landing is serialised by it, so there is no git lock.
- A change's `head` equals `base` while it is empty; otherwise its only parent is `base`.
- Git tree order: a directory sorts as if its name ended in `/` (`write_dir`). Any change to tree or
  commit encoding must keep `import_tree_matches_git_byte_for_byte` green.
- Commits must stay deterministic: their dates and author come from the changeset, never from the clock.
- Loose refs are written next to the packed-refs of the import clone; the loose ref wins.
- Never add `.github/workflows/` (no GitHub Actions on appnice repos). Commit author: 320388883+exit0-run@users.noreply.github.com.
