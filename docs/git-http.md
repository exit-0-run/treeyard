# Plain git for people: smart HTTP and a mirror

People keep plain git. `rootstock serve` also answers git smart HTTP under `/git`:

```sh
git clone http://127.0.0.1:7420/git myrepo
cd myrepo
# edit, commit
git push origin main
```

## What each command does

| Command | Server side |
|---|---|
| `git clone`, `git fetch`, `git pull` | `git upload-pack --stateless-rpc` on the export repo (`<data>/git`). Protocol v2 is passed through (`Git-Protocol` header). |
| `git push origin main` | Rootstock reads the ref updates itself, stores the pack with `git index-pack --fix-thin`, then lands under the same lock as agents. |
| `rootstock mirror --data D --remote URL [--every N]` | `git push URL refs/heads/main:refs/heads/main` from the export repo, never forced. |

## Push rules

1. Only `refs/heads/<branch>` (the imported branch, usually `main`) accepts pushes. Other refs get
   `ng ... rootstock accepts pushes to refs/heads/main only; agents use the API for other work`.
2. The pushed commit must contain main's exported commit (a fast-forward). Otherwise the server refuses
   and tells the person to fetch and rebase. Rootstock never rewrites main.
3. Deleting main is refused.
4. Each commit on the path from main to the pushed commit becomes one change (agent `git:<author name>`,
   intent = subject, message = full commit message). Each change goes through `land`. The pushed commits
   stay byte-identical, so the person's clone stays in sync and needs no rewrite.
5. Agents see the push at once: `read`, `grep` and `log` answer from the new main. An agent change
   opened before the push rebases onto it when it lands, like after any other landing.
6. A merge commit whose first parent is not on main (the person merged `origin/main` into local work)
   lands as one change: the server takes the parent that descends from main.

What a person sees on a forced push of diverged work (from
`cargo test --test git_http non_fast -- --nocapture`, 2026-10-17):

```
remote: rootstock: rejected: non-fast-forward: your commit does not contain main e6440dfe8256; run `git pull --rebase` and push again (Rootstock never rewrites main)
To http://127.0.0.1:58713/git
 ! [remote rejected] main -> main (non-fast-forward: your commit does not contain main e6440dfe8256; run `git pull --rebase` and push again (Rootstock never rewrites main))
error: failed to push some refs to 'http://127.0.0.1:58713/git'
```

A push without `--force` from a stale clone never reaches the server: git refuses it locally
(`[rejected] main -> main (fetch first)`), because the server advertises the current main.

## Mirror

`rootstock mirror --data D --remote URL` pushes main to a read-only mirror, for example a GitHub repo
that people browse. With `--every N` it keeps running and pushes every N seconds (a no-op when main did
not move). The push is never forced: if someone wrote to the mirror, the mirror push fails with
"if the mirror moved on its own, it is not read-only". Credentials for the remote come from the
normal git credential helper of the user who runs the command. Rootstock stores no token.
The tests use a local bare repo only.

## Measured

15-core CPU, 48 GB RAM, macOS, git 2.54, release build, 2026-10-17. Command:

```sh
ROOTSTOCK_SCRATCH=/private/tmp/rootstock-harness/005/real \
ROOTSTOCK_REAL_REPO=/private/tmp/rootstock-harness/oss/<repo> \
  cargo test --release --test git_http real_repo -- --ignored --nocapture
```

The test imports the repo's current branch, clones the export repo with `git clone --no-local`
(baseline: plain git, no HTTP), clones it again over Rootstock HTTP, then pushes one commit over HTTP.
Three runs each, seconds:

| Repo (rev) | Plain `git clone --no-local` | Clone over Rootstock HTTP | Push of one commit |
|---|---|---|---|
| ripgrep (3fce3b5) | 0.384, 0.386, 0.390, 0.363 | 0.433, 0.433, 0.493, 0.438 | 0.190, 0.227, 0.299, 0.209 |
| requests (611c6162) | 0.493, 0.502, 0.559, 0.528 | 0.550, 0.613, 0.584, 0.607 | 0.238, 0.299, 0.247, 0.261 |

(Four values: one warm-up run and three repeats.) The export repo of ripgrep passes `git fsck`. The
requests export shows 1 fsck error, the same `badTimezone` error that the upstream clone has in commit
5e6ecdad9f69 (`git fsck --strict` on the clone).

## Limits

- No authentication or permissions yet: anyone who reaches the port can push to main. Bind to
  127.0.0.1 (the default) or put the server behind a proxy that checks identity.
- The pushed pack sits in memory while it is parsed (body limit 2 GiB). Shallow clones cannot push.
- Pushes are not checked by CI or hooks; they land like an agent change.
- Each push adds one pack file to the export repo; nothing repacks it yet.
