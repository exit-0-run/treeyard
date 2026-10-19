//! Phase 3 coordination through the HTTP API: (a) intents, (b) live collisions and events,
//! (c) continuous rebase, (d) merge drivers, (e) landing modes, (f) the semantic guard.

mod common;

use common::*;
use serde_json::{Value, json};
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use treeyard::repo::Repo;

/// A fixture repo (f00..f03.txt, 30 lines each, plus `extra`) served with `config` as its
/// treeyard.toml.
fn setup(root: &Path, config: &str, extra: &[(&str, &str)]) -> (Arc<Repo>, String) {
    let src = fixture_repo(root, 4, 30, extra);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(data.join("treeyard.toml"), config).unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    let url = spawn_server(repo.clone());
    (repo, url)
}

fn open(url: &str, agent: &str, extra: Value) -> Value {
    let mut p = json!({"agent": agent, "intent": format!("work of {agent}")});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    call(url, "open_change", p)
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

fn read(url: &str, path: &str, at: &str) -> String {
    call(url, "read", json!({"path": path, "at": at}))["content"].as_str().unwrap().to_string()
}

fn write(url: &str, ch: &str, path: &str, content: &str) -> Value {
    call(url, "apply", json!({"change": ch, "edits": [{"path": path, "content": content}]}))
}

/// `text` with line `j` (0-based) replaced.
fn set_line(text: &str, j: usize, new: &str) -> String {
    text.lines().enumerate().map(|(i, l)| if i == j { format!("{new}\n") } else { format!("{l}\n") }).collect()
}

fn events(url: &str, p: Value) -> Vec<Value> {
    call(url, "events", p)["events"].as_array().unwrap().clone()
}

const LIB: &str = "use std::fmt;\n\npub fn parse(s: &str) -> u32 {\n    s.len() as u32\n}\n\npub fn render(n: u32) -> String {\n    format!(\"{n}\")\n}\n";

#[test]
fn a_intents_report_overlaps_at_open() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), "", &[("src/lib.rs", LIB)]);
    let a = open(&url, "a", json!({"paths": ["src/**"]}));
    assert_eq!(a["overlaps"], json!([]), "{a}");
    assert_eq!(a["declared"]["paths"], json!(["src/**"]));

    let b = open(&url, "b", json!({"paths": ["src/lib.rs"]}));
    let ov = b["overlaps"].as_array().unwrap();
    assert_eq!(ov.len(), 1, "{b}");
    assert_eq!(ov[0]["change"], a["id"]);
    assert_eq!(ov[0]["via"]["src/lib.rs"], "declared");
    assert!(b["advice"].as_str().unwrap().contains(a["id"].as_str().unwrap()), "{b}");

    // A edits the body of parse: a symbol intent on parse now meets A's edit.
    let new = LIB.replace("s.len() as u32", "s.trim().len() as u32");
    write(&url, &id(&a), "src/lib.rs", &new);
    let c = open(&url, "c", json!({"symbols": ["parse"]}));
    let hits: Vec<&Value> = c["overlaps"].as_array().unwrap().iter().filter(|o| o["change"] == a["id"]).collect();
    assert_eq!(hits.len(), 1, "{c}");
    assert_eq!(hits[0]["symbols"], json!(["parse"]));

    // render is untouched and nobody declared it.
    let d = open(&url, "d", json!({"symbols": ["src/lib.rs#render"]}));
    assert_eq!(d["overlaps"], json!([]), "{d}");
    let e = open(&url, "e", json!({"paths": ["docs/**"]}));
    assert_eq!(e["overlaps"], json!([]));
    // Declaring later works too.
    let e2 = call(&url, "declare", json!({"change": id(&e), "paths": ["src/"]}));
    assert!(e2["overlaps"].as_array().unwrap().len() >= 2, "{e2}");

    let ev = events(&url, json!({"kinds": ["intent_overlap"]}));
    assert!(ev.len() >= 4, "{ev:?}");
    assert!(ev.iter().any(|e| e["changes"] == json!([b["id"], a["id"]])));
    let bad = treeyard::client::call(&url, "open_change", json!({"agent": "x", "intent": "y", "paths": ["src/[a"]}));
    assert!(bad.is_err());
}

