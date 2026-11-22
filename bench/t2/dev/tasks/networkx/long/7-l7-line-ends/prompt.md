# Line formats: line ends

The readers of the edge list, adjacency list and multiline adjacency list formats (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) remove only `\n` from a line, so with a delimiter such as `,` a Windows file leaves `\r` in the last name, and a file with old Mac line ends (`\r` only) is read as one line. The writers always write `\n`. Handle line ends.

- Reading: in the `parse_*` and `read_*` functions a line may end with `\n`, `\r\n` or `\r`. Whatever a reader does with a line (comments, names, data, degrees), it does with the text without its line end, so `parse_adjlist(["a,b\r\n"], delimiter=",")` has the nodes `a` and `b`. The `read_*` functions split the file at every `\r\n`, `\r` and `\n`, so a file with `\r` line ends is read line by line.
- Writing: `write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and `write_multiline_adjlist` take `newline="\n"` and end every line they write with it, comment lines included: `"\r\n"` for Windows files, `"\r"` for old Mac files. Any other value raises `ValueError` before anything is written.

Document the parameter in each writer and the line-end rules in the module docstring of each of the three modules.

Keep the existing behaviour otherwise. Finish the work completely.
