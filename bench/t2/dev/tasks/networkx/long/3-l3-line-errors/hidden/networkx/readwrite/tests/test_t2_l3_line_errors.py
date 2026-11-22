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
