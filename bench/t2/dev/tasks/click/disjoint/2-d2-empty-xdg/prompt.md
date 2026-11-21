# get_app_dir: empty XDG_CONFIG_HOME

In `src/click/utils.py`, `get_app_dir` on Linux and other Unix systems uses `XDG_CONFIG_HOME` even when it is set to an empty string, and then returns a relative path. Treat an empty `XDG_CONFIG_HOME` as unset, as the XDG Base Directory spec says: `get_app_dir("Foo Bar")` then returns `~/.config/foo-bar` with `~` expanded.

Keep the existing behaviour otherwise. Finish the work completely.
