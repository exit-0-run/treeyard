# dotted_netmask: refuse masks outside 0..32

In `src/requests/utils.py`, make `dotted_netmask(mask)` raise `ValueError` when `mask` is not between 0 and 32. Today `dotted_netmask(-1)` returns a wrong value. `dotted_netmask(24)` stays `"255.255.255.0"`.

Keep the existing behaviour otherwise. Finish the work completely.
