//! examples/swarm-demo.sh runs end to end: both sides keep every edit, and only TreeYard warns
//! about the same-line conflict before land.

use std::process::Command;

#[test]
fn swarm_demo_runs_and_keeps_every_edit() {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("sh")
        .arg(format!("{root}/examples/swarm-demo.sh"))
        .args(["--agents", "4", "--pairs", "1", "--files", "50", "--bin", env!("CARGO_BIN_EXE_treeyard")])
        .env_remove("TREEYARD_TOKEN")
        .output()
        .expect("run swarm-demo.sh");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "swarm-demo failed:\n{text}\n{}", String::from_utf8_lossy(&out.stderr));
    let row = |name: &str| text.lines().find(|l| l.starts_with(&format!("| {name}"))).unwrap_or_default().to_string();
    let cells = |name: &str| -> Vec<String> { row(name).split('|').map(|c| c.trim().to_string()).collect() };
    assert_eq!(cells("Edits missing from main")[2..4], ["0 of 4", "0 of 4"], "{text}");
    assert_eq!(cells("Same-line conflicts warned before land")[2..4], ["0 of 1", "1 of 1"], "{text}");
    assert_eq!(cells("Conflicts at land")[2..4], ["1", "1"], "{text}");
}