#[test]
fn b_live_collisions_trial_merge_and_events() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), "", &[]);
    let base = read(&url, "f00.txt", "main");
    let a = id(&open(&url, "a", json!({})));
    let b = id(&open(&url, "b", json!({})));
    let c = id(&open(&url, "c", json!({})));
    let r = write(&url, &a, "f00.txt", &set_line(&base, 5, "A was here"));
    assert_eq!(r["collisions"], json!([]));

    let r = write(&url, &b, "f00.txt", &set_line(&base, 5, "B was here"));
    let col = &r["collisions"][0];
    assert_eq!(col["change"], a.as_str());
    assert_eq!(col["kind"], "conflict", "{r}");
    assert_eq!(col["files"][0]["reason"], "landing_order", "{r}");
    assert_eq!(col["files"][0]["hunks"]["mine"], json!([[6, 7]]));
    assert_eq!(col["files"][0]["hunks"]["theirs"], json!([[6, 7]]));

    // Server-sent events: a reader that starts now sees the next collision.
    let latest = call(&url, "events", json!({}))["latest"].as_u64().unwrap();
    let sse_url = format!("{url}/api/events?kinds=collision&since={latest}");
    let reader = std::thread::spawn(move || {
        let resp = ureq::get(&sse_url).timeout(Duration::from_secs(10)).call().unwrap();
        let mut lines = std::io::BufReader::new(resp.into_reader()).lines();
        let mut seen = Vec::new();
        while let Some(Ok(l)) = lines.next() {
            seen.push(l.clone());
            if l.starts_with("data: ") && l.contains("\"collision\"") {
                return (seen, serde_json::from_str::<Value>(&l[6..]).unwrap());
            }
        }
        panic!("stream ended: {seen:?}");
    });
    std::thread::sleep(Duration::from_millis(200));
    let r = write(&url, &c, "f00.txt", &set_line(&base, 8, "C was here"));
    let kinds: Vec<&str> = r["collisions"].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["overlap", "overlap"], "{r}");
    let (seen, ev) = reader.join().unwrap();
    assert!(seen.iter().any(|l| l == "event: hello"), "{seen:?}");
    assert_eq!(ev["kind"], "collision");
    assert_eq!(ev["file"]["kind"], "overlap");

    // Each pair and path is published once; another file adds nothing for f00.
    let before = events(&url, json!({"kinds": ["collision"]})).len();
    write(&url, &b, "f01.txt", "other\n");
    assert_eq!(events(&url, json!({"kinds": ["collision"]})).len(), before);
    assert_eq!(before, 3);
    // An edit far from everyone else's in the same file merges either way: no collision, no event.
    let d = id(&open(&url, "d", json!({})));
    let r = write(&url, &d, "f00.txt", &set_line(&base, 25, "D was here"));
    assert_eq!(r["collisions"], json!([]), "{r}");
    assert_eq!(events(&url, json!({"kinds": ["collision"]})).len(), before);

    // A long poll returns as soon as an event arrives.
    let latest = call(&url, "events", json!({}))["latest"].as_u64().unwrap();
    let u = url.clone();
    let waiter = std::thread::spawn(move || {
        let t = Instant::now();
        let r = call(&u, "events", json!({"since": latest, "kinds": ["landed"], "wait_ms": 10000}));
        (t.elapsed(), r)
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    let (took, r) = waiter.join().unwrap();
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(r["events"][0]["changes"], json!([a]));
}

#[test]
fn b_collision_names_the_side_that_already_conflicts_with_main() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), "", &[]);
    let base = read(&url, "f00.txt", "main");
    // a starts on the old main; x then changes the same line and lands; b starts on the new main.
    let a = id(&open(&url, "a", json!({})));
    let x = id(&open(&url, "x", json!({})));
    write(&url, &x, "f00.txt", &set_line(&base, 5, "X was here"));
    assert_eq!(call(&url, "land", json!({"change": x}))["status"], "landed");
    let now = read(&url, "f00.txt", "main");
    let b = id(&open(&url, "b", json!({})));
    write(&url, &a, "f00.txt", &set_line(&base, 5, "A was here"));
    // b edits a line far away: the pair merges either way. a cannot land on main, but that is a's
    // problem, not b's: b gets no warning, and a gets one collision with "main", not with b.
    let r = write(&url, &b, "f00.txt", &set_line(&now, 25, "B was here"));
    assert_eq!(r["collisions"], json!([]), "{r}");
    let r = write(&url, &a, "f01.txt", "touch\n");
    assert_eq!(r["collisions"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["collisions"][0]["change"], "main", "{r}");
    let f = &r["collisions"][0]["files"][0];
    assert_eq!((&f["path"], &f["reason"]), (&json!("f00.txt"), &json!("you_conflict_with_main")), "{r}");
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "conflict");
}

