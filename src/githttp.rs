//! Git smart HTTP for people, under `/git` of the same server: clone and fetch run
//! `git upload-pack` on the export repo; a push to the branch lands through `Inner::land_push`,
//! so it takes the same path as an agent's change. Also `mirror`: push the branch to a
//! read-only mirror remote.

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

use crate::audit::Record;
use crate::auth::Caller;
use crate::git;
use crate::repo::Repo;

/// Largest request body (a pushed pack) the git routes accept.
const MAX_BODY: usize = 2 << 30;

pub fn routes() -> Router<Arc<Repo>> {
    Router::new()
        .route("/git/info/refs", get(info_refs))
        .route("/git/git-upload-pack", post(upload_pack))
        .route("/git/git-receive-pack", post(receive_pack))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

/// One pkt-line: four hex digits of length (including themselves), then the data.
pub fn pkt(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:04x}", data.len() + 4).into_bytes();
    v.extend_from_slice(data);
    v
}

const FLUSH: &[u8] = b"0000";

fn reply(status: StatusCode, kind: &str, body: impl Into<Body>) -> Response {
    let mut r = (status, body.into()).into_response();
    if let Ok(v) = HeaderValue::from_str(kind) {
        r.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

fn error(status: StatusCode, msg: String) -> Response {
    reply(status, "text/plain; charset=utf-8", format!("{msg}\n"))
}

fn git_protocol(headers: &HeaderMap) -> Option<String> {
    let v = headers.get("git-protocol")?.to_str().ok()?;
    // Only pass the version through to upload-pack.
    v.split(':').all(|p| p.starts_with("version=")).then(|| v.to_string())
}

fn request_body(headers: &HeaderMap, body: Bytes) -> std::io::Result<Vec<u8>> {
    let gzip = headers.get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("gzip"));
    if !gzip {
        return Ok(body.to_vec());
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut out)?;
    Ok(out)
}

/// The caller of a git request, or the 401/403 reply. git sends the token as the HTTP Basic
/// password (or user name), e.g. from a credential helper.
async fn caller(repo: &Arc<Repo>, headers: &HeaderMap, write: bool) -> Result<Caller, Box<Response>> {
    let token = crate::server::token_of(headers);
    let r = repo.clone();
    let c = match tokio::task::spawn_blocking(move || r.auth.authenticate(token.as_deref())).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            let mut resp = error(StatusCode::UNAUTHORIZED, format!("{e:#}"));
            resp.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"rootstock\""));
            return Err(Box::new(resp));
        }
        Err(e) => return Err(Box::new(error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))),
    };
    let ok = if write { c.writes_any() } else { c.reads_all() };
    if !ok {
        let what = if write { "push (it is read-only)" } else { "clone (git needs every path; use the API)" };
        return Err(Box::new(error(StatusCode::FORBIDDEN, format!("token {} may not {what}", c.name))));
    }
    Ok(c)
}

