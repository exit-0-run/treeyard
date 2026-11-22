import io

import networkx as nx


def test_the_header_ends_with_the_newline():
    for write, read in [(nx.write_edgelist, nx.read_edgelist), (nx.write_adjlist, nx.read_adjlist),
                        (nx.write_multiline_adjlist, nx.read_multiline_adjlist)]:
        b = io.BytesIO()
        write(nx.DiGraph([(1, 2)]), b, header=True, newline="\r")
        data = b.getvalue()
        assert data.startswith(b"# networkx-graph: directed=True multigraph=False\r")
        assert b"\n" not in data
        assert type(read(io.BytesIO(data))) is nx.DiGraph, write.__name__
