# Line formats: sorted output

The edge list, adjacency list and multiline adjacency list writers (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) write nodes and edges in the order in which they were added, so two equal graphs can give different files. Add a `sort` option.

- `generate_edgelist`, `generate_adjlist`, `generate_multiline_adjlist` and the writers `write_edgelist`, `write_weighted_edgelist`, `write_adjlist`, `write_multiline_adjlist` take `sort=False`. With `sort=True` the lines do not depend on the order of insertion:
  - nodes come in the order of `str(node)`, and within a line or block the neighbours too; the edge list is ordered by the names of the two ends;
  - in an undirected graph each edge is written once, from the end whose `str` sorts first: the edge list writes `a b`, never `b a`, and an adjacency list lists the edge on the line (or in the block) of that end;
  - parallel edges of a multigraph come in the order of `str(key)`;
  - nodes or keys with the same `str` keep the order of the graph.
- For example the undirected edges `(c, a), (b, a), (c, b), (a, a)` give the edge list `a a`, `a b`, `a c`, `b c` and the adjacency list `a a b c`, `b c`, `c`. For a directed graph with edges `(3, 1), (1, 3), (2, 1), (10, 2)` the edge list is `1 3`, `10 2`, `2 1`, `3 1`.
- Without `sort=True` the output is as today.

Document the parameter in each function and the option in the module docstring of each of the three modules.

Keep the existing behaviour otherwise. Finish the work completely.
