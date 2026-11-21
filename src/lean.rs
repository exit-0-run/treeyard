//! The agent view of replies (`treeyard mcp --tools agent`, `POST /mcp?tools=agent`): the same
//! operations, with less to read. Change replies lose the fields agents never used (`base`, `head`,
//! `created`, `group`, `landed_as`, `model`, `session`, `view`, `revisions`) and every null or
//! empty value; `apply` sends `collisions` and `overlaps` in full only when they changed since the
//! last reply of that change, else one line; `grep` drops `at` and the index statistics. The
//! `all` view is the server's reply as is.

use serde_json::{Value, json};
use std::collections::HashMap;

/// Change fields agents never referenced in 110 recorded sessions.
const NEVER_USED: &[&str] = &["base", "head", "created", "group", "landed_as", "model", "session", "view", "revisions"];

/// Operations whose reply is one change.
const CHANGE_REPLIES: &[&str] = &["open_change", "declare", "apply", "change", "rebase", "resolve", "abandon"];

/// What the agent view remembers between replies: per change, the `collisions` and `overlaps`
/// it last sent in full.
#[derive(Default)]
pub struct Lean {
    sent: HashMap<String, [String; 2]>,
}

fn is_change(v: &Value) -> bool {
    v["id"].as_str().is_some_and(|s| s.starts_with("ch_")) && v.get("state").is_some()
}

/// Removes null, `[]` and `{}` values from objects, recursively (array items stay).
pub fn prune(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for x in m.values_mut() {
                prune(x);
            }
            m.retain(|_, x| match x {
                Value::Null => false,
                Value::Array(a) => !a.is_empty(),
                Value::Object(o) => !o.is_empty(),
                _ => true,
            });
        }
        Value::Array(a) => a.iter_mut().for_each(prune),
        _ => {}
    }
}

fn lean_change(v: &mut Value) {
    if let Some(m) = v.as_object_mut() {
        for k in NEVER_USED {
            m.remove(*k);
        }
    }
    prune(v);
}

/// One line for a list that did not change: counts and change ids.
fn summary(key: &str, list: &[Value]) -> String {
    let mut ids: Vec<&str> = list.iter().filter_map(|x| x["change"].as_str()).collect();
    ids.dedup();
    let mut kinds: Vec<String> = Vec::new();
    if key == "collisions" {
        let mut by: Vec<(&str, usize)> = Vec::new();
        for k in list.iter().filter_map(|x| x["kind"].as_str()) {
            match by.iter_mut().find(|(n, _)| *n == k) {
                Some(e) => e.1 += 1,
                None => by.push((k, 1)),
            }
        }
        kinds = by.into_iter().map(|(k, n)| format!("{n} {k}")).collect();
    }
    let kinds = if kinds.is_empty() { String::new() } else { format!(" ({})", kinds.join(", ")) };
    format!(
        "{} {key}{kinds} with {}: unchanged since your last reply of this change; call change for them in full",
        list.len(),
        ids.join(", ")
    )
}

/// Checkout fields the agent does not act on (it has the change id; `dir` is what it uses).
const CHECKOUT_NOISE: &[&str] = &["change", "state", "head", "files", "seconds"];

fn lean_checkout(v: &mut Value) {
    if let Some(m) = v.as_object_mut() {
        for k in CHECKOUT_NOISE {
            m.remove(*k);
        }
        if m.get("main_moved") == Some(&json!(false)) {
            m.remove("main_moved");
        }
    }
    prune(v);
}

