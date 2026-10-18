//! Per-repo settings from `<data>/treeyard.toml`: merge drivers, landing mode, coordination.
//!
//! The file lives in the data directory, not in the repo tree: it names commands the server
//! runs, so only the server owner may change it (an agent's change must never do that).

use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "treeyard.toml";

/// Written by `treeyard init`. Every setting is commented out: the defaults behave like git.
pub const TEMPLATE: &str = r#"# TreeYard repo settings (docs/coordination.md). Changes need a server restart.

# [coordination]
# continuous_rebase = true       # rebase open changes onto main after every land
# keep_conflicted = true         # a conflicted change still rebases; land waits until it is resolved

# Merge drivers run only where the line merge conflicts; first matching rule wins.
# [merge]
# syntax = "auto"                # "mergiraf" when found on PATH (or set mergiraf = "/path")
# [[merge.driver]]
# paths = ["Cargo.lock"]
# driver = "cargo-lock"          # 3-way merge per [[package]]; add command = "..." to regenerate
# [[merge.driver]]
# paths = ["CHANGELOG.md", "HISTORY.md", "**/registry.txt"]
# driver = "union"
# [[merge.driver]]
# paths = ["package-lock.json", "pnpm-lock.yaml", "poetry.lock", "uv.lock"]
# driver = "regenerate"
# command = "pnpm install --lockfile-only --offline"   # runs in a temp copy of the merged tree

# [landing]
# mode = "direct"                # "queue" (batch tested before main moves) or "optimistic"
# test = "cargo test --offline"  # runs in a temp copy of the tree; exit 0 = pass
# batch = 8
# timeout_s = 600

# [bisect]                       # commands the bisect operation may run (agents pick by name)
# test = "cargo test --offline"  # default; falls back to landing.test
# timeout_s = 600
# [bisect.commands]
# unit = "cargo test --offline --lib"

# [index]                        # derived data: "eager" (background), "lazy" (first query), "off"
# search = "lazy"                # trigram index for grep
# symbols = "lazy"               # tree-sitter symbol index (Rust, Python, Go, JS, TS)
"#;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub coordination: Coordination,
    pub merge: MergeConfig,
    pub landing: Landing,
    pub bisect: Bisect,
    pub index: IndexConfig,
}

/// `[bisect]`: the commands the `bisect` operation may run (agents pick one by name; they can
/// never send a command of their own).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bisect {
    /// Default command; falls back to `landing.test`.
    pub test: Option<String>,
    pub timeout_s: Option<u64>,
    /// Named commands, e.g. `unit = "cargo test --offline -p core"`.
    pub commands: std::collections::BTreeMap<String, String>,
}

/// `[index]`: when derived data is computed. `eager` = in the background after main moves,
/// `lazy` = on the first query that needs it, `off` = never (grep scans every file).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    pub search: IndexMode,
    pub symbols: IndexMode,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self { search: IndexMode::Lazy, symbols: IndexMode::Lazy }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexMode {
    Eager,
    #[default]
    Lazy,
    Off,
}

