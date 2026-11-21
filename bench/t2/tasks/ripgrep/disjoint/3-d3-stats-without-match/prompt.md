# printer: Stats::searches_without_match

In `crates/printer/src/stats.rs`, add a method `Stats::searches_without_match() -> u64` that returns the number of searches that found no match (all searches minus the searches with a match).

Keep the existing behaviour otherwise. Finish the work completely.
