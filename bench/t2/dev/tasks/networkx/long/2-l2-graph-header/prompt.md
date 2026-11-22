# Line formats: a header line with the graph type

Files in the edge list, adjacency list and multiline adjacency list formats (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) do not say whether the graph is directed or a multigraph, so the reader has to know and pass `create_using`. Add an optional header line.

- `write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and `write_multiline_adjlist` take `header=False`. With `header=True` the first line they write is `<comments> networkx-graph: directed=<True|False> multigraph=<True|False>`, with the `comments` argument of the call: with the default `"#"` it is `# networkx-graph: directed=True multigraph=False` for a `DiGraph`. The comment lines that the adjacency list writers write today come after it. Without `header=True` the output is as today.
- `parse_edgelist`, `parse_adjlist`, `parse_multiline_adjlist` and the `read_*` functions: when `create_using` is `None` and a header line comes before the first data line, the graph is a `Graph`, `DiGraph`, `MultiGraph` or `MultiDiGraph` as the header says. A header line is a line whose text, stripped, is the comments string followed by exactly the three words `networkx-graph:`, `directed=<True|False>` and `multigraph=<True|False>`; any other comment line is a comment as before, and so is a header line after the first data line. Other comment lines may come before the header. With `create_using` given the header changes nothing, and `comments=None` (edge list) means no header.

Document the parameter in each writer and the header in the module docstring of each of the three modules.

Keep the existing behaviour otherwise. Finish the work completely.
