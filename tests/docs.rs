//! Docs that must follow the code: the API reference is generated from the operation table,
//! and every operation of the table is named in the open RPC spec.
//! `ROOTSTOCK_BLESS=1 cargo test --test docs` rewrites docs/api.md.

use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn api_reference_matches_the_operation_table() {
    let want = rootstock::apidoc::markdown(&rootstock::rpc::tools());
    let path = root().join("docs/api.md");
    if std::env::var_os("ROOTSTOCK_BLESS").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(have == want, "docs/api.md is stale: run `cargo run -q -- api-doc > docs/api.md` (or ROOTSTOCK_BLESS=1)");
}

#[test]
fn rpc_spec_names_every_operation() {
    let spec = std::fs::read_to_string(root().join("docs/spec/rpc.md")).unwrap();
    for t in rootstock::rpc::tools() {
        let name = t["name"].as_str().unwrap();
        assert!(spec.contains(&format!("`{name}`")), "docs/spec/rpc.md does not name operation {name}");
    }
}

#[test]
fn relative_links_in_docs_resolve() {
    let mut files = vec![root().join("README.md"), root().join("CHANGELOG.md"), root().join("CONTRIBUTING.md")];
    let mut dirs = vec![root().join("docs")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "md") {
                files.push(p);
            }
        }
    }
    let link = regex::Regex::new(r"\]\(([^)#\s]+)(#[^)]*)?\)").unwrap();
    let mut bad = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for c in link.captures_iter(&text) {
            let target = &c[1];
            if target.contains("://") || target.starts_with("mailto:") {
                continue;
            }
            if !f.parent().unwrap().join(target).exists() {
                bad.push(format!("{} -> {target}", f.strip_prefix(root()).unwrap().display()));
            }
        }
    }
    assert!(bad.is_empty(), "broken relative links:\n{}", bad.join("\n"));
}

#[test]
fn demo_svg_is_generated_from_the_transcript() {
    let out = std::process::Command::new("python3")
        .args(["-I", "scripts/demo-svg.py", "assets/demo.txt"])
        .current_dir(root())
        .output()
        .expect("python3 runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let have = std::fs::read(root().join("assets/demo.svg")).unwrap();
    assert!(
        have == out.stdout,
        "assets/demo.svg is stale: python3 scripts/demo-svg.py assets/demo.txt > assets/demo.svg"
    );
}

#[test]
fn readme_and_site_show_the_recorded_swarm_demo() {
    let demo = std::fs::read_to_string(root().join("assets/demo.txt")).unwrap();
    let table: Vec<&str> = demo.lines().filter(|l| l.starts_with('|')).collect();
    assert!(table.len() > 5, "assets/demo.txt holds the swarm-demo table");
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    for line in &table {
        assert!(readme.contains(line), "README.md lacks the recorded demo line {line:?}");
    }
    let site = std::fs::read_to_string(root().join("site/index.html")).unwrap();
    assert!(
        !site.contains("<script src=") && !site.contains("stylesheet\" href=\"http"),
        "the site loads nothing remote"
    );
}
