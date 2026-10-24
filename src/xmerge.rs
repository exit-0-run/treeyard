//! Three-way line merge that gives the same output as `git merge-file -p --diff-algorithm histogram
//! -L main -L base -L change` (exit 0 = clean, else conflicts): a port of git's xdiff histogram diff
//! (xhistogram.c, with its fallback to Myers) and xdl_merge (xmerge.c) at merge-file's defaults
//! (level XDL_MERGE_ZEALOUS_ALNUM, style "merge", marker size 7, no favour, no whitespace flags).
//! Running it in process saves a `git` process per text merge, the largest cost of a land.
//!
//! Reference: git v2.54.0 xdiff/ (LGPL-2.1, Davide Libenzi, Johannes E. Schindelin; histogram by
//! the JGit and git authors). This is a re-implementation, checked against `git merge-file` by
//! `tests/xmerge_git.rs`.

use crate::xdiff::{File, Hunk, build_script, change_compact, classify, lines, myers_changed};
use std::collections::HashMap;

const MAX_CHAIN_LENGTH: usize = 64;
const MARKER: usize = 7;

/// A diff git cannot do (a bucket chain too long, xhistogram's -1); the caller falls back to git.
#[derive(Debug)]
pub struct Unsupported;

fn hashbits(size: usize) -> u32 {
    let (mut val, mut bits) = (1usize, 0u32);
    while val < size && bits < 32 {
        val <<= 1;
        bits += 1;
    }
    if bits == 0 { 1 } else { bits }
}

fn hashlong(v: usize, b: u32) -> usize {
    let v = v as u64;
    ((v + (v >> b)) & ((1u64 << b) - 1)) as usize
}

struct Rec {
    ptr: usize,
    cnt: usize,
    next: Option<usize>,
}

#[derive(Default, Clone, Copy)]
struct Region {
    begin1: usize,
    end1: usize,
    begin2: usize,
    end2: usize,
}

enum Lcs {
    Found(Region),
    Fallback,
}

/// find_lcs: lines are 1-based like xhistogram.c (`c1[l - 1]` is line `l`).
fn find_lcs(
    c1: &[usize],
    c2: &[usize],
    line1: usize,
    count1: usize,
    line2: usize,
    count2: usize,
) -> Result<Lcs, Unsupported> {
    let bits = hashbits(count1);
    let mut records: Vec<Option<usize>> = vec![None; 1 << bits];
    let mut arena: Vec<Rec> = Vec::with_capacity(count1 / 4 + 1);
    let mut line_map = vec![0usize; count1];
    let mut next_ptrs = vec![0usize; count1];
    let end1 = line1 + count1 - 1;
    let end2 = line2 + count2 - 1;
    // scanA
    let mut ptr = end1;
    while line1 <= ptr {
        let tbl = hashlong(c1[ptr - 1], bits);
        let mut rec = records[tbl];
        let mut chain_len = 0;
        let mut found = false;
        while let Some(r) = rec {
            if c1[arena[r].ptr - 1] == c1[ptr - 1] {
                next_ptrs[ptr - line1] = arena[r].ptr;
                arena[r].ptr = ptr;
                arena[r].cnt += 1;
                line_map[ptr - line1] = r;
                found = true;
                break;
            }
            rec = arena[r].next;
            chain_len += 1;
        }
        if !found {
            if chain_len == MAX_CHAIN_LENGTH {
                return Err(Unsupported);
            }
            arena.push(Rec { ptr, cnt: 1, next: records[tbl] });
            records[tbl] = Some(arena.len() - 1);
            line_map[ptr - line1] = arena.len() - 1;
        }
        if ptr == 0 {
            break;
        }
        ptr -= 1;
    }
    let cnt_of = |l: usize, arena: &[Rec]| arena[line_map[l - line1]].cnt;
    let mut lcs = Region::default();
    let mut index_cnt = MAX_CHAIN_LENGTH + 1;
    let mut has_common = false;
    let mut b_ptr = line2;
    while b_ptr <= end2 {
        // try_lcs
        let mut b_next = b_ptr + 1;
        let mut rec = records[hashlong(c2[b_ptr - 1], bits)];
        while let Some(r) = rec {
            rec = arena[r].next;
            if arena[r].cnt > index_cnt {
                if !has_common {
                    has_common = c1[arena[r].ptr - 1] == c2[b_ptr - 1];
                }
                continue;
            }
            let mut a_s = arena[r].ptr;
            if c1[a_s - 1] != c2[b_ptr - 1] {
                continue;
            }
            has_common = true;
            loop {
                let mut np = next_ptrs[a_s - line1];
                let mut bs = b_ptr;
                let mut ae = a_s;
                let mut be = bs;
                let mut rc = arena[r].cnt;
                while line1 < a_s && line2 < bs && c1[a_s - 2] == c2[bs - 2] {
                    a_s -= 1;
                    bs -= 1;
                    if 1 < rc {
                        rc = rc.min(cnt_of(a_s, &arena));
                    }
                }
                while ae < end1 && be < end2 && c1[ae] == c2[be] {
                    ae += 1;
                    be += 1;
                    if 1 < rc {
                        rc = rc.min(cnt_of(ae, &arena));
                    }
                }
                if b_next <= be {
                    b_next = be + 1;
                }
                if lcs.end1 - lcs.begin1 < ae - a_s || rc < index_cnt {
                    lcs = Region { begin1: a_s, end1: ae, begin2: bs, end2: be };
                    index_cnt = rc;
                }
                if np == 0 {
                    break;
                }
                let mut stop = false;
                while np <= ae {
                    np = next_ptrs[np - line1];
                    if np == 0 {
                        stop = true;
                        break;
                    }
                }
                if stop {
                    break;
                }
                a_s = np;
            }
        }
        b_ptr = b_next;
    }
    Ok(if has_common && MAX_CHAIN_LENGTH < index_cnt { Lcs::Fallback } else { Lcs::Found(lcs) })
}

