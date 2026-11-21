# parse_human_readable_size: KiB, MiB and GiB

In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffixes `KiB`, `MiB` and `GiB`, with the same meaning as `K`, `M` and `G`. Example: `"2KiB"` is 2048.

Keep the existing behaviour otherwise. Finish the work completely.
