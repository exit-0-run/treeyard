import networkx as nx
from networkx.readwrite._linewriter import LineWriter


def test_the_writer_quotes_names():
    assert LineWriter().name("New York") == '"New York"'
    assert LineWriter().name("plain") == "plain"
    assert LineWriter(",").name("a,b") == '"a,b"'
    G = nx.Graph([("a b", "c")])
    assert list(LineWriter().adjlist(G)) == ['"a b" c', "c"]
    assert list(LineWriter().edgelist(G, data=False)) == ['"a b" c']
