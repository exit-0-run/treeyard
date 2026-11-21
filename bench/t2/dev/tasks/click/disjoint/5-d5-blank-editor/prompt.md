# Editor: skip a blank VISUAL or EDITOR

In `src/click/_termui_impl.py`, `Editor.get_editor` returns the value of `VISUAL` or `EDITOR` even when it holds only whitespace, and starting it then fails. Skip a value that is empty after stripping whitespace and go on with the next variable. Example: with `VISUAL="  "` and `EDITOR="nano"`, `Editor().get_editor()` returns `"nano"`.

Keep the existing behaviour otherwise. Finish the work completely.
