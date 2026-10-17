//! HTTP API: `POST /api/<operation>` with a JSON body, answered with JSON; git smart HTTP
//! for people under `/git` (see `githttp`).

use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures_util::stream::{self, Stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

use crate::auth::{Denied, Unauthorized};
use crate::coord::Filter;
use crate::githttp;
use crate::repo::Repo;
use crate::{landing, rpc};

/// Largest API request body: a workspace sync sends file contents in batches below this.
pub const API_BODY: usize = 64 << 20;

/// Default cap on API requests that run at once (`--max-inflight`); more get `503 busy`.
pub const MAX_INFLIGHT: usize = 256;

/// Request limits of one server.
#[derive(Clone)]
pub struct Limits {
    /// API calls (all but the `events` long poll) that may run at once.
    inflight: Arc<Semaphore>,
    max: usize,
}

impl Limits {
    pub fn new(max_inflight: usize) -> Self {
        Self { inflight: Arc::new(Semaphore::new(max_inflight)), max: max_inflight }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(MAX_INFLIGHT)
    }
}

pub fn router(repo: Arc<Repo>) -> Router {
    router_with(repo, Limits::default())
}

pub fn router_with(repo: Arc<Repo>, limits: Limits) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/events", get(events).post(post_events))
        .route("/api/{op}", post(handle).layer(DefaultBodyLimit::max(API_BODY)))
        .layer(Extension(limits))
        .merge(githttp::routes())
        .with_state(repo)
}

async fn handle(
    State(repo): State<Arc<Repo>>,
    Extension(limits): Extension<Limits>,
    Path(op): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if op == "events" {
        return run(repo, op, &headers, body, None).await;
    }
    let Ok(permit) = limits.inflight.clone().try_acquire_owned() else {
        let msg = format!("server busy: {} API calls are running; retry after a short wait", limits.max);
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg, "retry": true})));
    };
    run(repo, op, &headers, body, Some(permit)).await
}

/// `POST /api/events` is the poll operation; `GET` is the event stream.
async fn post_events(State(repo): State<Arc<Repo>>, headers: HeaderMap, body: Bytes) -> (StatusCode, Json<Value>) {
    run(repo, "events".into(), &headers, body, None).await
}

/// The token of a request: `Authorization: Bearer <token>`, or the password (else the user)
/// of HTTP Basic, which is what git sends.
pub fn token_of(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.trim();
    if let Some(t) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) {
        return Some(t.trim().to_string());
    }
    let b = v.strip_prefix("Basic ").or_else(|| v.strip_prefix("basic "))?;
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD.decode(b.trim()).ok()?;
    let raw = String::from_utf8(raw).ok()?;
    let (user, pass) = raw.split_once(':').unwrap_or((&raw, ""));
    Some(if pass.is_empty() { user } else { pass }.to_string())
}

/// The HTTP status for an operation error: 401 (no valid token), 403 (no right), 422 (other).
pub fn status_of(e: &anyhow::Error) -> StatusCode {
    if e.downcast_ref::<Unauthorized>().is_some() {
        StatusCode::UNAUTHORIZED
    } else if e.downcast_ref::<Denied>().is_some() {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    }
}

