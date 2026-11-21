# Choice: compare with lower(), not casefold()

In `src/click/types.py`, a `Choice` with `case_sensitive=False` compares values after `str.casefold()`, so `"straße"` is accepted for the choice `"strasse"`. Use `str.lower()` instead. Case-insensitive matching ignores letter case and nothing else: every other character of the value must match the choice exactly. Example: `Choice(["strasse"], case_sensitive=False)` accepts `"STRASSE"` and rejects `"straße"`.

Keep the existing behaviour otherwise. Finish the work completely.
