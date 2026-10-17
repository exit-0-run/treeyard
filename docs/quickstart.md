# Quickstart

Ten minutes from an empty directory to a landed change that people can `git clone`. Every block
tagged `sh` on this page runs as is: `sh scripts/check-quickstart.sh docs/quickstart.md` runs them
in a new empty directory and stops at the first error. The transcript of the last run is at the end
of this page.

You need `git` (2.44 or newer), `python3` and a `rootstock` binary on `PATH`. Install one with
[install.md](install.md), or build it from source with `make release`.

## 1. A repository to serve

Any git repository with a `main` branch works. This one has one file with a typo.

```sh
mkdir demo-repo
git -C demo-repo init -q -b main
printf 'hello wrold\n' > demo-repo/README.md
git -C demo-repo add README.md
git -C demo-repo -c user.name=me -c user.email=me@example.com commit -q -m "first commit"
```

## 2. Start the server

`init` imports the tip of `main`. `serve` listens on `127.0.0.1:7420` (loopback only, until you
create a token: [access.md](access.md)).

```sh
rootstock init --data rs-data --from demo-repo
rootstock serve --data rs-data > serve.log 2>&1 &
SERVER=$!
sleep 1
rootstock call status
```

## 3. One agent: open a change, edit, land

An agent needs no checkout. It opens a change, sends an edit, and lands. The server does the merge.

```sh
CH=$(rootstock call open_change '{"agent": "a1", "intent": "fix a typo"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
echo "change: $CH"
rootstock call apply "{\"change\": \"$CH\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"wrold\", \"new\": \"world\"}}]}" > /dev/null
rootstock call land "{\"change\": \"$CH\"}" | grep '"status"'
```

## 4. People keep git

The landed change is a normal git commit on `main`, with the agent and the intent in its trailers.

```sh
git clone -q http://127.0.0.1:7420/git people-clone
cat people-clone/README.md
git -C people-clone log --format='%s | %an' -n 2
```

## 5. Two agents, one line: the conflict comes back as a task

```sh
A=$(rootstock call open_change '{"agent": "alice", "intent": "greet loudly"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
B=$(rootstock call open_change '{"agent": "bob", "intent": "greet politely"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
rootstock call apply "{\"change\": \"$A\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"hello\", \"new\": \"HELLO\"}}]}" > /dev/null
rootstock call apply "{\"change\": \"$B\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"hello\", \"new\": \"good day\"}}]}" | grep -m 1 '"kind"'
rootstock call land "{\"change\": \"$A\"}" | grep '"status"'
rootstock call land "{\"change\": \"$B\"}" | python3 -c 'import json,sys; r=json.load(sys.stdin); t=r["change"]["conflicts"][0]; print(r["status"], t["path"], "theirs_by", [x["agent"] for x in t["theirs_by"]])'
rootstock call resolve "{\"change\": \"$B\", \"path\": \"README.md\", \"take\": \"theirs\"}" > /dev/null
rootstock call apply "{\"change\": \"$B\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"HELLO\", \"new\": \"HELLO and good day\"}}]}" > /dev/null
rootstock call land "{\"change\": \"$B\"}" | grep '"status"'
```

Bob's `apply` already reports the collision (`"kind": "conflict"`) before anyone lands. His first
`land` returns `conflict` with a task that names Alice. He takes main's side (`resolve`), adds his
words to it with one more `apply`, and lands.

## 6. An agent that needs a real directory

A workspace is a copy-on-write clone of main, bound to one change. Tools, editors and test runners
work in it as in a checkout.

```sh
export ROOTSTOCK_HOME="$PWD/node"
rootstock workspace create ws1 --agent carol --intent "add a line" --git > /dev/null
WS=$(rootstock workspace path ws1)
echo "second line" >> "$WS/README.md"
git -C "$WS" status --short
rootstock workspace land ws1 | grep '"status"'
rootstock call read '{"path": "README.md"}' | grep '"content"'
```

## 7. Connect an agent through MCP

