//! Modelica source and AST contracts shared by runtime, validation, and UI crates.
//!
//! Rumoca owns parsing and source coordinates. This package owns AST extraction and
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

/// Rumoca's parsed declaration classifications, exposed by the Modelica AST
/// boundary for runtime and authoring projections.
pub use rumoca_core::{Causality, ClassType, OpBinary, OpUnary, Span, Token, Variability};
/// Rumoca's parsed definition tree consumed by AST/fact extractors.
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
        let parts: Vec<&str> = rumoca_core::qualified_name_segments(ctx).collect();
        for i in (0..parts.len().saturating_sub(1)).rev() {
            let prefix = parts[..=i].join(".");
            out.push(format!("{prefix}.{raw}"));
        }
    }
    out.push(raw.to_string());
    out
}
