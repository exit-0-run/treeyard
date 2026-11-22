# Text formats: byte order marks and line ends

The readers of the edge list, adjacency list, multiline adjacency list, Pajek and LEDA formats (`networkx/readwrite/edgelist.py`, `adjlist.py`, `multiline_adjlist.py`, `pajek.py` and `leda.py`) take a UTF-8 byte order mark at the start of a file as part of the first line, cannot read a UTF-16 file whose encoding the caller does not know, and remove only `\n` from a line, so a Windows file leaves `\r` in the last token and a file with old Mac line ends (`\r` only) is read as one line. The writers always write `\n` and never a byte order mark. Handle both.

Byte order marks:

- `read_edgelist`, `read_weighted_edgelist`, `read_adjlist`, `read_multiline_adjlist`, `read_pajek` and `read_leda`: when the encoding is UTF-8 (in any spelling, such as the default `"utf-8"` or `"UTF-8"`), a UTF-8 byte order mark at the start of the file is skipped.
- The same functions accept `encoding=None`: the encoding is then taken from the byte order mark at the start of the file (UTF-8, UTF-16 LE or UTF-16 BE), which is skipped, and is UTF-8 when the file has none.
- `write_edgelist`, `write_weighted_edgelist`, `write_adjlist`, `write_multiline_adjlist` and `write_pajek` take `bom=False`. With `bom=True` they write the byte order mark of their encoding before anything else, once: for UTF-8, UTF-16 LE and UTF-16 BE (in any spelling); for another encoding they raise `ValueError` before writing anything.

Line ends:

- Reading: in the `parse_*` and `read_*` functions of the five formats a line may end with `\n`, `\r\n` or `\r`. Whatever a reader does with a line (comments, names, data, degrees, Pajek and LEDA fields), it does with the text without its line end, so `parse_adjlist(["a,b\r\n"], delimiter=",")` has the nodes `a` and `b`. The `read_*` functions split the file at every `\r\n`, `\r` and `\n`, so a file with `\r` line ends is read line by line.
- Writing: the five writers take `newline="\n"` and end every line they write with it, comment lines included: `"\r\n"` for Windows files, `"\r"` for old Mac files. Any other value raises `ValueError` before anything is written (also before a byte order mark).

Document the parameters in each function and the rules in the module docstring of each of the five modules.
