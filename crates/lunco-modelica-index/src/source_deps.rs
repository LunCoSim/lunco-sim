//! Modelica source-root dependency discovery.
//!
//! The AST crate owns the shared pure source-root traversal. This module keeps
//! the Modelica index API stable for compiler admission and the runtime
//! registry; loading and lifecycle state remain with their respective hosts.

use rumoca_compile::parsing::ast::StoredDefinition;
use std::collections::HashSet;

/// Walk an AST and return the top-level root segments of qualified type
/// references. Bare names resolve in the current document and built-in
/// scalar types are handled by Rumoca, so neither requires a source-root
/// admission.
pub fn scan_source_root_deps(ast: &StoredDefinition) -> HashSet<String> {
    lunco_modelica_ast::ast_extract::source_root_dependencies_from_ast(ast)
        .into_iter()
        .collect()
}

/// Scan source text for the source-root segments it references.
pub fn scan_source_root_deps_from_source(source: &str, uri: &str) -> HashSet<String> {
    lunco_modelica_ast::parse_to_ast(source, uri)
        .map(|ast| scan_source_root_deps(&ast))
        .unwrap_or_default()
}
