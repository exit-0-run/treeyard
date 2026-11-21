# Styles: terminal hyperlinks

Add a keyword parameter `link: str | None = None` to `click.style` (documented in its docstring): when it is set, the text becomes an OSC 8 terminal hyperlink to that URL, written as `ESC ] 8 ; ; URL ESC \` before the text and `ESC ] 8 ; ; ESC \` after it, inside the other styles. Example: `click.style("docs", link="https://example.com")` is `"\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\\x1b[0m"`. Click must treat these sequences like the other style codes: `click.unstyle` and `echo` to a stream that is not a terminal remove them, and they take no columns when help output is measured and wrapped. That is the job of `strip_ansi` and the pattern behind it in `src/click/_compat.py`, which today knows only the CSI codes (`ESC [ ... m`); make it also remove OSC sequences ended by `ESC \` or by BEL (`\a`). `click.secho` passes the parameter on as it does the others.

Keep the existing behaviour otherwise. Finish the work completely.
