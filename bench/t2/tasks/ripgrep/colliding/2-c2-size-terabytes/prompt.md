# parse_human_readable_size: T suffix

In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffix `T` (terabytes, 2^40 bytes). Example: `"2T"` is 2199023255552. An overflow stays an error.

Keep the existing behaviour otherwise. Finish the work completely.
