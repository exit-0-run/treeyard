# CaseInsensitiveDict: | and |= operators

In `src/requests/structures.py`, give `CaseInsensitiveDict` the `|` and `|=` operators of `dict`. `a | other` returns a new `CaseInsensitiveDict` with the items of `a` updated by the mapping `other` (keys still match without case); `a |= other` updates `a` in place.

Keep the existing behaviour otherwise. Finish the work completely.
