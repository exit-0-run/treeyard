#!/usr/bin/env python3
"""Hidden behaviour checks for the hard task set (two agents per function): one per task, plus one
per pair that uses both new options together. Run against a repo's final main:

    python probes_hard.py REPO   -> one JSON object {check: true|false|"error: ..."}
Each check fails on the base commit 4aeb433.
"""
import json, sys

sys.path.insert(0, sys.argv[1])
import more_itertools as mi  # noqa: E402


def raises(exc, f):
    try:
        f()
    except exc:
        return True
    return False


CHECKS = {
    "windowed_strict": lambda: raises(ValueError, lambda: list(mi.windowed([1, 2, 3, 4], 3, step=2, strict=True)))
    and list(mi.windowed([1, 2, 3], 3, strict=True)) == [(1, 2, 3)],
    "windowed_skip_partial": lambda: list(mi.windowed([1, 2, 3, 4], 3, step=2, skip_partial=True)) == [(1, 2, 3)],
    "windowed_both": lambda: list(mi.windowed([1, 2, 3, 4, 5], 3, step=2, strict=True, skip_partial=True))
    == [(1, 2, 3), (3, 4, 5)],
    "first_pred": lambda: mi.first([1, 2, 3], pred=lambda x: x > 1) == 2
    and mi.first([1, 2], default=None, pred=lambda x: x > 5) is None,
    "first_too_short": lambda: raises(IndexError, lambda: mi.first([], too_short=IndexError)),
    "first_both": lambda: raises(KeyError, lambda: mi.first([1, 2], pred=lambda x: x > 5, too_short=KeyError)),
    "split_at_keep_empty": lambda: list(mi.split_at('a,,b', lambda c: c == ',', keep_empty=False)) == [['a'], ['b']],
    "split_at_separator_value": lambda: list(mi.split_at('a,b', ',')) == [['a'], ['b']],
    "split_at_both": lambda: list(mi.split_at('a,,b', ',', keep_empty=False)) == [['a'], ['b']],
    "padded_strict": lambda: raises(ValueError, lambda: list(mi.padded([1, 2, 3], 0, n=2, strict=True)))
    and list(mi.padded([1], 0, n=2, strict=True)) == [1, 0],
    "padded_left": lambda: list(mi.padded([1, 2], 0, n=4, left=True)) == [0, 0, 1, 2],
    "padded_both": lambda: list(mi.padded([1, 2], 0, n=3, left=True, strict=True)) == [0, 1, 2],
}
out = {}
for name, f in CHECKS.items():
    try:
        out[name] = bool(f())
    except Exception as e:  # a crash counts as a failed check
        out[name] = f"error: {type(e).__name__}: {e}"[:200]
print(json.dumps(out))