#[test]
fn c_continuous_rebase_surfaces_conflicts_early_and_keeps_stale_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), "[coordination]\ncontinuous_rebase = true\n", &[]);
    let f0 = read(&url, "f00.txt", "main");
    let f1 = read(&url, "f01.txt", "main");
    let a = id(&open(&url, "a", json!({})));
    let b = id(&open(&url, "b", json!({})));
    let c = id(&open(&url, "c", json!({})));
    let d = id(&open(&url, "d", json!({})));
    write(&url, &a, "f00.txt", &set_line(&f0, 1, "A line 1"));
    write(&url, &b, "f01.txt", &set_line(&f1, 1, "B line 1"));
    write(&url, &c, "f00.txt", &set_line(&f0, 1, "C line 1"));

    let landed = call(&url, "land", json!({"change": a}));
    assert_eq!(landed["status"], "landed");
    let rebased = landed["rebased"].as_array().unwrap();
    assert_eq!(rebased.len(), 3, "{landed}");
    let main = call(&url, "status", json!({}))["main"].clone();
    // C learns about its conflict now, before it calls land.
    let cv = call(&url, "change", json!({"change": c}));
    assert_eq!(cv["conflicts"][0]["path"], "f00.txt", "{cv}");
    assert_eq!(cv["conflicts"][0]["theirs_by"][0]["change"], a.as_str());
    let ev = events(&url, json!({"change": c, "kinds": ["conflict"]}));
    assert_eq!(ev[0]["cause"], "continuous_rebase", "{ev:?}");
    // B and the empty D moved onto the new main.
    let bv = call(&url, "change", json!({"change": b}));
    assert_eq!(bv["base"], main);
    assert!(bv["view"].is_string(), "{bv}");
    assert_eq!(call(&url, "change", json!({"change": d}))["base"], main);

    // B's client never saw A's edit and writes all of f00 from its old view: the server merges
    // it onto the rebased head instead of dropping A's line.
    let r = write(&url, &b, "f00.txt", &set_line(&f0, 20, "B line 20"));
    assert!(r["view"].is_string(), "the client is still stale: {r}");
    let head = call(&url, "change", json!({"change": b}))["head"].clone();
    let mine = read(&url, "f00.txt", &b);
    assert!(mine.contains("A line 1\n") && mine.contains("B line 20\n"), "{mine}");
    // A client that read the head and passes it as `seen` writes as is and clears the view.
    let r = call(
        &url,
        "apply",
        json!({"change": b, "seen": head, "edits": [{"path": "f00.txt", "content": mine.replace("B line 20", "B line 20 v2")}]}),
    );
    assert!(r["view"].is_null(), "{r}");
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    let now = read(&url, "f00.txt", "main");
    assert!(now.contains("A line 1\n") && now.contains("B line 20 v2\n"), "{now}");
    assert!(read(&url, "f01.txt", "main").contains("B line 1\n"));
    // An empty change that never had edits still cannot land.
    assert!(treeyard::client::call(&url, "land", json!({"change": d})).is_err());
}