async fn info_refs(
    State(repo): State<Arc<Repo>>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let service = q.get("service").map(String::as_str);
    if matches!(service, Some("git-upload-pack" | "git-receive-pack"))
        && let Err(resp) = caller(&repo, &headers, service == Some("git-receive-pack")).await
    {
        return *resp;
    }
    match service {
        Some("git-upload-pack") => {
            let proto = git_protocol(&headers);
            let v2 = proto.as_deref().is_some_and(|p| p.contains("version=2"));
            let mut cmd = tokio::process::Command::new("git");
            cmd.args(["upload-pack", "--stateless-rpc", "--advertise-refs"]).arg(repo.git_dir());
            if let Some(p) = &proto {
                cmd.env("GIT_PROTOCOL", p);
            }
            match cmd.stdin(Stdio::null()).output().await {
                Ok(out) if out.status.success() => {
                    let mut body = Vec::new();
                    if !v2 {
                        body.extend(pkt(b"# service=git-upload-pack\n"));
                        body.extend_from_slice(FLUSH);
                    }
                    body.extend(out.stdout);
                    reply(StatusCode::OK, "application/x-git-upload-pack-advertisement", body)
                }
                Ok(out) => error(StatusCode::INTERNAL_SERVER_ERROR, String::from_utf8_lossy(&out.stderr).into()),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("git upload-pack: {e}")),
            }
        }
        Some("git-receive-pack") => {
            let adv = tokio::task::spawn_blocking(move || {
                repo.with(|r| {
                    let main = r.main()?;
                    Ok((r.commit_for(&main)?, r.branch.clone()))
                })
            })
            .await;
            match adv {
                Ok(Ok((commit, branch))) => {
                    let caps = format!(
                        "report-status side-band-64k ofs-delta delete-refs quiet object-format=sha1 agent=rootstock/{}",
                        env!("CARGO_PKG_VERSION")
                    );
                    let mut body = pkt(b"# service=git-receive-pack\n");
                    body.extend_from_slice(FLUSH);
                    body.extend(pkt(format!("{commit} refs/heads/{branch}\0{caps}\n").as_bytes()));
                    body.extend_from_slice(FLUSH);
                    reply(StatusCode::OK, "application/x-git-receive-pack-advertisement", body)
                }
                Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        _ => error(StatusCode::FORBIDDEN, "rootstock serves git over smart HTTP only (git 1.6.6 or newer)".into()),
    }
}

/// Runs `git upload-pack --stateless-rpc` on the export repo and streams its output.
async fn upload_pack(State(repo): State<Arc<Repo>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(resp) = caller(&repo, &headers, false).await {
        return *resp;
    }
    let input = match request_body(&headers, body) {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, format!("bad request body: {e}")),
    };
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["upload-pack", "--stateless-rpc"]).arg(repo.git_dir());
    if let Some(p) = git_protocol(&headers) {
        cmd.env("GIT_PROTOCOL", p);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, format!("git upload-pack: {e}")),
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "git upload-pack: no pipes".into());
    };
    tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
        drop(stdin);
        let _ = child.wait().await;
    });
    let stream = tokio_util::io::ReaderStream::new(stdout);
    reply(StatusCode::OK, "application/x-git-upload-pack-result", Body::from_stream(stream))
}

/// A parsed receive-pack request: ref updates (old, new, ref), client capabilities and the pack.
#[derive(Debug, Default)]
pub struct PushRequest {
    pub commands: Vec<(String, String, String)>,
    pub caps: Vec<String>,
    pub pack: Vec<u8>,
}

pub fn parse_push(data: &[u8]) -> Result<PushRequest> {
    let mut req = PushRequest::default();
    let mut pos = 0;
    loop {
        let len_hex = data.get(pos..pos + 4).context("truncated pkt-line")?;
        let len = usize::from_str_radix(std::str::from_utf8(len_hex)?, 16).context("bad pkt-line length")?;
        if len == 0 {
            pos += 4;
            break;
        }
        if len < 4 {
            bail!("bad pkt-line length {len}");
        }
        let line = data.get(pos + 4..pos + len).context("truncated pkt-line")?;
        pos += len;
        let line = std::str::from_utf8(line).context("non UTF-8 command")?.trim_end_matches('\n');
        let (cmd, caps) = line.split_once('\0').unwrap_or((line, ""));
        if req.commands.is_empty() && !caps.is_empty() {
            req.caps = caps.split(' ').filter(|c| !c.is_empty()).map(str::to_string).collect();
        }
        if cmd.starts_with("shallow ") {
            bail!("pushes from shallow clones are not supported; run `git fetch --unshallow` first");
        }
        let parts: Vec<&str> = cmd.split(' ').collect();
        let [old, new, name] = parts.as_slice() else { bail!("bad push command {cmd:?}") };
        // Full hex ids only: they go into git command lines, so nothing may look like an option.
        let hex40 = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex40(old) || !hex40(new) || name.starts_with('-') {
            bail!("bad push command {cmd:?}: expected <old-id> <new-id> <ref>");
        }
        req.commands.push((old.to_string(), new.to_string(), name.to_string()));
    }
    req.pack = data[pos..].to_vec();
    Ok(req)
}

