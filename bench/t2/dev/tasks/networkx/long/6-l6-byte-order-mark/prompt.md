# Line formats: byte order marks

The readers of the edge list, adjacency list and multiline adjacency list formats (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) take a UTF-8 byte order mark at the start of a file as part of the first node name, and cannot read a UTF-16 file whose encoding the caller does not know. Handle byte order marks.

- `read_edgelist`, `read_weighted_edgelist`, `read_adjlist` and `read_multiline_adjlist`: when the encoding is UTF-8 (in any spelling, such as the default `"utf-8"`), a UTF-8 byte order mark at the start of the file is skipped.
- The same functions accept `encoding=None`: the encoding is then taken from the byte order mark at the start of the file (UTF-8, UTF-16 LE or UTF-16 BE), which is skipped, and is UTF-8 when the file has none.
- `write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and `write_multiline_adjlist` take `bom=False`. With `bom=True` they write the byte order mark of their encoding before anything else, once: for UTF-8, UTF-16 LE and UTF-16 BE (in any spelling); for another encoding they raise `ValueError` before writing anything.

Document the parameters and the reading rule in the module docstring of each of the three modules.

Keep the existing behaviour otherwise. Finish the work completely.