const LOCK_BASE: &str = "version = 3\n\n[[package]]\nname = \"aho\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"aho\",\n \"bstr\",\n]\n\n[[package]]\nname = \"bstr\"\nversion = \"1.0.0\"\n";
const CHANGES: &str = "# Changes\n\n## 1.0\n- first\n";

fn drivers_scenario(url: &str) -> (Value, Value) {
    let a = id(&open(url, "a", json!({})));
    let b = id(&open(url, "b", json!({})));
    let lib_a = LIB.replace("use std::fmt;\n", "use std::fmt;\nuse std::io;\n");
    let lib_b = LIB.replace("use std::fmt;\n", "use std::fmt;\nuse std::fs;\n");
    let edits = |log: &str, lock: &str, input: (&str, &str), gen_: &str, lib: &str| {
        json!([
            {"path": "CHANGELOG.md", "content": CHANGES.replace("## 1.0", &format!("- {log}\n## 1.0"))},
            {"path": "Cargo.lock", "content": lock},
            {"path": input.0, "content": input.1},
            {"path": "gen.txt", "content": gen_},
            {"path": "src/lib.rs", "content": lib},
        ])
    };
    // Each side bumps one package and adds one dependency at the same place in the file.
    let add = |lock: &str, dep: &str, version: &str| {
        lock.replace(" \"bstr\",\n]", &format!(" \"bstr\",\n \"{dep}\",\n]"))
            + &format!("\n[[package]]\nname = \"{dep}\"\nversion = \"{version}\"\n")
    };
    let lock_a = add(
        &LOCK_BASE.replace("name = \"aho\"\nversion = \"1.0.0\"", "name = \"aho\"\nversion = \"1.1.0\""),
        "cfg",
        "0.9.0",
    );
    let lock_b = add(
        &LOCK_BASE.replace("name = \"bstr\"\nversion = \"1.0.0\"", "name = \"bstr\"\nversion = \"1.2.0\""),
        "memchr",
        "2.0.0",
    );
    call(url, "apply", json!({"change": a, "edits": edits("from a", &lock_a, ("a.txt", "A\n"), "A\nb\n", &lib_a)}));
    call(url, "apply", json!({"change": b, "edits": edits("from b", &lock_b, ("b.txt", "B\n"), "a\nB\n", &lib_b)}));
    assert_eq!(call(url, "land", json!({"change": a}))["status"], "landed");
    let out = call(url, "land", json!({"change": b}));
    (out, json!({"lock_a": lock_a, "lock_b": lock_b}))
}

