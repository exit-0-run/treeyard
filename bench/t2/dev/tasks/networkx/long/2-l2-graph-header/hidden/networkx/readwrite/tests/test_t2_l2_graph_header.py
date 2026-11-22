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


@pytest.mark.parametrize("fmt", FORMATS)
def test_the_name_in_the_header(fmt):
    G = nx.DiGraph([(1, 2)], name='big "roads"')
    first = written(fmt, G, header=True).decode().split("\n")[0]
    assert first == '# networkx-graph: directed=True multigraph=False name="big \\"roads\\""'
    H = getattr(nx, f"read_{fmt}")(io.BytesIO(written(fmt, G, header=True)), nodetype=int)
    assert type(H) is nx.DiGraph and H.graph["name"] == 'big "roads"'
    H = getattr(nx, f"parse_{fmt}")(iter(["# networkx-graph: directed=False multigraph=True name=\"é\"", *(["1 2"] if fmt != "multiline_adjlist" else ["1 1", "2"])]))
    assert type(H) is nx.MultiGraph and H.graph["name"] == "é"


def test_a_bad_name_makes_a_comment():
    for line in ("# networkx-graph: directed=True multigraph=False name=roads",
                 "# networkx-graph: directed=True multigraph=False name=1",
                 "# networkx-graph: directed=True multigraph=False label=\"x\""):
        H = nx.parse_edgelist([line, "1 2"])
        assert type(H) is nx.Graph and "name" not in H.graph


@pytest.mark.parametrize("fmt", FORMATS)
def test_generate_yields_the_header_first(fmt):
    G = nx.MultiGraph([(1, 2)], name="g")
    lines = list(getattr(nx, f"generate_{fmt}")(G, header=True))
    assert lines[0] == '# networkx-graph: directed=False multigraph=True name="g"'
    assert lines[1:] == list(getattr(nx, f"generate_{fmt}")(G))
    lines = list(getattr(nx, f"generate_{fmt}")(G, header=True, comments="%"))
    assert lines[0] == '% networkx-graph: directed=False multigraph=True name="g"'


def test_parse_graph_header():
    assert nx.parse_graph_header(["# networkx-graph: directed=True multigraph=False", "1 2"]) == {
        "directed": True, "multigraph": False, "name": None}
    assert nx.parse_graph_header(["# a comment", "", '# networkx-graph: directed=False multigraph=True name="x"']) == {
        "directed": False, "multigraph": True, "name": "x"}
    assert nx.parse_graph_header(["1 2", "# networkx-graph: directed=True multigraph=False"]) is None
    assert nx.parse_graph_header(["% networkx-graph: directed=True multigraph=True"], comments="%")["multigraph"] is True
    assert nx.parse_graph_header([]) is None


def test_read_graph_header_reads_only_the_header(tmp_path):
    p = tmp_path / "g.edgelist"
    nx.write_edgelist(nx.DiGraph([(1, 2)], name="n"), p, header=True)
    assert nx.read_graph_header(p) == {"directed": True, "multigraph": False, "name": "n"}
    seen = []

    def lines():
        yield b"# networkx-graph: directed=False multigraph=False\n"
        seen.append(1)
        yield b"1 2\n"
        seen.append(2)
        yield b"oops \xff\n"

    assert nx.read_graph_header(lines()) == {"directed": False, "multigraph": False, "name": None}
    assert seen == []
