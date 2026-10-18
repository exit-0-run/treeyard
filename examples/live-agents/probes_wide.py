#!/usr/bin/env python3
"""Hidden behaviour checks for the wide task set (tasks_wide.py): one check per task, named as the
task, plus one `<function>_both` check per pair that uses both new options together (60 checks).
Run against a repo's final main:

    python probes_wide.py REPO   -> one JSON object {check: true|false|"error: ..."}
Each check fails on the base commit 4aeb433. Checks only use the public API and never hang.
"""
import json, sys
from itertools import islice
from operator import length_hint

sys.path.insert(0, sys.argv[1])
import more_itertools as mi  # noqa: E402


def raises(exc, f):
    try:
        f()
    except exc:
        return True
    return False


def lst(it, n=100):  # materialise at most n items, so a wrong infinite iterator cannot hang a check
    return list(islice(it, n))


odd = lambda x: x % 2 == 1  # noqa: E731


def peekable_length_hint():
    p = mi.peekable([1, 2, 3])
    p.peek()
    first = length_hint(p)
    next(p)
    second = length_hint(p)
    p.prepend(0)
    return (first, second, length_hint(p)) == (3, 2, 3)


CHECKS = {
    # ---- pairs: two options on one function, plus the two together
    "chunked_fillvalue": lambda: lst(mi.chunked([1, 2, 3, 4, 5], 3, fillvalue=0)) == [[1, 2, 3], [4, 5, 0]]
    and lst(mi.chunked([1, 2, 3], 3, fillvalue=0)) == [[1, 2, 3]],
    "chunked_as_tuples": lambda: lst(mi.chunked([1, 2, 3], 2, as_tuples=True)) == [(1, 2), (3,)]
    and all(type(c) is tuple for c in mi.chunked([1, 2, 3], 2, as_tuples=True)),
    "chunked_both": lambda: lst(mi.chunked([1, 2, 3], 2, fillvalue=0, as_tuples=True)) == [(1, 2), (3, 0)],

    "intersperse_leading": lambda: lst(mi.intersperse('x', [1, 2], leading=True)) == ['x', 1, 'x', 2]
    and lst(mi.intersperse('x', [], leading=True)) == []
    and lst(mi.intersperse('x', [1, 2, 3], n=2, leading=True)) == ['x', 1, 2, 'x', 3],
    "intersperse_trailing": lambda: lst(mi.intersperse('x', [1, 2], trailing=True)) == [1, 'x', 2, 'x']
    and lst(mi.intersperse('x', [], trailing=True)) == []
    and lst(mi.intersperse('x', [1, 2, 3], n=2, trailing=True)) == [1, 2, 'x', 3, 'x'],
    "intersperse_both": lambda: lst(mi.intersperse('x', [1, 2], leading=True, trailing=True)) == ['x', 1, 'x', 2, 'x']
    and lst(mi.intersperse('x', [1, 2, 3], n=2, leading=True, trailing=True)) == ['x', 1, 2, 'x', 3, 'x']
    and lst(mi.intersperse('x', [], leading=True, trailing=True)) == [],

    "lstrip_value": lambda: lst(mi.lstrip([0, 0, 1, 0], 0)) == [1, 0]
    and lst(mi.lstrip([0, 0, 1, 0], lambda x: x == 0)) == [1, 0],
    "lstrip_limit": lambda: lst(mi.lstrip([0, 0, 0, 1], lambda x: x == 0, limit=2)) == [0, 1]
    and lst(mi.lstrip([0, 0, 0, 1], lambda x: x == 0, limit=5)) == [1],
    "lstrip_both": lambda: lst(mi.lstrip([0, 0, 0, 1], 0, limit=1)) == [0, 0, 1],

    "one_pred": lambda: mi.one([1, 2, 3], pred=lambda x: x > 2) == 3
    and raises(ValueError, lambda: mi.one([1, 2, 3], pred=lambda x: x > 1))
    and raises(ValueError, lambda: mi.one([1], pred=lambda x: x > 5)),
    "one_default": lambda: mi.one([], default=None) is None and mi.one([7], default=0) == 7
    and raises(ValueError, lambda: mi.one([1, 2], default=0)),
    "one_both": lambda: mi.one([1, 2], pred=lambda x: x > 5, default=0) == 0
    and raises(ValueError, lambda: mi.one([1, 2, 3], pred=lambda x: x > 1, default=0)),

    "argmax_default": lambda: mi.argmax([], default=-1) == -1 and mi.argmax([1, 3, 2], default=-1) == 1
    and raises(ValueError, lambda: mi.argmax([])),
    "argmax_last": lambda: mi.argmax([1, 3, 3, 2], last=True) == 2 and mi.argmax([1, 3, 3, 2]) == 1
    and mi.argmax(['a', 'bb', 'cc'], key=len, last=True) == 2,
    "argmax_both": lambda: mi.argmax([], default=None, last=True) is None
    and mi.argmax([3, 1, 3], default=-1, last=True) == 2,

    "iterate_times": lambda: lst(mi.iterate(lambda x: 2 * x, 1, times=4)) == [1, 2, 4, 8]
    and lst(mi.iterate(lambda x: 2 * x, 1, times=0)) == [],
    "iterate_until": lambda: lst(mi.iterate(lambda x: 2 * x, 1, until=lambda x: x > 10)) == [1, 2, 4, 8]
    and lst(mi.iterate(lambda x: 2 * x, 1, until=lambda x: True)) == [],
    "iterate_both": lambda: lst(mi.iterate(lambda x: x + 1, 0, times=3, until=lambda x: x > 5)) == [0, 1, 2]
    and lst(mi.iterate(lambda x: x + 1, 0, times=10, until=lambda x: x > 2)) == [0, 1, 2],

    "last_pred": lambda: mi.last([1, 2, 3, 4], pred=odd) == 3 and mi.last(iter([1, 2, 3, 4]), pred=odd) == 3
    and mi.last([2, 4], None, pred=odd) is None,
    "last_too_short": lambda: raises(IndexError, lambda: mi.last([], too_short=IndexError))
    and raises(IndexError, lambda: mi.last(iter([]), too_short=IndexError('empty')))
    and mi.last([], 1, too_short=IndexError) == 1,
    "last_both": lambda: raises(KeyError, lambda: mi.last([2, 4], pred=odd, too_short=KeyError))
    and mi.last(iter([1, 2, 3]), pred=odd, too_short=KeyError) == 3,

    "split_into_fillvalue": lambda: lst(mi.split_into([1, 2, 3], [2, 3, 1], fillvalue=0)) == [[1, 2], [3, 0, 0], [0]]
    and lst(mi.split_into([1, 2, 3], [1, None], fillvalue=0)) == [[1], [2, 3]],
    "split_into_rest": lambda: lst(mi.split_into([1, 2, 3, 4, 5], [1, 2], rest=True)) == [[1], [2, 3], [4, 5]]
    and lst(mi.split_into([1, 2, 3], [1, 2], rest=True)) == [[1], [2, 3]],
    "split_into_both": lambda: lst(mi.split_into([1, 2, 3, 4, 5, 6], [2, 3], rest=True, fillvalue=0))
    == [[1, 2], [3, 4, 5], [6]]
    and lst(mi.split_into([1, 2, 3], [2, 3], rest=True, fillvalue=0)) == [[1, 2], [3, 0, 0]],

    "powerset_min_size": lambda: lst(mi.powerset([1, 2, 3], min_size=2)) == [(1, 2), (1, 3), (2, 3), (1, 2, 3)],
    "powerset_max_size": lambda: lst(mi.powerset([1, 2, 3], max_size=1)) == [(), (1,), (2,), (3,)],
    "powerset_both": lambda: lst(mi.powerset([1, 2, 3], min_size=1, max_size=2))
    == [(1,), (2,), (3,), (1, 2), (1, 3), (2, 3)],

    "tabulate_step": lambda: lst(mi.tabulate(lambda x: x * x, 0, step=2), 4) == [0, 4, 16, 36],
    "tabulate_stop": lambda: lst(mi.tabulate(lambda x: x * x, 0, stop=4)) == [0, 1, 4, 9],
    "tabulate_both": lambda: lst(mi.tabulate(lambda x: x, 1, step=3, stop=10)) == [1, 4, 7],

    # ---- singles, more.py
    "interleave_longest_fillvalue": lambda: lst(mi.interleave_longest([1, 2, 3], [4], fillvalue=0)) == [1, 4, 2, 0, 3, 0]
    and lst(mi.interleave_longest([1, 2, 3], [4])) == [1, 4, 2, 3],
    "locate_value": lambda: lst(mi.locate([0, 1, 0, 1], 1)) == [1, 3] and lst(mi.locate([0, 1, 0, 1])) == [1, 3],
    "replace_negative_count": lambda: raises(ValueError, lambda: lst(mi.replace([1, 1], lambda x: x == 1, [9], count=-1)))
    and lst(mi.replace([1, 1], lambda x: x == 1, [9], count=0)) == [1, 1],
    "side_effect_chunk_size": lambda: raises(ValueError, lambda: lst(mi.side_effect(lambda c: None, [1, 2], chunk_size=0)))
    and lst(mi.side_effect(lambda c: None, [1, 2, 3], chunk_size=2)) == [1, 2, 3],
    "collapse_negative_levels": lambda: raises(ValueError, lambda: lst(mi.collapse([[1], [2]], levels=-1)))
    and lst(mi.collapse([[1], [2]], levels=0)) == [[1], [2]],
    "peekable_length_hint": peekable_length_hint,
    "sample_weights_length": lambda: raises(ValueError, lambda: mi.sample([1, 2, 3], 2, weights=[1]))
    and raises(ValueError, lambda: mi.sample([1, 2], 1, weights=[1, 1, 1]))
    and sorted(mi.sample([1, 2, 3], 3, weights=[1, 2, 3])) == [1, 2, 3],
    "distinct_permutations_negative_r": lambda: raises(ValueError, lambda: lst(mi.distinct_permutations([1, 2], -1)))
    and lst(mi.distinct_permutations([1, 2], 0)) == [()],
    "interleave_evenly_negative_lengths": lambda: raises(
        ValueError, lambda: lst(mi.interleave_evenly([[1], [2]], lengths=[-1, 1]))),
    "duplicates_everseen_once": lambda: lst(mi.duplicates_everseen('mississippi', once=True)) == ['s', 'i', 'p']
    and lst(mi.duplicates_everseen('AaaBbbAa', str.lower, once=True)) == ['a', 'b']
    and lst(mi.duplicates_everseen('aaa')) == ['a', 'a'],
    "substrings_reverse": lambda: lst(mi.substrings('abc', reverse=True))
    == [('a', 'b', 'c'), ('a', 'b'), ('b', 'c'), ('a',), ('b',), ('c',)],
    "unique_to_each_key": lambda: [list(x) for x in mi.unique_to_each(['A', 'b'], ['B', 'c'], key=str.lower)]
    == [['A'], ['c']],
    "count_cycle_start": lambda: lst(mi.count_cycle('ab', 2, start=1)) == [(1, 'a'), (1, 'b'), (2, 'a'), (2, 'b')]
    and lst(mi.count_cycle('ab', start=5), 3) == [(5, 'a'), (5, 'b'), (6, 'a')],
    "nth_or_last_negative_n": lambda: raises(ValueError, lambda: mi.nth_or_last([1, 2], -1, 'd'))
    and mi.nth_or_last([1, 2], 5, 'd') == 2,
    "run_length_decode_negative": lambda: raises(ValueError, lambda: lst(mi.run_length.decode([('a', 2), ('b', -1)])))
    and lst(mi.run_length.decode([('a', 0), ('b', 1)])) == ['b'],
    "map_if_pred_none": lambda: lst(mi.map_if([0, 1, 2], None, str)) == [0, '1', '2'],
    "consecutive_groups_step": lambda: [list(g) for g in islice(mi.consecutive_groups([1, 3, 5, 8, 10], step=2), 10)]
    == [[1, 3, 5], [8, 10]]
    and [list(g) for g in islice(mi.consecutive_groups('acez', ord, step=2), 10)] == [['a', 'c', 'e'], ['z']],
    "only_pred": lambda: mi.only([1, 2, 3], pred=lambda x: x > 2) == 3 and mi.only([1, 2], pred=lambda x: x > 5) is None
    and mi.only([1, 2], 0, pred=lambda x: x > 5) == 0
    and raises(ValueError, lambda: mi.only([1, 2, 3], pred=lambda x: x > 1)),
    "stagger_offsets_iterator": lambda: lst(mi.stagger([1, 2, 3], offsets=iter((0, 1)))) == [(1, 2), (2, 3)],
    "zip_offset_offsets_iterator": lambda: lst(mi.zip_offset('abc', 'def', offsets=iter((0, 1))))
    == [('a', 'e'), ('b', 'f')],
    "iequals_key": lambda: mi.iequals('aB', 'Ab', key=str.lower) is True
    and mi.iequals('ab', 'ac', key=str.lower) is False,
    "unzip_fillvalue": lambda: [lst(x) for x in mi.unzip([(1, 2), (3,), (4, 5)], fillvalue=0)] == [[1, 3, 4], [2, 0, 5]],
    "extract_default": lambda: lst(mi.extract('abc', [0, 5, 1], default=None)) == ['a', None, 'b']
    and lst(mi.extract('abc', [0, 5, 6], monotonic=True, default='-')) == ['a', '-', '-'],
    "partitions_k": lambda: [[''.join(p) for p in part] for part in islice(mi.partitions('abc', k=2), 10)]
    == [['a', 'bc'], ['ab', 'c']]
    and lst(mi.partitions('abc', k=4)) == [],

    # ---- singles, recipes.py
    "tail_negative_n": lambda: raises(ValueError, lambda: lst(mi.tail(-1, [1, 2])))
    and raises(ValueError, lambda: lst(mi.tail(-1, iter([1, 2])))) and lst(mi.tail(0, [1])) == [],
    "grouper_validate_n": lambda: raises(ValueError, lambda: lst(mi.grouper('abc', 0)))
    and raises(ValueError, lambda: lst(mi.grouper('abc', -1, incomplete='ignore'))),
    "dotproduct_length_mismatch": lambda: raises(ValueError, lambda: mi.dotproduct([1, 2], [3]))
    and mi.dotproduct([1, 2], [3, 4]) == 11,
    "totient_validate_n": lambda: raises(ValueError, lambda: mi.totient(0))
    and raises(ValueError, lambda: mi.totient(-5)) and mi.totient(1) == 1 and mi.totient(9) == 6,
    "quantify_pred_none": lambda: mi.quantify([0, 1, 2, ''], None) == 2,
    "iter_index_negative_start": lambda: raises(ValueError, lambda: lst(mi.iter_index('abcab', 'a', -2)))
    and raises(ValueError, lambda: lst(mi.iter_index(iter('abcab'), 'a', -2)))
    and raises(ValueError, lambda: lst(mi.iter_index([1, 2, 1], 1, 0, -1))),
}
out = {}
for name, f in CHECKS.items():
    try:
        out[name] = bool(f())
    except Exception as e:  # a crash counts as a failed check
        out[name] = f"error: {type(e).__name__}: {e}"[:200]
print(json.dumps(out))
