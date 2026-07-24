//! Minimal byte-range diff for text-edit ops.
//!
//! The code-editor's commit path used to emit a whole-file
//! `ModelicaOp::ReplaceSource` on every debounced flush — coarse undo,
//! and incompatible with future CRDT text editing where structural ops
//! and text edits need to commute by byte range.
//!
//! `diff_to_edit` returns the smallest single splice that transforms
//! `old` into `new`: find the common prefix, find the common suffix,
//! the difference is one [`std::ops::Range<usize>`] + replacement
//! string. Encoded as a `ModelicaOp::EditText`, this gives:
//!
//! - **Finer undo granularity** — each commit is one byte-range edit,
//!   not a whole-file replace.
//! - **Comment & formatting preservation** — bytes outside the diff
//!   region are byte-identical across the edit.
//! - **CRDT-friendly** — non-overlapping range edits commute.
//!
//! ## Limitation: single-region only
//!
//! For *scattered* edits (e.g. a find/replace across the whole file),
//! the diff degenerates to one big edit covering the full changed
//! span — correct, just less granular than a real LCS-based multi-hunk
//! diff. For typical typing patterns where the user edits one place
//! at a time, single-region matches the natural granularity exactly.
//!
//! A future LCS / Myers implementation can be a drop-in upgrade behind
//! the same return shape (or a `Vec<(Range, String)>` extension).

use std::ops::Range;

/// Compute the smallest single-region byte splice that transforms
/// `old` into `new`. Returns `None` for a no-op (`old == new`).
///
/// Always returns char-boundary-aligned ranges so the resulting
/// `EditText` op is safe for `String::replace_range` / `&str` slicing.
///
/// Cost: O(n) where n is min(old.len(), new.len()).
pub fn diff_to_edit(old: &str, new: &str) -> Option<(Range<usize>, String)> {
    if old == new {
        return None;
    }
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();

    // Common prefix (in bytes).
    let mut prefix = 0;
    let prefix_max = old_bytes.len().min(new_bytes.len());
    while prefix < prefix_max && old_bytes[prefix] == new_bytes[prefix] {
        prefix += 1;
    }

    // Common suffix (in bytes), in the trailing portions after prefix.
    let mut suffix = 0;
    let max_suffix = (old_bytes.len() - prefix).min(new_bytes.len() - prefix);
    while suffix < max_suffix
        && old_bytes[old_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }

    // Back off to char boundaries on both sides. UTF-8 multi-byte
    // sequences must not be split or the resulting splice is invalid
    // UTF-8.
    while prefix > 0 && (!old.is_char_boundary(prefix) || !new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    while suffix > 0 {
        let old_pos = old_bytes.len() - suffix;
        let new_pos = new_bytes.len() - suffix;
        if old.is_char_boundary(old_pos) && new.is_char_boundary(new_pos) {
            break;
        }
        suffix -= 1;
    }

    // Guard against prefix + suffix overlapping after char-boundary
    // back-off (can happen if the full string is one multi-byte char
    // changing). Clamp prefix so the slice stays well-formed.
    let old_end = old_bytes.len().saturating_sub(suffix);
    let new_end = new_bytes.len().saturating_sub(suffix);
    let prefix = prefix.min(old_end).min(new_end);

    let replacement = new[prefix..new_end].to_string();
    let range = prefix..old_end;

    // Final sanity: a no-op range with empty replacement means we
    // computed identical strings — caller should treat that as None.
    if range.is_empty() && replacement.is_empty() {
        return None;
    }
    Some((range, replacement))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_returns_none() {
        assert_eq!(diff_to_edit("hello", "hello"), None);
        assert_eq!(diff_to_edit("", ""), None);
    }

    #[test]
    fn append_emits_tail_insertion() {
        let (range, repl) = diff_to_edit("hello", "hello world").unwrap();
        assert_eq!(range, 5..5);
        assert_eq!(repl, " world");
    }

    #[test]
    fn prepend_emits_head_insertion() {
        let (range, repl) = diff_to_edit("world", "hello world").unwrap();
        assert_eq!(range, 0..0);
        assert_eq!(repl, "hello ");
    }

    #[test]
    fn middle_replace_emits_minimal_span() {
        // "abcXdef" → "abcYdef"
        let (range, repl) = diff_to_edit("abcXdef", "abcYdef").unwrap();
        assert_eq!(range, 3..4);
        assert_eq!(repl, "Y");
    }

    #[test]
    fn middle_insert_emits_zero_width_range() {
        let (range, repl) = diff_to_edit("abcdef", "abcXdef").unwrap();
        assert_eq!(range, 3..3);
        assert_eq!(repl, "X");
    }

    #[test]
    fn middle_delete_emits_empty_replacement() {
        let (range, repl) = diff_to_edit("abcXdef", "abcdef").unwrap();
        assert_eq!(range, 3..4);
        assert_eq!(repl, "");
    }

    #[test]
    fn full_replace_when_no_common_substring() {
        let (range, repl) = diff_to_edit("abc", "xyz").unwrap();
        assert_eq!(range, 0..3);
        assert_eq!(repl, "xyz");
    }

    #[test]
    fn empty_to_nonempty() {
        let (range, repl) = diff_to_edit("", "hello").unwrap();
        assert_eq!(range, 0..0);
        assert_eq!(repl, "hello");
    }

    #[test]
    fn nonempty_to_empty() {
        let (range, repl) = diff_to_edit("hello", "").unwrap();
        assert_eq!(range, 0..5);
        assert_eq!(repl, "");
    }

    #[test]
    fn round_trip_holds_for_random_edits() {
        let cases = [
            ("model A end A;", "model B end B;"),
            ("Real x = 1.0;", "Real x = 2.5;"),
            (
                "// header\nmodel Foo\n  Real x;\nend Foo;",
                "// header\nmodel Foo\n  Real x = 0;\nend Foo;",
            ),
            ("model M\nend M;", "// added comment\nmodel M\nend M;"),
        ];
        for (old, new) in cases {
            let (range, repl) = diff_to_edit(old, new).unwrap();
            let mut s = old.to_string();
            s.replace_range(range.clone(), &repl);
            assert_eq!(s, new, "diff mismatch on {old:?} → {new:?}");
        }
    }

    #[test]
    fn unicode_safe_at_multibyte_prefix() {
        // "α" is 2 bytes (0xCE 0xB1). Prefix-match might land mid-char
        // before char-boundary correction.
        let old = "αβγ";
        let new = "αδγ";
        let (range, repl) = diff_to_edit(old, new).unwrap();
        // The replacement must round-trip when applied to old.
        let mut s = old.to_string();
        s.replace_range(range, &repl);
        assert_eq!(s, new);
    }

    #[test]
    fn unicode_safe_at_multibyte_suffix() {
        let old = "abcα";
        let new = "xyzα";
        let (range, repl) = diff_to_edit(old, new).unwrap();
        let mut s = old.to_string();
        s.replace_range(range, &repl);
        assert_eq!(s, new);
    }

    #[test]
    fn long_typing_session_minimal_span() {
        // Simulate adding a parameter inside a small model.
        let old = "model Foo\n  Real x;\nend Foo;";
        let new = "model Foo\n  parameter Real k = 1.0;\n  Real x;\nend Foo;";
        let (range, repl) = diff_to_edit(old, new).unwrap();
        // Range starts after the common "model Foo\n  " prefix and is
        // zero-width (pure insertion).
        assert_eq!(&old[range.start..range.end], "");
        assert_eq!(repl, "parameter Real k = 1.0;\n  ");
    }
}
