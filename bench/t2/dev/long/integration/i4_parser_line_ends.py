from networkx.readwrite._lineparser import LineParser


def test_the_parser_drops_any_line_end():
    assert LineParser(delimiter=",").split("a,b\r\n") == ["a", "b"]
    assert set(LineParser(delimiter=",").parse_adjlist(["a,b\r\n", "c,d\r"]).nodes) == {"a", "b", "c", "d"}
    assert list(LineParser(delimiter=",").parse_edgelist(["a,b\r\n"], data=False).edges) == [("a", "b")]
