//! `read` over the HTTP API: whole files and line ranges.

mod common;

use common::*;
use serde_json::{Value, json};
use std::sync::Arc;
use treeyard::repo::Repo;

fn serve(root: &std::path::Path) -> String {
    let src = fixture_repo(root, 1, 30, &[("blob.bin", "\0\u{1}\u{2}binary\n")]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn lines(from: usize, to: usize) -> String {
    (from - 1..to).map(|j| format!("file 0 line {j}\n")).collect()
}

fn fails(url: &str, p: Value) -> String {
    let e = treeyard::client::call(url, "read", p).expect_err("read should fail");
    format!("{e:#}")
}

#[test]
fn read_returns_a_line_range_or_the_whole_file() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve(tmp.path());

    // No range: the reply as before, plus total_lines.
    let all = call(&url, "read", json!({"path": "f00.txt"}));
    assert_eq!(all["content"], lines(1, 30));
    assert_eq!(all["total_lines"], 30);
    assert_eq!(all["size"], lines(1, 30).len());
    assert!(all.get("lines").is_none(), "{all}");
    for k in ["path", "at", "blob", "mode"] {
        assert!(all.get(k).is_some(), "{k} missing: {all}");
    }

    // A range in the middle.
    let mid = call(&url, "read", json!({"path": "f00.txt", "start": 10, "end": 12}));
    assert_eq!(mid["content"], lines(10, 12));
    assert_eq!(mid["lines"], json!([10, 12]));
    assert_eq!(mid["total_lines"], 30);
    assert_eq!(mid["blob"], all["blob"]);

    // Past the end: clamped to the last line.
    let tail = call(&url, "read", json!({"path": "f00.txt", "start": 28, "end": 1000}));
    assert_eq!(tail["content"], lines(28, 30));
    assert_eq!(tail["lines"], json!([28, 30]));

    // start only, end only.
    let from = call(&url, "read", json!({"path": "f00.txt", "start": 29}));
    assert_eq!(from["content"], lines(29, 30));
    assert_eq!(from["lines"], json!([29, 30]));
    let head = call(&url, "read", json!({"path": "f00.txt", "end": 2}));
    assert_eq!(head["content"], lines(1, 2));
    assert_eq!(head["lines"], json!([1, 2]));

    // Bad ranges get a clear error.
    assert!(fails(&url, json!({"path": "f00.txt", "start": 31})).contains("past the end"));
    assert!(fails(&url, json!({"path": "f00.txt", "start": 5, "end": 4})).contains("after end"));
    assert!(fails(&url, json!({"path": "f00.txt", "start": 0})).contains("1-based"));
}

#[test]
fn read_range_on_a_binary_file_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve(tmp.path());
    let whole = call(&url, "read", json!({"path": "blob.bin"}));
    assert!(whole["content_b64"].is_string(), "{whole}");
    assert!(whole.get("total_lines").is_none(), "{whole}");
    let e = fails(&url, json!({"path": "blob.bin", "start": 1, "end": 2}));
    assert!(e.contains("binary file"), "{e}");
}

#[test]
fn start_and_end_are_rejected_where_they_mean_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve(tmp.path());
    let e = treeyard::client::call(&url, "list", json!({"path": "", "start": 1})).expect_err("list takes no start");
    assert!(format!("{e:#}").contains("unknown field"), "{e:#}");
}

