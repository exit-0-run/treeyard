//! The agent tool set through the stdio bridge (`treeyard mcp --tools agent`), driven with JSON-RPC
//! as a harness drives it: the listed tools, the lean change replies, collisions sent once, the
//! read cap, grep context, conflicts as regions and `undoes` with its advice once.

mod common;

use common::{call, fixture_repo, spawn_server};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use treeyard::repo::Repo;

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

impl Bridge {
    /// The bridge, with default checkouts under `tmp`.
    fn start(url: &str, agent: &str, tools: &str, tmp: &Path) -> Bridge {
        let mut child = Command::new(env!("CARGO_BIN_EXE_treeyard"))
            .args(["mcp", "--url", url, "--agent", agent, "--tools", tools])
            .env("TMPDIR", tmp)
            .env_remove("TREEYARD_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Bridge { child, stdin, stdout, id: 0 }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// The tool's reply text (compact JSON) and its parsed value.
    fn tool(&mut self, name: &str, args: Value) -> (String, Value) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        let text = r["result"]["content"][0]["text"].as_str().unwrap().to_string();
        assert!(r["result"]["isError"] != true, "{name}: {text}");
        let v = serde_json::from_str(&text).unwrap();
        (text, v)
    }

    fn close(mut self) {
        drop(self.stdin);
        self.child.wait().unwrap();
    }
}

fn server(root: &Path) -> String {
    let big: String = (0..3000).map(|i| format!("value_{i} = compute({i})\n")).collect();
    let src = fixture_repo(root, 2, 30, &[("big.py", big.as_str())]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

#[test]
fn the_agent_tool_set_over_stdio() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path());
    let mut a = Bridge::start(&url, "a", "agent", tmp.path());
    let mut b = Bridge::start(&url, "b", "agent", tmp.path());

    let init = a.rpc("initialize", json!({}));
    assert!(init["result"]["instructions"].as_str().unwrap().contains("agent tool set"), "{init}");
    let list = a.rpc("tools/list", json!({}));
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    let mut want = treeyard::rpc::AGENT_TOOLS.to_vec();
    want.push("checkout");
    assert_eq!(names, want);

    // An empty change: only what an agent uses, compact.
    let (text, ch) = a.tool("open_change", json!({"intent": "edit f00", "paths": ["f00.txt"]}));
    assert!(!text.contains('\n') && !text.contains(": "), "compact JSON: {text}");
    for k in ["base", "head", "created", "model", "session", "view", "revisions", "landed_as", "group", "conflicts"] {
        assert!(ch.get(k).is_none(), "{k} in {ch}");
    }
    let ida = ch["id"].as_str().unwrap().to_string();
    let (_, chb) = b.tool("open_change", json!({"intent": "edit f00 too"}));
    let idb = chb["id"].as_str().unwrap().to_string();

    // a collision comes in full once, then as one line while it stays the same.
    a.tool("apply", json!({"change": ida, "edits": [{"path": "f00.txt", "replace": {"old": "file 0 line 3\n", "new": "a's line 3\n"}}]}));
    let (_, v) = b.tool("apply", json!({"change": idb, "edits": [{"path": "f00.txt", "replace": {"old": "file 0 line 3\n", "new": "b's line 3\n"}}]}));
    assert_eq!(v["collisions"][0]["kind"], "conflict", "{v}");
    let (_, v) = b.tool("apply", json!({"change": idb, "edits": [{"path": "f01.txt", "content": "b\n"}]}));
    assert!(v.get("collisions").is_none(), "{v}");
    assert!(v["collisions_unchanged"].as_str().unwrap().contains(&ida), "{v}");
    let (_, full) = b.tool("change", json!({"change": idb}));
    assert_eq!(full["collisions"][0]["change"], ida.as_str(), "change lists them in full: {full}");

    // read: a whole big file is cut at 50 KiB with a note; a range is not.
    let (text, r) = a.tool("read", json!({"path": "big.py"}));
    assert!(text.len() < 60 * 1024, "{} bytes", text.len());
    assert_eq!((r["truncated"].clone(), r["total_lines"].clone()), (json!(true), json!(3000)));
    assert!(r["note"].as_str().unwrap().contains("start ="), "{r}");
    let (_, r) = a.tool("read", json!({"path": "big.py", "start": 2990}));
    assert_eq!(r["lines"], json!([2990, 3000]));

    // grep: no index statistics; context lines.
    let (_, g) = a.tool("grep", json!({"pattern": "value_1500 ", "context": 1}));
    assert!(g.get("index").is_none() && g.get("at").is_none(), "{g}");
    assert_eq!(g["matches"][0]["before"], json!(["value_1499 = compute(1499)"]), "{g}");

    // a lands; b's land conflicts: regions, no whole files, the help once.
    let (_, l) = a.tool("land", json!({"change": ida}));
    assert_eq!(l["status"], "landed", "{l}");
    let (text, l) = b.tool("land", json!({"change": idb}));
    assert_eq!(l["status"], "conflict", "{l}");
    let c = &l["change"]["conflicts"][0];
    assert!(c.get("text").is_none() && c["regions"][0]["text"].as_str().unwrap().contains("a's line 3"), "{c}");
    assert_eq!(text.matches("Each conflict lists regions").count(), 1, "{text}");
    // take = ours drops a's landed line: undoes says so, with the advice once.
    let (text, v) = b.tool("resolve", json!({"change": idb, "path": "f00.txt", "take": "ours"}));
    assert_eq!(v["undoes"][0]["text"], "a's line 3\n", "{v}");
    assert_eq!(text.matches("another agent").count(), 1, "{text}");
    // ... and land holds it: status undoes, the entries with their text, the advice, nothing else.
    let land = list["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "land").unwrap().clone();
    assert!(
        land["inputSchema"]["properties"]["undo"].is_object()
            && land["inputSchema"]["properties"]["reason"].is_object()
    );
    assert!(land["description"].as_str().unwrap().contains("undoes (not landed"), "{land}");
    let (text, l) = b.tool("land", json!({"change": idb}));
    assert_eq!(l["status"], "undoes", "{l}");
    assert_eq!(l["undoes"][0]["text"], "a's line 3\n", "{l}");
    assert_eq!(l["undoes"][0]["agent"], "a", "{l}");
    assert!(l["undoes"][0].get("changeset").is_none() && l.get("merged_from").is_none(), "{l}");
    assert!(l["advice"].as_str().unwrap().contains("undo = true"), "{l}");
    assert!(text.len() < 1200, "{} bytes: {text}", text.len());
    let (_, l) = b.tool("land", json!({"change": idb, "undo": true, "reason": "my task replaces line 3"}));
    assert_eq!((l["status"].as_str(), l["undid"]["by"].as_str()), (Some("landed"), Some("b")), "{l}");
    a.close();
    b.close();
}

#[test]
fn tools_all_keeps_the_full_replies() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path());
    let mut a = Bridge::start(&url, "a", "all", tmp.path());
    let names = a.rpc("tools/list", json!({}))["result"]["tools"].as_array().unwrap().len();
    assert!(names > 40, "{names}");
    let (_, ch) = a.tool("open_change", json!({"intent": "x"}));
    assert!(ch.get("base").is_some() && ch.get("revisions").is_some(), "{ch}");
    let (_, g) = a.tool("grep", json!({"pattern": "line 3"}));
    assert!(g.get("index").is_some(), "{g}");
    // The read cap holds for every MCP client.
    let (_, r) = a.tool("read", json!({"path": "big.py"}));
    assert_eq!(r["truncated"], true);
    // The HTTP API is unchanged: a whole file.
    let whole = call(&url, "read", json!({"path": "big.py"}));
    assert!(whole.get("truncated").is_none() && whole["content"].as_str().unwrap().ends_with("compute(2999)\n"));
    a.close();
}

