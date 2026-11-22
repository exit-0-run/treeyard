import inspect

import networkx as nx
from networkx.readwrite._lineparser import LineParser


def edges(G):
    return sorted(map(tuple, map(sorted, (map(str, e[:2]) for e in G.edges))))


def test_the_class_and_its_settings():
    p = LineParser()
    assert (p.comments, p.delimiter, p.nodetype) == ("#", None, None)
    p = LineParser(comments="%", delimiter=",", nodetype=int)
    assert (p.comments, p.delimiter, p.nodetype) == ("%", ",", int)


def test_the_steps():
    assert LineParser().strip("a b # c") == "a b "
    assert LineParser(comments=None).strip("a#b") == "a#b"
    assert LineParser().split("a b\n") == ["a", "b"]
    assert LineParser(delimiter=",").split("a,b\n") == ["a", "b"]
    assert LineParser().node("7") == "7"
    assert LineParser(nodetype=int).node("7") == 7


def test_the_methods_return_what_the_functions_return():
    lines = ["# c", "1 2 {'w': 1}", "2 3"]
    assert edges(LineParser(nodetype=int).parse_edgelist(lines)) == edges(nx.parse_edgelist(lines, nodetype=int))
    assert LineParser(nodetype=int).parse_edgelist(lines)[1][2] == {"w": 1}
    assert edges(LineParser().parse_adjlist(["a b c", "d e"])) == edges(nx.parse_adjlist(["a b c", "d e"]))
    G = LineParser(nodetype=int).parse_multiline_adjlist(iter(["1 2", "2 {'w': 3}", "3", "4 0"]), create_using=nx.DiGraph)
    assert isinstance(G, nx.DiGraph) and G[1][2] == {"w": 3} and 4 in G
    G = LineParser().parse_edgelist(["a b 1"], data=(("w", int),))
    assert G["a"]["b"] == {"w": 1}


def test_the_functions_go_through_the_class(monkeypatch):
    calls = []
    for name in ("strip", "split", "node"):
        real = getattr(LineParser, name)

        def spy(self, *args, _real=real, _name=name, **kwargs):
            calls.append(_name)
            return _real(self, *args, **kwargs)

        monkeypatch.setattr(LineParser, name, spy)
    nx.parse_edgelist(["1 2 # x"], nodetype=int, data=False)
    assert {"strip", "split", "node"} <= set(calls)
    calls.clear()
    nx.parse_adjlist(["1 2"], nodetype=int)
    assert {"strip", "split", "node"} <= set(calls)
    calls.clear()
    nx.parse_multiline_adjlist(iter(["1 1", "2"]), nodetype=int)
    assert {"strip", "split", "node"} <= set(calls)


def test_a_subclass_changes_every_format():
    class Upper(LineParser):
        def node(self, text):
            return text.upper()

    assert set(Upper(nodetype=str).parse_adjlist(["a b"]).nodes) == {"A", "B"}
    assert set(Upper(nodetype=str).parse_edgelist(["a b"]).nodes) == {"A", "B"}
    assert set(Upper(nodetype=str).parse_multiline_adjlist(iter(["a 1", "b"])).nodes) == {"A", "B"}


def test_the_functions_are_kept():
    for f, names in [(nx.parse_edgelist, ["lines", "comments", "delimiter", "create_using", "nodetype", "data"]),
                     (nx.parse_adjlist, ["lines", "comments", "delimiter", "create_using", "nodetype"]),
                     (nx.parse_multiline_adjlist, ["lines", "comments", "delimiter", "create_using", "nodetype", "edgetype"])]:
        params = list(inspect.signature(f).parameters)
        assert params[: len(names)] == names
