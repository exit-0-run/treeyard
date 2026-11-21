use globset::{Glob, GlobSetBuilder};

#[test]
fn first_match() {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new("*.rs").unwrap());
    builder.add(Glob::new("src/**").unwrap());
    let set = builder.build().unwrap();
    assert_eq!(set.first_match("src/lib.rs"), Some(0));
    assert_eq!(set.first_match("src/a.txt"), Some(1));
    assert_eq!(set.first_match("x.txt"), None);
}
