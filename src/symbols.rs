//! Symbol index: definitions (functions, methods, types, classes, traits, modules, constants)
//! found by tree-sitter in Rust, Python, Go, JavaScript and TypeScript files.
//!
//! Derived data per (blob, language), persisted in SQLite with an index on the name: a
//! revision only parses the blobs that are new in it. Parsing runs outside the repo lock.

use anyhow::{Context, Result};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use crate::model::MODE_GITLINK;
use crate::repo::{Inner, Repo};
use crate::search::par_map;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS symbol_blobs (blob TEXT NOT NULL, lang TEXT NOT NULL, count INTEGER NOT NULL,
    PRIMARY KEY (blob, lang)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS symbols (name TEXT NOT NULL, blob TEXT NOT NULL, lang TEXT NOT NULL, kind TEXT NOT NULL,
    line INTEGER NOT NULL, end_line INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols (name);
CREATE INDEX IF NOT EXISTS symbols_blob ON symbols (blob, lang);
";

/// Files larger than this are not parsed.
const MAX_FILE: usize = 2 << 20;

const RUST_EXTRA: &str = "
(const_item name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.constant
(type_item name: (type_identifier) @name) @definition.type
";
const GO_EXTRA: &str = "
(const_spec name: (identifier) @name) @definition.constant
";

/// The language of a path by its extension.
pub fn lang_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "go" => "go",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        _ => return None,
    })
}

fn grammar(lang: &str) -> Option<(Language, String)> {
    let js = tree_sitter_javascript::TAGS_QUERY;
    let ts = tree_sitter_typescript::TAGS_QUERY;
    Some(match lang {
        "rust" => (tree_sitter_rust::LANGUAGE.into(), format!("{}\n{RUST_EXTRA}", tree_sitter_rust::TAGS_QUERY)),
        "python" => (tree_sitter_python::LANGUAGE.into(), tree_sitter_python::TAGS_QUERY.to_string()),
        "go" => (tree_sitter_go::LANGUAGE.into(), format!("{}\n{GO_EXTRA}", tree_sitter_go::TAGS_QUERY)),
        "javascript" => (tree_sitter_javascript::LANGUAGE.into(), js.to_string()),
        "typescript" => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), format!("{js}\n{ts}")),
        "tsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), format!("{js}\n{ts}")),
        _ => return None,
    })
}

/// One definition in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    /// 1-based, inclusive.
    pub line: u32,
    pub end_line: u32,
}

/// A parser and the compiled tags query for one language.
pub struct Extractor {
    parser: Parser,
    query: Query,
    name_idx: u32,
}

impl Extractor {
    pub fn new(lang: &str) -> Result<Self> {
        let (language, src) = grammar(lang).with_context(|| format!("no grammar for {lang}"))?;
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let query = Query::new(&language, &src).with_context(|| format!("tags query for {lang}"))?;
        let name_idx = query.capture_index_for_name("name").context("tags query has no @name")?;
        Ok(Self { parser, query, name_idx })
    }

    pub fn extract(&mut self, text: &[u8]) -> Vec<Symbol> {
        let Some(tree) = self.parser.parse(text, None) else { return vec![] };
        let names = self.query.capture_names();
        let mut cursor = QueryCursor::new();
        // One symbol per name node: the earliest pattern wins (the tags queries list the
        // specific patterns, such as a method inside an impl, before the generic ones).
        let mut by_name: HashMap<usize, (usize, Symbol)> = HashMap::new();
        let mut it = cursor.matches(&self.query, tree.root_node(), text);
        while let Some(m) = it.next() {
            let mut name = None;
            let mut def = None;
            for c in m.captures {
                if c.index == self.name_idx {
                    name = Some(c.node);
                } else if let Some(kind) = names[c.index as usize].strip_prefix("definition.") {
                    def = Some((kind, c.node));
                }
            }
            let (Some(n), Some((kind, node))) = (name, def) else { continue };
            let Ok(name) = n.utf8_text(text) else { continue };
            let sym = Symbol {
                name: name.to_string(),
                kind: kind.to_string(),
                line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            };
            match by_name.get(&n.start_byte()) {
                Some((p, _)) if *p <= m.pattern_index => {}
                _ => drop(by_name.insert(n.start_byte(), (m.pattern_index, sym))),
            }
        }
        let mut out: Vec<Symbol> = by_name.into_values().map(|(_, s)| s).collect();
        out.sort_by(|a, b| (a.line, &a.name).cmp(&(b.line, &b.name)));
        out
    }
}

/// Parses each (blob, lang, data) on all cores; one extractor per language per thread.
fn extract_all(items: &[(String, String, Vec<u8>)]) -> Vec<(String, String, Vec<Symbol>)> {
    par_map(items, |(blob, lang, data)| {
        let syms = with_extractor(lang, |ex| ex.extract(data)).unwrap_or_default();
        (blob.clone(), lang.clone(), syms)
    })
}

