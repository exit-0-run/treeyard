# parse_human_readable_size: B suffix

In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffix `B` for bytes. Example: `"100B"` is 100, the same as `"100"`.

Keep the existing behaviour otherwise. Finish the work completely.
