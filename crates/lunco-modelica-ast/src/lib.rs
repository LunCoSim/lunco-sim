//! Modelica source and AST contracts shared by runtime, validation, and UI crates.
//!
//! This package owns source normalization and parsing, AST extraction and
//! lossless mutation, diagram graph data, source-fragment rendering, interface
//! metadata, and lint facts. A validator, source editor, or co-simulation
//! projection can use the same authoritative Modelica representation.

pub mod ast_extract;
pub mod ast_mut;
pub mod diagram_model;
pub mod lint_facts;
pub mod pretty;

/// Rumoca's causality classification, re-exported from the Modelica AST
/// boundary so downstream domain projections do not depend on Rumoca's core
/// crate just to inspect parsed Modelica members.
pub use rumoca_core::Causality;
/// Rumoca's parsed definition tree, the value returned by this crate's parse
/// functions and consumed by AST/fact extractors.
pub use rumoca_ir_ast::StoredDefinition;

use std::borrow::Cow;

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
    let Some(rest) = qualified.strip_prefix(&prefix) else {
        return qualified;
    };
    rest.strip_prefix('.').unwrap_or(rest)
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
