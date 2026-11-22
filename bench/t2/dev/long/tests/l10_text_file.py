import io

import networkx as nx
from networkx.readwrite._textfile import TextFile


def test_the_class():
    assert TextFile().encoding == "utf-8"
    assert TextFile("latin-1").encoding == "latin-1"


def test_read_lines():
    assert list(TextFile().read_lines(io.BytesIO("a b\nc é\n".encode()))) == ["a b\n", "c é\n"]
    assert list(TextFile("latin-1").read_lines(io.BytesIO("x é\n".encode("latin-1")))) == ["x é\n"]


def test_write_lines():
    b = io.BytesIO()
    TextFile().write_lines(b, ["a b", "c é"])
    assert b.getvalue() == "a b\nc é\n".encode()
    b = io.BytesIO()
    TextFile("utf-16-le").write_lines(b, ["a"])
    assert b.getvalue() == "a\n".encode("utf-16-le")


def test_read_functions_go_through_the_class(monkeypatch):
    seen = []
    real = TextFile.read_lines

    def spy(self, f):
        seen.append(self.encoding)
        return real(self, f)

    monkeypatch.setattr(TextFile, "read_lines", spy)
    assert sorted(nx.read_edgelist(io.BytesIO(b"1 2\n"), nodetype=int).edges) == [(1, 2)]
    nx.read_weighted_edgelist(io.BytesIO(b"1 2 3.0\n"), encoding="latin-1")
    nx.read_adjlist(io.BytesIO(b"1 2\n"))
    nx.read_multiline_adjlist(io.BytesIO(b"1 1\n2\n"))
    assert seen == ["utf-8", "latin-1", "utf-8", "utf-8"]


def test_write_functions_write_every_line_through_the_class(monkeypatch):
    written = []
    real = TextFile.write_lines

    def spy(self, f, lines):
        lines = list(lines)
        written.append(lines)
        return real(self, f, lines)

    monkeypatch.setattr(TextFile, "write_lines", spy)
    G = nx.path_graph(2)
    b = io.BytesIO()
    nx.write_edgelist(G, b, data=False)
    assert written[-1] == ["0 1"] and b.getvalue() == b"0 1\n"
    nx.write_weighted_edgelist(nx.Graph([(1, 2, {"weight": 1.0})]), io.BytesIO())
    assert written[-1] == ["1 2 1.0"]
    b = io.BytesIO()
    nx.write_adjlist(G, b)
    assert written[-1][0].startswith("#") and written[-1][-2:] == ["0 1", "1"]
    assert b.getvalue().decode().split("\n")[:-1] == written[-1]
    nx.write_multiline_adjlist(G, io.BytesIO())
    assert written[-1][0].startswith("#") and written[-1][-3:] == ["0 1", "1 {}", "1 0"]


def test_files_are_unchanged(tmp_path):
    p = tmp_path / "g.adjlist"
    nx.write_adjlist(nx.path_graph(3), p)
    assert p.read_text().split("\n")[-4:] == ["0 1", "1 2", "2", ""]
    assert sorted(nx.read_adjlist(p, nodetype=int).edges) == [(0, 1), (1, 2)]