#[test]
fn read_range_on_an_empty_file_and_on_a_change() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 30, &[("empty.txt", "")]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));

    // An empty file: a range from line 1 is empty, not an error; past it is still an error.
    for p in [json!({"path": "empty.txt", "start": 1}), json!({"path": "empty.txt", "start": 1, "end": 5})] {
        let r = call(&url, "read", p);
        assert_eq!(r["content"], "", "{r}");
        assert_eq!(r["lines"], json!([1, 0]), "{r}");
        assert_eq!((r["total_lines"].as_u64(), r["size"].as_u64()), (Some(0), Some(0)), "{r}");
    }
    assert!(fails(&url, json!({"path": "empty.txt", "start": 2})).contains("past the end"));

    // `at` = a change id: the range comes from the change; `size` is the whole file.
    let ch =
        call(&url, "open_change", json!({"agent": "a", "intent": "edit line 5"}))["id"].as_str().unwrap().to_string();
    call(
        &url,
        "apply",
        json!({"change": ch, "edits": [{"path": "f00.txt", "replace": {"old": "file 0 line 4\n", "new": "changed\nand added\n"}}]}),
    );
    let r = call(&url, "read", json!({"path": "f00.txt", "at": ch, "start": 5, "end": 6}));
    assert_eq!(r["content"], "changed\nand added\n");
    assert_eq!(r["lines"], json!([5, 6]));
    assert_eq!(r["total_lines"], 31);
    let whole = call(&url, "read", json!({"path": "f00.txt", "at": ch}));
    assert_eq!(r["size"], whole["content"].as_str().unwrap().len(), "size counts the whole file");
    assert_eq!(call(&url, "read", json!({"path": "f00.txt", "start": 5, "end": 5}))["content"], "file 0 line 4\n");

    // The same through the stdio bridge, as an agent calls it.
    use std::io::{BufRead, Write};
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_treeyard"))
        .args(["mcp", "--url", &url, "--agent", "a"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let args = [json!({"path": "empty.txt", "start": 1}), json!({"path": "f00.txt", "at": ch, "start": 5, "end": 5})];
    let mut got = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let msg =
            json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call", "params": {"name": "read", "arguments": a}});
        writeln!(stdin, "{msg}").unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert!(reply["result"]["isError"] != true, "{reply}");
        got.push(serde_json::from_str::<Value>(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap());
    }
    drop(stdin);
    child.wait().unwrap();
    assert_eq!((got[0]["content"].as_str(), got[0]["lines"].clone()), (Some(""), json!([1, 0])));
    assert_eq!(got[1]["content"], "changed\n");
}

#[test]
fn max_bytes_cuts_a_whole_file_read_at_a_line_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve(tmp.path());
    let one = lines(1, 1).len();
    let v = call(&url, "read", json!({"path": "f00.txt", "max_bytes": one * 3 + 2}));
    assert_eq!(v["content"], lines(1, 3), "whole lines only: {v}");
    assert_eq!(
        (v["lines"].clone(), v["truncated"].clone(), v["total_lines"].clone()),
        (json!([1, 3]), json!(true), json!(30))
    );
    assert!(v["note"].as_str().unwrap().contains("start = 4"), "{v}");
    // A range is never cut; a file that fits is not marked.
    let r = call(&url, "read", json!({"path": "f00.txt", "start": 1, "max_bytes": 10}));
    assert_eq!(r["content"], lines(1, 30));
    let small = call(&url, "read", json!({"path": "f00.txt", "max_bytes": 1 << 20}));
    assert!(small.get("truncated").is_none() && small["content"] == lines(1, 30), "{small}");
}

#[test]
fn grep_context_adds_the_lines_around_each_match() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve(tmp.path());
    for index in [true, false] {
        let v = call(&url, "grep", json!({"pattern": "line (0|15)\\b", "context": 2, "index": index}));
        let m = v["matches"].as_array().unwrap();
        assert_eq!(m.len(), 2, "{v}");
        assert_eq!(m[0]["before"], json!([]), "nothing before line 1");
        assert_eq!(m[0]["after"], json!(["file 0 line 1", "file 0 line 2"]));
        assert_eq!(m[1]["before"], json!(["file 0 line 13", "file 0 line 14"]));
        assert_eq!(m[1]["after"], json!(["file 0 line 16", "file 0 line 17"]));
    }
    let plain = call(&url, "grep", json!({"pattern": "line 15"}));
    assert!(plain["matches"][0].get("before").is_none(), "{plain}");
}