impl Inner {
    /// Source blobs of `at` without symbol rows, up to `max_bytes`, as (blob, lang, data).
    pub fn symbol_todo(&mut self, at: &str, max_bytes: usize) -> Result<Vec<(String, String, Vec<u8>)>> {
        if self.symbol_done.is_empty() {
            let mut stmt = self.store.conn().prepare("SELECT blob, lang FROM symbol_blobs")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            self.symbol_done.extend(rows);
            self.symbol_done.insert((String::new(), String::new()));
        }
        let m = self.manifest(at)?;
        let mut out = Vec::new();
        let mut size = 0;
        let mut seen = HashSet::new();
        for (path, e) in m.iter() {
            let Some(lang) = lang_of(path) else { continue };
            let key = (e.blob.clone(), lang.to_string());
            if e.mode == MODE_GITLINK || self.symbol_done.contains(&key) || !seen.insert(key) {
                continue;
            }
            let data = self.blob(&e.blob)?;
            size += data.len();
            out.push((e.blob.clone(), lang.to_string(), if data.len() > MAX_FILE { vec![] } else { data }));
            if size >= max_bytes {
                break;
            }
        }
        Ok(out)
    }

    pub fn symbol_store(&mut self, rows: &[(String, String, Vec<Symbol>)]) -> Result<()> {
        self.store.tx(|s| {
            let c = s.conn();
            let mut b =
                c.prepare_cached("INSERT OR REPLACE INTO symbol_blobs (blob, lang, count) VALUES (?1, ?2, ?3)")?;
            let mut d = c.prepare_cached(
                "INSERT INTO symbols (name, blob, lang, kind, line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (blob, lang, syms) in rows {
                c.execute("DELETE FROM symbols WHERE blob = ?1 AND lang = ?2", params![blob, lang])?;
                b.execute(params![blob, lang, syms.len() as i64])?;
                for sy in syms {
                    d.execute(params![sy.name, blob, lang, sy.kind, sy.line, sy.end_line])?;
                }
            }
            Ok(())
        })?;
        self.symbol_done.extend(rows.iter().map(|(b, l, _)| (b.clone(), l.clone())));
        Ok(())
    }

    /// Symbol rows of `at` matching the name filter, with their paths.
    pub fn symbols(&mut self, at: Option<&str>, f: &SymbolQuery, allow: &dyn Fn(&str) -> bool) -> Result<Value> {
        let cs = self.resolve_at(at)?;
        // Any blob still missing (normally none: `prepare_symbols` ran first) is parsed here.
        let todo = self.symbol_todo(&cs, usize::MAX)?;
        if !todo.is_empty() {
            let rows = extract_all(&todo);
            self.symbol_store(&rows)?;
        }
        let m = self.manifest(&cs)?;
        let dir = f.path.as_deref().map(|d| d.trim_matches('/')).unwrap_or("");
        let mut paths: HashMap<(&str, &str), Vec<&str>> = HashMap::new();
        for (p, e) in m.iter() {
            if let Some(l) = lang_of(p)
                && (dir.is_empty() || p == dir || p.starts_with(&format!("{dir}/")))
                && allow(p)
            {
                paths.entry((e.blob.as_str(), l)).or_default().push(p.as_str());
            }
        }
        let (sql, a, b) = match (&f.name, &f.prefix) {
            (Some(n), _) => ("name = ?1 AND ?2 = ?2", n.clone(), String::new()),
            (None, Some(p)) => ("name >= ?1 AND name < ?2", p.clone(), format!("{p}\u{10ffff}")),
            (None, None) => ("?1 = ?1 AND ?2 = ?2", String::new(), String::new()),
        };
        let conn = self.store.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT name, blob, lang, kind, line, end_line FROM symbols WHERE {sql} ORDER BY name, blob, line"
        ))?;
        let rows = stmt.query_map(params![a, b], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u32>(4)?,
                r.get::<_, u32>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        let mut truncated = false;
        for row in rows {
            let (name, blob, lang, kind, line, end) = row?;
            if f.kind.as_ref().is_some_and(|k| *k != kind) {
                continue;
            }
            for p in paths.get(&(blob.as_str(), lang.as_str())).into_iter().flatten() {
                if out.len() == f.max {
                    truncated = true;
                    break;
                }
                out.push(json!({"name": name, "kind": kind, "path": p, "line": line, "end_line": end, "lang": lang}));
            }
        }
        out.sort_by(|x, y| (x["path"].as_str(), x["line"].as_u64()).cmp(&(y["path"].as_str(), y["line"].as_u64())));
        Ok(json!({"at": cs, "symbols": out, "truncated": truncated}))
    }

    /// Every definition in one file, in line order.
    pub fn outline(&mut self, path: &str, at: Option<&str>) -> Result<Value> {
        let cs = self.resolve_at(at)?;
        let e = self.manifest(&cs)?.get(path).cloned().with_context(|| format!("no such file {path}"))?;
        let lang = lang_of(path).with_context(|| format!("no symbol support for {path}"))?;
        let data = self.blob(&e.blob)?;
        let syms = with_extractor(lang, |ex| ex.extract(&data)).context("tree-sitter grammar failed to load")?;
        if !self.symbol_done.contains(&(e.blob.clone(), lang.to_string())) {
            self.symbol_store(&[(e.blob.clone(), lang.to_string(), syms.clone())])?;
        }
        let list: Vec<Value> = syms
            .iter()
            .map(|s| json!({"name": s.name, "kind": s.kind, "line": s.line, "end_line": s.end_line}))
            .collect();
        Ok(json!({"at": cs, "path": path, "lang": lang, "symbols": list}))
    }
}

