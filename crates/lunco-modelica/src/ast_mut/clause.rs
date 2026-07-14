//! Editing argument lists in place.
//!
//! Modelica annotations are nested argument lists —
//! `annotation(Icon(graphics={Text(...), Line(...)}))`. Every graphics / plot /
//! placement op is really "set one argument inside one of those lists". Doing
//! that as a splice (rather than rebuilding the annotation from the AST) is what
//! keeps a `Documentation(info="…")` sibling, a hand-written comment, or an
//! annotation entry we don't model from being destroyed by an unrelated edit.

use std::ops::Range;

use super::edit::Edit;
use super::text;

/// Replace the argument whose head is `head`, or append `rendered` to the list.
///
/// `group` includes its brackets. `rendered` is the complete argument text
/// (`Placement(...)`, `points = {…}`).
pub fn upsert_arg(edit: &mut Edit<'_>, group: Range<usize>, head: &str, rendered: &str) {
    let source = edit.source();
    let args = text::split_args(source, group.clone());
    match text::find_arg(source, &args, head) {
        Some(i) => edit.replace(args[i].clone(), rendered),
        None => match args.last() {
            Some(last) => edit.insert(last.end, format!(", {rendered}")),
            // Empty list: drop it in just before the closing bracket.
            None => edit.insert(group.end - 1, rendered),
        },
    }
}

/// Append an entry to an array group `{…}`.
pub fn append_entry(edit: &mut Edit<'_>, array: Range<usize>, rendered: &str) {
    let source = edit.source();
    let args = text::split_args(source, array.clone());
    match args.last() {
        Some(last) => edit.insert(last.end, format!(", {rendered}")),
        None => edit.insert(array.end - 1, rendered),
    }
}

/// Delete `args[index]`, taking one comma separator with it so the list stays
/// well-formed.
pub fn remove_entry(edit: &mut Edit<'_>, args: &[Range<usize>], index: usize) {
    let target = args[index].clone();
    if let Some(next) = args.get(index + 1) {
        // Swallow the comma that follows.
        edit.delete(target.start..next.start);
    } else if index > 0 {
        // Last entry: swallow the comma that precedes.
        edit.delete(args[index - 1].end..target.end);
    } else {
        edit.delete(target);
    }
}

/// The `(...)` of a `Name(...)` argument inside `group`.
pub fn call_group(source: &str, group: Range<usize>, name: &str) -> Option<Range<usize>> {
    let args = text::split_args(source, group);
    let i = text::find_arg(source, &args, name)?;
    let arg = args[i].clone();
    let head_at = arg.start + source[arg.clone()].find(name)?;
    text::paren_group_at(source, head_at + name.len())
}

/// The value range of a `name = <value>` argument inside `group`.
pub fn arg_value(source: &str, group: Range<usize>, name: &str) -> Option<Range<usize>> {
    let args = text::split_args(source, group);
    let i = text::find_arg(source, &args, name)?;
    let arg = args[i].clone();
    let eq = text::find_byte(source, arg.clone(), b'=')?;
    let mut start = eq + 1;
    let bytes = source.as_bytes();
    while start < arg.end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    let mut end = arg.end;
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    Some(start..end)
}
