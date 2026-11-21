use grep_cli::parse_human_readable_size;

#[test]
fn space_before_suffix() {
    assert_eq!(parse_human_readable_size("2 M").unwrap(), 2 << 20);
    assert_eq!(parse_human_readable_size("7  K").unwrap(), 7 << 10);
}
