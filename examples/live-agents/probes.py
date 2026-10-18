#!/usr/bin/env python3
"""Hidden behaviour checks for demo v2: one per task, run against a repo's final main.

    python probes.py REPO      -> prints one JSON object {task: true|false|"error: ..."}
The agents never see this file. Each check fails on the base commit 2fe1b2e.
"""
import json, sys, tracemalloc

sys.path.insert(0, sys.argv[1])
import more_itertools as mi  # noqa: E402


def numeric_range_consistent():
    r = mi.numeric_range(0.0, 1.0, 0.1)
    items = list(r)
    return len(r) == len(items) and all(x in r for x in items) and all(r.index(x) == i for i, x in enumerate(items))


def numeric_range_reversed():
    r = mi.numeric_range(0.0, 1.0, 0.1)
    return list(reversed(r)) == list(r)[::-1]


def seekable_zero_cache_peek():
    s = mi.seekable(iter([1, 2, 3]), maxlen=0)
    ok = s.peek() == 1 and next(s) == 1 and bool(s) and next(s) == 2
    return ok and list(s) == [3]


def bucket_no_phantom_key():
    b = mi.bucket(['a1', 'b1', 'a2'], key=lambda x: x[0])
    _ = 'c' in b
    return 'c' not in list(b) and sorted(b) == ['a', 'b']


def zip_broadcast_iter_once():
    calls = []

    class Once:
        def __iter__(self):
            calls.append(1)
            return iter([1, 2])

    list(mi.zip_broadcast(Once(), 'x'))
    return len(calls) == 1


def constrained_batches_max_count():
    try:
        list(mi.constrained_batches([b'1', b'2'], 10, max_count=0))
    except ValueError:
        return True
    return False


def split_maxsplit_zero_empty():
    p = lambda x: x == 0  # noqa: E731
    return (list(mi.split_before([], p, maxsplit=0)) == [] and list(mi.split_after([], p, maxsplit=0)) == []
            and list(mi.split_when([], lambda a, b: True, maxsplit=0)) == [])


def value_chain_keeps_type_error():
    try:
        list(mi.value_chain(map(len, ['ab', 'cde', 5])))
    except TypeError:
        return True
    return False


CHECKS = {
    "numeric_range_consistent": numeric_range_consistent,
    "numeric_range_reversed": numeric_range_reversed,
    "seekable_zero_cache_peek": seekable_zero_cache_peek,
    "bucket_no_phantom_key": bucket_no_phantom_key,
    "zip_broadcast_iter_once": zip_broadcast_iter_once,
    "constrained_batches_max_count": constrained_batches_max_count,
    "split_maxsplit_zero_empty": split_maxsplit_zero_empty,
    "value_chain_keeps_type_error": value_chain_keeps_type_error,
}
out = {}
for name, f in CHECKS.items():
    try:
        out[name] = bool(f())
    except Exception as e:  # a crash counts as a failed check
        out[name] = f"error: {type(e).__name__}: {e}"[:200]
print(json.dumps(out))
