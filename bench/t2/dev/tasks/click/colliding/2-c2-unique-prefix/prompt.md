# Choice: accept a unique prefix

In `src/click/types.py`, make `Choice.convert` accept an abbreviation: when the value matches no choice, but it has at least 3 characters and exactly one choice starts with it, return that choice. Example: `Choice(["production", "staging"]).convert("prod", None, None)` returns `"production"`. A value of 1 or 2 characters, or one that starts more than one choice, stays an error.

Keep the existing behaviour otherwise. Finish the work completely.
