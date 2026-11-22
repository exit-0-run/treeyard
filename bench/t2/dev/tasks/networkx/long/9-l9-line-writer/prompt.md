# Refactor: one writer for the line formats

`generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist` (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) each turn nodes into text with their own code. Move their bodies into a class `LineWriter` in a new module `networkx/readwrite/_linewriter.py`, and add a paragraph about it to the module docstring of each of the three modules.

- `LineWriter(delimiter=" ")`, kept as the attribute `delimiter`.
- `name(node)` returns the text of a node in a line, exactly as the generate functions write it (`str(node)` today).
- `edgelist(G, data=True)`, `adjlist(G)` and `multiline_adjlist(G)` yield the lines that `generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist` yield; every node name they write is `self.name(node)`, so a subclass that overrides `name` changes all three formats.

The generate functions do nothing but return what the `LineWriter` method gives, `LineWriter(delimiter, ...).adjlist(G)`. This holds for every option: whatever parameters the generate functions have now or get later, other than `G` and the `data` of the edge list, `LineWriter` takes them as keyword arguments with the same names and defaults. Keep `generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist`, with their signatures, in their modules. This is a pure refactor: every function yields exactly the lines it yields today.

Keep the existing behaviour otherwise. Finish the work completely.
