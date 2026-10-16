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
pub use rumoca_core::{
    Causality, ClassType, OpBinary, OpUnary, PredefinedComponentType, Span, Token, Variability,
};
/// Rumoca's parsed definition tree consumed by AST/fact extractors.
pub use rumoca_ir_ast::StoredDefinition;

pub mod resource_uri;
