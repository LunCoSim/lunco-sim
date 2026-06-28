//! Source-text editing helpers for `.usda` files.
//!
//! Phase 5 implements typed ops (`AddPrim`, `RemovePrim`,
//! `SetTranslate`) by splicing the canonical source text rather than
//! parsed-roundtripping through a serializer. This keeps comments
//! and formatting intact in untouched regions — important because
//! USD scenes are routinely co-edited with Omniverse / USDView /
//! Blender, and a roundtrip that reformats every spec would be a
//! nasty diff in shared repos.
//!
//! ## Path syntax
//!
//! Paths follow the USD convention: `"/World/Rover/WheelFL"`. The
//! splicers walk the `def|over|class [TypeName] "name" { ... }`
//! blocks at each level. Property children (attributes,
//! relationships) and variants are not addressable via this module —
//! they live at separate field names that don't collide with prim
//! children.
//!
//! ## Limitations
//!
//! - **Comments containing `{`/`}`** between the `def` line and the
//!   opening brace will fool the brace tracker. Real `.usda` files
//!   put braces only inside string literals (which we skip) and in
//!   block delimiters, so this works for hand-authored and
//!   tool-emitted files in practice.
//! - **`(metadata)` blocks** are passed through transparently — the
//!   walker treats `(` ... `)` like a string literal for nesting
//!   purposes.
//! - **Multi-byte UTF-8** is preserved (the helpers operate on
//!   `&str` and respect char boundaries).

use std::ops::Range;

/// Result of locating a prim block in a source buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimSpan {
    /// Byte range covering the entire `def ... { ... }` block,
    /// including any leading specifier keyword.
    pub block: Range<usize>,
    /// Byte range covering the body — strictly after the opening
    /// `{` and strictly before the matching `}`.
    pub body: Range<usize>,
}

/// Locate a prim by absolute USD path.
///
/// `"/World"` finds the top-level `def Xform "World" { ... }` block.
/// `"/World/Rover"` walks inside `World`'s body to find `Rover`.
///
/// Returns `None` if any segment fails to resolve.
pub fn find_prim(source: &str, path: &str) -> Option<PrimSpan> {
    let segments: Vec<&str> = path
        .strip_prefix('/')
        .unwrap_or(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return None;
    }
    let mut search_range = 0..source.len();
    let mut found: Option<PrimSpan> = None;
    for segment in segments {
        let span = find_prim_in_range(source, search_range.clone(), segment)?;
        search_range = span.body.clone();
        found = Some(span);
    }
    found
}

/// Find a top-level prim with the given name inside `range`.
///
/// "Top-level" means: not inside any nested `{...}`. The walker
/// tracks brace depth and only matches `def|over|class` keywords at
/// depth 0.
fn find_prim_in_range(source: &str, range: Range<usize>, name: &str) -> Option<PrimSpan> {
    let bytes = source.as_bytes();
    let mut i = range.start;
    while i < range.end {
        // Skip whitespace, comments, string literals, parens.
        i = skip_noise(bytes, i, range.end);
        if i >= range.end {
            break;
        }
        // Try to match a specifier keyword (def / over / class).
        let Some(kw_end) = match_specifier(bytes, i) else {
            i += 1;
            continue;
        };
        let block_start = i;
        let mut j = kw_end;
        // Optional type name (identifier that isn't a quoted string).
        j = skip_header_gap(bytes, j, range.end);
        if j < range.end && bytes[j] != b'"' {
            j = scan_identifier(bytes, j, range.end);
            j = skip_header_gap(bytes, j, range.end);
        }
        // Quoted prim name.
        if j >= range.end || bytes[j] != b'"' {
            i = j.max(i + 1);
            continue;
        }
        let name_start = j + 1;
        let mut k = name_start;
        while k < range.end && bytes[k] != b'"' {
            // No escape support — USD prim names disallow embedded quotes.
            k += 1;
        }
        if k >= range.end {
            return None;
        }
        let prim_name = &source[name_start..k];
        j = k + 1;
        // Optional `(metadata)` block before the body.
        j = skip_header_gap(bytes, j, range.end);
        if j < range.end && bytes[j] == b'(' {
            j = skip_balanced(bytes, j, range.end, b'(', b')')?;
            j = skip_header_gap(bytes, j, range.end);
        }
        // Body: opening brace, balanced contents, closing brace.
        if j >= range.end || bytes[j] != b'{' {
            i = j.max(i + 1);
            continue;
        }
        let body_start = j + 1;
        let body_end = find_matching_brace(bytes, body_start, range.end)?;
        let block_end = body_end + 1;
        if prim_name == name {
            return Some(PrimSpan {
                block: block_start..block_end,
                body: body_start..body_end,
            });
        }
        i = block_end;
    }
    None
}

