# status_codes: reason(code)

In `src/requests/status_codes.py`, add a function `reason(code)` that returns the first name of an HTTP status code, or `None` for a code that is not known. Examples: `reason(404)` is `"not_found"`, `reason(200)` is `"ok"`, `reason(999)` is `None`.

Keep the existing behaviour otherwise. Finish the work completely.
