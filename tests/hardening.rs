//! Hardening: crash during land and restart recovery (a real process killed at injected
//! failpoints), landing races, client network errors, request limits, bind and data-dir
//! safety, and hostile repo content and paths.

mod common;

use common::*;
use treeyard::repo::Repo;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_treeyard");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A `treeyard serve` child process.
struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start(data: &Path, failpoint: Option<&str>) -> Server {
        let port = free_port();
        let mut cmd = Command::new(BIN);
        cmd.args(["serve", "--data", data.to_str().unwrap(), "--addr", &format!("127.0.0.1:{port}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(f) = failpoint {
            cmd.env("TREEYARD_FAILPOINT", f);
        }
        let child = cmd.spawn().unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let start = Instant::now();
        while ureq::get(&format!("{url}/health")).call().is_err() {
            assert!(start.elapsed() < Duration::from_secs(20), "server on {url} did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Server { child, url }
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "server did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One raw call, no retries: the crash tests must see the broken connection.
fn raw(url: &str, op: &str, p: Value) -> Result<Value, String> {
    match ureq::post(&format!("{url}/api/{op}")).send_json(p) {
        Ok(r) => Ok(r.into_json().unwrap()),
        Err(ureq::Error::Status(code, r)) => Err(format!("HTTP {code}: {}", r.into_string().unwrap_or_default())),
        Err(e) => Err(format!("transport: {e}")),
    }
}

fn fsck(root: &Path, data: &Path) {
    let gd = data.join("git");
    let (ok, out, err) = git_status(root, &["--git-dir", gd.to_str().unwrap(), "fsck", "--strict", "--no-progress"]);
    assert!(ok && !err.contains("error"), "fsck: {out}{err}");
}

fn export_main(root: &Path, data: &Path) -> String {
    let gd = data.join("git");
    git(root, &["--git-dir", gd.to_str().unwrap(), "rev-parse", "refs/heads/main"]).trim().to_string()
}

fn setup(files: usize) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let src = fixture_repo(&root, files, 20, &[]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    (tmp, root, data)
}

fn open_and_edit(url: &str, agent: &str, path: &str, body: &str) -> String {
    let id = call(url, "open_change", json!({"agent": agent, "intent": format!("edit {path}")}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(url, "apply", json!({"change": id, "edits": [{"path": path, "content": body}]}));
    id
}

#[test]
fn crash_during_land_recovers_at_every_failpoint() {
    for (i, point) in treeyard::failpoint::POINTS.iter().enumerate() {
        let (_tmp, root, data) = setup(4);
        let before = export_main(&root, &data);

        // 1. A server that aborts (SIGABRT, no cleanup) at this point of the land.
        let mut s = Server::start(&data, Some(point));
        // The land writes the new blob, trees and commit, so every failpoint is on its path.
        let other = open_and_edit(&s.url, "other", "f01.txt", "other\n");
        let id = open_and_edit(&s.url, "crashy", "f00.txt", &format!("crash test {i}\n"));
        let r = raw(&s.url, "land", json!({"change": other}));
        assert!(r.is_err(), "{point}: the first land must hit the failpoint, got {r:?}");
        let st = s.wait_exit();
        assert!(!st.success(), "{point}: the server must die");
        drop(s);

        // 2. Restart without the failpoint: the repo is consistent and git agrees with it.
        let s = Server::start(&data, None);
        let status = call(&s.url, "status", json!({}));
        let main = export_main(&root, &data);
        assert_eq!(status["commit"], main.as_str(), "{point}: refs/heads/main == SQLite main after restart");
        fsck(&root, &data);
        let ch = call(&s.url, "change", json!({"change": other}));
        let landed_before_crash = matches!(*point, "land:after-db" | "land:after-ref");
        if landed_before_crash {
            assert_eq!(ch["state"], "landed", "{point}: the SQLite transaction was the commit point");
            assert_ne!(main, before);
        } else {
            assert_eq!(ch["state"], "open", "{point}: nothing landed before the transaction");
            assert_eq!(main, before, "{point}: main did not move");
        }

        // 3. Landing again works (or answers like the first land), and so does the next one.
        let r = call(&s.url, "land", json!({"change": other}));
        assert_eq!(r["status"], "landed", "{point}: {r}");
        assert_eq!(r["repeated"].as_bool().unwrap_or(false), landed_before_crash, "{point}: {r}");
        let r2 = call(&s.url, "land", json!({"change": id}));
        assert_eq!(r2["status"], "landed", "{point}: {r2}");
        let gd = data.join("git");
        let gd = gd.to_str().unwrap();
        let count = git(&root, &["--git-dir", gd, "rev-list", "--count", "main"]);
        assert_eq!(count.trim(), "3", "{point}: import + 2 lands, no duplicate commit");
        assert_eq!(git(&root, &["--git-dir", gd, "show", "main:f00.txt"]), format!("crash test {i}\n"));
        assert_eq!(git(&root, &["--git-dir", gd, "show", "main:f01.txt"]), "other\n");
        fsck(&root, &data);
        eprintln!("crash at {point}: recovered (landed before crash: {landed_before_crash})");
    }
}

#[test]
fn second_server_on_the_same_data_is_refused() {
    let (_tmp, _root, data) = setup(2);
    let _s = Server::start(&data, None);
    let out = Command::new(BIN)
        .args(["serve", "--data", data.to_str().unwrap(), "--addr", &format!("127.0.0.1:{}", free_port())])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("another treeyard server already serves"), "{err}");
}

#[test]
fn open_server_refuses_a_public_address() {
    let (_tmp, _root, data) = setup(2);
    let d = data.to_str().unwrap();
    let out = Command::new(BIN).args(["serve", "--data", d, "--addr", "0.0.0.0:0"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("refusing to serve 0.0.0.0:0 without auth"), "{err}");

    // With a token, or with --insecure-open, it starts.
    for extra in [vec!["--insecure-open"], vec![]] {
        if extra.is_empty() {
            let t = Command::new(BIN).args(["token", "create", "--data", d, "--name", "admin", "--admin"]).output();
            assert!(t.unwrap().status.success());
        }
        let mut child = Command::new(BIN)
            .args(["serve", "--data", d, "--addr", "0.0.0.0:0"])
            .args(&extra)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        let mut err = child.stderr.take().unwrap();
        let mut buf = [0u8; 256];
        let start = Instant::now();
        while !line.contains("listening") && start.elapsed() < Duration::from_secs(20) {
            let n = err.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            line.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(line.contains("treeyard listening"), "{extra:?}: {line}");
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[test]
fn duplicate_lands_race_and_land_once() {
    let (_tmp, root, data) = setup(24);
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let n = 24;
    let ids: Vec<String> =
        (0..n).map(|i| open_and_edit(&url, &format!("a{i}"), &format!("f{i:02}.txt"), &format!("a{i}\n"))).collect();
    let start = Arc::new(Barrier::new(2 * n));
    let handles: Vec<_> = ids
        .iter()
        .flat_map(|id| [id.clone(), id.clone()])
        .map(|id| {
            let (url, start) = (url.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                let r = call(&url, "land", json!({"change": id}));
                (id, r)
            })
        })
        .collect();
    let mut by_change: std::collections::HashMap<String, Vec<Value>> = Default::default();
    for h in handles {
        let (id, r) = h.join().unwrap();
        assert_eq!(r["status"], "landed", "{r}");
        by_change.entry(id).or_default().push(r);
    }
    for (id, rs) in &by_change {
        assert_eq!(rs[0]["commit"], rs[1]["commit"], "both lands of {id} name one commit");
        let repeated = rs.iter().filter(|r| r["repeated"] == true).count();
        assert_eq!(repeated, 1, "exactly one of the two lands of {id} did the work");
    }
    let gd = data.join("git");
    let count = git(&root, &["--git-dir", gd.to_str().unwrap(), "rev-list", "--count", "main"]);
    assert_eq!(count.trim().parse::<usize>().unwrap(), 1 + n);
    fsck(&root, &data);
}

#[test]
fn client_retries_until_the_server_comes_up() {
    let (_tmp, _root, data) = setup(2);
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}");
    let repo = Arc::new(Repo::open(&data).unwrap());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
            axum::serve(l, treeyard::server::router(repo)).await.unwrap();
        });
    });
    let t = Instant::now();
    // open_change is not safe to repeat, but a refused connection never reached the server.
    let ch = treeyard::client::call(&url, "open_change", json!({"agent": "early", "intent": "x"})).unwrap();
    assert!(ch["id"].as_str().unwrap().starts_with("ch_"));
    assert!(t.elapsed() >= Duration::from_millis(600), "the call waited for the server");
}

#[test]
fn busy_server_answers_503_and_clients_retry() {
    let (_tmp, _root, data) = setup(2);
    let repo = Arc::new(Repo::open(&data).unwrap());
    let url0 = spawn_router(treeyard::server::router_with(repo.clone(), treeyard::server::Limits::new(0)));
    let err = raw(&url0, "status", json!({})).unwrap_err();
    assert!(err.starts_with("HTTP 503") && err.contains("server busy"), "{err}");

    let url1 = spawn_router(treeyard::server::router_with(repo, treeyard::server::Limits::new(1)));
    let busy = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(16));
    let hs: Vec<_> = (0..16)
        .map(|_| {
            let (url, start, busy) = (url1.clone(), start.clone(), busy.clone());
            std::thread::spawn(move || {
                start.wait();
                for _ in 0..10 {
                    if raw(&url, "read", json!({"path": "f00.txt"})).is_err() {
                        busy.fetch_add(1, Ordering::Relaxed);
                    }
                    let r = treeyard::client::call(&url, "read", json!({"path": "f00.txt"})).unwrap();
                    assert!(r["content"].as_str().unwrap().contains("file 0 line 0"));
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    eprintln!("busy: {} of 160 raw calls got 503; all 160 retried calls succeeded", busy.load(Ordering::Relaxed));
}

/// Serves any router on a free port in the background.
fn spawn_router(router: axum::Router) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(l.local_addr().unwrap()).unwrap();
            axum::serve(l, router).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

/// A TCP proxy that forwards everything, except that it swallows the reply to the first
/// `POST /api/land` and cuts that connection: the server ran the land, the client never knows.
fn lossy_proxy(upstream: String) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        for client in l.incoming() {
            let Ok(client) = client else { continue };
            let server = TcpStream::connect(upstream.trim_start_matches("http://")).unwrap();
            let cut = Arc::new(AtomicBool::new(false));
            let (mut c_in, mut s_out) = (client.try_clone().unwrap(), server.try_clone().unwrap());
            let (cut1, dropped1) = (cut.clone(), dropped.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                while let Ok(n) = c_in.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let chunk = &buf[..n];
                    if chunk.windows(14).any(|w| w == b"POST /api/land") && !dropped1.swap(true, Ordering::SeqCst) {
                        cut1.store(true, Ordering::SeqCst);
                    }
                    if s_out.write_all(chunk).is_err() {
                        break;
                    }
                }
            });
            let (mut s_in, mut c_out) = (server, client);
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                while let Ok(n) = s_in.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if cut.load(Ordering::SeqCst) {
                        let _ = c_out.shutdown(std::net::Shutdown::Both);
                        let _ = s_in.shutdown(std::net::Shutdown::Both);
                        break;
                    }
                    if c_out.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    format!("http://{addr}")
}

#[test]
fn lost_land_reply_is_retried_and_lands_once() {
    let (_tmp, root, data) = setup(3);
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let proxy = lossy_proxy(url.clone());
    let id = open_and_edit(&proxy, "flaky", "f00.txt", "over a bad network\n");
    let r = treeyard::client::call(&proxy, "land", json!({"change": id})).unwrap();
    assert_eq!(r["status"], "landed", "{r}");
    assert_eq!(r["repeated"], true, "the first reply was lost; the retry saw the landed change");
    let gd = data.join("git");
    let count = git(&root, &["--git-dir", gd.to_str().unwrap(), "rev-list", "--count", "main"]);
    assert_eq!(count.trim(), "2", "one land, no duplicate");

    // A non-repeatable call whose reply is lost is not retried: the client says so.
    let proxy2 = lossy_proxy(url.clone());
    let e = treeyard::client::call(&proxy2, "land", json!({"change": "ch_000000000000"}));
    assert!(e.is_err());
}

#[test]
fn hostile_paths_and_sizes_are_refused_over_the_api() {
    let (_tmp, _root, data) = setup(2);
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let id =
        call(&url, "open_change", json!({"agent": "evil", "intent": "escape"}))["id"].as_str().unwrap().to_string();
    let long = "d/".repeat(2100) + "f";
    for p in ["../outside", "/etc/passwd", ".GIT/hooks/post-checkout", "a/.git/config", "x\\y", ".g\u{200c}it/x", &long]
    {
        let e = raw(&url, "apply", json!({"change": id, "edits": [{"path": p, "content": "x"}]})).unwrap_err();
        assert!(e.starts_with("HTTP 422") && e.contains("invalid path"), "{p:?}: {e}");
    }
    // File over a directory and directory over a file.
    let e = raw(&url, "apply", json!({"change": id, "edits": [{"path": "f00.txt/x", "content": "x"}]})).unwrap_err();
    assert!(e.contains("is a file"), "{e}");
    // Too many edits in one call.
    let edits: Vec<Value> = (0..10_001).map(|i| json!({"path": format!("many/{i}"), "content": ""})).collect();
    let e = raw(&url, "apply", json!({"change": id, "edits": edits})).unwrap_err();
    assert!(e.contains("at most 10000 edits"), "{e}");
    // A body over the 64 MiB limit never reaches the operation: 413, or the server closes the
    // connection while the client still sends (broken pipe), depending on timing.
    let big = "x".repeat(65 << 20);
    let e = raw(&url, "apply", json!({"change": id, "edits": [{"path": "big.txt", "content": big}]})).unwrap_err();
    assert!(e.starts_with("HTTP 413") || e.starts_with("transport:"), "{}", &e[..e.len().min(200)]);
    // A file over the 48 MiB API limit, inside the body limit.
    let big = "y".repeat(49 << 20);
    let e = raw(&url, "apply", json!({"change": id, "edits": [{"path": "big.txt", "content": big}]})).unwrap_err();
    assert!(e.contains("the API takes files up to"), "{}", &e[..e.len().min(200)]);
    // Unknown operation names are cut short; nothing was written by any of the above.
    let e = raw(&url, "../../status", json!({})).unwrap_err();
    assert!(e.starts_with("HTTP 404"), "{e}");
    let ch = call(&url, "change", json!({"change": id}));
    assert_eq!(ch["paths"], json!([]), "{ch}");
}

/// A source repo whose main tree holds `entries` (mode, name, blob or tree id) at its root.
fn hostile_repo(root: &Path, build: impl Fn(&Path) -> String) -> PathBuf {
    let src = root.join("hostile");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    let tree = build(&src);
    let commit = git(&src, &["commit-tree", "-m", "hostile", tree.trim()]);
    git(&src, &["update-ref", "refs/heads/main", commit.trim()]);
    src
}

fn mktree(dir: &Path, lines: &str) -> String {
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(["mktree"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(lines.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn blob(dir: &Path, body: &str) -> String {
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap().trim().to_string()
}

/// Builds the root tree of a hostile repo; returns its id.
type TreeBuilder = Box<dyn Fn(&Path) -> String>;

#[test]
fn hostile_repo_content_is_refused_at_import() {
    let cases: Vec<(&str, TreeBuilder)> = vec![
        (
            "names a .git directory",
            Box::new(|d| {
                let b = blob(d, "[core]\n\thooksPath = /tmp\n");
                let sub = mktree(d, &format!("100644 blob {b}\tconfig\n"));
                mktree(d, &format!("040000 tree {sub}\t.GIT\n100644 blob {b}\tok.txt\n"))
            }),
        ),
        (
            "has an empty, . or .. component",
            Box::new(|d| {
                let b = blob(d, "x\n");
                let sub = mktree(d, &format!("100644 blob {b}\tpasswd\n"));
                mktree(d, &format!("040000 tree {sub}\t..\n"))
            }),
        ),
        (
            "sits under the file",
            Box::new(|d| {
                let b = blob(d, "/etc");
                let sub = mktree(d, &format!("100644 blob {b}\tpasswd\n"));
                mktree(d, &format!("040000 tree {sub}\tlink\n120000 blob {b}\tlink\n"))
            }),
        ),
    ];
    for (want, build) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let src = hostile_repo(tmp.path(), build);
        let e = Repo::init(&tmp.path().join("data"), &src, "main", None).unwrap_err();
        assert!(format!("{e:#}").contains(want), "want {want:?}, got {e:#}");
    }
}

#[test]
fn hostile_push_is_refused() {
    let (_tmp, root, data) = setup(2);
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let clone = root.join("clone");
    git(&root, &["clone", "-q", &format!("{url}/git"), clone.to_str().unwrap()]);
    // Build a commit whose tree holds .GIT/config next to main's files.
    let b = blob(&clone, "[core]\n\tfsmonitor = touch /tmp/pwned\n");
    let sub = mktree(&clone, &format!("100644 blob {b}\tconfig\n"));
    let head_tree = git(&clone, &["rev-parse", "HEAD^{tree}"]);
    let mut lines = git(&clone, &["ls-tree", head_tree.trim()]);
    lines.push_str(&format!("040000 tree {sub}\t.GIT\n"));
    let tree = mktree(&clone, &lines);
    let commit = git(&clone, &["commit-tree", "-p", "HEAD", "-m", "evil", &tree]);
    let (ok, _, err) = git_status(&clone, &["push", "origin", &format!("{}:refs/heads/main", commit.trim())]);
    assert!(!ok, "the push must be rejected");
    assert!(err.contains("names a .git directory"), "{err}");
    assert_eq!(call(&url, "status", json!({}))["commit"], export_main(&root, &data).as_str());
    fsck(&root, &data);
}

#[test]
fn shallow_source_is_imported_without_a_transport_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let src = fixture_repo(root, 3, 5, &[]);
    for i in 0..4 {
        std::fs::write(src.join("f00.txt"), format!("v{i}\n")).unwrap();
        git(&src, &["commit", "-q", "-am", &format!("v{i}")]);
    }
    let shallow = root.join("shallow");
    let url = format!("file://{}", src.display());
    git(root, &["clone", "-q", "--depth", "2", &url, shallow.to_str().unwrap()]);
    assert_eq!(git(&shallow, &["rev-parse", "--is-shallow-repository"]).trim(), "true");
    let data = root.join("data");
    Repo::init_with_history(&data, &shallow, "main", None, 1).unwrap();
    let gd = data.join("git");
    let gd = gd.to_str().unwrap();
    assert!(Path::new(gd).join("shallow").exists(), "the export repo is shallow like its source");
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let id = open_and_edit(&url, "a", "f01.txt", "after a shallow import\n");
    assert_eq!(call(&url, "land", json!({"change": id}))["status"], "landed");
    assert_eq!(git(root, &["--git-dir", gd, "rev-list", "--count", "main"]).trim(), "3");
    assert_eq!(git(root, &["--git-dir", gd, "show", "main:f00.txt"]), "v3\n");
    fsck(root, &data);
}

/// A cold `manifest` stops a long replay at a changeset with a git commit and reads its tree
/// (`MANIFEST_WALK`). For every changeset on main (150 imported commits with deletes, mode
/// flips, symlinks, renames and deep paths, then 80 lands), it must equal a full replay.
#[test]
fn cold_manifest_from_git_equals_a_full_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 6, 5, &[]);
    use std::os::unix::fs::PermissionsExt;
    for i in 0..150 {
        let deep = format!("d/{}/e/f/g/h{}", i % 7, i % 3);
        std::fs::create_dir_all(src.join(&deep)).unwrap();
        std::fs::write(src.join(format!("{deep}/n{i}.txt")), format!("commit {i}\n")).unwrap();
        let f = src.join(format!("f{:02}.txt", i % 6));
        match i % 5 {
            0 if f.exists() => std::fs::remove_file(&f).unwrap(),
            1 if f.exists() => {
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            2 => {
                let _ = std::fs::remove_file(src.join("link"));
                std::os::unix::fs::symlink(format!("f{:02}.txt", i % 6), src.join("link")).unwrap();
            }
            3 if f.exists() => {
                git(&src, &["mv", f.file_name().unwrap().to_str().unwrap(), &format!("moved{i}.txt")]);
            }
            _ => std::fs::write(&f, format!("rewritten at {i}\n")).unwrap(),
        }
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", &format!("c{i}")]);
    }
    let data = tmp.path().join("data");
    Repo::init_with_history(&data, &src, "main", None, 150).unwrap();
    {
        let repo = Repo::open(&data).unwrap();
        for i in 0..80 {
            let ch =
                treeyard::rpc::call(&repo, "open_change", json!({"agent": format!("a{i}"), "intent": "x"})).unwrap();
            let id = ch["id"].as_str().unwrap().to_string();
            let path = if i % 4 == 0 { "f00.txt".to_string() } else { format!("land/{i}.txt") };
            let edit = if i % 8 == 0 {
                json!({"path": path, "delete": true})
            } else {
                json!({"path": path, "content": format!("{i}\n")})
            };
            let _ = treeyard::rpc::call(&repo, "apply", json!({"change": id, "edits": [edit]}));
            let _ = treeyard::rpc::call(&repo, "land", json!({"change": id}));
        }
    }
    let fast = Repo::open(&data).unwrap();
    let slow = Repo::open(&data).unwrap();
    slow.with(|r| {
        r.manifest_from_git = false;
        Ok(())
    })
    .unwrap();
    let mut cur = Some(fast.with(|r| r.main()).unwrap());
    let mut checked = 0;
    while let Some(id) = cur {
        let a = fast.with(|r| r.manifest(&id)).unwrap();
        let b = slow.with(|r| r.manifest(&id)).unwrap();
        assert_eq!(a, b, "manifest of {id} (step {checked} from main)");
        cur = fast.with(|r| r.changeset(&id)).unwrap().parents.first().cloned();
        checked += 1;
    }
    assert!(checked > 2 * treeyard::repo::MANIFEST_WALK, "checked {checked} changesets");
    eprintln!("cold manifests equal to a full replay: {checked} changesets");
}
