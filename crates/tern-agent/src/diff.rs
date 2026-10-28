//! Line diffs for the approval card before a file write (#75): the
//! harness computes the diff from the file on disk and the proposed text,
//! so the user sees exactly what changes before anything is saved.

use similar::{ChangeTag, TextDiff};

/// What a diff row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Added,
    Removed,
    /// Unchanged lines left out between two changed parts.
    Gap,
}

/// One row of a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    /// Line number in the old text (1-based).
    pub old: Option<usize>,
    /// Line number in the new text (1-based).
    pub new: Option<usize>,
    pub text: String,
}

/// A diff and its counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub rows: Vec<Row>,
    pub added: usize,
    pub removed: usize,
    /// The file did not exist: every line is new.
    pub new_file: bool,
}

/// The rows that change `old` into `new`, with `context` unchanged lines
/// around each change. `old` is `None` for a new file.
pub fn line_diff(old: Option<&str>, new: &str, context: usize) -> Diff {
    let before = old.unwrap_or("");
    let diff = TextDiff::from_lines(before, new);
    let mut out = Diff {
        new_file: old.is_none(),
        ..Diff::default()
    };
    for (i, group) in diff.grouped_ops(context).iter().enumerate() {
        if i > 0 {
            out.rows.push(Row {
                kind: RowKind::Gap,
                old: None,
                new: None,
                text: String::new(),
            });
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => RowKind::Same,
                    ChangeTag::Insert => {
                        out.added += 1;
                        RowKind::Added
                    }
                    ChangeTag::Delete => {
                        out.removed += 1;
                        RowKind::Removed
                    }
                };
                out.rows.push(Row {
                    kind,
                    old: change.old_index().map(|n| n + 1),
                    new: change.new_index().map(|n| n + 1),
                    text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_counted_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nb\nC\nd\ne\nf\ng\nh\n";
        let d = line_diff(Some(old), new, 1);
        assert_eq!((d.added, d.removed), (2, 1));
        assert!(d.rows.iter().any(|r| r.kind == RowKind::Gap));
        let c = d.rows.iter().find(|r| r.kind == RowKind::Added).unwrap();
        assert_eq!((c.text.as_str(), c.new), ("C", Some(3)));
        let n = line_diff(None, "x\ny\n", 3);
        assert!(n.new_file && n.added == 2 && n.removed == 0);
        assert!(line_diff(Some("same\n"), "same\n", 3).rows.is_empty());
    }
}
