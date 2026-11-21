# parse_human_readable_size: lower-case suffixes

In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffixes `k`, `m` and `g` in lower case, with the same meaning as `K`, `M` and `G`. Example: `"2k"` is 2048.

Keep the existing behaviour otherwise. Finish the work completely.
