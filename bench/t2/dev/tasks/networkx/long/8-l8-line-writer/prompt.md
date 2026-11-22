# Refactor: one writer for the line formats and Pajek

`generate_edgelist`, `generate_adjlist`, `generate_multiline_adjlist` and `generate_pajek` (`networkx/readwrite/edgelist.py`, `adjlist.py`, `multiline_adjlist.py` and `pajek.py`) each turn a graph into lines with their own code. Move their bodies into a class `LineWriter` in a new module `networkx/readwrite/_linewriter.py`, and add a paragraph about it to the module docstring of each of the four modules.

- `LineWriter(delimiter=" ")`, kept as the attribute `delimiter`.
- `name(node)` returns the text of a node in a line of the edge list, adjacency list and multiline adjacency list, exactly as their generate functions write it (`str(node)` today).
- `edgelist(G, data=True)`, `adjlist(G)` and `multiline_adjlist(G)` yield the lines that `generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist` yield; every node name they write is `self.name(node)`, so a subclass that overrides `name` changes all three formats.
- `pajek(G)` yields the lines that `generate_pajek` yields. (Pajek quotes names its own way; it does not use `name`.)

The generate functions do nothing but return what the `LineWriter` method gives: `LineWriter(delimiter, ...).adjlist(G)`, `LineWriter(...).pajek(G)`. This holds for every option: whatever parameters the generate functions have now or get later, other than `G`, the `delimiter` and the `data` of the edge list, `LineWriter` takes them as keyword arguments with the same names and defaults. Keep the four generate functions, with their signatures, in their modules, and `write_pajek` writing what `generate_pajek` yields. This is a pure refactor: every function yields exactly the lines it yields today.

Keep the existing behaviour otherwise. Finish the work completely.
