# Workspaces: a real directory per agent (phase 2)

Agents that only read and edit use the API. Agents that must build and run tests need a directory.
`treeyard workspace` gives each agent one, bound to one change on the server:

```sh
export TREEYARD_URL=http://127.0.0.1:7420          # a running `treeyard serve`
export TREEYARD_HOME=~/.treeyard                   # node home (default)
treeyard daemon &                                   # streams every workspace's edits to the server

treeyard workspace create fix-1 --agent claude-1 --intent "fix the glob bug" --setup "cargo build --offline -q"
cd "$(treeyard workspace path fix-1)"
# ... any tool edits files here: an editor, sed, Claude Code, Codex; tests run here ...
cargo test -p globset
treeyard workspace land fix-1                       # sync, then land on main
treeyard workspace remove fix-1
```

## What happens

1. `create` opens a change on the server (`open_change`, base = main unless `--base`).
2. The node materialises the base changeset once, in `bases/<changeset>/`. The first base comes from the
   `tree` and `blobs` operations. A later base starts as a clone of the newest base and fetches only the
   blobs that differ.
3. The workspace is a copy-on-write clone of the base (`ws/<name>/`).
4. Dependency layers are restored, the `--setup` command runs, and a missing layer is saved after it.
5. `.treeyard/env` holds the workspace environment: `TREEYARD_URL`, `TREEYARD_CHANGE`, `PORT` and a
   block of ports (10 unless `--ports N` or the config says otherwise), and a database name. `treeyard workspace env NAME` prints it as `export` lines;
   `treeyard workspace exec NAME -- CMD` runs a command with it and syncs after it (no command: the
   config's `run`).
6. `sync` (by hand, after `exec`, or from the daemon) sends every file that differs from the last sync as
   one new changeset of the same change. The server sees each agent's work live, and `apply` returns the
   `overlaps` with other open changes (also written to `.treeyard/overlaps.json`).
7. `remove` syncs first, under the workspace lock, so an edit that the daemon has not sent yet still
   reaches the change. When that sync fails (for example the server is down), nothing is deleted unless
   `--force`. `--abandon` drops the change instead. The record is marked `removing` before the directory
   goes; `treeyard daemon` finishes such a remove at start. Only a directory under `ws/` is deleted.
8. `restore NAME --to N` is undo inside the change. It syncs first, calls `restore_change` (the change
   gets a new head with the tree of checkpoint `N`; `0` = no edits), then rewrites only the files that
   differ and updates the sync state. Other files keep their mtimes, so a build stays warm. The head
   before the restore becomes a checkpoint (`restore.undo` in the reply): restore to it to undo.
   Checkpoints come from the `checkpoint` operation, for example once per agent turn
   ([mcp.md](mcp.md#one-checkpoint-per-turn)).

## Repo config file (`.treeyard/workspace.toml`)

A repo can say how its workspaces are made. `create` reads `.treeyard/workspace.toml` from the base
tree of the new change. A node file `<home>/workspace.toml` overrides it key by key, and CLI flags
override both. Every key is optional:

```toml
setup = "pnpm install --offline"         # like --setup
teardown = "docker compose down"         # remove runs it before it deletes the directory
layers = ["node_modules=pnpm-lock.yaml"] # like --layer
copy = [".env*", "certs/*.pem"]          # gitignored files to copy in from copy_from
copy_from = "~/src/app"                  # like --copy-from DIR
ports = 20                               # ports per workspace (default 10), like --ports N
run = "pnpm dev --port $PORT"            # what `workspace exec NAME` runs with no command
```

- `copy` globs use gitignore syntax. The lines of a root `.worktreeinclude` file (in the base tree or in
  the source directory) add more globs. A file is copied only when it is not tracked, not already there,
  and ignored by the workspace's `.gitignore`. Other files are skipped with a reason in the `copy` field of
  the reply, so a copied secret never reaches the server. The copy never walks into `.git`, `target`,
  `node_modules` or `.venv`. A relative `--copy-from` or `copy_from` is relative to the current directory.
- `teardown` runs with the workspace environment after the final sync. Its output goes to
  `<home>/logs/NAME.teardown.log`. A failure is reported in the `teardown` field of the reply; the remove
  still happens.
- The file is a normal repo file: an edit of it in a workspace syncs like any other file. Other paths
  under `.treeyard/` stay private to the node.
- Flags of `create`: `--setup CMD`, `--teardown CMD`, `--run CMD`, `--ports N`, `--copy-from DIR`,
  `--layer`, `--no-layers`.

Trust: the repo file runs commands on the node that creates the workspace, like a Makefile does when you
run `make`. Create workspaces only from repos whose main you trust. Commands that the server runs (tests
of the landing queue, regenerate drivers) are never read from the repo: they stay in
`<data>/treeyard.toml` ([docs/coordination.md](coordination.md)).

## States and logs

The record `meta/NAME.json` has a `state`. `workspace list` shows it with the other fields.

| State | Meaning |
|---|---|
| `creating` | the change is open; the base, the clone, the ports and the copied files are in progress |
| `setting_up` | layers are restored and the setup command runs |
| `ready` | done; only a ready workspace is synced by the daemon |
| `removing` | `remove` sent the last edits and deletes the directory |
| `failed` | a step failed: `error` holds the message; the record and the directory stay until `remove` |

`create_log` (`<home>/logs/NAME.create.log`) has one line per step with the seconds since the start.
`setup_log` (`<home>/logs/NAME.setup.log`) has the output of the setup command. `counts` has `files`
(files cloned), `layers_restored`, `layers_saved`, `files_copied` and `seconds`. A sync of a workspace in
`creating` is refused, and `remove` of such a workspace does not sync (it has no edits yet).

## Sparse workspaces (`--paths`)

`treeyard workspace create NAME --paths services/api/,libs/core/` makes a workspace with only these
directories plus the files at the root, like git's sparse checkout in cone mode. The files directly in
a parent of a listed directory (here `services/` and `libs/`) are there too. On a large monorepo this
cuts the clone and the scans of every sync.

- The paths are relative directories of the base tree, at most 200. They are kept in the record
  (`paths`), and `workspace list` shows them.
- The directories that are not there are never deletions: a sync compares only the files the workspace
  holds. A file written outside the cone (for example a new `other/x`) is not sent either, so an agent can
  not overwrite a file it never saw.
- With `--git`, the `.git` is set to the same cone (`git sparse-checkout set --cone`), so `git status`
  shows no deletions.
- The node still keeps the whole base once (`bases/<changeset>/`); only the workspace is sparse.
  `--paths` does not work with `--strategy overlay`, and a sparse create does not take a spare.

## Start from a dirty checkout (`--from-dir`)

`treeyard workspace create NAME --from-dir PATH` starts the agent from what a person sees in the git
checkout at PATH, not only from main. The new workspace gets every uncommitted edit of that checkout:

- files that differ from its HEAD, staged or not, and files deleted there;
- untracked files that its `.gitignore` does not ignore.

The files are written right after the clone (so the setup sees them) and sent as the change's first
changeset when the workspace is ready. The reply's `from_dir` field lists `written`, `deleted`,
`skipped` (with the reason, for example `outside --paths`) and the `sync`. PATH can be any directory of
the checkout. Files are copied whole: when the checkout's HEAD is not the base of the change, the copied
files also carry the differences between the two commits. Git runs there with `GIT_OPTIONAL_LOCKS=0` and
reads only, so it never blocks the person's own git. A PATH that is not a git checkout fails the create.

## Mirror: one fixed directory (`workspace mirror`)

Some apps run only from one fixed path, with one port or one database. `treeyard workspace mirror NAME
--into DIR` writes the files of NAME's change into DIR now and after every sync of NAME (by hand, after
`exec`, `land`, or from the daemon). Run the app in DIR; edit in the workspace.

- Only files that differ from the last mirror write are written. Unchanged files keep their mtimes, so
  file watchers and build tools see only real changes. Files that the change no longer has are deleted.
- DIR holds `.treeyard-mirror.json`, the list of files the mirror wrote. Files that the mirror never
  wrote (a local `.env`, build output) are left alone.
- DIR must not be under `<home>/ws`, and it must be new, empty, or an earlier mirror. One DIR serves one
  workspace at a time. `treeyard workspace mirror NAME --off` stops it and leaves DIR as it is.
- Only synced files reach DIR: gitignored files and layer directories do not.
- The sync reply has a `mirror` field (`written`, `deleted`, `kept`, or `error`). A mirror that cannot be
  written does not fail the sync.

## Limits and gc (`workspace gc`)

`treeyard workspace gc [--max 25] [--max-gb 50] [--dry-run]` removes workspaces until at most `--max` are
left and their directories use at most `--max-gb`. The order:

1. workspaces whose change landed or was abandoned, and workspaces whose create failed;
2. then the least recently synced (the last sync that sent a changeset, else the create time).

gc never removes a workspace that is younger than 10 minutes, that is in state `creating`, `setting_up`
or `removing`, or whose directory has edits that no sync sent yet (the same scan as `sync`). It removes
through `remove`, so the last edits are sent and `teardown` runs. Only one gc runs at a time on a node
(`<home>/gc.lock`); a second one stops with an error. The reply lists `removed` (with the change state and
the bytes), `skipped` (with the reason) and `freed_bytes`. Sizes are allocated blocks: on APFS a clone
counts in full, so the real disk cost is lower than the number.

`treeyard daemon --gc-every HOURS` runs gc at start and then every HOURS hours (default 0 = never), with
`--gc-max 25` and `--gc-max-gb 50`. Each gc that removed something is one JSON line.

## Clone strategies (`--strategy`)

| Strategy | Platform | How | Tested |
|---|---|---|---|
| `clone` | macOS, APFS | one `clonefile(2)` call clones the whole base directory | `cargo test` on this Mac |
| `reflink` | Linux btrfs/XFS (`FICLONE`), APFS (`clonefile` per file) | walk the tree, clone each file's blocks | Mac; Linux XFS in Docker (`make test-linux`) |
| `overlay` | Linux, root or a user namespace | `mount -t overlay` with the base as lower dir; writes go to `<ws>.upper` | Linux XFS in Docker as root (`make test-linux`) |
| `hardlink` | any | link each file to a read-only base; tools that replace files (sed -i, most editors) work, in-place writers fail | `cargo test` |
| `copy` | any | plain copy; the last fallback | `cargo test` |
| `auto` | any | `clone`, then `reflink`, then `copy`, by what the file system supports | `cargo test`, `make test-linux` |

`make test-linux` (2026-10-17, colima VM, kernel `Linux 6.8.0-117-generic`, image `rust:1`, cargo
1.99.1) printed for the container's overlayfs root: all suites `ok` (lib 15, workspace 9 with the
overlay test skipped). With `TMPDIR` on a loop-mounted XFS (`mkfs.xfs -m reflink=1`) as root:
`reflink_on_a_reflink_file_system ... ok` (`auto` picks `reflink`) and `overlay_workspace_when_root ... ok`.

Clones keep each file's mtime. The sync stat cache depends on it, and build tools see unchanged sources
as unchanged: `cargo build` in a new ripgrep workspace with the `target` layer is a no-op.

## Dependency layers (`--layer DIR=LOCKFILE[,LOCKFILE]`)

A layer is every directory named `DIR` in the workspace that is not a tracked directory, keyed by a
SHA-256 of `DIR`, OS, CPU and the lockfile contents. The first workspace with `--setup` builds it; later
workspaces with the same lockfiles get a copy-on-write clone (their own writes stay private). A changed
lockfile is a new key. Without `--layer`, layers come from the lockfiles at the root: `Cargo.lock` →
`target`, `pnpm-lock.yaml`/`package-lock.json`/`yarn.lock`/`bun.lockb` → `node_modules`,
`uv.lock`/`poetry.lock`/`Pipfile.lock`/`requirements*.txt` → `.venv`. Layer directories are never synced.

Caveat: a `.venv` holds absolute paths (script shebangs, editable installs that point at the first
workspace). Share it only if it is relocatable; the bench does not share it for requests.

## Warm pool (`treeyard daemon --spares N`, `treeyard workspace spare --count N`)

A spare is a ready directory for current main: a base clone with every saved layer restored. `create`
takes a matching spare with one rename. The daemon refills the pool in its own thread and drops spares
of an old main. On astro, cloning `node_modules` (562 directories) is most of the create time, and a
spare moves that cost out of the request.

## git in a workspace (`--git`)

With `--git`, the base gets a `.git`: HEAD and the index at the base commit, objects through an
alternate to a bare cache in the node home (`git.cache`, fetched from the server's `/git`). The clone
copies it, so `git status`, `git diff`, `git log` and even local commits work. The sync never sends
`.git`; the server gets files. `core.checkStat=minimal` keeps a fresh clone clean (new inodes and ctimes).

The node is polite to the agent's own git:

- The daemon does not sync a workspace while a git operation runs in it: `.git/MERGE_HEAD`,
  `rebase-merge`, `rebase-apply`, `CHERRY_PICK_HEAD` or `REVERT_HEAD` exists, or `.git/index.lock` is younger
  than 30 s (an older lock is stale and does not count). Such a sync would send half-applied files and
  conflict markers. The daemon tries again after 1 s, 5 s, 30 s and then every 120 s, and logs each skip
  as one JSON line (`skipped`, `retry_s`). `sync`, `exec` and `remove` by hand do not wait.
- `gc` skips such a workspace too.
- Git status probes that the node runs in a checkout (`create --from-dir`) set `GIT_OPTIONAL_LOCKS=0`, so
  they never take `index.lock` and never block a git command of the agent.

## Daemon

`treeyard daemon [--debounce-ms 300] [--rescan-s 30] [--spares N] [--gc-every HOURS]` watches `ws/` (FSEvents on macOS,
inotify on Linux, through the `notify` crate). A workspace syncs when no event came for the debounce time.
Events under `target`, `node_modules`, `.venv`, `.git`, `__pycache__`, `.pytest_cache` and `.treeyard`
are ignored; a rescan every 30 s catches anything missed. Each sync that sent something is one JSON line
on stdout.

## Node home layout

```text
node.lock                 node-wide lock (bases, port slots, spares)
bases/<changeset>/        materialised tree; bases/<changeset>.json = entries + stat cache
layers/<dir>-<key>/       dependency dirs at their workspace paths; <dir>-<key>.json lists them
spares/<id>/              warm pool; <id>.json = base, layer keys, git
ws/<name>/                the workspace; ws/<name>/.treeyard/env
meta/<name>.json          workspace record; meta/<name>.state.json = what the server has
logs/<name>.<step>.log    create (steps), setup and teardown output
workspace.toml            node config; overrides the repo's .treeyard/workspace.toml
gc.lock                   held while a gc runs
git.cache/                bare git cache for --git
```

## Limits

- One way only: the workspace pushes edits to the server. A server-side rebase does not rewrite the
  workspace's files; `land` rebases on the server, and a conflict comes back as a task in `land`'s reply.
- No lazy checkout: the base holds the whole tree, and a `--paths` workspace holds its whole cone (a
  virtual file system is phase 5).
- `overlay` needs mount rights. In a container that is `--privileged` or a user namespace with
  overlayfs; it was tested only as root in Docker.
- Removing a workspace with a large `node_modules` clone takes seconds (APFS deletes each cloned inode).
- No auth: anyone who reaches the server can sync into any change.