#[test]
fn d_merge_drivers_resolve_lockfiles_changelogs_generated_files_and_syntax() {
    let files: &[(&str, &str)] = &[
        ("CHANGELOG.md", CHANGES),
        ("Cargo.lock", LOCK_BASE),
        ("a.txt", "a\n"),
        ("b.txt", "b\n"),
        ("gen.txt", "a\nb\n"),
        ("src/lib.rs", LIB),
    ];
    // Control: without drivers every one of these paths conflicts.
    let tmp = tempfile::tempdir().unwrap();
    let (_r, url) = setup(tmp.path(), "", files);
    let (out, _) = drivers_scenario(&url);
    assert_eq!(out["status"], "conflict", "{out}");
    let mut paths: Vec<&str> =
        out["change"]["conflicts"].as_array().unwrap().iter().map(|c| c["path"].as_str().unwrap()).collect();
    paths.sort();
    assert_eq!(paths, ["CHANGELOG.md", "Cargo.lock", "gen.txt", "src/lib.rs"]);

    let mergiraf = std::env::var("MERGIRAF").unwrap_or_else(|_| "mergiraf".into());
    let have_mergiraf =
        std::process::Command::new(&mergiraf).arg("--version").output().is_ok_and(|o| o.status.success());
    let config = format!(
        "[merge]\nsyntax = \"auto\"\nmergiraf = \"{mergiraf}\"\n\
         [[merge.driver]]\npaths = [\"CHANGELOG.md\"]\ndriver = \"union\"\n\
         [[merge.driver]]\npaths = [\"Cargo.lock\"]\ndriver = \"cargo-lock\"\n\
         [[merge.driver]]\npaths = [\"gen.txt\"]\ndriver = \"regenerate\"\ncommand = \"cat a.txt b.txt > gen.txt\"\n"
    );
    let tmp = tempfile::tempdir().unwrap();
    let (_r, url) = setup(tmp.path(), &config, files);
    let (out, locks) = drivers_scenario(&url);
    let mut used: Vec<String> = out["drivers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|d| d["driver"].as_str().unwrap().to_string())
        .collect();
    used.sort();
    if have_mergiraf {
        assert_eq!(out["status"], "landed", "{out}");
        assert_eq!(used, ["cargo-lock", "mergiraf", "regenerate", "union"]);
        let lib = read(&url, "src/lib.rs", "main");
        assert!(lib.contains("use std::io;\n") && lib.contains("use std::fs;\n"), "{lib}");
    } else {
        eprintln!("mergiraf not found (set MERGIRAF=/path): src/lib.rs stays a conflict");
        assert_eq!(out["status"], "conflict", "{out}");
        assert_eq!(out["change"]["conflicts"].as_array().unwrap().len(), 1);
        assert_eq!(used, ["cargo-lock", "regenerate", "union"]);
        return;
    }
    let log = read(&url, "CHANGELOG.md", "main");
    assert_eq!(log, "# Changes\n\n- from b\n- from a\n## 1.0\n- first\n");
    let lock = read(&url, "Cargo.lock", "main");
    assert!(
        lock.contains("name = \"aho\"\nversion = \"1.1.0\"") && lock.contains("name = \"bstr\"\nversion = \"1.2.0\""),
        "{lock}"
    );
    assert!(!lock.contains("1.0.0"), "{lock} {locks}");
    assert!(lock.contains(" \"bstr\",\n \"cfg\",\n \"memchr\",\n]"), "{lock}");
    assert!(lock.find("name = \"cfg\"").unwrap() < lock.find("name = \"memchr\"").unwrap());
    assert_eq!(read(&url, "gen.txt", "main"), "A\nB\n");
}

const NOT_BROKEN: &str = "[landing]\nmode = \"{mode}\"\ntest = \"! grep -rq BROKEN .\"\nbatch = 4\nparallel = 1\n";

fn four_changes(url: &str) -> Vec<String> {
    (0..4)
        .map(|k| {
            let ch = id(&open(url, &format!("q{k}"), json!({})));
            let body = if k == 1 { "BROKEN\n".to_string() } else { format!("change {k}\n") };
            write(url, &ch, &format!("f{k:02}.txt"), &body);
            ch
        })
        .collect()
}

#[test]
fn e_queue_mode_tests_batches_and_bisects() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, url) = setup(tmp.path(), &NOT_BROKEN.replace("{mode}", "queue"), &[]);
    let ch = four_changes(&url);
    for c in &ch {
        assert_eq!(call(&url, "land", json!({"change": c}))["status"], "queued");
    }
    assert_eq!(call(&url, "queue", json!({}))["queue"].as_array().unwrap().len(), 4);
    let step = call(&url, "queue_run", json!({}));
    // 4 together fail; bisection tests prefixes 2 (fail) and 1 (pass): 3 test runs.
    assert_eq!(step["tests"].as_array().unwrap().len(), 3, "{step}");
    assert_eq!(step["landed"][0]["change"], ch[0].as_str());
    assert_eq!(step["landed"].as_array().unwrap().len(), 1);
    assert_eq!(step["rejected"]["change"], ch[1].as_str());
    let step = call(&url, "queue_run", json!({}));
    assert_eq!(step["landed"].as_array().unwrap().len(), 2, "{step}");
    assert_eq!(step["tests"].as_array().unwrap().len(), 1);
    assert_eq!(read(&url, "f02.txt", "main"), "change 2\n");
    assert_eq!(read(&url, "f03.txt", "main"), "change 3\n");
    assert!(read(&url, "f01.txt", "main").starts_with("file 1 line 0"));
    let reviews = call(&url, "reviews", json!({"change": ch[1]}))["reviews"].clone();
    assert_eq!(reviews[0]["kind"], "tests_failed", "{reviews}");
    assert_eq!(call(&url, "change", json!({"change": ch[1]}))["state"], "open");

    // The owner fixes the change; the background worker lands it.
    treeyard::landing::spawn_worker(repo.clone()).unwrap();
    write(&url, &ch[1], "f01.txt", "fixed\n");
    assert_eq!(call(&url, "land", json!({"change": ch[1]}))["status"], "queued");
    let ev = call(&url, "events", json!({"change": ch[1], "kinds": ["landed"], "wait_ms": 15000}));
    assert_eq!(ev["events"].as_array().unwrap().len(), 1, "{ev}");
    assert_eq!(read(&url, "f01.txt", "main"), "fixed\n");
}

