from requests.auth import HTTPBasicAuth


def test_repr_hides_the_password():
    assert repr(HTTPBasicAuth("user", "pw")) == "HTTPBasicAuth(username='user', password='***')"
    assert "pw" not in repr(HTTPBasicAuth("someone", "pw"))
