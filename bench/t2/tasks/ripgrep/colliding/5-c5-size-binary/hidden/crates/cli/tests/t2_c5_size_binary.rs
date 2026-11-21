use grep_cli::parse_human_readable_size;

#[test]
fn binary_suffixes() {
    assert_eq!(parse_human_readable_size("2KiB").unwrap(), 2 << 10);
    assert_eq!(parse_human_readable_size("1MiB").unwrap(), 1 << 20);
    assert_eq!(parse_human_readable_size("1GiB").unwrap(), 1 << 30);
}
