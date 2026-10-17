//! Line diff that gives the same hunks as git: a port of the Myers diff in git's xdiff
//! (xprepare.c: trim ends, discard unmatched and "too common" lines; xdiffi.c: xdl_split with
//! its cost heuristics, xdl_change_compact with the indent heuristic). Blame passes lines to
//! the parent through these hunks, so the same hunks give the same blame as `git blame`.
//!
//! Reference: git v2.54.0 xdiff/ (LGPL-2.1, Davide Libenzi; heuristics by the git authors).
//! This is a re-implementation of the algorithm, checked hunk by hunk against `git diff`.

use std::collections::HashMap;
use std::ops::Range;

const MAX_EQLIMIT: usize = 1024;
const SIMSCAN_WINDOW: i64 = 100;
const KPDIS_RUN: i64 = 4;
const MAX_COST_MIN: i64 = 256;
const HEUR_MIN_COST: i64 = 256;
const SNAKE_CNT: i64 = 20;
const K_HEUR: i64 = 4;
const LINE_MAX: i64 = i64::MAX;

/// One change: lines `before` of the old file became lines `after` of the new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub before: Range<usize>,
    pub after: Range<usize>,
}

/// Lines with their terminating `\n` (the last line may lack it), like xdiff's records.
pub fn lines(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in data.iter().enumerate() {
        if *b == b'\n' {
            out.push(&data[start..=i]);
            start = i + 1;
        }
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

/// git's integer square root approximation (xdl_bogosqrt).
fn bogosqrt(mut n: i64) -> i64 {
    let mut i = 1;
    while n > 0 {
        i <<= 1;
        n >>= 2;
    }
    i
}

struct File<'a> {
    recs: Vec<&'a [u8]>,
    /// Class (equal-content id) per record.
    class: Vec<usize>,
    /// `changed[i + 1]` for record i; entries 0 and nrec + 1 are sentinels (false).
    changed: Vec<bool>,
    /// Records kept for the Myers pass (xdf->reference_index).
    rindex: Vec<usize>,
    dstart: i64,
    dend: i64,
}

impl<'a> File<'a> {
    fn new(recs: Vec<&'a [u8]>, class: Vec<usize>) -> Self {
        let n = recs.len();
        File { recs, class, changed: vec![false; n + 2], rindex: Vec::new(), dstart: 0, dend: n as i64 - 1 }
    }
    fn nrec(&self) -> i64 {
        self.recs.len() as i64
    }
    fn ch(&self, i: i64) -> bool {
        self.changed[(i + 1) as usize]
    }
    fn set(&mut self, i: i64, v: bool) {
        self.changed[(i + 1) as usize] = v;
    }
    fn hash(&self, i: i64) -> usize {
        self.class[self.rindex[i as usize]]
    }
}

const DISCARD: u8 = 0;
const KEEP: u8 = 1;
const INVESTIGATE: u8 = 2;

fn clean_mmatch(action: &[u8], i: i64, mut s: i64, mut e: i64) -> bool {
    if i - s > SIMSCAN_WINDOW {
        s = i - SIMSCAN_WINDOW;
    }
    if e - i > SIMSCAN_WINDOW {
        e = i + SIMSCAN_WINDOW;
    }
    let (mut rdis0, mut rpdis0) = (0i64, 1i64);
    let mut r = 1;
    while i - r >= s {
        match action[(i - r) as usize] {
            DISCARD => rdis0 += 1,
            INVESTIGATE => rpdis0 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis0 == 0 {
        return false;
    }
    let (mut rdis1, mut rpdis1) = (0i64, 1i64);
    r = 1;
    while i + r <= e {
        match action[(i + r) as usize] {
            DISCARD => rdis1 += 1,
            INVESTIGATE => rpdis1 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis1 == 0 {
        return false;
    }
    rdis1 += rdis0;
    rpdis1 += rpdis0;
    rpdis1 * KPDIS_RUN < rpdis1 + rdis1
}

fn cleanup(f: &mut File, counts_other: &[usize]) {
    let mlim = bogosqrt(f.nrec()).min(MAX_EQLIMIT as i64) as usize;
    let mut action = vec![DISCARD; f.recs.len() + 1];
    for i in f.dstart..=f.dend {
        let nm = counts_other[f.class[i as usize]];
        action[i as usize] = if nm == 0 {
            DISCARD
        } else if nm >= mlim {
            INVESTIGATE
        } else {
            KEEP
        };
    }
    for i in f.dstart..=f.dend {
        let a = action[i as usize];
        if a == KEEP || (a == INVESTIGATE && !clean_mmatch(&action, i, f.dstart, f.dend)) {
            f.rindex.push(i as usize);
        } else {
            f.set(i, true);
        }
    }
}

struct Kv {
    v: Vec<i64>,
    off: i64,
}

impl Kv {
    fn get(&self, d: i64) -> i64 {
        self.v[(d + self.off) as usize]
    }
    fn set(&mut self, d: i64, x: i64) {
        self.v[(d + self.off) as usize] = x;
    }
}

struct Split {
    i1: i64,
    i2: i64,
    min_lo: bool,
    min_hi: bool,
}

#[allow(clippy::too_many_arguments)]
fn split(
    a: &File,
    off1: i64,
    lim1: i64,
    b: &File,
    off2: i64,
    lim2: i64,
    kf: &mut Kv,
    kb: &mut Kv,
    need_min: bool,
    mxcost: i64,
) -> Split {
    let (dmin, dmax) = (off1 - lim2, lim1 - off2);
    let (fmid, bmid) = (off1 - off2, lim1 - lim2);
    let odd = (fmid - bmid) & 1 != 0;
    let (mut fmin, mut fmax, mut bmin, mut bmax) = (fmid, fmid, bmid, bmid);
    kf.set(fmid, off1);
    kb.set(bmid, lim1);
    let mut ec = 1i64;
    loop {
        let mut got_snake = false;
        if fmin > dmin {
            fmin -= 1;
            kf.set(fmin - 1, -1);
        } else {
            fmin += 1;
        }
        if fmax < dmax {
            fmax += 1;
            kf.set(fmax + 1, -1);
        } else {
            fmax -= 1;
        }
        let mut d = fmax;
        while d >= fmin {
            let mut i1 = if kf.get(d - 1) >= kf.get(d + 1) { kf.get(d - 1) + 1 } else { kf.get(d + 1) };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 < lim1 && i2 < lim2 && a.hash(i1) == b.hash(i2) {
                i1 += 1;
                i2 += 1;
            }
            if i1 - prev1 > SNAKE_CNT {
                got_snake = true;
            }
            kf.set(d, i1);
            if odd && bmin <= d && d <= bmax && kb.get(d) <= i1 {
                return Split { i1, i2, min_lo: true, min_hi: true };
            }
            d -= 2;
        }
        if bmin > dmin {
            bmin -= 1;
            kb.set(bmin - 1, LINE_MAX);
        } else {
            bmin += 1;
        }
        if bmax < dmax {
            bmax += 1;
            kb.set(bmax + 1, LINE_MAX);
        } else {
            bmax -= 1;
        }
        d = bmax;
        while d >= bmin {
            let mut i1 = if kb.get(d - 1) < kb.get(d + 1) { kb.get(d - 1) } else { kb.get(d + 1) - 1 };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 > off1 && i2 > off2 && a.hash(i1 - 1) == b.hash(i2 - 1) {
                i1 -= 1;
                i2 -= 1;
            }
            if prev1 - i1 > SNAKE_CNT {
                got_snake = true;
            }
            kb.set(d, i1);
            if !odd && fmin <= d && d <= fmax && i1 <= kf.get(d) {
                return Split { i1, i2, min_lo: true, min_hi: true };
            }
            d -= 2;
        }
        if need_min {
            ec += 1;
            continue;
        }
        if got_snake && ec > HEUR_MIN_COST {
            let (mut best, mut s1, mut s2) = (0i64, 0i64, 0i64);
            d = fmax;
            while d >= fmin {
                let dd = if d > fmid { d - fmid } else { fmid - d };
                let i1 = kf.get(d);
                let i2 = i1 - d;
                let v = (i1 - off1) + (i2 - off2) - dd;
                if v > K_HEUR * ec
                    && v > best
                    && off1 + SNAKE_CNT <= i1
                    && i1 < lim1
                    && off2 + SNAKE_CNT <= i2
                    && i2 < lim2
                {
                    let mut k = 1;
                    while a.hash(i1 - k) == b.hash(i2 - k) {
                        if k == SNAKE_CNT {
                            best = v;
                            (s1, s2) = (i1, i2);
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return Split { i1: s1, i2: s2, min_lo: true, min_hi: false };
            }
            best = 0;
            d = bmax;
            while d >= bmin {
                let dd = if d > bmid { d - bmid } else { bmid - d };
                let i1 = kb.get(d);
                let i2 = i1 - d;
                let v = (lim1 - i1) + (lim2 - i2) - dd;
                if v > K_HEUR * ec
                    && v > best
                    && off1 < i1
                    && i1 <= lim1 - SNAKE_CNT
                    && off2 < i2
                    && i2 <= lim2 - SNAKE_CNT
                {
                    let mut k = 0;
                    while a.hash(i1 + k) == b.hash(i2 + k) {
                        if k == SNAKE_CNT - 1 {
                            best = v;
                            (s1, s2) = (i1, i2);
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return Split { i1: s1, i2: s2, min_lo: false, min_hi: true };
            }
        }
        if ec >= mxcost {
            let (mut fbest, mut fbest1) = (-1i64, -1i64);
            d = fmax;
            while d >= fmin {
                let mut i1 = kf.get(d).min(lim1);
                let mut i2 = i1 - d;
                if lim2 < i2 {
                    i1 = lim2 + d;
                    i2 = lim2;
                }
                if fbest < i1 + i2 {
                    fbest = i1 + i2;
                    fbest1 = i1;
                }
                d -= 2;
            }
            let (mut bbest, mut bbest1) = (LINE_MAX, LINE_MAX);
            d = bmax;
            while d >= bmin {
                let mut i1 = off1.max(kb.get(d));
                let mut i2 = i1 - d;
                if i2 < off2 {
                    i1 = off2 + d;
                    i2 = off2;
                }
                if i1 + i2 < bbest {
                    bbest = i1 + i2;
                    bbest1 = i1;
                }
                d -= 2;
            }
            return if (lim1 + lim2) - bbest < fbest - (off1 + off2) {
                Split { i1: fbest1, i2: fbest - fbest1, min_lo: true, min_hi: false }
            } else {
                Split { i1: bbest1, i2: bbest - bbest1, min_lo: false, min_hi: true }
            };
        }
        ec += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn recs_cmp(
    a: &mut File,
    mut off1: i64,
    mut lim1: i64,
    b: &mut File,
    mut off2: i64,
    mut lim2: i64,
    kf: &mut Kv,
    kb: &mut Kv,
    need_min: bool,
    mxcost: i64,
) {
    while off1 < lim1 && off2 < lim2 && a.hash(off1) == b.hash(off2) {
        off1 += 1;
        off2 += 1;
    }
    while off1 < lim1 && off2 < lim2 && a.hash(lim1 - 1) == b.hash(lim2 - 1) {
        lim1 -= 1;
        lim2 -= 1;
    }
    if off1 == lim1 {
        for i in off2..lim2 {
            let r = b.rindex[i as usize] as i64;
            b.set(r, true);
        }
    } else if off2 == lim2 {
        for i in off1..lim1 {
            let r = a.rindex[i as usize] as i64;
            a.set(r, true);
        }
    } else {
        let s = split(a, off1, lim1, b, off2, lim2, kf, kb, need_min, mxcost);
        recs_cmp(a, off1, s.i1, b, off2, s.i2, kf, kb, s.min_lo, mxcost);
        recs_cmp(a, s.i1, lim1, b, s.i2, lim2, kf, kb, s.min_hi, mxcost);
    }
}

// ---- xdl_change_compact with the indent heuristic --------------------------------------

const MAX_INDENT: i32 = 200;
const MAX_BLANKS: i32 = 20;
const START_OF_FILE_PENALTY: i32 = 1;
const END_OF_FILE_PENALTY: i32 = 21;
const TOTAL_BLANK_WEIGHT: i32 = -30;
const POST_BLANK_WEIGHT: i32 = 6;
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
const INDENT_WEIGHT: i32 = 60;
const INDENT_HEURISTIC_MAX_SLIDING: i64 = 100;

fn get_indent(rec: &[u8]) -> i32 {
    let mut ret = 0;
    for &c in rec {
        if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            return ret;
        } else if c == b' ' {
            ret += 1;
        } else if c == b'\t' {
            ret += 8 - ret % 8;
        }
        if ret >= MAX_INDENT {
            return MAX_INDENT;
        }
    }
    -1
}

#[derive(Default)]
struct Measure {
    end_of_file: bool,
    indent: i32,
    pre_blank: i32,
    pre_indent: i32,
    post_blank: i32,
    post_indent: i32,
}

fn measure_split(f: &File, split: i64) -> Measure {
    let mut m = Measure::default();
    if split >= f.nrec() {
        m.end_of_file = true;
        m.indent = -1;
    } else {
        m.indent = get_indent(f.recs[split as usize]);
    }
    m.pre_indent = -1;
    let mut i = split - 1;
    while i >= 0 {
        m.pre_indent = get_indent(f.recs[i as usize]);
        if m.pre_indent != -1 {
            break;
        }
        m.pre_blank += 1;
        if m.pre_blank == MAX_BLANKS {
            m.pre_indent = 0;
            break;
        }
        i -= 1;
    }
    m.post_indent = -1;
    i = split + 1;
    while i < f.nrec() {
        m.post_indent = get_indent(f.recs[i as usize]);
        if m.post_indent != -1 {
            break;
        }
        m.post_blank += 1;
        if m.post_blank == MAX_BLANKS {
            m.post_indent = 0;
            break;
        }
        i += 1;
    }
    m
}

#[derive(Default, Clone, Copy)]
struct Score {
    effective_indent: i32,
    penalty: i32,
}

fn score_add_split(m: &Measure, s: &mut Score) {
    if m.pre_indent == -1 && m.pre_blank == 0 {
        s.penalty += START_OF_FILE_PENALTY;
    }
    if m.end_of_file {
        s.penalty += END_OF_FILE_PENALTY;
    }
    let post_blank = if m.indent == -1 { 1 + m.post_blank } else { 0 };
    let total_blank = m.pre_blank + post_blank;
    s.penalty += TOTAL_BLANK_WEIGHT * total_blank;
    s.penalty += POST_BLANK_WEIGHT * post_blank;
    let indent = if m.indent != -1 { m.indent } else { m.post_indent };
    let any_blanks = total_blank != 0;
    s.effective_indent += indent;
    if indent == -1 || m.pre_indent == -1 {
    } else if indent > m.pre_indent {
        s.penalty += if any_blanks { RELATIVE_INDENT_WITH_BLANK_PENALTY } else { RELATIVE_INDENT_PENALTY };
    } else if indent == m.pre_indent {
    } else if m.post_indent != -1 && m.post_indent > indent {
        s.penalty += if any_blanks { RELATIVE_OUTDENT_WITH_BLANK_PENALTY } else { RELATIVE_OUTDENT_PENALTY };
    } else {
        s.penalty += if any_blanks { RELATIVE_DEDENT_WITH_BLANK_PENALTY } else { RELATIVE_DEDENT_PENALTY };
    }
}

fn score_cmp(s1: &Score, s2: &Score) -> i32 {
    let c = (s1.effective_indent > s2.effective_indent) as i32 - (s1.effective_indent < s2.effective_indent) as i32;
    INDENT_WEIGHT * c + (s1.penalty - s2.penalty)
}

#[derive(Clone, Copy)]
struct Group {
    start: i64,
    end: i64,
}

fn group_init(f: &File) -> Group {
    let mut g = Group { start: 0, end: 0 };
    while f.ch(g.end) {
        g.end += 1;
    }
    g
}

fn group_next(f: &File, g: &mut Group) -> bool {
    if g.end == f.nrec() {
        return false;
    }
    g.start = g.end + 1;
    g.end = g.start;
    while f.ch(g.end) {
        g.end += 1;
    }
    true
}

fn group_previous(f: &File, g: &mut Group) -> bool {
    if g.start == 0 {
        return false;
    }
    g.end = g.start - 1;
    g.start = g.end;
    while f.ch(g.start - 1) {
        g.start -= 1;
    }
    true
}

fn group_slide_down(f: &mut File, g: &mut Group) -> bool {
    if g.end < f.nrec() && f.class[g.start as usize] == f.class[g.end as usize] {
        f.set(g.start, false);
        g.start += 1;
        f.set(g.end, true);
        g.end += 1;
        while f.ch(g.end) {
            g.end += 1;
        }
        true
    } else {
        false
    }
}

fn group_slide_up(f: &mut File, g: &mut Group) -> bool {
    if g.start > 0 && f.class[(g.start - 1) as usize] == f.class[(g.end - 1) as usize] {
        g.start -= 1;
        f.set(g.start, true);
        g.end -= 1;
        f.set(g.end, false);
        while f.ch(g.start - 1) {
            g.start -= 1;
        }
        true
    } else {
        false
    }
}

fn change_compact(f: &mut File, o: &File) {
    let mut g = group_init(f);
    let mut go = group_init(o);
    loop {
        if g.end != g.start {
            let mut groupsize;
            let mut earliest_end;
            let mut end_matching_other;
            loop {
                groupsize = g.end - g.start;
                end_matching_other = -1;
                while group_slide_up(f, &mut g) {
                    group_previous(o, &mut go);
                }
                earliest_end = g.end;
                if go.end > go.start {
                    end_matching_other = g.end;
                }
                loop {
                    if !group_slide_down(f, &mut g) {
                        break;
                    }
                    group_next(o, &mut go);
                    if go.end > go.start {
                        end_matching_other = g.end;
                    }
                }
                if groupsize == g.end - g.start {
                    break;
                }
            }
            if g.end == earliest_end {
                // No shifting is possible.
            } else if end_matching_other != -1 {
                while go.end == go.start {
                    group_slide_up(f, &mut g);
                    group_previous(o, &mut go);
                }
            } else {
                let mut shift = earliest_end;
                if g.end - groupsize - 1 > shift {
                    shift = g.end - groupsize - 1;
                }
                if g.end - INDENT_HEURISTIC_MAX_SLIDING > shift {
                    shift = g.end - INDENT_HEURISTIC_MAX_SLIDING;
                }
                let mut best_shift = -1;
                let mut best = Score::default();
                while shift <= g.end {
                    let mut score = Score::default();
                    score_add_split(&measure_split(f, shift), &mut score);
                    score_add_split(&measure_split(f, shift - groupsize), &mut score);
                    if best_shift == -1 || score_cmp(&score, &best) <= 0 {
                        best = score;
                        best_shift = shift;
                    }
                    shift += 1;
                }
                while g.end > best_shift {
                    group_slide_up(f, &mut g);
                    group_previous(o, &mut go);
                }
            }
        }
        if !group_next(f, &mut g) {
            break;
        }
        group_next(o, &mut go);
    }
}

/// The hunks that turn `a` into `b`, as git finds them with zero context lines (what
/// `git blame` and `git diff -U0` use): Myers with the indent heuristic, after git's
/// `trim_common_tail` (xdiff-interface.c), which drops an equal tail in 1 KiB blocks.
pub fn diff(a: &[u8], b: &[u8]) -> Vec<Hunk> {
    let keep = trim_common_tail(a, b);
    diff_records(&a[..a.len() - keep], &b[..b.len() - keep])
}

/// Bytes of equal tail that git leaves out before the diff (whole 1024-byte blocks, minus
/// the rest of the line the cut falls in).
fn trim_common_tail(a: &[u8], b: &[u8]) -> usize {
    const BLK: usize = 1024;
    let smaller = a.len().min(b.len());
    let mut trimmed = 0;
    while BLK + trimmed <= smaller
        && a[a.len() - trimmed - BLK..a.len() - trimmed] == b[b.len() - trimmed - BLK..b.len() - trimmed]
    {
        trimmed += BLK;
    }
    let ap = &a[a.len() - trimmed..];
    let mut recovered = 0;
    while recovered < trimmed {
        recovered += 1;
        if ap[recovered - 1] == b'\n' {
            break;
        }
    }
    trimmed - recovered
}

fn diff_records(a: &[u8], b: &[u8]) -> Vec<Hunk> {
    let (ra, rb) = (lines(a), lines(b));
    let mut classes: HashMap<&[u8], usize> = HashMap::new();
    let mut counts: Vec<[usize; 2]> = Vec::new();
    let ca = classify(&ra, &mut classes, &mut counts, 0);
    let cb = classify(&rb, &mut classes, &mut counts, 1);
    let count1: Vec<usize> = counts.iter().map(|c| c[0]).collect();
    let count2: Vec<usize> = counts.iter().map(|c| c[1]).collect();
    let mut f1 = File::new(ra, ca);
    let mut f2 = File::new(rb, cb);
    // xdl_trim_ends
    let lim = f1.recs.len().min(f2.recs.len());
    let mut i = 0;
    while i < lim && f1.class[i] == f2.class[i] {
        i += 1;
    }
    f1.dstart = i as i64;
    f2.dstart = i as i64;
    let rest = lim - i;
    let mut j = 0;
    while j < rest && f1.class[f1.recs.len() - 1 - j] == f2.class[f2.recs.len() - 1 - j] {
        j += 1;
    }
    f1.dend = f1.nrec() - j as i64 - 1;
    f2.dend = f2.nrec() - j as i64 - 1;
    cleanup(&mut f1, &count2);
    cleanup(&mut f2, &count1);
    let (n1, n2) = (f1.rindex.len() as i64, f2.rindex.len() as i64);
    let ndiags = n1 + n2 + 3;
    let mut kf = Kv { v: vec![0; ndiags as usize + 1], off: n2 + 1 };
    let mut kb = Kv { v: vec![0; ndiags as usize + 1], off: n2 + 1 };
    let mxcost = bogosqrt(ndiags).max(MAX_COST_MIN);
    recs_cmp(&mut f1, 0, n1, &mut f2, 0, n2, &mut kf, &mut kb, false, mxcost);
    change_compact(&mut f1, &f2);
    change_compact(&mut f2, &f1);
    // xdl_build_script, in forward order
    let mut out = Vec::new();
    let (mut i1, mut i2) = (f1.nrec(), f2.nrec());
    while i1 >= 0 || i2 >= 0 {
        if f1.ch(i1 - 1) || f2.ch(i2 - 1) {
            let (l1, l2) = (i1, i2);
            while f1.ch(i1 - 1) {
                i1 -= 1;
            }
            while f2.ch(i2 - 1) {
                i2 -= 1;
            }
            out.push(Hunk { before: i1 as usize..l1 as usize, after: i2 as usize..l2 as usize });
        }
        i1 -= 1;
        i2 -= 1;
    }
    out.reverse();
    out
}

/// Gives each distinct line a class id and counts its occurrences per side.
fn classify<'a>(
    recs: &[&'a [u8]],
    classes: &mut HashMap<&'a [u8], usize>,
    counts: &mut Vec<[usize; 2]>,
    side: usize,
) -> Vec<usize> {
    recs.iter()
        .map(|r| {
            let n = classes.len();
            let c = *classes.entry(r).or_insert(n);
            if c >= counts.len() {
                counts.push([0, 0]);
            }
            counts[c][side] += 1;
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_edits() {
        assert_eq!(diff(b"a\nb\nc\n", b"a\nB\nc\n"), vec![Hunk { before: 1..2, after: 1..2 }]);
        assert_eq!(diff(b"", b"x\n"), vec![Hunk { before: 0..0, after: 0..1 }]);
        assert_eq!(diff(b"x\n", b"x"), vec![Hunk { before: 0..1, after: 0..1 }]);
        assert!(diff(b"same\n", b"same\n").is_empty());
    }

    #[test]
    fn indent_heuristic_keeps_the_function_whole() {
        // git puts the added function after the blank line, not across the closing brace.
        let a = b"fn a() {\n    1\n}\n\nfn c() {\n    3\n}\n";
        let b = b"fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n\nfn c() {\n    3\n}\n";
        assert_eq!(diff(a, b), vec![Hunk { before: 4..4, after: 4..8 }]);
    }
}
