//! Pure Modelica source analysis shared by runtime, validation, and UI crates.
//!
//! This package owns parse-time facts only: source normalization, recovered
//! AST extraction, interface metadata, and lint facts. It deliberately has no
//! Bevy, document, worker, storage, or renderer dependency, so a validator or
//! a co-simulation projection can use the same authoritative extraction path
//! without linking the Modelica workbench.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ast_extract;
pub mod lint_facts;

/// Rumoca's causality classification, re-exported from the Modelica AST
/// boundary so downstream domain projections do not depend on Rumoca's core
/// crate just to inspect parsed Modelica members.
pub use rumoca_core::Causality;
/// Rumoca's parsed definition tree, the value returned by this crate's parse
/// functions and consumed by AST/fact extractors.
pub use rumoca_ir_ast::StoredDefinition;

use std::borrow::Cow;

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
