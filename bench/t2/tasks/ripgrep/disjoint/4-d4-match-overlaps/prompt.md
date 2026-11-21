# matcher: Match::overlaps

In `crates/matcher/src/lib.rs`, add a method `Match::overlaps(&self, other: &Match) -> bool` that returns true when the two matches share at least one byte offset. An empty match overlaps nothing. Example: 0..5 and 4..8 overlap; 0..4 and 4..8 do not.

Keep the existing behaviour otherwise. Finish the work completely.
