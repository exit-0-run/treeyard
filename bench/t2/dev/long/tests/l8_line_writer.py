import inspect

import networkx as nx
from networkx.readwrite._linewriter import LineWriter


def graph():
    G = nx.Graph()
    G.add_edge(1, 2, w=3)
    G.add_edge(2, 3)
    return G


def test_the_class():
    assert LineWriter().delimiter == " "
    assert LineWriter(",").delimiter == ","
    assert LineWriter().name(3) == "3"


def test_the_methods_yield_what_the_functions_yield():
    G = graph()
    assert list(LineWriter().edgelist(G)) == list(nx.generate_edgelist(G))
    assert list(LineWriter(",").edgelist(G, data=False)) == list(nx.generate_edgelist(G, ",", data=False))
    assert list(LineWriter().edgelist(G, data=["w"])) == ["1 2 3", "2 3"]
    assert list(LineWriter().adjlist(G)) == list(nx.generate_adjlist(G)) == ["1 2", "2 3", "3"]
    assert list(LineWriter().multiline_adjlist(G)) == list(nx.generate_multiline_adjlist(G))
    D = nx.MultiDiGraph([(1, 2), (1, 2)])
    assert list(LineWriter().adjlist(D)) == ["1 2 2", "2"]
    assert list(LineWriter().multiline_adjlist(D)) == ["1 2", "2 {}", "2 {}", "2 0"]


def test_the_functions_go_through_the_class(monkeypatch):
    monkeypatch.setattr(LineWriter, "name", lambda self, node: f"<{node}>")
    G = graph()
    assert list(nx.generate_adjlist(G))[0] == "<1> <2>"
    assert list(nx.generate_edgelist(G, data=False))[0] == "<1> <2>"
    assert list(nx.generate_multiline_adjlist(G))[:2] == ["<1> 1", "<2> {'w': 3}"]


def test_a_subclass_changes_every_format():
    class Hex(LineWriter):
        def name(self, node):
            return hex(node)

    G = graph()
    assert list(Hex().adjlist(G))[0] == "0x1 0x2"
    assert list(Hex().edgelist(G, data=False))[0] == "0x1 0x2"


def test_every_option_of_the_functions_is_an_option_of_the_class():
    options = set(inspect.signature(LineWriter).parameters)
    for f in (nx.generate_edgelist, nx.generate_adjlist, nx.generate_multiline_adjlist):
        assert set(inspect.signature(f).parameters) - {"G", "data"} <= options, f.__name__


def test_writing_files_is_unchanged(tmp_path):
    p = tmp_path / "g.edgelist"
    nx.write_edgelist(graph(), p, data=False)
    assert p.read_bytes() == b"1 2\n2 3\n"


def test_pajek_is_a_method_too(monkeypatch):
    G = nx.Graph([(1, 2), (2, 3)])
    G.nodes[1]["x"] = 0.5
    assert list(LineWriter().pajek(G)) == list(nx.generate_pajek(G))
    assert list(LineWriter(",").pajek(G))[:2] == ["*vertices 3", "1 1 0.5 0.0 ellipse"]
    monkeypatch.setattr(LineWriter, "pajek", lambda self, G: iter(["*vertices 0", "*edges"]))
    assert list(nx.generate_pajek(G)) == ["*vertices 0", "*edges"]
    b = __import__("io").BytesIO()
    nx.write_pajek(G, b)
    assert b.getvalue() == b"*vertices 0\n*edges\n"


def test_every_option_of_generate_pajek_is_an_option_of_the_class():
    options = set(inspect.signature(LineWriter).parameters)
    assert set(inspect.signature(nx.generate_pajek).parameters) - {"G"} <= options
