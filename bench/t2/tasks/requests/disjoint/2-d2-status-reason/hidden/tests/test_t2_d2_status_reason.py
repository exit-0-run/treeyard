from requests.status_codes import reason


def test_reason_returns_the_first_name():
    assert reason(404) == "not_found"
    assert reason(200) == "ok"
    assert reason(999) is None
