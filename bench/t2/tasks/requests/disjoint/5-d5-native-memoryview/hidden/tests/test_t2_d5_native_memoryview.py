from requests._internal_utils import to_native_string


def test_memoryview_is_accepted():
    assert to_native_string(memoryview(b"abc")) == "abc"
    assert to_native_string(b"xyz") == "xyz"
