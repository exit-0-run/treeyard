use ignore::types::TypesBuilder;

#[test]
fn roc_file_type() {
    let mut builder = TypesBuilder::new();
    builder.add_defaults();
    builder.select("roc");
    let types = builder.build().unwrap();
    assert!(types.matched("main.roc", false).is_whitelist());
    assert!(!types.matched("main.rs", false).is_whitelist());
}