fn mark(f: &mut File, line: usize, count: usize) {
    for l in line..line + count {
        f.set(l as i64 - 1, true);
    }
}

/// histogram_diff over lines `line1..+count1` of `f1` and `line2..+count2` of `f2` (1-based).
fn histogram(
    f1: &mut File,
    f2: &mut File,
    mut line1: usize,
    mut count1: usize,
    mut line2: usize,
    mut count2: usize,
) -> Result<(), Unsupported> {
    loop {
        if count1 == 0 && count2 == 0 {
            return Ok(());
        }
        if count1 == 0 {
            mark(f2, line2, count2);
            return Ok(());
        }
        if count2 == 0 {
            mark(f1, line1, count1);
            return Ok(());
        }
        match find_lcs(&f1.class, &f2.class, line1, count1, line2, count2)? {
            Lcs::Fallback => {
                let (a, b) = (line1 - 1..line1 - 1 + count1, line2 - 1..line2 - 1 + count2);
                let (ca, cb) = myers_changed(&f1.recs[a.clone()], &f2.recs[b.clone()]);
                f1.changed[a.start + 1..a.end + 1].copy_from_slice(&ca);
                f2.changed[b.start + 1..b.end + 1].copy_from_slice(&cb);
                return Ok(());
            }
            Lcs::Found(lcs) if lcs.begin1 == 0 && lcs.begin2 == 0 => {
                mark(f1, line1, count1);
                mark(f2, line2, count2);
                return Ok(());
            }
            Lcs::Found(lcs) => {
                histogram(f1, f2, line1, lcs.begin1 - line1, line2, lcs.begin2 - line2)?;
                count1 = line1 + count1 - 1 - lcs.end1;
                line1 = lcs.end1 + 1;
                count2 = line2 + count2 - 1 - lcs.end2;
                line2 = lcs.end2 + 1;
            }
        }
    }
}

/// xdl_do_diff with histogram, then xdl_change_compact both ways and xdl_build_script.
fn diff<'a>(a: Vec<&'a [u8]>, b: Vec<&'a [u8]>) -> Result<Vec<Hunk>, Unsupported> {
    let mut classes: HashMap<&[u8], usize> = HashMap::new();
    let mut counts: Vec<[usize; 2]> = Vec::new();
    let ca = classify(&a, &mut classes, &mut counts, 0);
    let cb = classify(&b, &mut classes, &mut counts, 1);
    let (n1, n2) = (a.len(), b.len());
    let mut f1 = File::new(a, ca);
    let mut f2 = File::new(b, cb);
    histogram(&mut f1, &mut f2, 1, n1, 1, n2)?;
    change_compact(&mut f1, &mut f2, false, true);
    change_compact(&mut f2, &mut f1, false, true);
    Ok(build_script(&f1, &f2))
}

/// One entry of xdl_do_merge's list. mode 0 = conflict, 1 = take side 1, 2 = take side 2,
/// 4 = identical after refining (skipped on output).
#[derive(Clone, Copy)]
struct M {
    mode: u8,
    i0: i64,
    chg0: i64,
    i1: i64,
    chg1: i64,
    i2: i64,
    chg2: i64,
}

