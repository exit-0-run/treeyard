from requests.structures import CaseInsensitiveDict


def test_union_operators():
    a = CaseInsensitiveDict({"Accept": "text/html", "X-A": "1"})
    b = a | {"accept": "application/json", "X-B": "2"}
    assert isinstance(b, CaseInsensitiveDict)
    assert b["ACCEPT"] == "application/json"
    assert b["x-b"] == "2" and b["x-a"] == "1"
    assert a["accept"] == "text/html"
    a |= {"X-C": "3"}
    assert a["x-c"] == "3"