/// The fixed calls of one small task, before and after `apply` could open the change and write
/// its checkout: open_change, apply, checkout, land (4) against apply, land (2). Another agent
/// lands in another file meanwhile, which asks for no rebase.
#[test]
fn the_minimal_flow_is_apply_then_land() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path());
    let mut a = Bridge::start(&url, "a", "agent", tmp.path());
    a.rpc("initialize", json!({}));
    let list = a.rpc("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    let apply = tools.iter().find(|t| t["name"] == "apply").unwrap();
    assert!(apply["inputSchema"]["properties"]["checkout"].is_object(), "the bridge's own parameter: {apply}");
    assert_eq!(apply["inputSchema"]["required"], json!(["edits"]), "{apply}");
    for t in ["apply", "land"] {
        let d = tools.iter().find(|x| x["name"] == t).unwrap()["description"].as_str().unwrap();
        assert!(d.contains("intent") && d.contains("checkout.dir"), "{t}: {d}");
    }
    let bytes = serde_json::to_string(&list["result"]).unwrap().len();
    eprintln!("agent tools/list: {bytes} bytes");

    // Before: 4 calls (open_change, apply, checkout, land).
    let (_, ch) = a.tool("open_change", json!({"intent": "edit f00 line 1"}));
    let id = ch["id"].as_str().unwrap().to_string();
    a.tool("apply", json!({"change": id, "checkout": false, "edits": [{"path": "f00.txt", "replace": {"old": "file 0 line 1\n", "new": "one\n"}}]}));
    let (_, co) = a.tool("checkout", json!({"change": id}));
    assert!(co.get("head").is_none() && co.get("seconds").is_none(), "lean checkout: {co}");
    assert_eq!(a.tool("land", json!({"change": id})).1["status"], "landed");

    // After: 2 calls. apply with intent opens the change, applies, writes the checkout.
    let (text, v) = a.tool(
        "apply",
        json!({"intent": "edit f00 line 2", "paths": ["f00.txt"], "edits": [{"path": "f00.txt", "replace": {"old": "file 0 line 2\n", "new": "two\n"}}]}),
    );
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["agent"], "a", "the bridge names the agent: {v}");
    let dir = std::path::PathBuf::from(v["checkout"]["dir"].as_str().unwrap());
    assert!(dir.starts_with(tmp.path()), "{v}");
    assert!(std::fs::read_to_string(dir.join("f00.txt")).unwrap().contains("two\n"));
    assert!(v["checkout"].get("main_moved").is_none() && v["checkout"].get("advice").is_none(), "{v}");
    eprintln!("apply with intent and checkout: {} bytes", text.len());
    // Another agent lands in another file: no main_moved, no advice, nothing to do.
    let other =
        call(&url, "apply", json!({"agent": "b", "intent": "f01", "edits": [{"path": "f01.txt", "content": "b\n"}]}));
    assert_eq!(call(&url, "land", json!({"change": other["id"]}))["status"], "landed");
    let (_, co) = a.tool("checkout", json!({"change": id}));
    assert!(co.get("main_moved").is_none() && co.get("advice").is_none(), "{co}");
    assert_eq!(co["main_moved_elsewhere"], true, "{co}");
    let (_, l) = a.tool("land", json!({"change": id}));
    assert_eq!(l["status"], "landed", "{l}");
    assert_eq!(std::fs::read_to_string(dir.join("f01.txt")).unwrap(), "b\n", "the landed tree is in the directory");

    // A failed apply with intent leaves no open change behind.
    let r = a.rpc("tools/call", json!({"name": "apply", "arguments": {"intent": "x", "edits": [{"path": "f00.txt", "replace": {"old": "no such text", "new": "y"}}]}}));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(r["result"]["isError"] == true && text.contains("was abandoned"), "{text}");
    assert_eq!(call(&url, "changes", json!({"state": "open"}))["changes"], json!([]));
    // change and intent together, or neither, is an error.
    let r = a.rpc("tools/call", json!({"name": "apply", "arguments": {"change": id, "intent": "x", "edits": []}}));
    assert!(r["result"]["isError"] == true, "{r}");
    a.close();
}