impl Lean {
    /// Rewrites the reply `v` of operation `op` for the agent view.
    pub fn shape(&mut self, op: &str, v: &mut Value) {
        if op == "checkout" {
            lean_checkout(v);
            return;
        }
        if v.get("checkout").is_some_and(Value::is_object) {
            lean_checkout(&mut v["checkout"]);
        }
        if CHANGE_REPLIES.contains(&op) && is_change(v) {
            let id = v["id"].as_str().unwrap_or_default().to_string();
            if op == "apply" || op == "change" {
                let sent = self.sent.entry(id.clone()).or_default();
                for (i, key) in ["collisions", "overlaps"].into_iter().enumerate() {
                    let Some(list) = v.get(key).and_then(Value::as_array).cloned() else { continue };
                    let now = Value::Array(list.clone()).to_string();
                    if op == "apply" && !list.is_empty() && sent[i] == now {
                        v[format!("{key}_unchanged")] = json!(summary(key, &list));
                        v.as_object_mut().expect("a change").remove(key);
                    }
                    sent[i] = now;
                }
            }
            if op == "abandon" {
                self.sent.remove(&id);
            }
            lean_change(v);
            return;
        }
        match op {
            "land" | "restore_change" => {
                if v["change"].is_object() {
                    lean_change(&mut v["change"]);
                }
                // The entries of a held land: the agent needs path, lines, whose and the text.
                if let Some(list) = v.get_mut("undoes").and_then(Value::as_array_mut) {
                    for e in list.iter_mut().filter_map(Value::as_object_mut) {
                        e.remove("changeset");
                    }
                }
                if matches!(v["status"].as_str(), Some("landed" | "already_on_main"))
                    && let Some(id) = v["change"].as_str()
                {
                    self.sent.remove(id);
                }
                prune(v);
            }
            "changes" => {
                if let Some(a) = v["changes"].as_array_mut() {
                    a.iter_mut().for_each(lean_change);
                }
            }
            "grep" => {
                if let Some(m) = v.as_object_mut() {
                    m.remove("at");
                    m.remove("index");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(collisions: Value) -> Value {
        json!({"id": "ch_1", "state": "open", "agent": "a", "intent": "x", "base": "b", "head": "h", "model": null,
            "paths": ["f"], "conflicts": [], "declared": {"paths": [], "symbols": []}, "revisions": 2,
            "collisions": collisions, "overlaps": [], "undoes": []})
    }

    #[test]
    fn a_change_loses_unused_and_empty_fields() {
        let mut v = change(json!([]));
        Lean::default().shape("open_change", &mut v);
        assert_eq!(v, json!({"id": "ch_1", "state": "open", "agent": "a", "intent": "x", "paths": ["f"]}));
    }

    #[test]
    fn unchanged_collisions_become_one_line() {
        let mut lean = Lean::default();
        let c = json!([{"change": "ch_2", "kind": "conflict", "path": "f"}]);
        let mut v = change(c.clone());
        lean.shape("apply", &mut v);
        assert_eq!(v["collisions"], c, "new: in full");
        let mut v = change(c.clone());
        lean.shape("apply", &mut v);
        assert!(v.get("collisions").is_none());
        assert_eq!(
            v["collisions_unchanged"],
            "1 collisions (1 conflict) with ch_2: unchanged since your last reply of this change; call change for them in full"
        );
        let mut v = change(json!([{"change": "ch_2", "kind": "overlap", "path": "f"}]));
        lean.shape("apply", &mut v);
        assert_eq!(v["collisions"][0]["kind"], "overlap", "changed: in full again");
        // `change` always sends them in full.
        let mut v = change(json!([{"change": "ch_2", "kind": "overlap", "path": "f"}]));
        lean.shape("change", &mut v);
        assert!(v["collisions"].is_array());
    }

    #[test]
    fn a_held_land_is_compact() {
        let mut v = json!({"status": "undoes", "change": "ch_b", "advice": "put them back", "merged_from": null,
            "undoes": [{"path": "m.py", "lines": [2, 3], "agent": "a", "intent": "lower case", "changeset": "c0ffee",
                "change": "ch_a", "text": "x\n"}]});
        Lean::default().shape("land", &mut v);
        assert_eq!(
            v,
            json!({"status": "undoes", "change": "ch_b", "advice": "put them back",
                "undoes": [{"path": "m.py", "lines": [2, 3], "agent": "a", "intent": "lower case", "change": "ch_a", "text": "x\n"}]})
        );
    }

    #[test]
    fn grep_drops_index_statistics() {
        let mut v = json!({"at": "x", "matches": [], "truncated": false, "index": {"used": true}});
        Lean::default().shape("grep", &mut v);
        assert_eq!(v, json!({"matches": [], "truncated": false}));
    }
}
