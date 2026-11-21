# globset: Glob::has_wildcards

In `crates/globset/src/glob.rs`, add a method `Glob::has_wildcards() -> bool` that returns true when the pattern has a wildcard (`*`, `?`, `**`), a character class (`[...]`) or an alternation (`{a,b}`), and false when it can match only one literal path.

Keep the existing behaviour otherwise. Finish the work completely.