/// Report-status lines plus progress messages, framed for side-band-64k when the client asked.
fn push_response(sideband: bool, messages: &[String], report: &[String]) -> Vec<u8> {
    let mut inner = Vec::new();
    for line in report {
        inner.extend(pkt(format!("{line}\n").as_bytes()));
    }
    inner.extend_from_slice(FLUSH);
    if !sideband {
        return inner;
    }
    let mut out = Vec::new();
    for m in messages {
        let mut band = vec![2u8];
        band.extend_from_slice(format!("rootstock: {m}\n").as_bytes());
        out.extend(pkt(&band));
    }
    for chunk in inner.chunks(65515) {
        let mut band = vec![1u8];
        band.extend_from_slice(chunk);
        out.extend(pkt(&band));
    }
    out.extend_from_slice(FLUSH);
    out
}

/// Handles one push: stores the pack, then lands each update of the branch under the lock.
pub fn handle_push(repo: &Repo, who: &Caller, data: &[u8]) -> Result<Vec<u8>> {
    let req = parse_push(data)?;
    if req.commands.is_empty() {
        // The probe git sends before a large push.
        return Ok(Vec::new());
    }
    let sideband = req.caps.iter().any(|c| c == "side-band-64k");
    let mut messages = Vec::new();
    let mut report = Vec::new();
    let learned = if req.pack.is_empty() { Ok(None) } else { git::index_pack(repo.git_dir(), &req.pack).map(Some) };
    let pack = match learned {
        Ok(p) => p,
        Err(e) => {
            let e = format!("{e:#}").replace('\n', " ");
            report.push(format!("unpack {e}"));
            report.extend(req.commands.iter().map(|(_, _, n)| format!("ng {n} unpacker error")));
            return Ok(push_response(sideband, &[], &report));
        }
    };
    report.push("unpack ok".to_string());
    repo.with(|r| {
        if let Some(p) = &pack {
            r.odb.learn(p.ids.iter().cloned());
        }
        let mut accepted = false;
        let want = format!("refs/heads/{}", r.branch);
        for (old, new, name) in &req.commands {
            if *name != want {
                let why = format!("rootstock accepts pushes to {want} only; agents use the API for other work");
                messages.push(format!("rejected {name}: {why}"));
                report.push(format!("ng {name} {why}"));
                continue;
            }
            let out = check_push_paths(r, who, new).and_then(|_| r.land_push(old, new));
            audit_push(r, who, new, &out);
            match out {
                Ok(v) => {
                    let n = v["changes"].as_array().map_or(0, Vec::len);
                    let ids: Vec<&str> = v["changes"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|c| c["change"].as_str()).collect())
                        .unwrap_or_default();
                    messages.push(format!(
                        "landed {n} commit(s) on {} as {}; agents see them now",
                        r.branch,
                        if ids.is_empty() { "-".to_string() } else { ids.join(", ") }
                    ));
                    report.push(format!("ok {name}"));
                    accepted = true;
                }
                Err(e) => {
                    let why = format!("{e:#}").replace('\n', " ");
                    messages.push(format!("rejected: {why}"));
                    report.push(format!("ng {name} {why}"));
                }
            }
        }
        // Like git's push quarantine: objects of a rejected push leave the repo again.
        if let (false, Some(p)) = (accepted, &pack) {
            p.remove(repo.git_dir())?;
            r.odb.unlearn(&p.ids)?;
        }
        Ok(())
    })?;
    Ok(push_response(sideband, &messages, &report))
}

/// A restricted token may push only commits whose changes stay inside its write globs.
fn check_push_paths(r: &mut crate::repo::Inner, who: &Caller, new: &str) -> Result<()> {
    if who.writes_all() || new.bytes().all(|b| b == b'0') {
        return Ok(());
    }
    let main = r.main()?;
    let cur = r.commit_for(&main)?;
    for (path, _) in git::diff_tree(r.odb.dir(), &cur, new)? {
        who.require_write(&path)?;
    }
    Ok(())
}

