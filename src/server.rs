//! HTTP API: `POST /api/<operation>` with a JSON body, answered with JSON.

use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::repo::Repo;
use crate::rpc;

pub fn router(repo: Arc<Repo>) -> Router {
    Router::new().route("/health", get(|| async { "ok" })).route("/api/{op}", post(handle)).with_state(repo)
}

async fn handle(State(repo): State<Arc<Repo>>, Path(op): Path<String>, body: Bytes) -> (StatusCode, Json<Value>) {
    let params = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid JSON: {e}")}))),
        }
    };
    match tokio::task::spawn_blocking(move || rpc::call(&repo, &op, params)).await {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)),
        Ok(Err(e)) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": format!("{e:#}")}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

pub async fn serve(repo: Arc<Repo>, addr: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("rootstock listening on http://{}", listener.local_addr()?);
    axum::serve(listener, router(repo)).await?;
    Ok(())
}