`rootstock mcp` is a stdio MCP server. This sends two JSON-RPC messages by hand and counts the tools.
Setup for Claude Code, Codex and OpenCode: [mcp.md](mcp.md).

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | rootstock mcp --agent a1 | tail -n 1 | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]["tools"]), "tools")'
```

## 8. Stop

```sh
kill "$SERVER"
```

Next: [concepts.md](concepts.md) for the model, [api.md](api.md) for every operation,
[workspaces.md](workspaces.md) and [coordination.md](coordination.md) for the details.

## Transcript of the last check

macOS (below): the `rootstock` binary from `dist/install.sh` in an empty prefix, `PATH` limited to
that prefix and the system directories. The same blocks also passed on 2026-10-18 in `alpine:3`
containers with the static Linux binaries (arm64 native, x86_64 under emulation), git 2.54.0.

<!-- transcript:start -->
```text
# rootstock 0.1.0 | Darwin arm64 | 2026-10-17T22:21Z | fresh dir rootstock-quickstart.I3Y35G
$ mkdir demo-repo
$ git -C demo-repo init -q -b main
$ printf 'hello wrold\n' > demo-repo/README.md
$ git -C demo-repo add README.md
$ git -C demo-repo -c user.name=me -c user.email=me@example.com commit -q -m "first commit"
$ rootstock init --data rs-data --from demo-repo
imported main from demo-repo as changeset 84b5212f3d58
$ rootstock serve --data rs-data > serve.log 2>&1 &
$ SERVER=$!
$ sleep 1
$ rootstock call status
{
  "branch": "main",
  "cache": {
    "manifests": 1,
    "objects": 0
  },
  "commit": "1c346904095aea2eacbcfc64c8e879b0303cfc17",
  "files": 1,
  "main": "84b5212f3d58b6f31db5a4d339955e1eb1cca9447e55fa3f4674edcce5d3885f",
  "open_changes": []
}
$ CH=$(rootstock call open_change '{"agent": "a1", "intent": "fix a typo"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
$ echo "change: $CH"
change: ch_d4eb51181474
$ rootstock call apply "{\"change\": \"$CH\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"wrold\", \"new\": \"world\"}}]}" > /dev/null
$ rootstock call land "{\"change\": \"$CH\"}" | grep '"status"'
  "status": "landed"
$ git clone -q http://127.0.0.1:7420/git people-clone
$ cat people-clone/README.md
hello world
$ git -C people-clone log --format='%s | %an' -n 2
fix a typo | a1
first commit | me
$ A=$(rootstock call open_change '{"agent": "alice", "intent": "greet loudly"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
$ B=$(rootstock call open_change '{"agent": "bob", "intent": "greet politely"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
$ rootstock call apply "{\"change\": \"$A\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"hello\", \"new\": \"HELLO\"}}]}" > /dev/null
$ rootstock call apply "{\"change\": \"$B\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"hello\", \"new\": \"good day\"}}]}" | grep -m 1 '"kind"'
          "kind": "conflict",
$ rootstock call land "{\"change\": \"$A\"}" | grep '"status"'
  "status": "landed"
$ rootstock call land "{\"change\": \"$B\"}" | python3 -c 'import json,sys; r=json.load(sys.stdin); t=r["change"]["conflicts"][0]; print(r["status"], t["path"], "theirs_by", [x["agent"] for x in t["theirs_by"]])'
conflict README.md theirs_by ['alice']
$ rootstock call resolve "{\"change\": \"$B\", \"path\": \"README.md\", \"take\": \"theirs\"}" > /dev/null
$ rootstock call apply "{\"change\": \"$B\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"HELLO\", \"new\": \"HELLO and good day\"}}]}" > /dev/null
$ rootstock call land "{\"change\": \"$B\"}" | grep '"status"'
  "status": "landed"
$ export ROOTSTOCK_HOME="$PWD/node"
$ rootstock workspace create ws1 --agent carol --intent "add a line" --git > /dev/null
$ WS=$(rootstock workspace path ws1)
$ echo "second line" >> "$WS/README.md"
$ git -C "$WS" status --short
 M README.md
$ rootstock workspace land ws1 | grep '"status"'
    "status": "landed"
$ rootstock call read '{"path": "README.md"}' | grep '"content"'
  "content": "HELLO and good day world\nsecond line\n",
$ printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | rootstock mcp --agent a1 | tail -n 1 | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]["tools"]), "tools")'
35 tools
$ kill "$SERVER"
```
<!-- transcript:end -->
