import pytest

from requests.utils import dotted_netmask


def test_mask_outside_the_range_is_refused():
    with pytest.raises(ValueError):
        dotted_netmask(-1)
    with pytest.raises(ValueError):
        dotted_netmask(33)
    assert dotted_netmask(24) == "255.255.255.0"
    assert dotted_netmask(0) == "0.0.0.0"
