//! MCP over Streamable HTTP (`POST /mcp`): the endpoint an MCP gateway (for example one that serves a
//! company's own MCP servers to its members) connects to. Same tools and caller rules as `/api`, stateless.

mod common;

use common::*;
use serde_json::{Value, json};
use std::sync::Arc;
use treeyard::repo::Repo;

struct Setup {
    _tmp: tempfile::TempDir,
    url: String,
    agent: String,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 3, &[("src/a.rs", "pub fn a() -> i32 {\n    1\n}\n")]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    let (_, _admin) = repo.auth.create("root", true, &["**".into()], &["**".into()]).unwrap();
    let (_, agent) = repo.auth.create("alice", false, &["**".into()], &["src/**".into()]).unwrap();
    let url = spawn_server(repo);
    Setup { _tmp: tmp, url, agent }
}

/// (status, Www-Authenticate, body) of one POST to /mcp.
fn post(url: &str, auth: Option<&str>, body: &Value) -> (u16, Option<String>, Value) {
    let mut req = ureq::post(&format!("{url}/mcp")).set("Content-Type", "application/json");
    if let Some(a) = auth {
        req = req.set("Authorization", a);
    }
    let resp = match req.send_string(&body.to_string()) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("POST /mcp: {e}"),
    };
    let status = resp.status();
    let challenge = resp.header("WWW-Authenticate").map(str::to_string);
    let text = resp.into_string().unwrap();
    let body = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap() };
    (status, challenge, body)
}

fn tool(url: &str, auth: &str, name: &str, args: Value) -> (bool, String) {
    let msg = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let (status, _, r) = post(url, Some(auth), &msg);
    assert_eq!(status, 200, "{r}");
    let res = &r["result"];
    (res["isError"].as_bool().unwrap_or(false), res["content"][0]["text"].as_str().unwrap().to_string())
}

#[test]
fn without_a_token_initialize_is_401_with_a_bearer_challenge() {
    let s = setup();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}});
    let (status, challenge, body) = post(&s.url, None, &init);
    assert_eq!(status, 401, "{body}");
    assert_eq!(challenge.as_deref(), Some("Bearer realm=\"treeyard\""));
    let (status, _, _) = post(&s.url, Some("Bearer rst_bogus_x"), &init);
    assert_eq!(status, 401);
}

#[test]
fn a_session_lists_and_calls_tools_as_the_tokens_agent() {
    let s = setup();
    let bearer = format!("Bearer {}", s.agent);
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}});
    let (status, _, r) = post(&s.url, Some(&bearer), &init);
    assert_eq!(status, 200, "{r}");
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "treeyard");

    let (status, _, r) = post(&s.url, Some(&bearer), &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    assert_eq!((status, r), (202, Value::Null));

    let batch = json!([
        {"jsonrpc": "2.0", "id": 2, "method": "ping"},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/list"},
    ]);
    let (_, _, r) = post(&s.url, Some(&bearer), &batch);
    assert_eq!(r[0]["id"], 2);
    let tools = r[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), treeyard::rpc::tools().len());

    let (failed, text) = tool(&s.url, &bearer, "whoami", json!({}));
    assert!(!failed && text.contains("\"name\": \"alice\""), "{text}");
    let (failed, text) = tool(&s.url, &bearer, "land", json!({"change": "ch_nope"}));
    assert!(failed && text.contains("unknown change"), "{text}");

    // The token's write globs hold here as on /api.
    let (_, text) = tool(&s.url, &bearer, "open_change", json!({"intent": "edit outside src"}));
    let ch: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(ch["agent"], "alice", "a non-admin token always acts as its own agent");
    let edit = json!({"change": ch["id"], "edits": [{"path": "f00.txt", "content": "x\n"}]});
    let (failed, text) = tool(&s.url, &bearer, "apply", edit);
    assert!(failed && text.contains("may not write f00.txt"), "{text}");
}

#[test]
fn a_bare_token_works_as_a_gateway_sends_it() {
    let s = setup();
    let (failed, text) = tool(&s.url, &s.agent, "whoami", json!({}));
    assert!(!failed && text.contains("\"name\": \"alice\""), "{text}");
}

#[test]
fn get_and_delete_are_405() {
    let s = setup();
    for method in ["GET", "DELETE"] {
        match ureq::request(method, &format!("{}/mcp", s.url)).call() {
            Err(ureq::Error::Status(405, r)) => assert_eq!(r.header("Allow"), Some("POST")),
            other => panic!("{method} /mcp: {other:?}"),
        }
    }
}
