# Data of the long set

- `prompts/<id>.md`: the prompt of each task (`# title`, a blank line, the text).
- `patches/<k>-<id>.alone.patch`: the task alone on the pin (its `reference.patch`).
- `patches/<k>-<id>.step.patch`: step k of the reference order, on the tree of tasks 1..k-1, where it
  differs from the task alone (the features after the first meet earlier features; the refactors move the
  combined code).
- `patches/without-<k>.patch`: the tree of all tasks but k, on the pin (for the integration checks of
  `validate`).
- `tests/`, `integration/`: the hidden tests (one per task) and the integration tests (one per pair named in
  `taskdefs_long.py`).

How they were made, outside the kit: each feature was written on the pin; the steps of the features were
made by merging them in order with a token-level three-way merge, the few blocks it could not merge fixed by
hand; the three refactors were made by one generator applied to whatever features a tree holds (the
reference, and each tree without one task). Each patch carries enough context (5 lines, more where a hunk
would match twice) that its edits apply at exactly one place.
