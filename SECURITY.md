# Security

TreeYard writes the history of a repository. This file gives the threat model, the controls that
TreeYard has, the tests that check them, and the limits that are known.

## Report a problem

Send the report to 320388883+exit0-run@users.noreply.github.com. Do not open a public issue for a security problem.

## Assets

1. The history of `main`: the SQLite metadata (`<data>/treeyard.db`) and the export repo (`<data>/git`).
2. The file system of the machine that runs the server, the node daemon or a workspace.
3. API tokens. The database keeps only the SHA-256 hash of each token.

## Actors and trust

| Actor | Trust | What it can do |
|---|---|---|
| Server operator | full | owns the data directory and `treeyard.toml`; only the operator sets test, bisect and regenerate commands |
| Agent with a token | partial | reads and writes the paths of its token globs; lands its own changes |
| Person with `git push` | partial | pushes fast-forward commits to the branch; each commit lands like an agent change |
| Anyone else on the network | none | must get no access |
| Repository content | none | file names, symlinks, trees and blobs can come from an attacker (an imported repo or a push) |

## Threats and controls

| Threat | Control | Test |
|---|---|---|
| Anyone on the network reads the repo or lands on main | `serve` refuses an address other than loopback while no API token exists. `--insecure-open` overrides this. | `open_server_refuses_a_public_address` |
| A path escapes the work tree (`../x`, `/etc/passwd`) | `safety::check_path` refuses empty, `.` and `..` components, absolute paths, NUL and backslash in every `apply`, import, push, workspace and test run | `hostile_paths_and_sizes_are_refused_over_the_api`, `hostile_repo_content_is_refused_at_import`, `safety::tests` |
| A path writes into a git directory (`.git/hooks`, `.GIT/config`, `.g‌it` on HFS+, `git~1` on NTFS) | `safety::is_dotgit` compares without case, without HFS+ ignorable code points and without trailing dots and spaces | `hostile_paths_are_refused`, `hostile_push_is_refused` |
| A symlink in the tree redirects a later write (`link -> /etc`, then `link/passwd`) | `safety::check_tree` refuses a tree that has a file where a directory of another path is. `safety::dest` never writes through a symlink. | `tree_with_a_file_over_a_directory_is_refused`, `dest_never_writes_through_a_symlink` |
| A hostile push leaves objects in the export repo | TreeYard deletes the pack of a rejected push (like git's push quarantine) | `hostile_push_is_refused` (`git fsck --strict` is clean after the rejection) |
| A large request uses all memory | HTTP body limit 64 MiB. A file in `apply`: 48 MiB. Edits per `apply`: 10 000. Path: 4096 bytes. Component: 255 bytes. `read` inline: 32 MiB. | `hostile_paths_and_sizes_are_refused_over_the_api` |
| Too many requests at once | At most `--max-inflight` API calls (default 256) run at once. Other calls get `503 busy`, and the client retries them. The `events` long poll does not count. | `busy_server_answers_503_and_clients_retry` |
| Two servers write one data directory | `serve` holds an exclusive lock on `<data>/serve.lock` | `second_server_on_the_same_data_is_refused` |
| A crash during land corrupts history | Order of writes: git objects, then one SQLite transaction (the commit point), then the git ref. `Repo::open` writes the ref again from SQLite. | `crash_during_land_recovers_at_every_failpoint` (the process aborts at 6 points) |
| A lost reply makes a client land twice | `land` of a landed change answers like the first land (`"repeated": true`). The client repeats only reads and `land`. | `lost_land_reply_is_retried_and_lands_once`, `duplicate_lands_race_and_land_once` |
| A token leaks into a log | Tokens are stored as hashes. The audit log stores the token id, never the secret. | `tests/iam.rs` |
| An agent runs a command on the server | Agents cannot send commands. `bisect`, `test` and `regenerate` run only commands from `treeyard.toml`. | `tests/derived.rs` (bisect), `tests/coordination.rs` |

The server never runs a hook or a filter from the repository. It reads git objects with `git cat-file`
and writes loose objects itself. It runs `git merge-file`, `git upload-pack` and `git index-pack` on its
own export repo, with arguments that it makes itself. Revision ids from a push must be full hex ids.

## Durability

A process crash at any point of a land is safe (see the test above). Power loss is a different case:
TreeYard does not call `fsync` on loose objects, and SQLite runs with `synchronous = NORMAL` in WAL
mode. After a power loss, the last landed change can be missing. Then the next `Repo::open` points the
ref at the last change that SQLite has.

## Known limits

- There is no TLS. Put a TLS proxy in front of a server that listens beyond loopback.
- Tokens do not expire. Revoke a token with `treeyard token revoke`.
- There is no rate limit per token. The in-flight limit applies to the server as a whole.
- A workspace `sync` follows a symlink that an agent puts in place of a tracked directory, so it can
  send a file from outside the workspace. Run agents in a sandbox when they are not trusted.
- On a case-insensitive file system (APFS default, NTFS) two paths that differ only in case share one
  file in a workspace. git has the same limit. The server keeps both paths.
- A commit message can contain lines that look like `TreeYard-*` trailers. Tools must take the
  trailers from the last paragraph only.
- The data directory and `treeyard.toml` must be writable only by the operator.

## Where secrets live

TreeYard keeps no secret in the repository or in its data directory, except token hashes. An agent
gets its token from the `TREEYARD_TOKEN` environment variable. A mirror remote uses the git
credential helper of the operator.
