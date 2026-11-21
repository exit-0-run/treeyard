use grep_printer::Stats;

#[test]
fn searches_without_match() {
    let mut stats = Stats::new();
    stats.add_searches(5);
    stats.add_searches_with_match(2);
    assert_eq!(stats.searches_without_match(), 3);
    assert_eq!(Stats::new().searches_without_match(), 0);
}