#[test]
fn e_queue_mode_with_parallel_tests_changes_alone_when_a_batch_fails() {
    // Scripted 40-agent runs with 1 change in 4 broken: serial bisects made the batch queue slower
    // than a plain git queue, because one broken change held up its whole batch. With parallel = 4 a
    // failing batch tests its changes alone, side by side; the broken one goes back at once and the
    // rest land together in the next step. A passing batch costs one run.
    let tmp = tempfile::tempdir().unwrap();
    // Each test run takes 1 s, so the step's wall time shows whether the runs were side by side.
    let cfg = "[landing]\nmode = \"queue\"\ntest = \"sleep 1; ! grep -rq BROKEN .\"\nbatch = 4\nparallel = 4\n";
    let (_repo, url) = setup(tmp.path(), cfg, &[]);
    let ch = four_changes(&url);
    for c in &ch {
        assert_eq!(call(&url, "land", json!({"change": c}))["status"], "queued");
    }
    let t0 = std::time::Instant::now();
    let step = call(&url, "queue_run", json!({}));
    let wall = t0.elapsed().as_secs_f64();
    assert_eq!(step["tests"].as_array().unwrap().len(), 1, "{step}");
    let alone = step["alone"].as_array().unwrap();
    assert_eq!(alone.len(), 4, "{step}");
    assert_eq!(alone.iter().filter(|p| p["ok"] == false).count(), 1, "{step}");
    // The batch, then 4 runs at once: about 2 s, not 5.
    assert!(wall < 2.9, "the changes were not tested side by side: {wall:.1} s");
    assert_eq!(step["landed"].as_array().unwrap().len(), 0, "{step}");
    assert_eq!(step["rejected"][0]["change"], ch[1].as_str(), "{step}");
    assert_eq!(call(&url, "land_status", json!({"change": ch[1]}))["verdict"], "rejected");
    let step = call(&url, "queue_run", json!({}));
    let landed: Vec<&str> = step["landed"].as_array().unwrap().iter().map(|l| l["change"].as_str().unwrap()).collect();
    assert_eq!(landed, [ch[0].as_str(), ch[2].as_str(), ch[3].as_str()], "{step}");
    assert_eq!(step["tests"].as_array().unwrap().len(), 1, "{step}");
}