async fn run(
    repo: Arc<Repo>,
    op: String,
    headers: &HeaderMap,
    body: Bytes,
    permit: Option<OwnedSemaphorePermit>,
) -> (StatusCode, Json<Value>) {
    if op.len() > 64 || !op.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "unknown operation"})));
    }
    let token = token_of(headers);
    let params = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid JSON: {e}")}))),
        }
    };
    let run = move || {
        let _permit = permit;
        let caller = repo.auth.authenticate(token.as_deref())?;
        rpc::call_as(&repo, &caller, &op, params)
    };
    match tokio::task::spawn_blocking(run).await {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)),
        Ok(Err(e)) => (status_of(&e), Json(json!({"error": format!("{e:#}")}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

#[derive(Deserialize)]
struct EventQuery {
    since: Option<u64>,
    change: Option<String>,
    agent: Option<String>,
    /// Comma-separated event kinds.
    kinds: Option<String>,
}

/// `GET /api/events`: server-sent events, one JSON event per message (`id` = seq). Starts with
/// the kept events after `since` (or `Last-Event-ID`), then follows live.
async fn events(
    State(repo): State<Arc<Repo>>,
    headers: HeaderMap,
    Query(q): Query<EventQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let token = token_of(&headers);
    if let Err(e) = repo.auth.authenticate(token.as_deref()) {
        return (status_of(&e), Json(json!({"error": format!("{e:#}")}))).into_response();
    }
    event_stream(repo, headers, q).into_response()
}

fn event_stream(
    repo: Arc<Repo>,
    headers: HeaderMap,
    q: EventQuery,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let f = Filter {
        change: q.change,
        agent: q.agent,
        kinds: q
            .kinds
            .map(|k| k.split(',').map(str::to_string).filter(|k| !k.is_empty()).collect())
            .unwrap_or_default(),
    };
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let since = q.since.or(last).unwrap_or(0);
    let rx = repo.events.subscribe();
    let backlog = repo.events.poll(since, &f, 5000, std::time::Duration::ZERO);
    let queue: VecDeque<Value> = backlog["events"].as_array().cloned().unwrap_or_default().into();
    let hello = Event::default()
        .event("hello")
        .data(json!({"epoch": repo.events.epoch, "latest": backlog["latest"]}).to_string());
    let state = (Some(hello), queue, rx, f, since);
    let s = stream::unfold(state, |(mut hello, mut queue, mut rx, f, mut last)| async move {
        if let Some(h) = hello.take() {
            return Some((Ok(h), (None, queue, rx, f, last)));
        }
        loop {
            let ev = match queue.pop_front() {
                Some(e) => e,
                None => match rx.recv().await {
                    Ok(e) => e,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
            };
            let seq = ev["seq"].as_u64().unwrap_or(0);
            if seq <= last || !f.matches(&ev) {
                continue;
            }
            last = seq;
            let msg =
                Event::default().id(seq.to_string()).event(ev["kind"].as_str().unwrap_or("event")).data(ev.to_string());
            return Some((Ok(msg), (None, queue, rx, f, last)));
        }
    });
    Sse::new(s).keep_alive(KeepAlive::default())
}

/// How `serve` runs: request limits and whether an open server may listen beyond loopback.
#[derive(Clone, Default)]
pub struct ServeOptions {
    pub limits: Limits,
    /// Allow a non-loopback address while no API token exists (anyone on the network could
    /// read the repo and push to main).
    pub insecure_open: bool,
}

/// Refuses to serve an open server (no tokens) on an address other machines can reach.
pub fn check_bind(repo: &Repo, addr: &str, insecure_open: bool) -> Result<()> {
    if crate::safety::is_loopback(addr) || insecure_open || repo.auth.enabled()? {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to serve {addr} without auth: there are no API tokens, so anyone who reaches \
         the port could read the repo and land on main. Create one with `treeyard token create \
         --data DIR --name admin --admin`, bind 127.0.0.1, or pass --insecure-open"
    )
}

/// Holds an exclusive lock on the data directory while a server runs, so a second server
/// on the same data cannot interleave writes (the repo mutex is per process).
pub struct DataLock(#[allow(dead_code)] std::fs::File);

pub fn lock_data(data: &std::path::Path) -> Result<DataLock> {
    use anyhow::Context as _;
    if !data.join("treeyard.db").exists() {
        anyhow::bail!("{} is not a treeyard repo (run `treeyard init` first)", data.display());
    }
    let path = data.join("serve.lock");
    let f = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(DataLock(f)),
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("another treeyard server already serves {} (lock {})", data.display(), path.display())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {}", path.display())),
    }
}

pub async fn serve(repo: Arc<Repo>, addr: &str) -> Result<()> {
    serve_with(repo, addr, ServeOptions::default()).await
}

pub async fn serve_with(repo: Arc<Repo>, addr: &str, opts: ServeOptions) -> Result<()> {
    check_bind(&repo, addr, opts.insecure_open)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let at = listener.local_addr()?;
    eprintln!("treeyard listening on http://{at} (API /api/<op>, events /api/events, git clone http://{at}/git)");
    if landing::spawn_worker(repo.clone()).is_some() {
        eprintln!("treeyard landing worker started");
    }
    if crate::indexer::spawn(repo.clone()).is_some() {
        eprintln!("treeyard indexer started (eager derived data)");
    }
    if !repo.auth.enabled()? {
        eprintln!("treeyard: no API tokens yet, so the server is open; `treeyard token create` turns auth on");
    }
    axum::serve(listener, router_with(repo, opts.limits)).await?;
    Ok(())
}
