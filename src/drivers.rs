//! Merge drivers for paths where the line merge conflicts: union (changelogs, registries),
//! a 3-way merge of Cargo.lock per `[[package]]`, and syntax-aware merge through mergiraf.
//! Regeneration by a command needs the whole merged tree, so `repo.rs` runs it after the
//! per-path pass (see `Inner::regenerate`).

use anyhow::{Context, Result};
use diffy::{ConflictStyle, MergeOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// Marker length no real file uses, so the union driver can find its own markers.
const MARK: usize = 31;

/// Line merge where every conflict keeps both sides' lines (ours first), like git's
/// `merge=union`.
pub fn union(base: &str, ours: &str, theirs: &str) -> String {
    let merged = match MergeOptions::new()
        .set_conflict_marker_length(MARK)
        .set_conflict_style(ConflictStyle::Merge)
        .merge(base, ours, theirs)
    {
        Ok(m) => return m,
        Err(m) => m,
    };
    let (open, mid, close) = ("<".repeat(MARK), "=".repeat(MARK), ">".repeat(MARK));
    let mut out = String::new();
    for line in merged.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n');
        if bare.starts_with(&open) || bare == mid || bare.starts_with(&close) {
            continue;
        }
        out.push_str(line);
    }
    out
}

/// One field of a lockfile block: a scalar line or an array of items.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Field {
    Scalar(String),
    Array(Vec<String>),
}

type Block = Vec<(String, Field)>;

/// Splits a Cargo.lock into blocks keyed by `name version source` for packages, or by the
/// first line for other blocks (the header comment, `version = 3`, `[metadata]`).
fn lock_blocks(text: &str) -> Option<(Vec<String>, BTreeMap<String, Block>)> {
    let mut order = Vec::new();
    let mut blocks = BTreeMap::new();
    for chunk in text.split("\n\n") {
        let chunk = chunk.trim_matches('\n');
        if chunk.is_empty() {
            continue;
        }
        let mut fields: Block = Vec::new();
        let mut lines = chunk.lines().peekable();
        while let Some(line) = lines.next() {
            if line.trim_end().ends_with("= [") && !line.starts_with('#') {
                let key = line.split_once(" = ").map_or(line, |(k, _)| k).to_string();
                let mut items = Vec::new();
                loop {
                    let item = lines.next()?;
                    if item.trim() == "]" {
                        break;
                    }
                    items.push(item.to_string());
                }
                fields.push((key, Field::Array(items)));
            } else {
                let key = line.split_once(" = ").map_or(line, |(k, _)| k).to_string();
                fields.push((key, Field::Scalar(line.to_string())));
            }
        }
        let get = |k: &str| {
            fields.iter().find(|(f, _)| f == k).and_then(|(_, v)| match v {
                Field::Scalar(s) => s.split_once(" = ").map(|(_, v)| v.trim_matches('"').to_string()),
                Field::Array(_) => None,
            })
        };
        let key = if chunk.starts_with("[[package]]") {
            format!("~pkg {} {} {}", get("name")?, get("version")?, get("source").unwrap_or_default())
        } else {
            chunk.lines().next()?.to_string()
        };
        if blocks.insert(key.clone(), fields).is_some() {
            return None;
        }
        order.push(key);
    }
    Some((order, blocks))
}

fn merge3_value<T: Clone + PartialEq>(b: Option<&T>, o: Option<&T>, t: Option<&T>) -> Result<Option<T>, ()> {
    if o == t || b == t {
        Ok(o.cloned())
    } else if b == o {
        Ok(t.cloned())
    } else {
        Err(())
    }
}

