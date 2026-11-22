# Refactor: the bytes of the line formats in one class

The `read_*` and `write_*` functions of the edge list, adjacency list and multiline adjacency list formats (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) each decode or encode lines with their own code. Move that into a class `TextFile` in a new module `networkx/readwrite/_textfile.py`, and add a paragraph about it to the module docstring of each of the three modules.

- `TextFile(encoding="utf-8")`, kept as the attribute `encoding`.
- `read_lines(f)` returns the lines of the open file `f` as text, each with its line end, exactly as the `read_*` functions read them (`["a b\n", "c d\n"]`).
- `write_lines(f, lines)` writes each of `lines` (text without its line end) to the binary file `f`, followed by a line end (`\n` today), as the `write_*` functions write them.
- `read_edgelist`, `read_weighted_edgelist`, `read_adjlist` and `read_multiline_adjlist` read every line only through `TextFile(...).read_lines(path)`, with their `encoding`. `write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and `write_multiline_adjlist` write every line, the comment lines of the adjacency list writers included, only through one call of `TextFile(...).write_lines(path, lines)`.

This holds for every option about how the text is stored as bytes: `TextFile` takes the `encoding` today, and any later option of the `read_*` or `write_*` functions of that kind (such as a byte order mark or a line end) is a keyword argument of `TextFile` with the same name and default, used by `read_lines` or `write_lines`. Keep all eight `read_*` and `write_*` functions, with their signatures, in their modules. This is a pure refactor: every file is read and written exactly as today.

Keep the existing behaviour otherwise. Finish the work completely.
