import io

import pytest

import networkx as nx
from networkx.readwrite._textfile import TextFile


def test_the_file_names_the_file_of_an_error(tmp_path):
    p = tmp_path / "g.txt"
    p.write_bytes(b"1 2\nx 3\n")
    with pytest.raises(nx.LineParseError) as info:
        TextFile().read(p, nx.parse_edgelist, nodetype=int)
    assert info.value.filename == str(p) and info.value.lineno == 2
    b = io.BytesIO(b"x\n")
    b.name = "named.adj"
    with pytest.raises(nx.LineParseError) as info:
        TextFile().read(b, nx.parse_adjlist, nodetype=int)
    assert str(info.value).startswith("named.adj:1: ")