#[allow(clippy::too_many_arguments)]
fn append(list: &mut Vec<M>, mode: u8, i0: i64, chg0: i64, i1: i64, chg1: i64, i2: i64, chg2: i64) {
    if let Some(m) = list.last_mut()
        && (i1 <= m.i1 + m.chg1 || i2 <= m.i2 + m.chg2)
    {
        if mode != m.mode {
            m.mode = 0;
        }
        m.chg0 = i0 + chg0 - m.i0;
        m.chg1 = i1 + chg1 - m.i1;
        m.chg2 = i2 + chg2 - m.i2;
        return;
    }
    list.push(M { mode, i0, chg0, i1, chg1, i2, chg2 });
}

fn eol_crlf(recs: &[&[u8]], i: usize) -> i32 {
    let n = recs.len();
    let crlf = |r: &[u8]| (r.len() > 1 && r[r.len() - 2] == b'\r') as i32;
    if i + 1 < n {
        return crlf(recs[i]);
    }
    if n == 0 {
        return -1;
    }
    if recs[i].last() == Some(&b'\n') {
        return crlf(recs[i]);
    }
    if i == 0 {
        return -1;
    }
    crlf(recs[i - 1])
}

fn cr_needed(s1: &[&[u8]], s2: &[&[u8]], base: &[&[u8]], m: &M) -> bool {
    let at = |i: i64| if i > 0 { (i - 1) as usize } else { 0 };
    let mut needs = eol_crlf(s1, at(m.i1));
    if needs != 0 {
        needs = eol_crlf(s2, at(m.i2));
    }
    if needs != 0 {
        needs = eol_crlf(base, 0);
    }
    needs > 0
}

fn copy(out: &mut Vec<u8>, recs: &[&[u8]], i: i64, count: i64, needs_cr: bool, add_nl: bool) {
    if count < 1 {
        return;
    }
    let part = &recs[i as usize..(i + count) as usize];
    for r in part {
        out.extend_from_slice(r);
    }
    if add_nl && part.last().is_some_and(|r| r.last() != Some(&b'\n')) {
        if needs_cr {
            out.push(b'\r');
        }
        out.push(b'\n');
    }
}

fn marker(out: &mut Vec<u8>, c: u8, name: Option<&str>, needs_cr: bool) {
    out.extend(std::iter::repeat_n(c, MARKER));
    if let Some(n) = name {
        out.push(b' ');
        out.extend_from_slice(n.as_bytes());
    }
    if needs_cr {
        out.push(b'\r');
    }
    out.push(b'\n');
}

fn alnum(recs: &[&[u8]], i: i64, chg: i64) -> bool {
    (i..i + chg).any(|k| recs[k as usize].iter().any(|b| b.is_ascii_alphanumeric()))
}

