use grep_cli::parse_human_readable_size;

#[test]
fn lower_case_suffixes() {
    assert_eq!(parse_human_readable_size("2k").unwrap(), 2 << 10);
    assert_eq!(parse_human_readable_size("3m").unwrap(), 3 << 20);
    assert_eq!(parse_human_readable_size("4g").unwrap(), 4 << 30);
}
