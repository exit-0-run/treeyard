# parse_human_readable_size: space before the suffix

In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept spaces between the number and the suffix. Example: `"2 M"` is 2097152, the same as `"2M"`.

Keep the existing behaviour otherwise. Finish the work completely.
