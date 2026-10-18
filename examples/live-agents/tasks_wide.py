"""Wide task set for the live coding-agent demo on more-itertools (base commit 4aeb433).

TASKS = [(name, title, description), ...], 50 entries:
- 10 pairs (20 tasks): two independent options on the SAME function, so the two agents' edits collide
  (chunked, intersperse, lstrip, one, argmax, iterate, last, split_into, powerset, tabulate);
- 30 single tasks, each on its own function (24 in more.py, 6 in recipes.py).
Hidden checks: probes_wide.py (one per task plus `<function>_both` per pair). Reference: wide_ref.patch.
"""

PAIRS = [
    ("chunked_fillvalue", "chunked_as_tuples"),
    ("intersperse_leading", "intersperse_trailing"),
    ("lstrip_value", "lstrip_limit"),
    ("one_pred", "one_default"),
    ("argmax_default", "argmax_last"),
    ("iterate_times", "iterate_until"),
    ("last_pred", "last_too_short"),
    ("split_into_fillvalue", "split_into_rest"),
    ("powerset_min_size", "powerset_max_size"),
    ("tabulate_step", "tabulate_stop"),
]

TASKS = [
    # ------------------------------------------------------------------ pairs (same function)
    ("chunked_fillvalue", "chunked: add fillvalue",
     "Add a keyword-only parameter fillvalue to chunked(iterable, n, strict=False). When it is given, pad the "
     "last list with fillvalue up to length n: list(chunked([1, 2, 3, 4, 5], 3, fillvalue=0)) gives "
     "[[1, 2, 3], [4, 5, 0]]. Full chunks are unchanged, and when fillvalue is not given the behaviour is the "
     "same as today (fillvalue has no effect when n is None)."),
    ("chunked_as_tuples", "chunked: add as_tuples",
     "Add a keyword-only parameter as_tuples=False to chunked(iterable, n, strict=False). With as_tuples=True, "
     "yield tuples instead of lists: list(chunked([1, 2, 3], 2, as_tuples=True)) gives [(1, 2), (3,)]. "
     "The default keeps yielding lists."),

    ("intersperse_leading", "intersperse: add leading",
     "Add a keyword-only parameter leading=False to intersperse(e, iterable, n=1). With leading=True, also "
     "yield e before the first item (or first group of n items): list(intersperse('x', [1, 2], leading=True)) "
     "gives ['x', 1, 'x', 2], and list(intersperse('x', [1, 2, 3], n=2, leading=True)) gives "
     "['x', 1, 2, 'x', 3]. An empty iterable still yields nothing."),
    ("intersperse_trailing", "intersperse: add trailing",
     "Add a keyword-only parameter trailing=False to intersperse(e, iterable, n=1). With trailing=True, also "
     "yield e after the last item: list(intersperse('x', [1, 2], trailing=True)) gives [1, 'x', 2, 'x'], and "
     "list(intersperse('x', [1, 2, 3], n=2, trailing=True)) gives [1, 2, 'x', 3, 'x']. An empty iterable still "
     "yields nothing."),

    ("lstrip_value", "lstrip: accept a value to strip",
     "Let lstrip(iterable, pred) take a value instead of a predicate: when pred is not callable, strip the "
     "leading items equal to it. list(lstrip([0, 0, 1, 0], 0)) gives [1, 0]. A callable pred works as today."),
    ("lstrip_limit", "lstrip: add limit",
     "Add a keyword-only parameter limit=None to lstrip(iterable, pred). When limit is given, strip at most "
     "limit items from the start: list(lstrip([0, 0, 0, 1], lambda x: x == 0, limit=2)) gives [0, 1], and with "
     "limit=5 it gives [1]. limit=None strips as today."),

    ("one_pred", "one: add pred",
     "Add a keyword-only parameter pred=None to one(iterable, too_short=None, too_long=None). When pred is "
     "given, only the items for which pred(item) is true count: one([1, 2, 3], pred=lambda x: x > 2) returns 3; "
     "if two or more items match, raise as for too many items (ValueError or too_long); if none match, raise as "
     "for too few (ValueError or too_short)."),
    ("one_default", "one: add default",
     "Add a keyword-only parameter default to one(iterable, too_short=None, too_long=None), with no default "
     "value of its own. When default is given and the iterable is empty, return default instead of raising: "
     "one([], default=None) returns None and one([7], default=0) returns 7. Too many items still raise: "
     "one([1, 2], default=0) raises ValueError."),

    ("argmax_default", "argmax: add default",
     "Add a keyword-only parameter default to argmax(iterable, *, key=None), with no default value of its own. "
     "When default is given, return it for an empty iterable instead of raising: argmax([], default=-1) returns "
     "-1. Non-empty input is unchanged (argmax([1, 3, 2], default=-1) returns 1), and without default an empty "
     "iterable still raises ValueError."),
    ("argmax_last", "argmax: add last",
     "Add a keyword-only parameter last=False to argmax(iterable, *, key=None). With last=True, return the index "
     "of the last occurrence of the maximum instead of the first: argmax([1, 3, 3, 2], last=True) returns 2 "
     "(argmax([1, 3, 3, 2]) still returns 1), and argmax(['a', 'bb', 'cc'], key=len, last=True) returns 2."),

    ("iterate_times", "iterate: add times",
     "Add a keyword-only parameter times=None to iterate(func, start). When times is given, yield at most that "
     "many items: list(iterate(lambda x: 2 * x, 1, times=4)) gives [1, 2, 4, 8], and times=0 yields nothing. "
     "times=None keeps the iterator infinite."),
    ("iterate_until", "iterate: add until",
     "Add a keyword-only parameter until=None to iterate(func, start). When until is given, stop before the "
     "first value v for which until(v) is true: list(iterate(lambda x: 2 * x, 1, until=lambda x: x > 10)) gives "
     "[1, 2, 4, 8]; if until(start) is true, nothing is yielded."),

    ("last_pred", "last: add pred",
     "Add a keyword-only parameter pred=None to last(iterable, default=...). When pred is given, return the last "
     "item for which pred(item) is true, for sequences and plain iterators alike: last([1, 2, 3, 4], "
     "pred=lambda x: x % 2) returns 3. If no item matches, return default or raise ValueError as for an empty "
     "iterable today."),
    ("last_too_short", "last: add too_short",
     "Add a keyword-only parameter too_short=None to last(iterable, default=...), like one(): when there is no "
     "item to return and no default is given, raise too_short (an exception class or instance) instead of "
     "ValueError. last([], too_short=IndexError) raises IndexError; last([], 1, too_short=IndexError) returns 1."),

    ("split_into_fillvalue", "split_into: add fillvalue",
     "Add a keyword-only parameter fillvalue to split_into(iterable, sizes), with no default value of its own. "
     "When it is given, a list that comes out shorter than its size because the iterable ran out is padded with "
     "fillvalue: list(split_into([1, 2, 3], [2, 3, 1], fillvalue=0)) gives [[1, 2], [3, 0, 0], [0]]. A None "
     "size (take the rest) is never padded: list(split_into([1, 2, 3], [1, None], fillvalue=0)) gives "
     "[[1], [2, 3]]."),
    ("split_into_rest", "split_into: add rest",
     "Add a keyword-only parameter rest=False to split_into(iterable, sizes). With rest=True, items left over "
     "after the last size are yielded as one more list: list(split_into([1, 2, 3, 4, 5], [1, 2], rest=True)) "
     "gives [[1], [2, 3], [4, 5]]. When nothing is left over, no extra list is yielded: "
     "list(split_into([1, 2, 3], [1, 2], rest=True)) gives [[1], [2, 3]]."),

    ("powerset_min_size", "powerset: add min_size",
     "Add a keyword-only parameter min_size=0 to the powerset(iterable) recipe in recipes.py: yield only the "
     "subsets with at least min_size items, in the usual order. list(powerset([1, 2, 3], min_size=2)) gives "
     "[(1, 2), (1, 3), (2, 3), (1, 2, 3)]."),
    ("powerset_max_size", "powerset: add max_size",
     "Add a keyword-only parameter max_size=None to the powerset(iterable) recipe in recipes.py: yield only the "
     "subsets with at most max_size items, in the usual order. list(powerset([1, 2, 3], max_size=1)) gives "
     "[(), (1,), (2,), (3,)]."),

    ("tabulate_step", "tabulate: add step",
     "Add a keyword-only parameter step=1 to the tabulate(function, start=0) recipe in recipes.py: the argument "
     "advances by step instead of 1. take(4, tabulate(lambda x: x * x, 0, step=2)) gives [0, 4, 16, 36]."),
    ("tabulate_stop", "tabulate: add stop",
     "Add a keyword-only parameter stop=None to the tabulate(function, start=0) recipe in recipes.py: when stop "
     "is given, the iterator ends before the argument reaches stop (exclusive, like range): "
     "list(tabulate(lambda x: x * x, 0, stop=4)) gives [0, 1, 4, 9]. stop=None keeps it infinite."),

    # ------------------------------------------------------------------ singles, more.py
    ("interleave_longest_fillvalue", "interleave_longest: add fillvalue",
     "Add a keyword-only parameter fillvalue to interleave_longest(*iterables), with no default value of its "
     "own. When it is given, yield fillvalue in place of the items of exhausted iterables instead of skipping "
     "them: list(interleave_longest([1, 2, 3], [4], fillvalue=0)) gives [1, 4, 2, 0, 3, 0]. Without fillvalue "
     "the result is unchanged ([1, 4, 2, 3])."),
    ("locate_value", "locate: accept a value",
     "Let locate(iterable, pred=bool, window_size=None) take a value instead of a predicate: when pred is not "
     "callable and window_size is not given, yield the indexes of the items equal to pred. "
     "list(locate([0, 1, 0, 1], 1)) gives [1, 3]. Callable predicates and the default work as today."),
    ("replace_negative_count", "replace: a negative count is accepted",
     "replace(iterable, pred, substitutes, count=None, window_size=1) silently treats a negative count like 0 "
     "and replaces nothing. A negative count must raise ValueError (when the result is iterated or earlier); "
     "count=0 still means no replacements: list(replace([1, 1], lambda x: x == 1, [9], count=0)) gives [1, 1]."),
    ("side_effect_chunk_size", "side_effect: chunk_size=0 drops every item",
     "side_effect(func, iterable, chunk_size=0) yields nothing at all and never calls func, so every item is "
     "silently lost (a negative chunk_size fails with a confusing error). chunk_size must raise ValueError when "
     "it is less than 1; chunk_size=None and positive sizes work as today."),
    ("collapse_negative_levels", "collapse: a negative levels is accepted",
     "collapse(iterable, levels=-1) does not fail but wraps the input in an extra list: "
     "list(collapse([[1], [2]], levels=-1)) gives [[[1], [2]]]. A negative levels must raise ValueError; "
     "levels=0 still yields the top-level items unchanged ([[1], [2]])."),
    ("peekable_length_hint", "peekable: add __length_hint__",
     "Give peekable a __length_hint__ method, so operator.length_hint works on it: it returns the number of "
     "cached items plus the length hint of the underlying iterator. For p = peekable([1, 2, 3]): after p.peek(), "
     "length_hint(p) is 3; after next(p) it is 2; after p.prepend(0) it is 3. Today it is always 0."),
    ("sample_weights_length", "sample: weights of the wrong length are accepted",
     "sample(iterable, k, weights=...) silently stops at the shorter of iterable and weights: "
     "sample([1, 2, 3], 2, weights=[1]) returns [1]. When weights has a different number of entries than "
     "iterable has items (more or fewer), raise ValueError. Matching lengths work as today."),
    ("distinct_permutations_negative_r", "distinct_permutations: a negative r yields nothing",
     "distinct_permutations([1, 2], -1) silently yields nothing, while derangements raises for a negative r. "
     "A negative r must raise ValueError; r=0 still yields one empty tuple ([()])."),
    ("interleave_evenly_negative_lengths", "interleave_evenly: negative lengths are accepted",
     "interleave_evenly(iterables, lengths=[-1, 1]) silently yields nothing. When any given length is negative, "
     "raise ValueError (when the result is iterated or earlier)."),
    ("duplicates_everseen_once", "duplicates_everseen: add once",
     "Add a keyword-only parameter once=False to duplicates_everseen(iterable, key=None). With once=True, each "
     "duplicated element is yielded only once, at its first repeat: list(duplicates_everseen('mississippi', "
     "once=True)) gives ['s', 'i', 'p'], and list(duplicates_everseen('AaaBbbAa', str.lower, once=True)) gives "
     "['a', 'b']. The default is unchanged."),
    ("substrings_reverse", "substrings: add reverse",
     "Add a keyword-only parameter reverse=False to substrings(iterable). With reverse=True, yield the longest "
     "substrings first; substrings of the same length still come left to right: "
     "list(substrings('abc', reverse=True)) gives [('a', 'b', 'c'), ('a', 'b'), ('b', 'c'), ('a',), ('b',), "
     "('c',)]."),
    ("unique_to_each_key", "unique_to_each: add key",
     "Add a keyword-only parameter key=None to unique_to_each(*iterables). When key is given, elements are "
     "compared by key(element), and the original elements are returned: "
     "unique_to_each(['A', 'b'], ['B', 'c'], key=str.lower) returns [['A'], ['c']]."),
    ("count_cycle_start", "count_cycle: add start",
     "Add a keyword-only parameter start=0 to count_cycle(iterable, n=None): the cycle counter starts at start "
     "instead of 0, and n is still the number of cycles. list(count_cycle('ab', 2, start=1)) gives "
     "[(1, 'a'), (1, 'b'), (2, 'a'), (2, 'b')]; without n, count_cycle('ab', start=5) begins "
     "(5, 'a'), (5, 'b'), (6, 'a')."),
    ("nth_or_last_negative_n", "nth_or_last: a negative n returns the default",
     "nth_or_last([1, 2], -1, 'd') returns 'd' as if the iterable were empty (and other negative n fail with a "
     "confusing islice error). A negative n must raise ValueError, with or without a default; non-negative n "
     "works as today (nth_or_last([1, 2], 5, 'd') returns 2)."),
    ("run_length_decode_negative", "run_length.decode: negative counts are accepted",
     "run_length.decode([('a', 2), ('b', -1)]) silently drops the pair with the negative count. A negative "
     "count must raise ValueError when the result is iterated; a count of 0 still yields nothing for that item "
     "(list(run_length.decode([('a', 0), ('b', 1)])) gives ['b'])."),
    ("map_if_pred_none", "map_if: accept pred=None",
     "Let map_if(iterable, pred, func, func_else=None) take pred=None to mean bool, like partition and "
     "filter do: list(map_if([0, 1, 2], None, str)) gives [0, '1', '2']. Today it raises TypeError."),
    ("consecutive_groups_step", "consecutive_groups: add step",
     "Add a keyword-only parameter step=1 to consecutive_groups(iterable, ordering=None): two items are "
     "consecutive when their positions differ by step. [list(g) for g in consecutive_groups([1, 3, 5, 8, 10], "
     "step=2)] gives [[1, 3, 5], [8, 10]], and with ordering=ord, consecutive_groups('acez', ord, step=2) gives "
     "[['a', 'c', 'e'], ['z']]. step=1 is today's behaviour."),
    ("only_pred", "only: add pred",
     "Add a keyword-only parameter pred=None to only(iterable, default=None, too_long=None). When pred is "
     "given, only the items for which pred(item) is true count: only([1, 2, 3], pred=lambda x: x > 2) returns 3; "
     "if none match, return default (only([1, 2], 0, pred=lambda x: x > 5) returns 0); if two or more match, "
     "raise as for too many items."),
    ("stagger_offsets_iterator", "stagger: offsets cannot be an iterator",
     "stagger(iterable, offsets=iter((0, 1))) raises TypeError because it calls len() on offsets. Accept any "
     "iterable of offsets: list(stagger([1, 2, 3], offsets=iter((0, 1)))) gives [(1, 2), (2, 3)]."),
    ("zip_offset_offsets_iterator", "zip_offset: offsets cannot be an iterator",
     "zip_offset(*iterables, offsets=iter((0, 1))) raises TypeError because it calls len() on offsets. Accept "
     "any iterable of offsets: list(zip_offset('abc', 'def', offsets=iter((0, 1)))) gives [('a', 'e'), "
     "('b', 'f')]."),
    ("iequals_key", "iequals: add key",
     "Add a keyword-only parameter key=None to iequals(*iterables). When key is given, elements are compared by "
     "key(element): iequals('aB', 'Ab', key=str.lower) is True, and iequals('ab', 'ac', key=str.lower) is "
     "False."),
    ("unzip_fillvalue", "unzip: add fillvalue",
     "Add a keyword-only parameter fillvalue to unzip(iterable), with no default value of its own. Today an "
     "element that is too short ends the affected output iterables early. When fillvalue is given, a missing "
     "position contributes fillvalue instead (the number of outputs still comes from the first element): "
     "[list(x) for x in unzip([(1, 2), (3,), (4, 5)], fillvalue=0)] gives [[1, 3, 4], [2, 0, 5]]."),
    ("extract_default", "extract: add default",
     "Add a keyword-only parameter default to extract(iterable, indices, *, monotonic=False), with no default "
     "value of its own. When it is given, an index beyond the end of the iterable yields default instead of "
     "raising IndexError, in both modes: list(extract('abc', [0, 5, 1], default=None)) gives ['a', None, 'b'], "
     "and list(extract('abc', [0, 5, 6], monotonic=True, default='-')) gives ['a', '-', '-']."),
    ("partitions_k", "partitions: add k",
     "Add a keyword-only parameter k=None to partitions(iterable). When k is given, yield only the "
     "order-preserving partitions into exactly k parts, in the usual order: partitions('abc', k=2) yields "
     "[['a'], ['b', 'c']] and [['a', 'b'], ['c']]; k larger than the number of items yields nothing."),

    # ------------------------------------------------------------------ singles, recipes.py
    ("tail_negative_n", "tail: a negative n behaves differently for lists and iterators",
     "tail(-1, [1, 2]) returns an empty iterator, while tail(-1, iter([1, 2])) raises ValueError from deque. A "
     "negative n must raise ValueError for every kind of iterable; tail(0, ...) still yields nothing."),
    ("grouper_validate_n", "grouper: n=0 yields nothing",
     "grouper('abc', 0) silently yields nothing, and so does a negative n. n must be at least 1: raise "
     "ValueError for n < 1, whatever incomplete is."),
    ("dotproduct_length_mismatch", "dotproduct: vectors of different lengths are accepted",
     "dotproduct([1, 2], [3]) returns 3 by silently ignoring the extra item, unlike math.sumprod, which raises. "
     "Raise ValueError when the two iterables have different lengths; dotproduct([1, 2], [3, 4]) still "
     "returns 11."),
    ("totient_validate_n", "totient: non-positive n gives nonsense",
     "totient(0) returns 0 and totient(-5) returns -5, but Euler's totient is only defined for n >= 1. Raise "
     "ValueError for n < 1; totient(1) is still 1 and totient(9) is still 6."),
    ("quantify_pred_none", "quantify: accept pred=None",
     "Let quantify(iterable, pred=bool) take pred=None to mean bool, like partition does: "
     "quantify([0, 1, 2, ''], None) returns 2. Today it raises TypeError."),
    ("iter_index_negative_start", "iter_index: a negative start or stop behaves differently by input type",
     "iter_index('abcab', 'a', -2) quietly counts the start from the end of a string or list, while "
     "iter_index(iter('abcab'), 'a', -2) raises an islice error, and a negative stop on a list "
     "(iter_index([1, 2, 1], 1, 0, -1)) is also accepted. A negative start or stop must raise ValueError for "
     "every kind of iterable (when the result is iterated or earlier)."),
]

assert len(TASKS) == 50 and len({t[0] for t in TASKS}) == 50
assert all(a in {t[0] for t in TASKS} and b in {t[0] for t in TASKS} for a, b in PAIRS)
