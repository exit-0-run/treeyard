//! Conflicts as regions: the parts of a conflicted file that need a decision, instead of the
//! three whole files. A conflicted path of a change holds the merge with markers
//! (`<<<<<<< main`, `=======`, `>>>>>>> change`); each region is one marker block of that text,
//! with its line numbers and a few lines of context, so an agent can resolve it with `replace`
//! (old = the region's `text`) without reading the file.

use serde_json::{Value, json};

/// Lines of context before and after a region.
pub const CONTEXT: usize = 3;
/// Lines of one region's `text`; a longer region is cut (`cut: true`).
pub const REGION_LINES: usize = 80;
/// Regions listed per path; the rest are counted in `regions_more`.
pub const REGIONS_MAX: usize = 12;
/// Bytes of region text (text plus context) in one reply, over all conflicts of the change.
pub const BUDGET: usize = 48 * 1024;
/// A side of a conflict without regions (delete, mode, binary) is sent inline up to this size.
pub const SMALL_SIDE: usize = 4 * 1024;

/// One marker block: 0-based first and last line (inclusive) of `<<<<<<<` .. `>>>>>>>`.
#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub first: usize,
    pub last: usize,
}

fn is_marker(line: &str, c: char) -> bool {
    let l = line.trim_end_matches(['\n', '\r']);
    l.len() >= 7 && l.chars().take(7).all(|x| x == c) && (l.len() == 7 || l.as_bytes()[7] == b' ')
}

/// Marker blocks of `lines` (each line with its end of line), in order. An unclosed block is
/// not a block.
pub fn blocks(lines: &[&str]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_marker(lines[i], '<') {
            let mut sep = false;
            let mut j = i + 1;
            while j < lines.len() {
                if is_marker(lines[j], '=') {
                    sep = true;
                } else if (is_marker(lines[j], '>') && sep) || is_marker(lines[j], '<') {
                    break;
                }
                j += 1;
            }
            if j < lines.len() && is_marker(lines[j], '>') {
                out.push(Block { first: i, last: j });
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Byte budget shared by the conflicts of one reply.
pub struct Budget {
    pub left: usize,
    pub cut: bool,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { left: BUDGET, cut: false }
    }
}

/// The regions of a conflicted text with markers, as JSON, and how many were left out.
pub fn regions(text: &str, budget: &mut Budget) -> (Vec<Value>, usize) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let all = blocks(&lines);
    let mut out = Vec::new();
    let mut more = 0;
    for b in &all {
        if out.len() >= REGIONS_MAX {
            more += 1;
            budget.cut = true;
            continue;
        }
        let n = b.last - b.first + 1;
        let shown = n.min(REGION_LINES);
        let body: String = lines[b.first..b.first + shown].concat();
        let before: String = lines[b.first.saturating_sub(CONTEXT)..b.first].concat();
        let after: String = lines[b.last + 1..(b.last + 1 + CONTEXT).min(lines.len())].concat();
        let size = body.len() + before.len() + after.len();
        if size > budget.left {
            budget.cut = true;
            more += 1;
            continue;
        }
        budget.left -= size;
        let mut r = json!({"lines": [b.first + 1, b.last + 1], "text": body, "before": before, "after": after});
        if shown < n {
            r["cut"] = json!(true);
            budget.cut = true;
        }
        out.push(r);
    }
    (out, more)
}

/// Text of one side when it is small enough to send without a request.
pub fn small(text: Option<String>, full: bool, limit: usize) -> Value {
    match text {
        Some(t) if full || t.len() <= limit => Value::String(t),
        _ => Value::Null,
    }
}

/// How to resolve, once per reply that has conflicts.
pub fn help(change: &str) -> String {
    format!(
        "Each conflict lists regions: marker blocks of the path as your change holds it now (read with at = {change} \
         gives the same lines; `<<<<<<< main` .. `=======` is main's side, `=======` .. `>>>>>>> change` yours). \
         Resolve a path in one apply: one replace per region with old = the region's text (cut regions: read their \
         lines first), or the whole content; or resolve with take = ours | theirs | base | delete. Then land again. \
         change with full = true returns the three whole files."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "a\nb\nc\nd\n<<<<<<< main\nm\n=======\no\n>>>>>>> change\ne\nf\ng\nh\n";

    #[test]
    fn one_region_with_context() {
        let mut b = Budget::default();
        let (r, more) = regions(T, &mut b);
        assert_eq!(more, 0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["lines"], json!([5, 9]));
        assert_eq!(r[0]["text"], "<<<<<<< main\nm\n=======\no\n>>>>>>> change\n");
        assert_eq!(r[0]["before"], "b\nc\nd\n");
        assert_eq!(r[0]["after"], "e\nf\ng\n");
        // Replacing the region's text resolves the file.
        assert_eq!(T.replacen(r[0]["text"].as_str().unwrap(), "x\n", 1), "a\nb\nc\nd\nx\ne\nf\ng\nh\n");
    }

    #[test]
    fn regions_past_the_budget_are_counted_not_sent() {
        let big: String = (0..40).map(|i| format!("<<<<<<< main\n{i}\n=======\n-{i}\n>>>>>>> change\nk\n")).collect();
        let mut b = Budget { left: 200, cut: false };
        let (r, more) = regions(&big, &mut b);
        assert!(b.cut && !r.is_empty() && r.len() + more == 40, "{} + {more}", r.len());
    }

    #[test]
    fn long_region_is_cut_and_says_so() {
        let mut t = String::from("<<<<<<< main\n");
        for i in 0..200 {
            t.push_str(&format!("{i}\n"));
        }
        t.push_str("=======\n>>>>>>> change\n");
        let (r, _) = regions(&t, &mut Budget::default());
        assert_eq!(r[0]["cut"], true);
        assert_eq!(r[0]["lines"], json!([1, 203]));
        assert_eq!(r[0]["text"].as_str().unwrap().lines().count(), REGION_LINES);
    }

    #[test]
    fn unclosed_or_crlf_markers() {
        let lines: Vec<&str> =
            "<<<<<<< main\r\nx\r\n=======\r\ny\r\n>>>>>>> change\r\n<<<<<<< main\nz\n".split_inclusive('\n').collect();
        assert_eq!(blocks(&lines), vec![Block { first: 0, last: 4 }]);
    }
}
