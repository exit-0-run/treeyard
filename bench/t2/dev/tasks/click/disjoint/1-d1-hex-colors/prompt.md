# style: hex colours

In `src/click/termui.py`, make `style` accept a hex colour string `#rrggbb` (hex digits in either case) for `fg` and `bg`, rendered as a 24-bit colour, the same as an RGB tuple. Example: `click.style("x", fg="#ff8800")` is `"\x1b[38;2;255;136;0mx\x1b[0m"`, and `bg="#0000FF"` gives `\x1b[48;2;0;0;255m`. Other strings that are not colour names stay an error.

Keep the existing behaviour otherwise. Finish the work completely.
