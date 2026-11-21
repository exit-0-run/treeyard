use grep_cli::parse_human_readable_size;

#[test]
fn terabytes() {
    assert_eq!(parse_human_readable_size("2T").unwrap(), 2u64 << 40);
    assert!(parse_human_readable_size("99999999T").is_err());
}
