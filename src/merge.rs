//! Three-way merge of one path: entry level first, then line-level text merge.

use anyhow::Result;

use crate::model::{ConflictKind, Entry, MODE_GITLINK, MODE_LINK};

/// Blob access the merge needs.
pub trait Blobs {
    fn read_blob(&mut self, id: &str) -> Result<Vec<u8>>;
    fn write_blob(&mut self, data: &[u8]) -> Result<String>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Merged {
    Clean(Option<Entry>),
    /// `materialized` is what the change holds at the path until it is resolved: the text with
    /// conflict markers for content conflicts, otherwise our side.
    Conflict {
        kind: ConflictKind,
        materialized: Option<Entry>,
    },
}

fn text(data: &[u8]) -> Option<&str> {
    if data.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    std::str::from_utf8(data).ok()
}

fn merge_mode(base: Option<u32>, ours: u32, theirs: u32) -> Option<u32> {
    if ours == theirs || base == Some(theirs) {
        Some(ours)
    } else if base == Some(ours) {
        Some(theirs)
    } else {
        None
    }
}

pub fn merge3(
    blobs: &mut impl Blobs,
    base: Option<&Entry>,
    ours: Option<&Entry>,
    theirs: Option<&Entry>,
) -> Result<Merged> {
    if ours == theirs || base == theirs {
        return Ok(Merged::Clean(ours.cloned()));
    }
    if base == ours {
        return Ok(Merged::Clean(theirs.cloned()));
    }
    let (o, t) = match (ours, theirs) {
        (Some(o), Some(t)) => (o, t),
        // One side deleted, the other changed it.
        _ => {
            return Ok(Merged::Conflict { kind: ConflictKind::ModifyDelete, materialized: ours.cloned() });
        }
    };
    let ours_conflict = |kind| Merged::Conflict { kind, materialized: Some(o.clone()) };
    let Some(mode) = merge_mode(base.map(|b| b.mode), o.mode, t.mode) else {
        return Ok(ours_conflict(ConflictKind::Mode));
    };
    let blob = if o.blob == t.blob || base.is_some_and(|b| b.blob == t.blob) {
        o.blob.clone()
    } else if base.is_some_and(|b| b.blob == o.blob) {
        t.blob.clone()
    } else {
        let special = |e: &Entry| e.mode == MODE_LINK || e.mode == MODE_GITLINK;
        if special(o) || special(t) {
            return Ok(ours_conflict(ConflictKind::Binary));
        }
        let base_data = match base {
            Some(b) if !special(b) => blobs.read_blob(&b.blob)?,
            _ => Vec::new(),
        };
        let (od, td) = (blobs.read_blob(&o.blob)?, blobs.read_blob(&t.blob)?);
        let (Some(bt), Some(ot), Some(tt)) = (text(&base_data), text(&od), text(&td)) else {
            return Ok(ours_conflict(ConflictKind::Binary));
        };
        match diffy::merge(bt, ot, tt) {
            Ok(merged) => blobs.write_blob(merged.as_bytes())?,
            Err(with_markers) => {
                let kind = if base.is_some() { ConflictKind::Content } else { ConflictKind::AddAdd };
                let blob = blobs.write_blob(with_markers.as_bytes())?;
                return Ok(Merged::Conflict { kind, materialized: Some(Entry { blob, mode }) });
            }
        }
    };
    Ok(Merged::Clean(Some(Entry { blob, mode })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::object_id;
    use crate::model::{MODE_EXEC, MODE_FILE};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem(HashMap<String, Vec<u8>>);

    impl Blobs for Mem {
        fn read_blob(&mut self, id: &str) -> Result<Vec<u8>> {
            Ok(self.0[id].clone())
        }
        fn write_blob(&mut self, data: &[u8]) -> Result<String> {
            let id = object_id("blob", data);
            self.0.insert(id.clone(), data.to_vec());
            Ok(id)
        }
    }

    fn file(m: &mut Mem, s: &str) -> Entry {
        Entry { blob: m.write_blob(s.as_bytes()).unwrap(), mode: MODE_FILE }
    }

    #[test]
    fn disjoint_line_edits_merge() {
        let mut m = Mem::default();
        let b = file(&mut m, "a\nb\nc\nd\ne\n");
        let o = file(&mut m, "A\nb\nc\nd\ne\n");
        let t = file(&mut m, "a\nb\nc\nd\nE\n");
        let Merged::Clean(Some(r)) = merge3(&mut m, Some(&b), Some(&o), Some(&t)).unwrap() else {
            panic!("expected clean merge")
        };
        assert_eq!(m.0[&r.blob], b"A\nb\nc\nd\nE\n");
    }

    #[test]
    fn same_line_edits_conflict_with_markers() {
        let mut m = Mem::default();
        let b = file(&mut m, "a\nb\nc\n");
        let o = file(&mut m, "a\nours\nc\n");
        let t = file(&mut m, "a\ntheirs\nc\n");
        match merge3(&mut m, Some(&b), Some(&o), Some(&t)).unwrap() {
            Merged::Conflict { kind: ConflictKind::Content, materialized: Some(e) } => {
                let s = String::from_utf8(m.0[&e.blob].clone()).unwrap();
                assert!(s.contains("<<<<<<<") && s.contains("ours") && s.contains("theirs"));
            }
            other => panic!("expected content conflict, got {other:?}"),
        }
    }

    #[test]
    fn delete_against_edit_conflicts() {
        let mut m = Mem::default();
        let b = file(&mut m, "a\n");
        let t = file(&mut m, "b\n");
        let r = merge3(&mut m, Some(&b), None, Some(&t)).unwrap();
        assert!(matches!(r, Merged::Conflict { kind: ConflictKind::ModifyDelete, .. }));
    }

    #[test]
    fn mode_change_and_content_change_combine() {
        let mut m = Mem::default();
        let b = file(&mut m, "a\nb\n");
        let o = Entry { mode: MODE_EXEC, ..b.clone() };
        let t = file(&mut m, "a\nB\n");
        let r = merge3(&mut m, Some(&b), Some(&o), Some(&t)).unwrap();
        assert_eq!(r, Merged::Clean(Some(Entry { blob: t.blob.clone(), mode: MODE_EXEC })));
    }
}
