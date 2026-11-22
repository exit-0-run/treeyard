import io

from networkx.readwrite._textfile import TextFile


def test_the_file_handles_line_ends():
    b = io.BytesIO()
    TextFile(newline="\r\n").write_lines(b, ["a", "b"])
    assert b.getvalue() == b"a\r\nb\r\n"
    assert list(TextFile().read_lines(io.BytesIO(b"a\rb\r\nc"))) == ["a\r", "b\r\n", "c"]
