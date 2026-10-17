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
   block of 10 ports, and a database name. `treeyard workspace env NAME` prints it as `export` lines;
   `treeyard workspace exec NAME -- CMD` runs a command with it and syncs after it.
6. `sync` (by hand, after `exec`, or from the daemon) sends every file that differs from the last sync as
   one new changeset of the same change. The server sees each agent's work live, and `apply` returns the
   `overlaps` with other open changes (also written to `.treeyard/overlaps.json`).
7. `remove` syncs first, under the workspace lock, so an edit that the daemon has not sent yet still
   reaches the change. When that sync fails (for example the server is down), nothing is deleted unless
   `--force`. `--abandon` drops the change instead. The record is marked `removing` before the directory
   goes; `treeyard daemon` finishes such a remove at start. Only a directory under `ws/` is deleted.

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

## Daemon

`treeyard daemon [--debounce-ms 300] [--rescan-s 30] [--spares N]` watches `ws/` (FSEvents on macOS,
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
git.cache/                bare git cache for --git
```

## Limits

- One way only: the workspace pushes edits to the server. A server-side rebase does not rewrite the
  workspace's files; `land` rebases on the server, and a conflict comes back as a task in `land`'s reply.
- No partial or lazy checkout: the base holds the whole tree (a virtual file system is phase 5).
- `overlay` needs mount rights. In a container that is `--privileged` or a user namespace with
  overlayfs; it was tested only as root in Docker.
- Removing a workspace with a large `node_modules` clone takes seconds (APFS deletes each cloned inode).
- No auth: anyone who reaches the server can sync into any change.
