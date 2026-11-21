use grep_matcher::Match;

#[test]
fn match_overlaps() {
    assert!(Match::new(0, 5).overlaps(&Match::new(4, 8)));
    assert!(!Match::new(0, 4).overlaps(&Match::new(4, 8)));
    assert!(!Match::zero(2).overlaps(&Match::new(0, 5)));
}
