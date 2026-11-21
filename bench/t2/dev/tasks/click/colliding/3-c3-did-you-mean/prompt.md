# Choice: suggest the closest choice

In `src/click/types.py`, when `Choice.convert` rejects a value, add a hint with the closest choice to the error message, found with `difflib.get_close_matches(..., n=1, cutoff=0.8)` among the normalized choices. Example: `Choice(["red", "green"]).convert("gren", None, None)` fails with the message `'gren' is not one of 'red', 'green'. Did you mean 'green'?`. When no choice is close, the message stays as it is.

Keep the existing behaviour otherwise. Finish the work completely.
