# Text formats: compressed files

The readers and writers of `networkx/readwrite` compress only by the file name (`.gz`, `.bz2`): a compressed file with another name, or a compressed open file, cannot be read, and an open file cannot be written compressed. Add compression by content to the edge list, adjacency list, multiline adjacency list, Pajek, LEDA, GML, graph6, sparse6, GraphML and GEXF formats (`edgelist.py`, `adjlist.py`, `multiline_adjlist.py`, `pajek.py`, `leda.py`, `gml.py`, `graph6.py`, `sparse6.py`, `graphml.py`, `gexf.py`).

- Reading: `read_edgelist`, `read_weighted_edgelist`, `read_adjlist`, `read_multiline_adjlist`, `read_pajek`, `read_leda`, `read_gml`, `read_graph6`, `read_sparse6`, `read_graphml` and `read_gexf` read a file compressed with gzip, bz2 or xz whatever its name, also from an open file: the compression is recognised by the first bytes (gzip `1f 8b`, bz2 `BZh`, xz `fd 37 7a 58 5a 00`). Data compressed several times in a row is uncompressed every time. Other files are read as today.
- Writing: `write_edgelist`, `write_weighted_edgelist`, `write_adjlist`, `write_multiline_adjlist`, `write_pajek`, `write_gml`, `write_graph6`, `write_sparse6`, `write_graphml_xml`, `write_graphml_lxml` (so `write_graphml`) and `write_gexf` take `compression=None`. With `"gzip"`, `"bz2"` or `"xz"` the bytes they write today are written compressed with it, to a file name or into an open binary file; an open file stays open, with the compressed data complete. Another value raises `ValueError`, and so does a compression with a file name whose ending already compresses (`.gz`, `.gzip`, `.bz2`, `.xz`).
- A file name ending in `.xz` is compressed with xz when written and uncompressed when read, as `.gz` and `.bz2` are today (`networkx.utils.open_file`).
- With `compression=None` and no compressed input everything is as today.

Document the parameter in each writer, the reading rule in each reader and both in the module docstring of each of the ten modules.