fn merge_block(b: Option<&Block>, o: &Block, t: &Block) -> Option<Block> {
    let find = |blk: Option<&Block>, k: &str| blk.and_then(|x| x.iter().find(|(f, _)| f == k).map(|(_, v)| v.clone()));
    let mut keys: Vec<String> = t.iter().map(|(k, _)| k.clone()).collect();
    for (k, _) in o {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let mut out = Vec::new();
    for k in keys {
        let (bv, ov, tv) = (find(b, &k), find(Some(o), &k), find(Some(t), &k));
        let v = match (&bv, &ov, &tv) {
            // Arrays (dependency lists) merge as sets: an item stays unless one side removed it.
            (_, Some(Field::Array(oi)), Some(Field::Array(ti))) => {
                let bi: BTreeSet<&String> = match &bv {
                    Some(Field::Array(x)) => x.iter().collect(),
                    _ => BTreeSet::new(),
                };
                let (os, ts): (BTreeSet<&String>, BTreeSet<&String>) = (oi.iter().collect(), ti.iter().collect());
                let mut items: Vec<String> = os
                    .union(&ts)
                    .filter(|x| (os.contains(*x) && ts.contains(*x)) || !bi.contains(*x))
                    .map(|x| (*x).clone())
                    .collect();
                items.sort();
                Some(Field::Array(items))
            }
            _ => merge3_value(bv.as_ref(), ov.as_ref(), tv.as_ref()).ok()?,
        };
        if let Some(v) = v {
            out.push((k, v));
        }
    }
    Some(out)
}

fn render_block(b: &Block) -> String {
    let mut s = String::new();
    for (k, v) in b {
        match v {
            Field::Scalar(line) => s.push_str(line),
            Field::Array(items) => {
                s.push_str(&format!("{k} = [\n"));
                for i in items {
                    s.push_str(i);
                    s.push('\n');
                }
                s.push(']');
            }
        }
        s.push('\n');
    }
    s
}

/// 3-way merge of a Cargo.lock: packages are added or removed per side; a package both sides
/// changed merges per field, with dependency lists as sets. `None` when a real conflict
/// remains (both sides changed one scalar differently) or the file does not parse.
pub fn cargo_lock(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let (_, bb) = lock_blocks(base)?;
    let (oorder, ob) = lock_blocks(ours)?;
    let (torder, tb) = lock_blocks(theirs)?;
    let mut keys: BTreeSet<&String> = ob.keys().collect();
    keys.extend(tb.keys());
    keys.extend(bb.keys());
    if replaced_twice(&bb, &ob, &tb) {
        return None;
    }
    let mut merged: BTreeMap<String, Block> = BTreeMap::new();
    for k in keys {
        let (b, o, t) = (bb.get(k), ob.get(k), tb.get(k));
        let v = match (o, t) {
            (Some(o), Some(t)) if o != t => Some(merge_block(b, o, t)?),
            _ => merge3_value(b, o, t).ok()?,
        };
        if let Some(v) = v {
            merged.insert(k.clone(), v);
        }
    }
    // Non-package blocks keep theirs' order (then ours'); packages sort by name and version,
    // as cargo writes them.
    let mut out: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    let heads = torder.iter().chain(oorder.iter()).filter(|k| !k.starts_with("~pkg ") && *k != "[metadata]");
    for k in heads {
        if seen.insert(k.clone())
            && let Some(b) = merged.get(k)
        {
            out.push(render_block(b));
        }
    }
    let mut pkgs: Vec<(&String, &Block)> = merged.iter().filter(|(k, _)| k.starts_with("~pkg ")).collect();
    pkgs.sort_by_key(|(k, _)| pkg_order(k));
    out.extend(pkgs.into_iter().map(|(_, b)| render_block(b)));
    if let Some(m) = merged.get("[metadata]") {
        out.push(render_block(m));
    }
    Some(out.join("\n"))
}

/// (name, version parts with numbers compared as numbers, source): the order cargo writes.
fn pkg_order(key: &str) -> (String, Vec<(u64, String)>, String) {
    let mut it = key.splitn(4, ' ').skip(1);
    let name = it.next().unwrap_or_default().to_string();
    let version = it.next().unwrap_or_default();
    let parts = version.split(['.', '-', '+']).map(|p| (p.parse().unwrap_or(u64::MAX), p.to_string())).collect();
    (name, parts, it.next().unwrap_or_default().to_string())
}

/// Package names whose base version both sides replaced, each with a different new version:
/// the lockfile would list two versions where the history has one.
fn replaced_twice(b: &BTreeMap<String, Block>, o: &BTreeMap<String, Block>, t: &BTreeMap<String, Block>) -> bool {
    let versions = |m: &BTreeMap<String, Block>, name: &str| -> BTreeSet<String> {
        m.keys().filter(|k| pkg_order(k).0 == name && k.starts_with("~pkg ")).cloned().collect()
    };
    let names: BTreeSet<String> = b.keys().filter(|k| k.starts_with("~pkg ")).map(|k| pkg_order(k).0).collect();
    names.iter().any(|n| {
        let (bv, ov, tv) = (versions(b, n), versions(o, n), versions(t, n));
        let (o_new, t_new): (BTreeSet<_>, BTreeSet<_>) = (&ov - &bv, &tv - &bv);
        let gone_both = !(&(&bv - &ov) & &(&bv - &tv)).is_empty();
        gone_both && !o_new.is_empty() && !t_new.is_empty() && o_new != t_new
    })
}

/// Syntax-aware merge with mergiraf. `None` if mergiraf leaves a conflict or fails.
pub fn mergiraf(bin: &str, path: &str, base: &str, ours: &str, theirs: &str) -> Result<Option<String>> {
    let dir = tempfile_dir()?;
    let (b, o, t, out) = (dir.join("base"), dir.join("ours"), dir.join("theirs"), dir.join("out"));
    std::fs::write(&b, base)?;
    std::fs::write(&o, ours)?;
    std::fs::write(&t, theirs)?;
    let res = Command::new(bin)
        .args(["merge", "--timeout", "5000", "-p", path, "-o"])
        .arg(&out)
        .args([&b, &o, &t])
        .output()
        .with_context(|| format!("running {bin}"))?;
    let merged = std::fs::read_to_string(&out).ok();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(match merged {
        Some(m) if res.status.success() && !m.lines().any(|l| l.starts_with("<<<<<<<")) => Some(m),
        _ => None,
    })
}

/// A fresh private temp dir (removed by the caller).
pub fn tempfile_dir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("treeyard-{}-{:016x}", std::process::id(), rand::random::<u64>()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_keeps_both_sides() {
        let base = "# Changes\n\n## 1.0\n- old\n";
        let ours = "# Changes\n\n- ours\n## 1.0\n- old\n";
        let theirs = "# Changes\n\n- theirs\n## 1.0\n- old\n";
        assert!(diffy::merge(base, ours, theirs).is_err(), "plain merge must conflict");
        assert_eq!(union(base, ours, theirs), "# Changes\n\n- ours\n- theirs\n## 1.0\n- old\n");
    }

    fn pkg(name: &str, version: &str, deps: &[&str]) -> String {
        let mut s = format!("[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n");
        if !deps.is_empty() {
            s.push_str("dependencies = [\n");
            for d in deps {
                s.push_str(&format!(" \"{d}\",\n"));
            }
            s.push_str("]\n");
        }
        s
    }

    fn lock(pkgs: &[String]) -> String {
        let mut s = "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 3\n".to_string();
        for p in pkgs {
            s.push('\n');
            s.push_str(p);
        }
        s
    }

    #[test]
    fn cargo_lock_merges_independent_bumps_and_new_deps() {
        let base = lock(&[pkg("aho", "1.0.0", &[]), pkg("app", "0.1.0", &["aho", "bstr"]), pkg("bstr", "1.0.0", &[])]);
        let ours = lock(&[
            pkg("aho", "1.1.0", &[]),
            pkg("app", "0.1.0", &["aho", "bstr", "cfg"]),
            pkg("bstr", "1.0.0", &[]),
            pkg("cfg", "1.0.0", &[]),
        ]);
        let theirs = lock(&[
            pkg("aho", "1.0.0", &[]),
            pkg("app", "0.1.0", &["aho", "bstr", "memchr"]),
            pkg("bstr", "1.2.0", &[]),
            pkg("memchr", "2.0.0", &[]),
        ]);
        assert!(diffy::merge(&base, &ours, &theirs).is_err(), "plain merge must conflict");
        let want = lock(&[
            pkg("aho", "1.1.0", &[]),
            pkg("app", "0.1.0", &["aho", "bstr", "cfg", "memchr"]),
            pkg("bstr", "1.2.0", &[]),
            pkg("cfg", "1.0.0", &[]),
            pkg("memchr", "2.0.0", &[]),
        ]);
        assert_eq!(cargo_lock(&base, &ours, &theirs).unwrap(), want);
    }

    #[test]
    fn cargo_lock_conflicts_when_both_sides_pick_a_different_version() {
        let base = lock(&[pkg("aho", "1.0.0", &[]), pkg("app", "0.1.0", &["aho"])]);
        let ours = lock(&[pkg("aho", "1.0.0", &[]), pkg("app", "0.2.0", &["aho"])]);
        let theirs = lock(&[pkg("aho", "1.0.0", &[]), pkg("app", "0.3.0", &["aho"])]);
        // Both sides replaced app 0.1.0 with a different version: a real conflict.
        assert_eq!(cargo_lock(&base, &ours, &theirs), None);
        let ours = base
            .replace("name = \"aho\"\nversion = \"1.0.0\"", "name = \"aho\"\nversion = \"1.0.0\"\nchecksum = \"aa\"");
        let theirs = base
            .replace("name = \"aho\"\nversion = \"1.0.0\"", "name = \"aho\"\nversion = \"1.0.0\"\nchecksum = \"bb\"");
        assert_eq!(cargo_lock(&base, &ours, &theirs), None);
    }
}