/// One audit row per landed commit of a push (or one row for a rejected push).
fn audit_push(r: &mut crate::repo::Inner, who: &Caller, new: &str, out: &Result<serde_json::Value>) {
    let base = Record { actor: who.name.clone(), token: who.token.clone(), op: "push".into(), ..Record::default() };
    let mut rows = Vec::new();
    match out {
        Ok(v) => {
            for c in v["changes"].as_array().into_iter().flatten() {
                let cs = c["changeset"].as_str().unwrap_or_default().to_string();
                let meta = r.changeset(&cs).ok();
                rows.push(Record {
                    agent: meta.as_ref().map(|m| m.meta.agent.clone()),
                    intent: meta.as_ref().map(|m| m.meta.intent.clone()),
                    session: Some("git push".into()),
                    change: c["change"].as_str().map(str::to_string),
                    changeset: Some(cs),
                    commit: c["commit"].as_str().map(str::to_string),
                    status: "landed".into(),
                    paths: meta.map(|m| m.changes.into_keys().collect()).unwrap_or_default(),
                    ..base.clone()
                });
            }
        }
        Err(e) => {
            let denied = e.downcast_ref::<crate::auth::Denied>().is_some();
            rows.push(Record {
                commit: Some(new.to_string()),
                status: if denied { "denied" } else { "rejected" }.into(),
                detail: Some(format!("{e:#}").chars().take(500).collect()),
                ..base
            });
        }
    }
    for rec in rows {
        if let Err(e) = r.audit_add(rec) {
            eprintln!("rootstock: audit write failed for push: {e:#}");
        }
    }
}

async fn receive_pack(State(repo): State<Arc<Repo>>, headers: HeaderMap, body: Bytes) -> Response {
    let who = match caller(&repo, &headers, true).await {
        Ok(c) => c,
        Err(resp) => return *resp,
    };
    let data = match request_body(&headers, body) {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, format!("bad request body: {e}")),
    };
    match tokio::task::spawn_blocking(move || handle_push(&repo, &who, &data)).await {
        Ok(Ok(out)) => reply(StatusCode::OK, "application/x-git-receive-pack-result", out),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Pushes the branch of the export repo in `data` to `remote` (never forced: a mirror that
/// moved on its own is an error). Returns the pushed commit.
pub fn mirror(data: &Path, remote: &str) -> Result<String> {
    let gd = data.join("git");
    if !gd.join("HEAD").exists() {
        bail!("{} is not a rootstock data directory", data.display());
    }
    let branch = git::run(Some(&gd), &["symbolic-ref", "--short", "HEAD"])?.trim().to_string();
    let commit = git::run(Some(&gd), &["rev-parse", "--verify", &format!("refs/heads/{branch}")])?.trim().to_string();
    let spec = format!("refs/heads/{branch}:refs/heads/{branch}");
    git::run(Some(&gd), &["push", "--quiet", "--porcelain", remote, &spec]).with_context(|| {
        format!("mirror push to {remote} failed; if the mirror moved on its own, it is not read-only")
    })?;
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_caps_and_pack() {
        let (a, b) = ("1".repeat(40), "2".repeat(40));
        let mut body = pkt(format!("{a} {b} refs/heads/main\0report-status side-band-64k\n").as_bytes());
        body.extend(pkt(format!("{a} {} refs/heads/x\n", "0".repeat(40)).as_bytes()));
        body.extend_from_slice(FLUSH);
        body.extend_from_slice(b"PACKdata");
        let req = parse_push(&body).unwrap();
        assert_eq!(req.commands.len(), 2);
        assert_eq!(req.commands[0], (a.clone(), b, "refs/heads/main".to_string()));
        assert_eq!(req.caps, ["report-status", "side-band-64k"]);
        assert_eq!(req.pack, b"PACKdata");
        assert!(parse_push(FLUSH).unwrap().commands.is_empty());
        assert!(parse_push(b"00").is_err());
        assert!(parse_push(&pkt(format!("shallow {a}").as_bytes())).is_err());
        // Ids that are not full hex (an option, a short id) are refused before git sees them.
        for bad in [format!("{a} --output=/tmp/x refs/heads/main"), format!("{a} abc refs/heads/main")] {
            let mut body = pkt(bad.as_bytes());
            body.extend_from_slice(FLUSH);
            assert!(parse_push(&body).is_err(), "{bad}");
        }
    }

    #[test]
    fn report_is_framed_in_band_one() {
        let plain = push_response(false, &["hi".into()], &["unpack ok".into(), "ok refs/heads/main".into()]);
        assert_eq!(plain, b"000eunpack ok\n0017ok refs/heads/main\n0000");
        let banded = push_response(true, &["hi".into()], &["unpack ok".into()]);
        assert!(banded.starts_with(b"0013\x02rootstock: hi\n"));
        assert!(banded.ends_with(b"0000"));
    }
}