/// The lines `[start, end)` of the definition of `name` in `text`, by tree-sitter, when the
/// language of `path` is supported and defines it. Intents use it for `path#name`.
pub fn definition_range(path: &str, text: &str, name: &str) -> Option<(usize, usize)> {
    let lang = lang_of(path)?;
    let hit = with_extractor(lang, |ex| ex.extract(text.as_bytes()))?.into_iter().find(|s| s.name == name)?;
    Some((hit.line as usize, hit.end_line as usize + 1))
}

thread_local! {
    static EXTRACTORS: std::cell::RefCell<HashMap<String, Extractor>> = std::cell::RefCell::new(HashMap::new());
}

/// Runs `f` with this thread's extractor for `lang` (compiled once per thread).
fn with_extractor<R>(lang: &str, f: impl FnOnce(&mut Extractor) -> R) -> Option<R> {
    EXTRACTORS.with(|ex| {
        let mut ex = ex.borrow_mut();
        if !ex.contains_key(lang) {
            ex.insert(lang.to_string(), Extractor::new(lang).ok()?);
        }
        ex.get_mut(lang).map(f)
    })
}

/// Filters of the `symbols` operation.
#[derive(Default)]
pub struct SymbolQuery {
    pub name: Option<String>,
    pub prefix: Option<String>,
    pub kind: Option<String>,
    pub path: Option<String>,
    pub max: usize,
}

/// Parses missing blobs of `at` in chunks outside the repo lock. Returns how many it parsed.
pub fn prepare(repo: &Repo, at: &str) -> Result<usize> {
    let mut done = 0;
    loop {
        let todo = repo.with(|r| r.symbol_todo(at, 8 << 20))?;
        if todo.is_empty() {
            return Ok(done);
        }
        done += todo.len();
        let rows = extract_all(&todo);
        repo.with(|r| r.symbol_store(&rows))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(lang: &str, src: &str) -> Vec<(String, String, u32)> {
        let mut ex = Extractor::new(lang).unwrap();
        ex.extract(src.as_bytes()).into_iter().map(|s| (s.name, s.kind, s.line)).collect()
    }

    #[test]
    fn every_language_finds_its_definitions() {
        let rs = names(
            "rust",
            "struct Point { x: i32 }\nimpl Point {\n    fn norm(&self) -> i32 { 0 }\n}\nfn main() {}\nconst MAX: u32 = 3;\ntrait Shape {}\n",
        );
        for want in [
            ("Point", "class", 1),
            ("norm", "method", 3),
            ("main", "function", 5),
            ("MAX", "constant", 6),
            ("Shape", "interface", 7),
        ] {
            assert!(rs.contains(&(want.0.into(), want.1.into(), want.2)), "{want:?} not in {rs:?}");
        }
        let py = names("python", "class Repo:\n    def land(self):\n        pass\n\ndef main():\n    pass\n");
        assert!(
            py.contains(&("Repo".into(), "class".into(), 1)) && py.contains(&("land".into(), "function".into(), 2)),
            "{py:?}"
        );
        let go = names(
            "go",
            "package x\n\ntype Server struct{}\n\nfunc (s *Server) Serve() {}\n\nfunc New() *Server { return nil }\n",
        );
        assert!(go.contains(&("Server".into(), "type".into(), 3)), "{go:?}");
        assert!(
            go.contains(&("Serve".into(), "method".into(), 5)) && go.contains(&("New".into(), "function".into(), 7)),
            "{go:?}"
        );
        let js = names("javascript", "class Queue {\n  push(x) {}\n}\nfunction drain() {}\nconst f = () => 1;\n");
        assert!(
            js.contains(&("Queue".into(), "class".into(), 1)) && js.contains(&("push".into(), "method".into(), 2)),
            "{js:?}"
        );
        assert!(
            js.contains(&("drain".into(), "function".into(), 4)) && js.contains(&("f".into(), "function".into(), 5)),
            "{js:?}"
        );
        let ts = names(
            "typescript",
            "interface Change { id: string }\nexport function land(c: Change): void {}\nclass A { run(): void {} }\n",
        );
        assert!(
            ts.contains(&("Change".into(), "interface".into(), 1))
                && ts.contains(&("land".into(), "function".into(), 2)),
            "{ts:?}"
        );
        assert!(ts.contains(&("run".into(), "method".into(), 3)), "{ts:?}");
        let tsx = names("tsx", "export function App() { return <div/>; }\n");
        assert!(tsx.contains(&("App".into(), "function".into(), 1)), "{tsx:?}");
    }
}
