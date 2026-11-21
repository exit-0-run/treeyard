# to_native_string: accept memoryview

In `src/requests/_internal_utils.py`, make `to_native_string` accept a `memoryview` of bytes too: `to_native_string(memoryview(b"abc"))` returns `"abc"`.

Keep the existing behaviour otherwise. Finish the work completely.
