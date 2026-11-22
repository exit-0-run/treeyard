import io

import pytest

import networkx as nx

FORMATS = ("edgelist", "adjlist", "multiline_adjlist")
CLASSES = (nx.Graph, nx.DiGraph, nx.MultiGraph, nx.MultiDiGraph)


def written(fmt, G, **kw):
    b = io.BytesIO()
    getattr(nx, f"write_{fmt}")(G, b, **kw)
    return b.getvalue()


@pytest.mark.parametrize("fmt", FORMATS)
@pytest.mark.parametrize("cls", CLASSES)
def test_the_header_line(fmt, cls):
    G = cls([(1, 2)])
    first = written(fmt, G, header=True).decode().split("\n")[0]
    assert first == f"# networkx-graph: directed={G.is_directed()} multigraph={G.is_multigraph()}"
    assert not written(fmt, G).decode().startswith("# networkx-graph:")


def test_weighted_edgelist_and_comments():
    G = nx.DiGraph()
    G.add_edge(1, 2, weight=3.0)
    b = io.BytesIO()
    nx.write_weighted_edgelist(G, b, header=True, comments="%")
    assert b.getvalue().decode().split("\n")[0] == "% networkx-graph: directed=True multigraph=False"


@pytest.mark.parametrize("fmt", FORMATS)
@pytest.mark.parametrize("cls", CLASSES)
def test_readers_build_the_type_of_the_header(fmt, cls):
    G = cls([(1, 2), (2, 3)])
    if G.is_multigraph():
        G.add_edge(1, 2)
    H = getattr(nx, f"read_{fmt}")(io.BytesIO(written(fmt, G, header=True)), nodetype=int)
    assert type(H) is cls
    assert H.number_of_edges() == G.number_of_edges()


def test_create_using_wins_and_late_headers_are_comments():
    lines = ["# networkx-graph: directed=True multigraph=False", "1 2"]
    assert type(nx.parse_edgelist(lines, create_using=nx.Graph)) is nx.Graph
    assert type(nx.parse_edgelist(lines)) is nx.DiGraph
    assert type(nx.parse_edgelist(["1 2", lines[0]])) is nx.Graph
    assert type(nx.parse_adjlist(["# other comment", lines[0], "1 2"])) is nx.DiGraph
    assert type(nx.parse_adjlist(["# networkx-graph: directed=maybe multigraph=False", "1 2"])) is nx.Graph
    assert type(nx.parse_multiline_adjlist(iter(["# networkx-graph: directed=False multigraph=True", "1 1", "2"]))) is nx.MultiGraph


def test_without_the_header_nothing_changes():
    G = nx.path_graph(3)
    assert written("edgelist", G, data=False) == b"0 1\n1 2\n"
    assert type(nx.parse_edgelist(["0 1"])) is nx.Graph
