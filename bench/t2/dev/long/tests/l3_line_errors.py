import io

import pytest

import networkx as nx


def test_the_exception():
    assert issubclass(nx.LineParseError, TypeError)
    e = nx.LineParseError("bad", 3, "x y\n")
    assert (e.message, e.lineno, e.line, e.filename) == ("bad", 3, "x y", None)
    assert str(e) == "line 3: bad"
    e.filename = "g.txt"
    assert str(e) == "g.txt:3: bad"


def test_edgelist_errors_say_the_line():
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_edgelist(["# comment", "", "1 2", "x 3"], nodetype=int)
    assert (e.value.lineno, e.value.line) == (4, "x 3")
    assert str(e.value).startswith("line 4: Failed to convert nodes x,3")
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_edgelist(["1 2 {broken"], nodetype=int)
    assert e.value.lineno == 1
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_edgelist(["1 2 3", "1 3 x"], nodetype=int, data=(("w", int),))
    assert e.value.lineno == 2 and "Failed to convert w data x" in e.value.message


def test_adjlist_errors_say_the_line():
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_adjlist(["1 2", "y 3"], nodetype=int)
    assert e.value.lineno == 2 and "Failed to convert node (y)" in str(e.value)
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_adjlist(["1 2", "#", "3 z"], nodetype=int)
    assert e.value.lineno == 3


def test_multiline_errors_say_the_line():
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_multiline_adjlist(iter(["1 1", "2", "3 x"]), nodetype=int)
    assert e.value.lineno == 3
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_multiline_adjlist(iter(["1 2", "2", "q"]), nodetype=int)
    assert e.value.lineno == 3 and e.value.line == "q"
    with pytest.raises(nx.LineParseError) as e:
        nx.parse_multiline_adjlist(iter(["1 1", "2 {}", "3 2", "4 {}"]), nodetype=int)
    assert e.value.lineno == 3 and "Failed to find neighbor for node (3)" in str(e.value)


def test_read_functions_add_the_file_name(tmp_path):
    p = tmp_path / "g.txt"
    p.write_text("1 2\nz 3\n")
    with pytest.raises(nx.LineParseError) as e:
        nx.read_edgelist(p, nodetype=int)
    assert e.value.filename == str(p)
    assert str(e.value).startswith(f"{p}:2: Failed to convert nodes z,3")
    with pytest.raises(nx.LineParseError) as e:
        nx.read_adjlist(io.BytesIO(b"1 2\nz 3\n"), nodetype=int)
    assert e.value.filename is None and str(e.value).startswith("line 2: ")
    q = tmp_path / "m.txt"
    q.write_text("1 1\nw\n")
    with pytest.raises(nx.LineParseError) as e:
        nx.read_multiline_adjlist(str(q), nodetype=int)
    assert (e.value.filename, e.value.lineno) == (str(q), 2)


def test_old_handlers_still_catch_it():
    with pytest.raises(TypeError, match="Failed to convert"):
        nx.parse_adjlist(["a b"], nodetype=int)


def where(call, *args):
    with pytest.raises(nx.LineParseError) as info:
        call(*args)
    err = info.value
    assert isinstance(err, nx.NetworkXError) and isinstance(err, TypeError)
    assert err.__cause__ is not None or err.lineno >= 1
    return err.lineno, err.line


def test_the_exception_is_a_networkx_error():
    assert issubclass(nx.LineParseError, nx.NetworkXError)
    assert issubclass(nx.LineParseError, TypeError)


@pytest.mark.parametrize(
    ("text", "lineno", "line"),
    [("*vertices x", 1, "*vertices x"),
     ('*vertices 1\n1 "a', 2, '1 "a'),
     ("*vertices 1\n1", 2, "1"),
     ('*vertices 2\n1 a\n2 b\n*edges\n1 "2', 5, '1 "2'),
     ("*vertices 2\n1 a\n2 b\n*matrix\n0 x", 5, "0 x")],
)
def test_pajek_errors_say_the_line(text, lineno, line):
    assert where(nx.parse_pajek, text) == (lineno, line)
    assert where(nx.parse_pajek, text.split("\n")) == (lineno, line)


def test_pajek_missing_vertex_lines():
    lineno, line = where(nx.parse_pajek, ["*vertices 3", "1 a"])
    assert lineno == 1 and line == "*vertices 3"


LEDA = ["LEDA.GRAPH", "string", "int"]


@pytest.mark.parametrize(
    ("rest", "lineno", "line"),
    [(["x"], 4, "x"),
     (["-1", "2", "|{a}|", "|{b}|", "z"], 8, "z"),
     (["-1", "# c", "", "2", "|{a}|", "|{b}|", "1", "1 2 0"], 11, "1 2 0"),
     (["-1", "2", "|{a}|", "|{b}|", "1", "1 5 0 |{x}|"], 9, "1 5 0 |{x}|")],
)
def test_leda_errors_say_the_line(rest, lineno, line):
    assert where(nx.parse_leda, LEDA + rest) == (lineno, line)
    assert where(nx.parse_leda, "\n".join(LEDA + rest)) == (lineno, line)


def test_leda_missing_lines():
    lineno, line = where(nx.parse_leda, LEDA + ["-1", "2", "|{a}|"])
    assert lineno == 7 and line == ""


def test_pajek_and_leda_readers_add_the_file_name(tmp_path):
    p = tmp_path / "g.net"
    p.write_bytes(b"*vertices x\n")
    with pytest.raises(nx.LineParseError) as info:
        nx.read_pajek(p)
    assert info.value.filename == str(p) and str(info.value).startswith(f"{p}:1: ")
    p = tmp_path / "g.leda"
    p.write_bytes(b"LEDA.GRAPH\nstring\nint\nx\n")
    with pytest.raises(nx.LineParseError) as info:
        nx.read_leda(p)
    assert info.value.filename == str(p) and str(info.value).startswith(f"{p}:4: ")
