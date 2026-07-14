//! Byte-level scanning of Modelica source.
//!
//! The splice engine ([`super::edit`]) needs to answer questions the AST can't:
//! *where does this declaration end?*, *where is its modifier list?*, *where do
//! I insert a new equation?* rumoca's spans are precise but partial — a
//! `Component`'s `location` covers only `Real m`, not its modifiers, binding,
//! description or the terminating `;`. So structural anchors are found by
//! scanning source; **values** are still taken from AST spans, which are exact.
//!
//! Everything here is lexically aware: string literals and comments are skipped,
//! so a `;` inside `"a;b"` or a `(` inside `// (` never counts.

use std::ops::Range;

use rumoca_compile::parsing::ast::{ClassDef, Component};

/// Visit every *code* byte in `range` — outside string literals and comments —
/// with its bracket-nesting depth. Return `false` from `f` to stop.
///
/// An opening bracket is reported at the depth *outside* it and its matching
/// closer at the same depth, so both parens of a top-level group read as depth 0.
fn for_each_code<F>(source: &str, range: Range<usize>, mut f: F)
where
    F: FnMut(usize, u8, i32) -> bool,
{
    let src = source.as_bytes();
    let end = range.end.min(src.len());
    let mut i = range.start;
    let mut depth = 0i32;
    while i < end {
        let b = src[i];
        // Comments.
        if b == b'/' && i + 1 < src.len() {
            if src[i + 1] == b'/' {
                i += 2;
                while i < end && src[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if src[i + 1] == b'*' {
                i += 2;
                while i + 1 < end && !(src[i] == b'*' && src[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(end);
                continue;
            }
        }
        // String literal.
        if b == b'"' {
            i += 1;
            while i < end {
                if src[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if src[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        match b {
            b'(' | b'[' | b'{' => {
                if !f(i, b, depth) {
                    return;
                }
                depth += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                if !f(i, b, depth) {
                    return;
                }
            }
            _ => {
                if !f(i, b, depth) {
                    return;
                }
            }
        }
        i += 1;
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// First code byte at or after `from` that isn't whitespace.
pub fn first_code_byte(source: &str, from: usize) -> Option<usize> {
    let mut out = None;
    for_each_code(source, from..source.len(), |i, b, _| {
        if b.is_ascii_whitespace() {
            true
        } else {
            out = Some(i);
            false
        }
    });
    out
}

/// The bracket group opened at `open`, as a range **including** both brackets.
pub fn matching_close(source: &str, open: usize) -> Option<Range<usize>> {
    let mut end = None;
    for_each_code(source, open..source.len(), |i, b, d| {
        if i == open {
            return true;
        }
        if d == 0 && matches!(b, b')' | b']' | b'}') {
            end = Some(i + 1);
            return false;
        }
        true
    });
    end.map(|e| open..e)
}

/// The `(...)` group whose `(` is the first code byte at or after `from`.
/// Returns `None` when the next thing isn't an open paren.
pub fn paren_group_at(source: &str, from: usize) -> Option<Range<usize>> {
    let start = first_code_byte(source, from)?;
    if source.as_bytes().get(start) != Some(&b'(') {
        return None;
    }
    matching_close(source, start)
}

/// Index just past the `;` terminating the statement that starts at `from`.
pub fn statement_end(source: &str, from: usize) -> Option<usize> {
    let mut out = None;
    for_each_code(source, from..source.len(), |i, b, d| {
        if b == b';' && d == 0 {
            out = Some(i + 1);
            false
        } else {
            true
        }
    });
    out
}

/// First `byte` at nesting depth 0 within `range`.
pub fn find_byte(source: &str, range: Range<usize>, byte: u8) -> Option<usize> {
    let mut hit = None;
    for_each_code(source, range, |i, b, d| {
        if d == 0 && b == byte {
            hit = Some(i);
            false
        } else {
            true
        }
    });
    hit
}

/// Start of the line containing `pos`.
pub fn line_start(source: &str, pos: usize) -> usize {
    let src = source.as_bytes();
    let mut i = pos.min(src.len());
    while i > 0 && src[i - 1] != b'\n' {
        i -= 1;
    }
    i
}

/// End of the statement *containing* `anchor`.
///
/// [`statement_end`] scans forward from where you point it, which breaks when
/// the anchor is inside brackets — and an `Equation`'s span is its first token,
/// so for `connect(a, b);` it points at `a`, one level deep. From there the
/// closing `)` drives the depth negative and the statement's `;` is never seen
/// at depth 0. Scanning from the start of the line fixes that; the loop guards
/// the case where an earlier statement shares the line.
pub fn statement_end_containing(source: &str, anchor: usize) -> Option<usize> {
    let mut from = line_start(source, anchor);
    loop {
        let end = statement_end(source, from)?;
        if end > anchor {
            return Some(end);
        }
        from = end;
    }
}

/// Whole-word `kw` at nesting depth 0 within `range`.
pub fn find_keyword(source: &str, range: Range<usize>, kw: &str) -> Option<usize> {
    let src = source.as_bytes();
    let mut hit = None;
    for_each_code(source, range.clone(), |i, _b, d| {
        if d != 0 || i + kw.len() > range.end {
            return true;
        }
        if !source[i..].starts_with(kw) {
            return true;
        }
        let before_ok = i == 0 || !is_ident_byte(src[i - 1]);
        let after = i + kw.len();
        let after_ok = after >= src.len() || !is_ident_byte(src[after]);
        if before_ok && after_ok {
            hit = Some(i);
            return false;
        }
        true
    });
    hit
}

/// Top-level comma-separated argument ranges inside `group` (which includes its
/// brackets). Empty groups yield an empty list.
pub fn split_args(source: &str, group: Range<usize>) -> Vec<Range<usize>> {
    if group.end <= group.start + 1 {
        return Vec::new();
    }
    let inner = group.start + 1..group.end - 1;
    let mut out = Vec::new();
    let mut start = inner.start;
    for_each_code(source, inner.clone(), |i, b, d| {
        if b == b',' && d == 0 {
            out.push(start..i);
            start = i + 1;
        }
        true
    });
    out.push(start..inner.end);
    out.retain(|r| !source[r.clone()].trim().is_empty());
    out
}

/// The leading identifier of an argument — `Placement` in `Placement(...)`,
/// `points` in `points = {…}`.
pub fn arg_head(source: &str, arg: Range<usize>) -> &str {
    let s = source[arg].trim_start();
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// Index into `args` of the argument whose head identifier is `name`.
pub fn find_arg(source: &str, args: &[Range<usize>], name: &str) -> Option<usize> {
    args.iter().position(|r| arg_head(source, r.clone()) == name)
}

/// Declaration prefixes that sit *before* the type name and so belong to the
/// declaration's byte extent (rumoca's `Component::location` starts at the type).
const DECL_PREFIXES: &[&str] = &[
    "parameter",
    "constant",
    "discrete",
    "input",
    "output",
    "flow",
    "stream",
    "inner",
    "outer",
    "replaceable",
    "final",
    "redeclare",
    "each",
];

/// Rewind from a type-name token over any declaration prefix keywords.
pub fn decl_start(source: &str, type_start: usize) -> usize {
    let src = source.as_bytes();
    let mut i = type_start.min(src.len());
    loop {
        let mut j = i;
        while j > 0 && matches!(src[j - 1], b' ' | b'\t') {
            j -= 1;
        }
        let word_end = j;
        let mut k = j;
        while k > 0 && is_ident_byte(src[k - 1]) {
            k -= 1;
        }
        if k == word_end || !DECL_PREFIXES.contains(&&source[k..word_end]) {
            break;
        }
        i = k;
    }
    i
}

/// Grow `stmt` to swallow the line it sits on: leading indentation (when only
/// whitespace precedes it) and one trailing newline. This is the range to
/// **delete** so removing an element doesn't leave a blank line behind.
pub fn line_extent(source: &str, stmt: Range<usize>) -> Range<usize> {
    let src = source.as_bytes();
    let mut start = stmt.start;
    while start > 0 && matches!(src[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    if start > 0 && src[start - 1] != b'\n' {
        start = stmt.start; // something else shares the line — keep it
    }
    let mut end = stmt.end;
    while end < src.len() && matches!(src[end], b' ' | b'\t') {
        end += 1;
    }
    if end < src.len() && src[end] == b'\n' {
        end += 1;
    } else {
        end = stmt.end;
    }
    start..end
}

/// Indentation of the line containing `pos`.
pub fn indent_at(source: &str, pos: usize) -> String {
    let src = source.as_bytes();
    let mut start = pos.min(src.len());
    while start > 0 && src[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = start;
    while end < src.len() && matches!(src[end], b' ' | b'\t') {
        end += 1;
    }
    source[start..end].to_string()
}

// ---------------------------------------------------------------------------
// Component anchors
// ---------------------------------------------------------------------------

/// Full byte extent of a component declaration: prefixes through the `;`.
pub fn component_extent(source: &str, comp: &Component) -> Option<Range<usize>> {
    let anchor = comp.location.start as usize;
    let end = statement_end(source, anchor)?;
    Some(decl_start(source, anchor)..end)
}

/// Byte position just past the component's name and any `[dims]` subscript —
/// where a modifier list `(...)` would start.
pub fn component_after_name(source: &str, comp: &Component) -> usize {
    let mut pos = comp.name_token.location.end as usize;
    if let Some(i) = first_code_byte(source, pos) {
        if source.as_bytes().get(i) == Some(&b'[') {
            if let Some(g) = matching_close(source, i) {
                pos = g.end;
            }
        }
    }
    pos
}

/// The component's modifier group — the `(...)` right after its name.
pub fn component_modifier_group(source: &str, comp: &Component) -> Option<Range<usize>> {
    paren_group_at(source, component_after_name(source, comp))
}

/// The `annotation(...)` clause inside a statement: `(keyword_start, group)`.
pub fn annotation_clause(source: &str, stmt: Range<usize>) -> Option<(usize, Range<usize>)> {
    let kw = find_keyword(source, stmt, "annotation")?;
    let group = paren_group_at(source, kw + "annotation".len())?;
    Some((kw, group))
}

// ---------------------------------------------------------------------------
// Class anchors
// ---------------------------------------------------------------------------

/// Byte extent of a class, from its `model`/`package`/… keyword through the
/// `;` after `end Name`.
///
/// `ClassDef::location` starts at the class *name*, so rewind over the kind
/// keyword and any `partial`/`encapsulated`/`operator` prefix.
pub fn class_extent(source: &str, class: &ClassDef) -> Option<Range<usize>> {
    let src = source.as_bytes();
    let mut i = class.location.start as usize;
    loop {
        let mut j = i;
        while j > 0 && matches!(src[j - 1], b' ' | b'\t') {
            j -= 1;
        }
        let word_end = j;
        let mut k = j;
        while k > 0 && is_ident_byte(src[k - 1]) {
            k -= 1;
        }
        const CLASS_PREFIXES: &[&str] = &[
            "model",
            "class",
            "package",
            "record",
            "block",
            "connector",
            "function",
            "type",
            "operator",
            "partial",
            "encapsulated",
            "expandable",
            "pure",
            "impure",
            "final",
            "replaceable",
        ];
        if k == word_end || !CLASS_PREFIXES.contains(&&source[k..word_end]) {
            break;
        }
        i = k;
    }
    let end = statement_end(source, class.location.end as usize)?;
    Some(i..end)
}

/// Position of the `end` keyword that closes `class`.
pub fn class_end_keyword(source: &str, class: &ClassDef) -> Option<usize> {
    let name_tok = class.end_name_token.as_ref()?;
    let src = source.as_bytes();
    let mut i = name_tok.location.start as usize;
    while i > 0 && matches!(src[i - 1], b' ' | b'\t' | b'\n' | b'\r') {
        i -= 1;
    }
    let word_end = i;
    while i > 0 && is_ident_byte(src[i - 1]) {
        i -= 1;
    }
    (&source[i..word_end] == "end").then_some(i)
}

/// The class-level `annotation(...)` clause: `(keyword_start, group)`.
///
/// Distinguished from the `annotation(...)` on a *component declaration* by
/// position: the class annotation is the statement immediately before
/// `end Name;`, so search only the bytes after the last element.
pub fn class_annotation_clause(
    source: &str,
    class: &ClassDef,
) -> Option<(usize, Range<usize>)> {
    let end_kw = class_end_keyword(source, class)?;
    let last = last_element_end(source, class).unwrap_or(class.location.start as usize);
    let kw = find_keyword(source, last..end_kw, "annotation")?;
    let group = paren_group_at(source, kw + "annotation".len())?;
    Some((kw, group))
}

/// End of the last component / equation statement in the class body — the point
/// after which only the class annotation and `end Name;` may appear.
fn last_element_end(source: &str, class: &ClassDef) -> Option<usize> {
    let mut last = None;
    for comp in class.components.values() {
        if let Some(r) = component_extent(source, comp) {
            last = Some(last.map_or(r.end, |l: usize| l.max(r.end)));
        }
    }
    for eq in class.equations.iter().chain(class.initial_equations.iter()) {
        if let Some(loc) = eq.get_location() {
            if let Some(end) = statement_end_containing(source, loc.start as usize) {
                last = Some(last.map_or(end, |l: usize| l.max(end)));
            }
        }
    }
    for nested in class.classes.values() {
        if let Some(r) = class_extent(source, nested) {
            last = Some(last.map_or(r.end, |l: usize| l.max(r.end)));
        }
    }
    last
}

/// Where a new component declaration goes: after the last existing component,
/// else at the top of the class body (before `equation` / the annotation /
/// `end`).
pub fn component_insert_point(source: &str, class: &ClassDef) -> Option<usize> {
    if let Some(end) = class
        .components
        .values()
        .filter_map(|c| component_extent(source, c).map(|r| r.end))
        .max()
    {
        return Some(end);
    }
    class_body_start(class)
}

/// Start of the class body — just past the header line (name, description).
fn class_body_start(class: &ClassDef) -> Option<usize> {
    let mut pos = class.name.location.end as usize;
    // Skip the description string, if any.
    if let Some(tok) = class.description.last() {
        pos = pos.max(tok.location.end as usize);
    }
    Some(pos)
}

/// The point at the end of the class body that new *sections* must precede:
/// the start of the class annotation statement if there is one, else the `end`
/// keyword. A class annotation must stay the last element before `end Name;`,
/// so appending (say) an `equation` section after it would not parse.
pub fn class_tail_anchor(source: &str, class: &ClassDef) -> Option<usize> {
    if let Some((kw, _)) = class_annotation_clause(source, class) {
        // Rewind to the start of the annotation's own line so an insertion
        // lands above it rather than mid-line.
        let src = source.as_bytes();
        let mut i = kw;
        while i > 0 && matches!(src[i - 1], b' ' | b'\t') {
            i -= 1;
        }
        return Some(i);
    }
    class_end_keyword(source, class)
}

/// Where a new equation goes: after the last equation, else just past the
/// `equation` keyword. `None` when the class has no `equation` section yet —
/// the caller must create one.
pub fn equation_insert_point(source: &str, class: &ClassDef) -> Option<usize> {
    if let Some(end) = class
        .equations
        .iter()
        .filter_map(|eq| eq.get_location())
        .filter_map(|loc| statement_end_containing(source, loc.start as usize))
        .max()
    {
        return Some(end);
    }
    class
        .equation_keyword
        .as_ref()
        .map(|t| t.location.end as usize)
}
