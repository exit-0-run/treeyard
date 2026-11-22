import networkx as nx
from networkx.readwrite._linewriter import LineWriter


def test_the_writer_yields_the_header():
    G = nx.DiGraph([(1, 2)], name="g")
    first = '# networkx-graph: directed=True multigraph=False name="g"'
    assert list(LineWriter(header=True).edgelist(G, data=False)) == [first, "1 2"]
    assert list(LineWriter(header=True).adjlist(G))[0] == first
    assert list(LineWriter(header=True, comments="%").multiline_adjlist(G))[0] == "%" + first[1:]
