//! Modelica source and AST contracts shared by runtime, validation, and UI crates.
//!
//! This package owns source normalization and parsing, AST extraction and
//! lossless mutation, diagram graph data, source-fragment rendering, interface
//! metadata, and lint facts. A validator, source editor, or co-simulation
//! projection can use the same authoritative Modelica representation.

pub mod annotations;
pub mod ast_extract;
pub mod ast_mut;
pub mod diagram_model;
pub mod icon_transform;
pub mod lint_facts;
pub mod pretty;
pub mod source_memo;

/// Rumoca's causality classification, re-exported from the Modelica AST
/// boundary so downstream domain projections do not depend on Rumoca's core
/// crate just to inspect parsed Modelica members.
pub use rumoca_core::Causality;
/// Rumoca's parsed definition tree, the value returned by this crate's parse
/// functions and consumed by AST/fact extractors.
pub use rumoca_ir_ast::StoredDefinition;

/// Generate the fully-qualified candidates prescribed by the Modelica name
/// lookup scope walk for a possibly-relative reference.
///
/// The caller probes these candidates against its authoritative class store;
/// this package owns only the deterministic candidate order so diagram,
/// inheritance, and editor projections cannot drift apart.
pub fn scope_chain_candidates(raw: &str, ctx: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(ctx) = ctx {
        let parts: Vec<&str> = qualified_name_segments(ctx).collect();
        for i in (0..parts.len().saturating_sub(1)).rev() {
            let prefix = parts[..=i].join(".");
            out.push(format!("{prefix}.{raw}"));
        }
    }
    out.push(raw.to_string());
    out
}

use std::borrow::Cow;

/// Borrow the dotted segments of a Modelica name, preserving quoted identifiers
/// and bracketed subscripts. This only identifies boundaries; syntax validity
/// remains the canonical parser's responsibility.
pub fn qualified_name_segments(name: &str) -> impl Iterator<Item = &str> {
    let mut remaining = Some(name);
    std::iter::from_fn(move || {
        let segment = remaining.take()?;
        let mut quote = None;
        let mut escaped = false;
        let mut brackets = 0usize;
        for (index, byte) in segment.bytes().enumerate() {
            if let Some(delimiter) = quote {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == delimiter {
                    quote = None;
                }
                continue;
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'[' => brackets += 1,
                b']' => brackets = brackets.saturating_sub(1),
                b'.' if brackets == 0 => {
                    remaining = Some(&segment[index + 1..]);
                    return Some(&segment[..index]);
                }
                _ => {}
            }
        }
        Some(segment)
    })
}

/// Remove the authored `within` prefix from a qualified name.
///
/// Rumoca stores a class's name relative to its source document while callers
/// often address it through the document's package-qualified name. Keeping
/// this normalization at the AST boundary lets source editors and Modelica
/// projections share the same package rule.
pub fn strip_within_prefix<'a>(
    qualified: &'a str,
    within: Option<&rumoca_ir_ast::Name>,
) -> &'a str {
    let Some(within) = within else {
        return qualified;
    };
    let prefix = within.to_string();
    qualified
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(qualified)
}

#[cfg(test)]
mod qualified_name_tests {
    #[test]
    fn qualified_segments_preserve_quoted_dots_escapes_and_subscripts() {
        for (name, expected) in [
            ("Root.Part", vec!["Root", "Part"]),
            ("Root.'Part.Name'.Leaf", vec!["Root", "'Part.Name'", "Leaf"]),
            (
                r"Root.'Part\'.Name'.Leaf",
                vec!["Root", r"'Part\'.Name'", "Leaf"],
            ),
            ("Root.'A[B].C'.Leaf", vec!["Root", "'A[B].C'", "Leaf"]),
            (
                "Root.bus[data.medium].pin",
                vec!["Root", "bus[data.medium]", "pin"],
            ),
            (
                r#"Root.bus[lookup("a]b.c")].pin"#,
                vec!["Root", r#"bus[lookup("a]b.c")]"#, "pin"],
            ),
            ("'Part.Name'", vec!["'Part.Name'"]),
        ] {
            assert_eq!(
                super::qualified_name_segments(name).collect::<Vec<_>>(),
                expected
            );
        }
        assert_eq!(
            super::ast_extract::parent_qualified("Root.'Part.Name'"),
            "Root"
        );
        assert_eq!(super::ast_extract::parent_qualified("'Part.Name'"), "");
        assert_eq!(
            super::scope_chain_candidates("Target", Some("Root.'Part.Name'.Leaf")),
            vec!["Root.'Part.Name'.Target", "Root.Target", "Target"]
        );
    }

    #[test]
    fn within_prefix_requires_a_package_segment_boundary() {
        let syntax = super::parse_to_syntax("within Root.B; model Part end Part;", "within.mo");
        let ast = syntax.parsed().expect("valid inline source");
        for (qualified, expected) in [
            ("Root.B.Part", "Part"),
            ("Root.BPart", "Root.BPart"),
            ("Root.BAD.Part", "Root.BAD.Part"),
            ("Root.B", "Root.B"),
            ("Part", "Part"),
        ] {
            assert_eq!(
                super::strip_within_prefix(qualified, ast.within.as_ref()),
                expected
            );
        }
    }
}

/// Normalize text accepted at the Modelica source boundary.
///
/// Windows editors commonly write a UTF-8 byte-order mark. It is metadata,
/// not Modelica syntax, and Rumoca does not treat it as whitespace before the
/// first token. Replace it with three ASCII spaces instead of removing it:
/// the UTF-8 BOM occupies three bytes, so source spans and diagnostics keep
/// the same offsets. CRLF is intentionally left unchanged; the Modelica
/// lexer handles it and changing line endings would alter authored source
/// coordinates.
pub fn normalize_modelica_source(source: &str) -> Cow<'_, str> {
    let Some(rest) = source.strip_prefix('\u{feff}') else {
        return Cow::Borrowed(source);
    };

    let mut normalized = String::with_capacity(source.len());
    normalized.push_str("   ");
    normalized.push_str(rest);
    Cow::Owned(normalized)
}

/// Parse Modelica source through the workspace's canonical normalized syntax
/// boundary and retain Rumoca's structured recovery information.
pub fn parse_to_syntax(source: &str, file_label: &str) -> rumoca_phase_parse::SyntaxFile {
    let normalized = normalize_modelica_source(source);
    rumoca_phase_parse::parse_to_syntax(&normalized, file_label)
}

/// Parse Modelica source strictly through the same normalized boundary.
pub fn parse_to_ast(source: &str, file_label: &str) -> anyhow::Result<StoredDefinition> {
    let normalized = normalize_modelica_source(source);
    rumoca_phase_parse::parse_to_ast(&normalized, file_label)
}

/// Parse Modelica source with Rumoca's recovery parser through the normalized
/// source boundary.
pub fn parse_to_recovered_ast(source: &str, file_label: &str) -> StoredDefinition {
    let normalized = normalize_modelica_source(source);
    rumoca_phase_parse::parse_to_recovered_ast(&normalized, file_label)
}
