import inspect
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


def test_open(tmp_path):
    p = tmp_path / "a.txt"
    with TextFile().open(p, "wb") as f:
        f.write(b"x\n")
    assert f.closed and p.read_bytes() == b"x\n"
    with TextFile().open(str(p), "rb") as f:
        assert f.read() == b"x\n"
    assert f.closed
    b = io.BytesIO(b"y\n")
    with TextFile().open(b, "rb") as f:
        assert f is b
    assert not b.closed
    import gzip

    with TextFile().open(tmp_path / "a.gz", "wb") as f:
        f.write(b"z\n")
    assert gzip.decompress((tmp_path / "a.gz").read_bytes()) == b"z\n"


def test_read_and_write(tmp_path):
    p = tmp_path / "b.txt"
    TextFile().write(p, ["a b", "c"])
    assert p.read_bytes() == b"a b\nc\n"
    assert TextFile().read(p, list) == ["a b\n", "c\n"]
    assert TextFile().read(p, lambda lines, n: n * len(list(lines)), n=3) == 6
    b = io.BytesIO()
    TextFile("utf-16-le").write(b, ["é"])
    assert b.getvalue() == "é\n".encode("utf-16-le") and not b.closed


def test_pajek_and_leda_go_through_the_class(monkeypatch):
    reads, writes = [], []
    real_read, real_write = TextFile.read_lines, TextFile.write_lines

    def spy_read(self, f):
        reads.append(self.encoding)
        return real_read(self, f)

    def spy_write(self, f, lines):
        lines = list(lines)
        writes.append(lines)
        return real_write(self, f, lines)

    monkeypatch.setattr(TextFile, "read_lines", spy_read)
    monkeypatch.setattr(TextFile, "write_lines", spy_write)
    b = io.BytesIO()
    nx.write_pajek(nx.path_graph(2), b, encoding="latin-1")
    assert writes[-1] == list(nx.generate_pajek(nx.path_graph(2)))
    assert sorted(nx.read_pajek(io.BytesIO(b.getvalue()), encoding="latin-1")) == ["0", "1"]
    nx.read_leda(io.BytesIO(b"LEDA.GRAPH\nstring\nint\n-1\n1\n|{a}|\n0\n"))
    assert reads == ["latin-1", "UTF-8"]


def test_every_format_opens_its_file_through_the_class(monkeypatch, tmp_path):
    opened = []
    real = TextFile.open

    def spy(self, path, mode):
        opened.append(mode)
        return real(self, path, mode)

    monkeypatch.setattr(TextFile, "open", spy)
    G = nx.path_graph(3)
    for fmt in ("edgelist", "adjlist", "multiline_adjlist", "pajek", "gml", "graph6", "sparse6", "graphml", "gexf"):
        opened.clear()
        p = tmp_path / f"g.{fmt}"
        getattr(nx, f"write_{fmt}")(G, p)
        H = getattr(nx, f"read_{fmt}")(p)
        assert len(H) == 3, fmt
        # write_graphml without lxml hands its open file on to write_graphml_xml, which opens it again
        assert opened[0] == "wb" and opened[-1] == "rb" and set(opened) == {"wb", "rb"}, (fmt, opened)
    opened.clear()
    nx.write_weighted_edgelist(nx.Graph([(1, 2, {"weight": 1.0})]), tmp_path / "w")
    nx.read_weighted_edgelist(tmp_path / "w")
    (tmp_path / "l").write_bytes(b"LEDA.GRAPH\nstring\nint\n-1\n1\n|{a}|\n0\n")
    nx.read_leda(tmp_path / "l")
    assert opened == ["wb", "rb", "rb"]


def test_the_byte_options_are_options_of_the_class():
    options = set(inspect.signature(TextFile).parameters)
    for f in (nx.read_edgelist, nx.read_weighted_edgelist, nx.read_adjlist, nx.read_multiline_adjlist, nx.read_pajek,
              nx.read_leda, nx.write_edgelist, nx.write_weighted_edgelist, nx.write_adjlist, nx.write_multiline_adjlist,
              nx.write_pajek):
        kinds = set(inspect.signature(f).parameters) & {"encoding", "bom", "newline", "compression"}
        assert kinds <= options, f.__name__
