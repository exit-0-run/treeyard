# globset: GlobSet::first_match

In `crates/globset/src/lib.rs`, add a method `GlobSet::first_match(path) -> Option<usize>` that returns the sequence number of the first glob (in the order they were added) that matches the path, or `None` when no glob matches.

Keep the existing behaviour otherwise. Finish the work completely.
