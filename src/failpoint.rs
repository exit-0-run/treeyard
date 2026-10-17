//! Crash injection for the recovery tests. `TREEYARD_FAILPOINT=<name>[,<name>...]` makes the
//! process abort (no unwinding, no flush, like `kill -9`) when it reaches a named point. The
//! points sit between the steps of a land and of an object write; see tests/hardening.rs.

use std::sync::OnceLock;

/// Every failpoint name, in the order a land reaches them.
pub const POINTS: &[&str] =
    &["land:before-objects", "odb:after-tmp", "land:after-objects", "land:in-tx", "land:after-db", "land:after-ref"];

fn armed() -> &'static [String] {
    static ARMED: OnceLock<Vec<String>> = OnceLock::new();
    ARMED.get_or_init(|| {
        std::env::var("TREEYARD_FAILPOINT")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    })
}

/// Aborts the process if `name` is armed. Costs one empty-slice check when nothing is armed.
pub fn hit(name: &str) {
    let a = armed();
    if !a.is_empty() && a.iter().any(|p| p == name) {
        eprintln!("treeyard: failpoint {name} reached, aborting");
        std::process::abort();
    }
}