/// Merges `ours` and `theirs` (both changed from `base`) like `git merge-file`, with `theirs`
/// (main) as side 1 and `ours` (the change) as side 2. `Ok(Ok(text))` when clean,
/// `Ok(Err(text with markers))` on conflicts.
pub fn merge(base: &str, ours: &str, theirs: &str) -> Result<Result<String, String>, Unsupported> {
    let (b, s1, s2) = (lines(base.as_bytes()), lines(theirs.as_bytes()), lines(ours.as_bytes()));
    let x1 = diff(b.clone(), s1.clone())?;
    let x2 = diff(b.clone(), s2.clone())?;
    if x1.is_empty() {
        return Ok(Ok(ours.to_string()));
    }
    if x2.is_empty() {
        return Ok(Ok(theirs.to_string()));
    }
    // xdl_do_merge. A hunk is (i1 = base start, chg1 = base count, i2 = side start, chg2).
    let h = |x: &Hunk| (x.before.start as i64, x.before.len() as i64, x.after.start as i64, x.after.len() as i64);
    let mut list: Vec<M> = Vec::new();
    let (mut p1, mut p2) = (0usize, 0usize);
    while p1 < x1.len() && p2 < x2.len() {
        let (a_i1, a_c1, a_i2, a_c2) = h(&x1[p1]);
        let (b_i1, b_c1, b_i2, b_c2) = h(&x2[p2]);
        if a_i1 + a_c1 < b_i1 {
            append(&mut list, 1, a_i1, a_c1, a_i2, a_c2, b_i2 - b_i1 + a_i1, a_c1);
            p1 += 1;
            continue;
        }
        if b_i1 + b_c1 < a_i1 {
            append(&mut list, 2, b_i1, b_c1, a_i2 - a_i1 + b_i1, b_c1, b_i2, b_c2);
            p2 += 1;
            continue;
        }
        let same = a_i1 == b_i1
            && a_c1 == b_c1
            && a_c2 == b_c2
            && (0..a_c2).all(|k| s1[(a_i2 + k) as usize] == s2[(b_i2 + k) as usize]);
        if !same {
            let off = a_i1 - b_i1;
            let ffo = off + a_c1 - b_c1;
            let (mut i0, mut i1, mut i2) = (a_i1, a_i2, b_i2);
            if off > 0 {
                i0 -= off;
                i1 -= off;
            } else {
                i2 += off;
            }
            let (mut chg0, mut chg1, mut chg2) = (a_i1 + a_c1 - i0, a_i2 + a_c2 - i1, b_i2 + b_c2 - i2);
            if ffo < 0 {
                chg0 -= ffo;
                chg1 -= ffo;
            } else {
                chg2 += ffo;
            }
            append(&mut list, 0, i0, chg0, i1, chg1, i2, chg2);
        }
        let (e1, e2) = (a_i1 + a_c1, b_i1 + b_c1);
        if e1 >= e2 {
            p2 += 1;
        }
        if e2 >= e1 {
            p1 += 1;
        }
    }
    for x in &x1[p1..] {
        let (i1, c1, i2, c2) = h(x);
        append(&mut list, 1, i1, c1, i2, c2, i1 + s2.len() as i64 - b.len() as i64, c1);
    }
    for x in &x2[p2..] {
        let (i1, c1, i2, c2) = h(x);
        append(&mut list, 2, i1, c1, i1 + s1.len() as i64 - b.len() as i64, c1, i2, c2);
    }
    // xdl_refine_conflicts
    let mut refined: Vec<M> = Vec::with_capacity(list.len());
    for m in list {
        if m.mode != 0 || m.chg1 == 0 || m.chg2 == 0 {
            refined.push(m);
            continue;
        }
        let t1 = s1[m.i1 as usize..(m.i1 + m.chg1) as usize].to_vec();
        let t2 = s2[m.i2 as usize..(m.i2 + m.chg2) as usize].to_vec();
        let script = diff(t1, t2)?;
        if script.is_empty() {
            refined.push(M { mode: 4, ..m });
            continue;
        }
        for x in &script {
            refined.push(M {
                mode: 0,
                i1: x.before.start as i64 + m.i1,
                chg1: x.before.len() as i64,
                i2: x.after.start as i64 + m.i2,
                chg2: x.after.len() as i64,
                ..m
            });
        }
    }
    // xdl_simplify_non_conflicts, with simplify_if_no_alnum (ZEALOUS_ALNUM)
    let mut list = refined;
    let mut k = 0;
    while k + 1 < list.len() {
        let (m, n) = (list[k], list[k + 1]);
        let (begin, end) = (m.i1 + m.chg1, n.i1);
        if m.mode != 0 || n.mode != 0 || (end - begin > 3 && alnum(&s1, begin, end - begin)) {
            k += 1;
        } else {
            list[k].chg1 = n.i1 + n.chg1 - m.i1;
            list[k].chg2 = n.i2 + n.chg2 - m.i2;
            list.remove(k + 1);
        }
    }
    // xdl_fill_merge_buffer
    let mut out = Vec::with_capacity(base.len() + ours.len() + theirs.len());
    let mut i = 0i64;
    let mut conflicts = 0;
    for m in &list {
        if m.mode == 0 {
            conflicts += 1;
            let cr = cr_needed(&s1, &s2, &b, m);
            copy(&mut out, &s1, i, m.i1 - i, false, false);
            marker(&mut out, b'<', Some("main"), cr);
            copy(&mut out, &s1, m.i1, m.chg1, cr, true);
            marker(&mut out, b'=', None, cr);
            copy(&mut out, &s2, m.i2, m.chg2, cr, true);
            marker(&mut out, b'>', Some("change"), cr);
        } else if m.mode & 3 != 0 {
            copy(&mut out, &s1, i, m.i1 - i, false, false);
            if m.mode & 1 != 0 {
                let cr = cr_needed(&s1, &s2, &b, m);
                copy(&mut out, &s1, m.i1, m.chg1, cr, m.mode & 2 != 0);
            }
            if m.mode & 2 != 0 {
                copy(&mut out, &s2, m.i2, m.chg2, false, false);
            }
        } else {
            continue;
        }
        i = m.i1 + m.chg1;
    }
    copy(&mut out, &s1, i, s1.len() as i64 - i, false, false);
    // Every slice ends at a `\n` or at the end of a &str, and markers are ASCII.
    let text = String::from_utf8(out).expect("merge output is UTF-8");
    Ok(if conflicts == 0 { Ok(text) } else { Err(text) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_edits_merge() {
        assert_eq!(
            merge("a\nb\nc\nd\ne\n", "A\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n").unwrap(),
            Ok("A\nb\nc\nd\nE\n".into())
        );
    }

    #[test]
    fn two_appends_conflict() {
        let got = merge("x\n", "x\nchange\n", "x\nmain\n").unwrap();
        assert_eq!(got, Err("x\n<<<<<<< main\nmain\n=======\nchange\n>>>>>>> change\n".into()));
    }
}
