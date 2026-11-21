# Choice: refuse a value that matches two choices

In `src/click/types.py`, `Choice.convert` returns the first choice when a value matches more than one choice (possible with `case_sensitive=False` and the choices `"a"` and `"A"`). Make it fail instead, with the message `'a' matches more than one choice: 'a', 'A'.` A value is valid only when it matches exactly one choice, as a whole.

Keep the existing behaviour otherwise. Finish the work completely.
