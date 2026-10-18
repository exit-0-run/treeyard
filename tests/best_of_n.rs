//! Best-of-N: changes that try the same task share a group (`open_change` `group`); `group`
//! compares them and `land_one_of` lands one and abandons the others.

mod common;

use common::*;
use serde_json::json;
use std::sync::Arc;
use treeyard::repo::Repo;

#[test]
fn land_one_of_lands_the_pick_and_abandons_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 3, 5, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));

    let mut ids = Vec::new();
    for (k, body) in ["one line\n", "two\nlines\n", "three\nlines\nhere\n"].iter().enumerate() {
        let ch =
            call(&url, "open_change", json!({"agent": format!("try-{k}"), "intent": "fix f00", "group": "fix-f00"}));
        assert_eq!(ch["group"], "fix-f00");
        let id = ch["id"].as_str().unwrap().to_string();
        call(&url, "apply", json!({"change": id, "edits": [{"path": "f00.txt", "content": body}]}));
        ids.push(id);
    }
    let other = call(&url, "open_change", json!({"agent": "x", "intent": "not in the group"}))["id"].clone();

    let g = call(&url, "group", json!({"group": "fix-f00"}));
    let m = g["members"].as_array().unwrap();
    assert_eq!(m.len(), 3, "{g}");
    let three = m.iter().find(|x| x["change"] == ids[2].as_str()).unwrap();
    assert_eq!(three["diff"], json!({"files": 1, "added": 3, "removed": 5}), "{g}");
    assert!(m.iter().all(|x| x["state"] == "open"));

    let e = treeyard::client::call(&url, "land_one_of", json!({"group": "fix-f00", "change": other})).unwrap_err();
    assert!(format!("{e:#}").contains("not in group"), "{e:#}");

    let r = call(&url, "land_one_of", json!({"group": "fix-f00", "change": ids[1]}));
    assert_eq!(r["status"], "landed", "{r}");
    let mut gone: Vec<String> = serde_json::from_value(r["abandoned"].clone()).unwrap();
    gone.sort();
    let mut want = vec![ids[0].clone(), ids[2].clone()];
    want.sort();
    assert_eq!(gone, want);
    assert_eq!(call(&url, "read", json!({"path": "f00.txt"}))["content"], "two\nlines\n");
    let g = call(&url, "group", json!({"group": "fix-f00"}));
    for x in g["members"].as_array().unwrap() {
        let want = if x["change"] == ids[1].as_str() { "landed" } else { "abandoned" };
        assert_eq!(x["state"], want, "{g}");
    }
    assert_eq!(call(&url, "change", json!({"change": other}))["state"], "open", "other changes stay");

    // Again: the land answers as before, nothing left to abandon.
    let again = call(&url, "land_one_of", json!({"group": "fix-f00", "change": ids[1]}));
    assert_eq!(again["status"], "landed");
    assert_eq!(again["abandoned"], json!([]));

    assert!(treeyard::client::call(&url, "group", json!({"group": "nope"})).is_err());
    let bad = treeyard::client::call(&url, "open_change", json!({"agent": "y", "intent": "i", "group": " "}));
    assert!(bad.is_err(), "an empty group name is refused");
}