#[test]
fn e_optimistic_mode_bisects_and_reverts_through_the_api() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), &NOT_BROKEN.replace("{mode}", "optimistic"), &[]);
    let ch = four_changes(&url);
    let mut sets = Vec::new();
    for c in &ch {
        let r = call(&url, "land", json!({"change": c}));
        assert_eq!(r["status"], "landed", "{r}");
        sets.push(r["changeset"].clone());
    }
    let step = call(&url, "test_main", json!({}));
    assert_eq!(step["status"], "red", "{step}");
    assert_eq!(step["culprit"], sets[1]);
    assert_eq!(step["revert"]["status"], "landed");
    assert!(read(&url, "f01.txt", "main").starts_with("file 1 line 0"));
    assert_eq!(read(&url, "f03.txt", "main"), "change 3\n");
    let reviews = call(&url, "reviews", json!({"change": ch[1]}))["reviews"].clone();
    assert_eq!(reviews[0]["kind"], "reverted_by_bisect", "{reviews}");
    assert_eq!(call(&url, "test_main", json!({}))["status"], "green");
    assert_eq!(call(&url, "test_main", json!({}))["status"], "idle");
    let kinds: Vec<String> = events(&url, json!({"kinds": ["test_failed", "reverted", "test_passed"]}))
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["test_failed", "reverted", "test_passed"]);
}

#[test]
fn e_optimistic_mode_waits_for_a_fix_when_the_culprit_cannot_be_reverted() {
    // The wide50 cloud run: a culprit whose revert conflicts kept main red, every step bisected again,
    // and every later change stayed "pending" until its agent gave up waiting.
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), &NOT_BROKEN.replace("{mode}", "optimistic"), &[]);
    let land = |agent: &str, path: &str, body: &str| {
        let ch = id(&open(&url, agent, json!({})));
        write(&url, &ch, path, body);
        let r = call(&url, "land", json!({"change": ch}));
        assert_eq!(r["status"], "landed", "{r}");
        (ch, r["changeset"].as_str().unwrap().to_string())
    };
    let (c1, cs1) = land("a", "f01.txt", "BROKEN\n");
    land("b", "f01.txt", "BROKEN too\n"); // the same line: the revert of c1 conflicts
    let (c3, _) = land("c", "f02.txt", "fine\n");
    let step = call(&url, "test_main", json!({}));
    assert_eq!((step["status"].as_str(), step["culprit"].as_str()), (Some("red"), Some(cs1.as_str())), "{step}");
    assert_ne!(step["revert"]["status"], "landed", "{step}");
    let st = call(&url, "land_status", json!({"change": c1}));
    assert_eq!(st["verdict"], "broke_main", "{st}");
    let st = call(&url, "land_status", json!({"change": c3}));
    assert_eq!((st["verdict"].as_str(), st["red_change"].as_str()), (Some("main_red"), Some(c1.as_str())), "{st}");

    // While red, a new main costs one test and no bisect.
    let (c4, _) = land("d", "f03.txt", "ok\n");
    let step = call(&url, "test_main", json!({}));
    assert_eq!((step["status"].as_str(), step["known"].as_bool()), (Some("red"), Some(true)), "{step}");
    assert_eq!(step["tests"].as_array().unwrap().len(), 1, "{step}");
    assert_eq!(call(&url, "land_status", json!({"change": c4}))["verdict"], "main_red");

    // The culprit's agent fixes forward while another agent breaks a different file.
    land("a", "f01.txt", "fixed\n");
    let (_c6, cs6) = land("e", "f00.txt", "BROKEN\n");
    assert_eq!(call(&url, "test_main", json!({}))["status"], "fixed");
    assert_eq!(call(&url, "land_status", json!({"change": c1}))["verdict"], "passed");
    let step = call(&url, "test_main", json!({}));
    assert_eq!((step["status"].as_str(), step["culprit"].as_str()), (Some("red"), Some(cs6.as_str())), "{step}");
    assert_eq!(step["revert"]["status"], "landed", "{step}");
    assert_eq!(call(&url, "test_main", json!({}))["status"], "green");
    assert_eq!(call(&url, "land_status", json!({"change": c3}))["verdict"], "passed");
}

