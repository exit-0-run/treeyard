use globset::Glob;

#[test]
fn has_wildcards() {
    assert!(Glob::new("src/*.rs").unwrap().has_wildcards());
    assert!(Glob::new("{a,b}").unwrap().has_wildcards());
    assert!(Glob::new("[ab].txt").unwrap().has_wildcards());
    assert!(Glob::new("**/x").unwrap().has_wildcards());
    assert!(!Glob::new("src/main.rs").unwrap().has_wildcards());
}