/// One call where agents used several: a file as grep `path`, several ranges or files per `read`.
#[test]
fn grep_on_a_file_and_read_of_several_ranges_or_files_through_the_bridge() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path());
    let mut a = Bridge::start(&url, "a", "agent", tmp.path());
    let (_, v) = a.tool("grep", json!({"pattern": "line 7$", "path": "f01.txt"}));
    assert_eq!(v, json!({"matches": [{"line": 8, "path": "f01.txt", "text": "file 1 line 7"}], "truncated": false}));
    let (_, v) = a.tool("read", json!({"path": "big.py", "ranges": [[1, 1], [2999, 3000]]}));
    assert_eq!(v["ranges"][1]["content"], "value_2998 = compute(2998)\nvalue_2999 = compute(2999)\n", "{v}");
    assert!(v.get("truncated").is_none(), "ranges are never cut: {v}");
    let (text, v) = a.tool("read", json!({"files": [{"path": "big.py"}, {"path": "f00.txt", "start": 2, "end": 2}]}));
    assert_eq!(v["files"][0]["truncated"], true, "the 50 KB cap holds for files without a range");
    assert_eq!(v["files"][1]["content"], "file 0 line 1\n");
    assert!(text.len() < 56 * 1024, "{} bytes", text.len());
    a.close();
}
