import gzip
import io
import lzma

from networkx.readwrite._textfile import TextFile


def test_the_file_compresses_and_uncompresses():
    b = io.BytesIO()
    TextFile(compression="xz").write_lines(b, ["a b", "c"])
    assert lzma.decompress(b.getvalue()) == b"a b\nc\n" and not b.closed
    b = io.BytesIO()
    TextFile(compression="gzip").write(b, ["x"])
    assert gzip.decompress(b.getvalue()) == b"x\n"
    assert list(TextFile().read_lines(io.BytesIO(gzip.compress(b"a b\nc\n")))) == ["a b\n", "c\n"]
    assert TextFile().read(io.BytesIO(lzma.compress(b"p q\n")), list) == ["p q\n"]
