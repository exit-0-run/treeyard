import pytest

import networkx as nx
from networkx.readwrite._lineparser import LineParser


def test_the_parser_raises_line_errors():
    with pytest.raises(nx.LineParseError) as e:
        LineParser(nodetype=int).parse_adjlist(["1 2", "x 3"])
    assert e.value.lineno == 2
    with pytest.raises(nx.LineParseError) as e:
        LineParser(nodetype=int).parse_edgelist(["#", "1 y"])
    assert e.value.lineno == 2
    with pytest.raises(nx.LineParseError) as e:
        LineParser(nodetype=int).parse_multiline_adjlist(iter(["1 1", "v"]))
    assert e.value.lineno == 2