fn match_specifier(bytes: &[u8], i: usize) -> Option<usize> {
    for kw in ["def", "over", "class"] {
        let kb = kw.as_bytes();
        if i + kb.len() <= bytes.len()
            && &bytes[i..i + kb.len()] == kb
            && (i + kb.len() == bytes.len()
                || !is_ident_byte(bytes[i + kb.len()]))
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
        {
            return Some(i + kb.len());
        }
    }
    None
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b':'
}

fn scan_identifier(bytes: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && is_ident_byte(bytes[i]) {
        i += 1;
    }
    i
}

fn skip_inline_ws(bytes: &[u8], mut i: usize, end: usize) -> usize {
    while i < end {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            _ => break,
        }
    }
    i
}

/// Skip whitespace (incl. newlines) and `#`-comments. Used in the
/// prim-header walker to bridge the gaps between specifier, type
/// name, prim name, optional `(metadata)`, and the opening `{`.
fn skip_header_gap(bytes: &[u8], mut i: usize, end: usize) -> usize {
    while i < end {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'#' => {
                while i < end && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    i
}

fn skip_noise(bytes: &[u8], mut i: usize, end: usize) -> usize {
    loop {
        if i >= end {
            return i;
        }
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'#' => {
                while i < end && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                // Skip a quoted string (no escape support).
                i += 1;
                while i < end && bytes[i] != b'"' {
                    i += 1;
                }
                if i < end {
                    i += 1;
                }
            }
            _ => return i,
        }
    }
}

fn skip_balanced(
    bytes: &[u8],
    start: usize,
    end: usize,
    open: u8,
    close: u8,
) -> Option<usize> {
    if start >= end || bytes[start] != open {
        return None;
    }
    let mut depth = 1;
    let mut i = start + 1;
    while i < end {
        match bytes[i] {
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'"' => {
                i += 1;
                while i < end && bytes[i] != b'"' {
                    i += 1;
                }
            }
            b'#' => {
                while i < end && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_matching_brace(bytes: &[u8], body_start: usize, end: usize) -> Option<usize> {
    let mut depth = 1;
    let mut i = body_start;
    while i < end {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' => {
                i += 1;
                while i < end && bytes[i] != b'"' {
                    i += 1;
                }
            }
            b'#' => {
                while i < end && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' => {
                if let Some(after) = skip_balanced(bytes, i, end, b'(', b')') {
                    i = after;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

// ─────────────────────────────────────────────────────────────────────
// Splice operations
// ─────────────────────────────────────────────────────────────────────

/// Append a child `def TypeName "name" { }` block inside the body of
/// the prim at `parent_path` (or at the top of the file when
/// `parent_path == "/"`).
///
/// Returns the new full source on success. `None` if the parent path
/// can't be resolved.
pub fn append_child_prim(
    source: &str,
    parent_path: &str,
    type_name: Option<&str>,
    name: &str,
) -> Option<String> {
    let snippet = match type_name {
        Some(ty) => format!("\ndef {} \"{}\"\n{{\n}}\n", ty, name),
        None => format!("\ndef \"{}\"\n{{\n}}\n", name),
    };
    if parent_path == "/" || parent_path.is_empty() {
        let mut out = source.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&snippet);
        return Some(out);
    }
    let span = find_prim(source, parent_path)?;
    // Insert just before the closing brace, ensuring a leading newline.
    let mut out = String::with_capacity(source.len() + snippet.len() + 4);
    let body_end = span.body.end;
    out.push_str(&source[..body_end]);
    if !source[..body_end].ends_with('\n') {
        out.push('\n');
    }
    // Indent every line of the snippet by 4 spaces. Cheap and matches
    // hand-authored convention.
    for line in snippet.trim_start_matches('\n').lines() {
        if line.is_empty() {
            out.push('\n');
            continue;
        }
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&source[body_end..]);
    Some(out)
}

/// Remove the prim at `path` together with its entire `def ... { }`
/// block. Returns the new source or `None` if the path doesn't
/// resolve.
pub fn remove_prim(source: &str, path: &str) -> Option<String> {
    let span = find_prim(source, path)?;
    // Expand to also drop the line containing the def keyword's
    // leading whitespace and the trailing newline so we don't leave
    // a blank gap.
    let mut start = span.block.start;
    while start > 0
        && (source.as_bytes()[start - 1] == b' '
            || source.as_bytes()[start - 1] == b'\t')
    {
        start -= 1;
    }
    if start > 0 && source.as_bytes()[start - 1] == b'\n' {
        start -= 1;
    }
    let mut end = span.block.end;
    if end < source.len() && source.as_bytes()[end] == b'\n' {
        end += 1;
    }
    let mut out = String::with_capacity(source.len() - (end - start));
    out.push_str(&source[..start]);
    out.push_str(&source[end..]);
    Some(out)
}

/// Set the `xformOp:translate` value on the prim at `path`. If the
/// translate property is already authored, its right-hand side is
/// replaced; otherwise a new line is inserted at the top of the
/// prim body together with `xformOpOrder` if absent.
///
/// Returns the new source or `None` if the path doesn't resolve.
pub fn set_translate(source: &str, path: &str, value: [f64; 3]) -> Option<String> {
    let span = find_prim(source, path)?;
    let body = &source[span.body.clone()];
    let formatted = format!("({}, {}, {})", value[0], value[1], value[2]);

    // Replace existing translate, if any.
    if let Some(rel) = find_attribute_value_range(body, "xformOp:translate") {
        let abs = (span.body.start + rel.start)..(span.body.start + rel.end);
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..abs.start]);
        out.push_str(&formatted);
        out.push_str(&source[abs.end..]);
        return Some(out);
    }

    // Insert a new translate line at the top of the body.
    let mut snippet = String::new();
    if !body.contains("xformOpOrder") {
        snippet.push_str("    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n");
    }
    snippet.push_str(&format!(
        "    double3 xformOp:translate = {}\n",
        formatted
    ));
    let mut out = String::with_capacity(source.len() + snippet.len() + 1);
    let insert_at = span.body.start;
    out.push_str(&source[..insert_at]);
    if !source[..insert_at].ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&snippet);
    out.push_str(&source[insert_at..]);
    Some(out)
}

/// Set an attribute value on the prim at `path`. If the attribute is already
/// authored, its right-hand side is replaced; otherwise a new attribute
/// line is inserted at the top of the prim body with the given `type_name`.
///
/// Returns the new source or `None` if the path doesn't resolve.
pub fn set_attribute(
    source: &str,
    path: &str,
    name: &str,
    type_name: &str,
    value: &str,
) -> Option<String> {
    let span = find_prim(source, path)?;
    let body = &source[span.body.clone()];

    // Replace existing value range, if any.
    if let Some(rel) = find_attribute_value_range(body, name) {
        let abs = (span.body.start + rel.start)..(span.body.start + rel.end);
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..abs.start]);
        out.push_str(value);
        out.push_str(&source[abs.end..]);
        return Some(out);
    }

    // Insert a new attribute line at the top of the body.
    let snippet = format!("    {} {} = {}\n", type_name, name, value);
    let mut out = String::with_capacity(source.len() + snippet.len() + 1);
    let insert_at = span.body.start;
    out.push_str(&source[..insert_at]);
    if !source[..insert_at].ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&snippet);
    out.push_str(&source[insert_at..]);
    Some(out)
}

/// Find the byte range of an attribute's right-hand-side value
/// expression within a prim body. Matches both unannotated lines
/// (`xformOp:translate = (...)`) and typed declarations
/// (`double3 xformOp:translate = (...)`).
///
/// The returned range covers everything from the `=`'s following
/// whitespace through the value (a balanced `(...)`/`[...]` block,
/// a quoted string, or a token up to end-of-line).
fn find_attribute_value_range(body: &str, attr: &str) -> Option<Range<usize>> {
    let needle = attr.as_bytes();
    let bytes = body.as_bytes();
    let mut i = 0;
    // Brace/bracket/paren depth WITHIN the prim body. The body we're
    // handed is the parent prim's body, so depth 0 == directly on the
    // parent. Child `def|over|class { ... }` blocks (and `(metadata)` /
    // array values) sit at depth ≥ 1; we must NOT match an attribute
    // there or `SetTranslate`/`SetAttribute` on a parent that lacks the
    // attribute would clobber a descendant's value (CQ-503).
    let mut depth: i32 = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'(' | b'[' => {
                depth += 1;
                i += 1;
                continue;
            }
            b'}' | b')' | b']' => {
                depth -= 1;
                i += 1;
                continue;
            }
            b'"' => {
                // Skip a quoted string (no escape support) so braces /
                // the needle inside string literals never count.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        if depth == 0
            && i + needle.len() <= bytes.len()
            && &bytes[i..i + needle.len()] == needle
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && (i + needle.len() == bytes.len()
                || !is_ident_byte(bytes[i + needle.len()]))
        {
            // Match — find `=`.
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j] != b'=' && bytes[j] != b'\n' {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b'=' {
                return None;
            }
            j += 1;
            j = skip_inline_ws(bytes, j, bytes.len());
            let value_start = j;
            // Value: balanced (...) or [...] or quoted string or
            // bare token to end-of-line.
            let value_end = match bytes.get(j) {
                Some(b'(') => skip_balanced(bytes, j, bytes.len(), b'(', b')')?,
                Some(b'[') => skip_balanced(bytes, j, bytes.len(), b'[', b']')?,
                Some(b'"') => {
                    let mut k = j + 1;
                    while k < bytes.len() && bytes[k] != b'"' {
                        k += 1;
                    }
                    if k < bytes.len() {
                        k + 1
                    } else {
                        return None;
                    }
                }
                _ => {
                    let mut k = j;
                    while k < bytes.len() && bytes[k] != b'\n' {
                        k += 1;
                    }
                    k
                }
            };
            return Some(value_start..value_end);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY: &str = "#usda 1.0\n\ndef Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n    }\n}\n";

    #[test]
    fn find_prim_top_level() {
        let span = find_prim(TINY, "/World").expect("found");
        assert_eq!(&TINY[span.block.clone()], "def Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n    }\n}");
    }

    #[test]
    fn find_prim_nested() {
        let span = find_prim(TINY, "/World/Ball").expect("found");
        assert_eq!(&TINY[span.block.clone()], "def Sphere \"Ball\"\n    {\n    }");
    }

    #[test]
    fn find_prim_missing_returns_none() {
        assert!(find_prim(TINY, "/World/Nope").is_none());
        assert!(find_prim(TINY, "/Nope").is_none());
    }

    #[test]
    fn append_child_to_root() {
        let out = append_child_prim(TINY, "/", Some("Xform"), "Rover").unwrap();
        assert!(out.contains("def Xform \"Rover\""));
        // Original World prim untouched.
        assert!(out.contains("def Xform \"World\""));
    }

    #[test]
    fn append_child_to_nested_prim() {
        let out = append_child_prim(TINY, "/World", Some("Cube"), "Body").unwrap();
        // The new Cube is inside World's body, before the closing brace.
        let world = find_prim(&out, "/World").unwrap();
        assert!(out[world.body.clone()].contains("def Cube \"Body\""));
        // And reachable as /World/Body.
        assert!(find_prim(&out, "/World/Body").is_some());
    }

    #[test]
    fn remove_existing_prim() {
        let out = remove_prim(TINY, "/World/Ball").unwrap();
        assert!(!out.contains("Ball"));
        // World survives.
        assert!(find_prim(&out, "/World").is_some());
    }

    #[test]
    fn remove_top_level_prim() {
        let out = remove_prim(TINY, "/World").unwrap();
        assert!(!out.contains("World"));
        assert!(out.contains("#usda 1.0"));
    }

    #[test]
    fn set_translate_inserts_when_absent() {
        let out = set_translate(TINY, "/World/Ball", [1.0, 2.0, 3.0]).unwrap();
        assert!(out.contains("xformOp:translate = (1, 2, 3)"));
        assert!(out.contains("xformOpOrder"));
    }

    #[test]
    fn set_translate_replaces_when_present() {
        let with_existing = "#usda 1.0\ndef Xform \"World\"\n{\n    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    double3 xformOp:translate = (0, 0, 0)\n}\n";
        let out = set_translate(with_existing, "/World", [5.0, 6.0, 7.0]).unwrap();
        assert!(out.contains("xformOp:translate = (5, 6, 7)"));
        assert!(!out.contains("(0, 0, 0)"));
        // xformOpOrder line not duplicated.
        assert_eq!(out.matches("xformOpOrder").count(), 1);
    }

    #[test]
    fn set_attribute_inserts_when_absent() {
        let out = set_attribute(TINY, "/World/Ball", "inputs:roughness", "float", "0.5").unwrap();
        assert!(out.contains("float inputs:roughness = 0.5"));
    }

    #[test]
    fn set_attribute_replaces_when_present() {
        let with_existing = "#usda 1.0\ndef Xform \"World\"\n{\n    float inputs:roughness = 0.8\n}\n";
        let out = set_attribute(with_existing, "/World", "inputs:roughness", "float", "0.2").unwrap();
        assert!(out.contains("inputs:roughness = 0.2"));
        assert!(!out.contains("0.8"));
    }

    #[test]
    fn build_a_rover_from_blank() {
        let blank = "#usda 1.0\n";
        let s = append_child_prim(blank, "/", Some("Xform"), "Rover").unwrap();
        let s = append_child_prim(&s, "/Rover", Some("Cube"), "Body").unwrap();
        let s = append_child_prim(&s, "/Rover", Some("Cube"), "WheelFL").unwrap();
        let s = set_translate(&s, "/Rover/WheelFL", [1.0, 0.0, 1.0]).unwrap();
        let s = append_child_prim(&s, "/Rover", Some("Cube"), "WheelFR").unwrap();
        let s = set_translate(&s, "/Rover/WheelFR", [1.0, 0.0, -1.0]).unwrap();

        // All four prims resolve.
        assert!(find_prim(&s, "/Rover").is_some());
        assert!(find_prim(&s, "/Rover/Body").is_some());
        assert!(find_prim(&s, "/Rover/WheelFL").is_some());
        assert!(find_prim(&s, "/Rover/WheelFR").is_some());
        // Translates land where expected.
        assert!(s.contains("xformOp:translate = (1, 0, 1)"));
        assert!(s.contains("xformOp:translate = (1, 0, -1)"));
    }

    #[test]
    fn set_attribute_on_parent_does_not_clobber_child() {
        // CQ-503 regression: the parent prim has NO `inputs:roughness`,
        // but its child does. Setting it on the parent must ADD it to the
        // parent and leave the child's value untouched — the depth-blind
        // splicer used to overwrite the descendant.
        let src = "#usda 1.0\ndef Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n        float inputs:roughness = 0.8\n    }\n}\n";
        let out = set_attribute(src, "/World", "inputs:roughness", "float", "0.2").unwrap();

        // Child value preserved.
        let ball = find_prim(&out, "/World/Ball").expect("child survives");
        assert!(
            out[ball.body.clone()].contains("inputs:roughness = 0.8"),
            "child's roughness was clobbered: {out}"
        );
        // Parent got its own authoring.
        assert!(out.contains("inputs:roughness = 0.2"));
        // Exactly two authorings now (parent + child), neither lost.
        assert_eq!(out.matches("inputs:roughness").count(), 2);
        assert!(out.contains("0.8"));
        assert!(out.contains("0.2"));
    }

    #[test]
    fn set_translate_on_parent_does_not_clobber_child() {
        // CQ-503 regression for the `set_translate` route (shares the
        // same `find_attribute_value_range` scaffolding).
        let src = "#usda 1.0\ndef Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n        double3 xformOp:translate = (9, 9, 9)\n    }\n}\n";
        let out = set_translate(src, "/World", [1.0, 2.0, 3.0]).unwrap();

        // Child translate untouched.
        let ball = find_prim(&out, "/World/Ball").expect("child survives");
        assert!(
            out[ball.body.clone()].contains("xformOp:translate = (9, 9, 9)"),
            "child's translate was clobbered: {out}"
        );
        // Parent got its own translate.
        assert!(out.contains("xformOp:translate = (1, 2, 3)"));
        assert!(out.contains("(9, 9, 9)"));
        // Still parseable.
        let mut parser = openusd::usda::parser::Parser::new(&out);
        parser.parse().expect("parses cleanly");
    }

    #[test]
    fn round_trips_through_openusd_parser() {
        // Whatever we splice must remain parseable by the canonical
        // parser — the asset loader uses it, so any divergence here
        // breaks the 3D viewport pipeline silently.
        let s = append_child_prim("#usda 1.0\n", "/", Some("Xform"), "Rover").unwrap();
        let s = append_child_prim(&s, "/Rover", Some("Cube"), "Body").unwrap();
        let s = set_translate(&s, "/Rover/Body", [0.0, 1.0, 0.0]).unwrap();
        let mut parser = openusd::usda::parser::Parser::new(&s);
        parser.parse().expect("parses cleanly");
    }
}
