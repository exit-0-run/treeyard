import codecs
import io

from networkx.readwrite._textfile import TextFile


def test_the_file_handles_byte_order_marks():
    data = codecs.BOM_UTF16_LE + "a b\nc d\n".encode("utf-16-le")
    assert list(TextFile(encoding=None).read_lines(io.BytesIO(data))) == ["a b\n", "c d\n"]
    assert list(TextFile().read_lines(io.BytesIO(codecs.BOM_UTF8 + b"x y\n"))) == ["x y\n"]
    b = io.BytesIO()
    TextFile(bom=True).write_lines(b, ["a b"])
    assert b.getvalue() == codecs.BOM_UTF8 + b"a b\n"