#[test]
fn f_guard_flags_an_edit_that_a_rebase_makes_vanish() {
    // History: c1 adds a stray dbg! line, c2 removes it. An agent does c2 on top of c1 while
    // main is still before c1, the replay finding: its fix lands first and becomes a no-op.
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 2, 30, &[]);
    let f = src.join("f00.txt");
    let orig = std::fs::read_to_string(&f).unwrap();
    std::fs::write(&f, set_line(&orig, 10, "dbg!(x);")).unwrap();
    git(&src, &["commit", "-qam", "add debugging"]);
    std::fs::write(&f, set_line(&orig, 25, "fixed line 25")).unwrap();
    git(&src, &["commit", "-qam", "remove stray dbg! and fix line 25"]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", Some("HEAD~2")).unwrap();
    let repo = Repo::open(&data).unwrap();
    let c1 = git(&src, &["rev-parse", "HEAD~1"]).trim().to_string();
    let c2 = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
    repo.with(|r| {
        let cs1 = r.import_commit(&c1)?;
        let cs2 = r.import_commit(&c2)?;
        let ch = r.open_change("fixer", "remove stray dbg!", None, None, Some(&cs1))?;
        r.apply_entries(&ch.id, r.changeset(&cs2)?.changes.into_iter().collect(), None)?;
        let out = r.land(&ch.id)?;
        assert_eq!(out["status"], "landed", "{out}");
        let reviews = r.reviews_view(Some("open"), Some(&ch.id))?;
        let rv = &reviews["reviews"][0];
        assert_eq!(rv["kind"], "absorbed_edit", "{reviews}");
        assert_eq!(rv["detail"]["path"], "f00.txt");
        assert_eq!(rv["detail"]["whole_file"], false);
        let hunks = rv["detail"]["hunks"].as_array().unwrap();
        assert_eq!(hunks.len(), 1, "only the dbg! hunk vanished: {rv}");
        assert!(hunks[0].as_str().unwrap().contains("-dbg!(x);"));

        // The same fix without the extra line becomes empty: already_on_main plus a review.
        let ch = r.open_change("fixer2", "remove stray dbg! only", None, None, Some(&cs1))?;
        let only = Some(treeyard::model::Entry { blob: r.put_blob(orig.as_bytes())?, mode: 0o100644 });
        r.apply_entries(&ch.id, vec![("f00.txt".into(), only)], None)?;
        let out = r.land(&ch.id)?;
        assert_eq!(out["status"], "already_on_main", "{out}");
        let kinds: Vec<Value> = r.reviews_view(Some("open"), Some(&ch.id))?["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["kind"].clone())
            .collect();
        assert_eq!(kinds, [json!("absorbed_edit"), json!("already_on_main")]);
        assert_eq!(r.reviews_view(Some("open"), Some(&ch.id))?["reviews"][0]["detail"]["whole_file"], true);
        Ok(())
    })
    .unwrap();
    // Closing a review through the API.
    let url = spawn_server(Arc::new(repo));
    let all = call(&url, "reviews", json!({}))["reviews"].as_array().unwrap().len();
    assert_eq!(all, 3);
    assert_eq!(call(&url, "review_close", json!({"id": 1}))["state"], "done");
    assert_eq!(call(&url, "reviews", json!({}))["reviews"].as_array().unwrap().len(), 2);
}

#[test]
fn mcp_agent_declares_intent_and_polls_events_over_stdio() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, url) = setup(tmp.path(), "", &[]);
    let other = id(&open(&url, "http-agent", json!({"paths": ["f00.txt"]})));
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_treeyard"))
        .args(["mcp", "--url", &url, "--agent", "mcp-agent"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "open_change", "arguments": {"intent": "edit f00", "paths": ["f00.txt"]}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "events", "arguments": {"kinds": ["intent_overlap"], "wait_ms": 1000}}}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for m in &msgs {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let replies: Vec<Value> =
        String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let names: Vec<&str> =
        replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["declare", "events", "reviews", "review_close", "queue", "queue_run", "test_main", "config"] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let text = |i: usize| -> Value {
        serde_json::from_str(replies[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let opened = text(2);
    assert_eq!(opened["overlaps"][0]["change"], other.as_str(), "{opened}");
    let ev = text(3);
    assert_eq!(ev["events"][0]["agents"], json!(["mcp-agent", "http-agent"]), "{ev}");
}