#[test]
fn grep_path_takes_a_file_as_well_as_a_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 2, 5, &[("src/a.py", "class A:\n    pass\n"), ("src/ab.py", "class A:\n")]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let paths = |v: &Value| -> Vec<String> {
        v["matches"].as_array().unwrap().iter().map(|m| m["path"].as_str().unwrap().to_string()).collect()
    };
    for index in [true, false] {
        // A file: only its matches (not src/ab.py, which shares the prefix).
        let v = call(&url, "grep", json!({"pattern": "class A", "path": "src/a.py", "index": index}));
        assert_eq!(paths(&v), ["src/a.py"], "{v}");
        assert!(v.get("note").is_none(), "{v}");
        let v = call(&url, "grep", json!({"pattern": "line 3", "path": "f01.txt", "index": index}));
        assert_eq!(paths(&v), ["f01.txt"], "{v}");
        // A directory, with or without slashes, as before.
        for dir in ["src", "src/", "/src/"] {
            let v = call(&url, "grep", json!({"pattern": "class A", "path": dir, "index": index}));
            assert_eq!(paths(&v), ["src/a.py", "src/ab.py"], "{dir}: {v}");
        }
        // A file and the change's own version of it.
        let ch = call(
            &url,
            "apply",
            json!({"agent": "a", "intent": "x", "edits": [{"path": "src/a.py", "content": "class B:\n"}]}),
        );
        let at = ch["id"].as_str().unwrap();
        let v = call(&url, "grep", json!({"pattern": "class", "path": "src/a.py", "at": at, "index": index}));
        assert_eq!(v["matches"][0]["text"], "class B:", "{v}");
        call(&url, "abandon", json!({"change": at}));
        // A path that names nothing says so; a real path without matches does not.
        let v = call(&url, "grep", json!({"pattern": "class", "path": "src/nope.py", "index": index}));
        assert!(v["note"].as_str().unwrap().contains("no file or directory src/nope.py"), "{v}");
        let v = call(&url, "grep", json!({"pattern": "zzz", "path": "src/a.py", "index": index}));
        assert!(v.get("note").is_none(), "{v}");
    }
}

#[test]
fn read_takes_several_ranges_and_several_files() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve(tmp.path());
    let v = call(&url, "read", json!({"path": "f00.txt", "ranges": [[2, 3], [10, 10], [29, 1000]]}));
    assert_eq!(
        v["ranges"],
        json!([
            {"lines": [2, 3], "content": lines(2, 3)},
            {"lines": [10, 10], "content": lines(10, 10)},
            {"lines": [29, 30], "content": lines(29, 30)},
        ]),
        "{v}"
    );
    assert_eq!(v["total_lines"], 30);
    assert!(v.get("content").is_none() && v.get("lines").is_none(), "{v}");
    assert!(fails(&url, json!({"path": "f00.txt", "ranges": [[1, 2]], "start": 1})).contains("not both"));
    assert!(fails(&url, json!({"path": "f00.txt", "ranges": [[40, 41]]})).contains("past the end"));

    let v = call(
        &url,
        "read",
        json!({"files": [{"path": "f00.txt", "start": 5, "end": 6}, {"path": "nope.txt"}, {"path": "f00.txt", "ranges": [[1, 1], [30, 30]]}, {"path": "blob.bin"}]}),
    );
    let f = v["files"].as_array().unwrap();
    assert_eq!(f.len(), 4, "{v}");
    assert_eq!((f[0]["content"].clone(), f[0]["lines"].clone()), (json!(lines(5, 6)), json!([5, 6])));
    assert!(f[1]["error"].as_str().unwrap().contains("no such file nope.txt"), "{v}");
    assert_eq!(f[2]["ranges"][1]["content"], lines(30, 30));
    assert!(f[3].get("content_b64").is_some(), "{v}");
    // Files without a range share max_bytes.
    let one = lines(1, 1).len();
    let v = call(&url, "read", json!({"files": [{"path": "f00.txt"}, {"path": "f00.txt"}], "max_bytes": one * 4}));
    assert_eq!(v["files"][1]["content"], lines(1, 2), "{v}");
    assert!(fails(&url, json!({"files": [{"path": "f00.txt"}], "path": "f00.txt"})).contains("path or files"));
    assert!(fails(&url, json!({"files": [{"path": "f00.txt"}], "start": 1})).contains("per file"));
    assert!(fails(&url, json!({})).contains("read needs path"));
    assert!(fails(&url, json!({"files": []})).contains("1 to 20"));
}
