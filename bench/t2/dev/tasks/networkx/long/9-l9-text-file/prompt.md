# Refactor: the files of the text formats in one class

The `read_*` and `write_*` functions of `networkx/readwrite` open their file with the decorator `networkx.utils.open_file`, and those of the edge list, adjacency list, multiline adjacency list, Pajek and LEDA formats each decode or encode lines with their own code. Move that into a class `TextFile` in a new module `networkx/readwrite/_textfile.py`, and add a paragraph about it to the module docstring of each module that uses it.

- `TextFile(encoding="utf-8")`, kept as the attribute `encoding`.
- `open(path, mode)` is a context manager that gives the open binary file, as `open_file` opens it: a file name or `pathlib.Path` is opened in `mode` (`"rb"` or `"wb"`; through the compression its ending names, as `open_file` does) and closed when the block ends; any other object (an open file, an iterable of lines) is given as it is and not closed.
- `read_lines(f)` returns the lines of the open file `f` as text, each with its line end, exactly as the `read_*` functions read them (`["a b\n", "c d\n"]`); lines that are already text pass as they are.
- `write_lines(f, lines)` writes each of `lines` (text without its line end) to the open binary file `f`, followed by a line end (`\n` today), as the `write_*` functions write them.
- `read(path, parse, *args, **kwargs)` opens `path` with `self.open(path, "rb")` and returns `parse(self.read_lines(f), *args, **kwargs)`, doing whatever the `read_*` functions do around their parse call; `write(path, lines)` opens `path` with `self.open(path, "wb")` and calls `self.write_lines(f, lines)` once.
- `read_edgelist`, `read_adjlist`, `read_multiline_adjlist`, `read_pajek` and `read_leda` do nothing but `return TextFile(encoding).read(path, parse_x, <their other arguments>)` (`read_weighted_edgelist` calls `read_edgelist`). `write_edgelist`, `write_adjlist`, `write_multiline_adjlist` and `write_pajek` write every line, the comment lines of the adjacency list writers included, only through one call of `TextFile(...).write(path, lines)` (`write_weighted_edgelist` calls `write_edgelist`).
- `read_gml`, `write_gml`, `read_graph6`, `write_graph6`, `read_sparse6`, `write_sparse6`, `read_graphml`, `write_graphml_xml`, `write_graphml_lxml`, `read_gexf` and `write_gexf` open their file with `TextFile().open(path, mode)` instead of the decorator. No function in these modules uses `open_file` any more.

This holds for every option about how the text is stored as bytes: any parameter of the `read_*` or `write_*` functions of the five line formats named `encoding`, `bom`, `newline` or `compression`, now or later, is a keyword argument of `TextFile` with the same name and default, used by `read_lines` or `write_lines`. Keep all these functions, with their signatures, in their modules. This is a pure refactor: every file is read and written exactly as today.

Keep the existing behaviour otherwise. Finish the work completely.
