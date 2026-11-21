# Choice: ignore spaces around the value without case

In `src/click/types.py`, make a `Choice` with `case_sensitive=False` also ignore whitespace around the value, so that a pasted value with a stray space still matches. Example: `Choice(["red", "green"], case_sensitive=False).convert("  Red ", None, None)` returns `"red"`. A `Choice` with `case_sensitive=True` keeps rejecting `" red"`.

Keep the existing behaviour otherwise. Finish the work completely.