impl Bisect {
    /// The command for `name` (None = the default), with the landing test as the last resort.
    pub fn command(&self, name: Option<&str>, landing: &Landing) -> Result<String> {
        match name {
            Some(n) => self.commands.get(n).cloned().with_context(|| {
                let known: Vec<&String> = self.commands.keys().collect();
                format!("no bisect command {n:?} in treeyard.toml (known: {known:?})")
            }),
            None => self
                .test
                .clone()
                .or_else(|| landing.test.clone())
                .context("set [bisect] test (or landing.test) in treeyard.toml"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Coordination {
    pub continuous_rebase: bool,
    /// A conflicted change keeps rebasing (each conflict is merged again from its own base and
    /// sides onto the new main); only `land` waits until every conflict is resolved.
    pub keep_conflicted: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    /// `off` (default), `auto` (mergiraf when installed) or `mergiraf` (must be installed).
    pub syntax: Option<String>,
    /// Path of the mergiraf binary; default: `mergiraf` on PATH.
    pub mergiraf: Option<String>,
    #[serde(rename = "driver")]
    pub drivers: Vec<DriverRule>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DriverRule {
    /// Globs (gitignore style without negation): `Cargo.lock` matches at any depth.
    pub paths: Vec<String>,
    /// `union`, `cargo-lock`, `regenerate`, `ours` or `theirs`.
    pub driver: String,
    /// For `regenerate` (and optionally `cargo-lock`): a shell command run in a temp copy of the
    /// merged tree; afterwards the server reads the path back.
    pub command: Option<String>,
    pub timeout_s: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Direct,
    Queue,
    Optimistic,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Landing {
    pub mode: Mode,
    pub test: Option<String>,
    pub batch: usize,
    pub timeout_s: u64,
}

impl Default for Landing {
    fn default() -> Self {
        Self { mode: Mode::Direct, test: None, batch: 8, timeout_s: 600 }
    }
}

/// A glob set that matches a path at any depth when the glob has no `/` (like gitignore).
pub fn globs(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let p = p.trim().trim_start_matches("./");
        if p.is_empty() {
            continue;
        }
        let full = if p.contains('/') { p.trim_start_matches('/').to_string() } else { format!("**/{p}") };
        let glob = |g: &str| GlobBuilder::new(g).literal_separator(true).build();
        b.add(glob(full.trim_end_matches('/')).with_context(|| format!("bad glob {p:?}"))?);
        if p.ends_with('/') || !p.contains(['*', '?', '[']) {
            // A directory name also covers everything below it.
            b.add(glob(&format!("{}/**", full.trim_end_matches('/')))?);
        }
    }
    Ok(b.build()?)
}

pub struct Drivers {
    rules: Vec<(GlobSet, DriverRule)>,
}

impl Drivers {
    pub fn rule(&self, path: &str) -> Option<&DriverRule> {
        self.rules.iter().find(|(g, _)| g.is_match(path)).map(|(_, r)| r)
    }
}

impl Config {
    pub fn load(data: &Path) -> Result<Self> {
        let path = data.join(FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let cfg: Config = toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        cfg.drivers()?;
        if let Some(s) = &cfg.merge.syntax
            && !["off", "auto", "mergiraf"].contains(&s.as_str())
        {
            bail!("{}: merge.syntax must be off, auto or mergiraf, not {s:?}", path.display());
        }
        if cfg.landing.mode != Mode::Direct && cfg.landing.test.is_none() {
            bail!("{}: landing.mode {:?} needs landing.test", path.display(), cfg.landing.mode);
        }
        Ok(cfg)
    }

    pub fn drivers(&self) -> Result<Drivers> {
        let mut rules = Vec::new();
        for r in &self.merge.drivers {
            if !["union", "cargo-lock", "regenerate", "ours", "theirs"].contains(&r.driver.as_str()) {
                bail!("unknown merge driver {:?}: use union, cargo-lock, regenerate, ours or theirs", r.driver);
            }
            if r.driver == "regenerate" && r.command.is_none() {
                bail!("merge driver regenerate for {:?} needs a command", r.paths);
            }
            rules.push((globs(&r.paths)?, r.clone()));
        }
        Ok(Drivers { rules })
    }

    /// The mergiraf binary to use, if syntax-aware merge is on and the binary runs.
    pub fn mergiraf(&self) -> Result<Option<String>> {
        let mode = self.merge.syntax.as_deref().unwrap_or("off");
        if mode == "off" {
            return Ok(None);
        }
        let bin = self.merge.mergiraf.clone().unwrap_or_else(|| "mergiraf".into());
        let ok = std::process::Command::new(&bin).arg("--version").output().is_ok_and(|o| o.status.success());
        match (ok, mode) {
            (true, _) => Ok(Some(bin)),
            (false, "mergiraf") => bail!("merge.syntax = \"mergiraf\" but {bin} does not run"),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults_and_globs_match_at_any_depth() {
        let cfg: Config = toml::from_str(TEMPLATE).unwrap();
        assert!(!cfg.coordination.continuous_rebase && cfg.merge.drivers.is_empty());
        let g = globs(&["Cargo.lock".into(), "docs/".into(), "src/*.rs".into()]).unwrap();
        assert!(g.is_match("Cargo.lock") && g.is_match("crates/x/Cargo.lock"));
        assert!(g.is_match("docs/a/b.md") && g.is_match("src/main.rs"));
        assert!(!g.is_match("src/a/b.rs") && !g.is_match("Cargo.toml"));
    }

    #[test]
    fn unknown_driver_is_rejected() {
        let cfg: Config = toml::from_str("[[merge.driver]]\npaths = [\"x\"]\ndriver = \"magic\"\n").unwrap();
        assert!(cfg.drivers().is_err());
    }
}
