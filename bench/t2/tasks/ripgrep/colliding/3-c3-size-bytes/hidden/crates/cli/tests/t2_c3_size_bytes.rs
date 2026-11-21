use grep_cli::parse_human_readable_size;

#[test]
fn bytes_suffix() {
    assert_eq!(parse_human_readable_size("100B").unwrap(), 100);
}
