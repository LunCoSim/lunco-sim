//! Modelica source-root dependency discovery.
//!
//! This is the single pure scanner used by compiler admission and the
//! runtime source-root registry. It only inspects the parsed Modelica AST;
//! loading and lifecycle state belong to their respective hosts.

use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
use std::collections::HashSet;

/// Walk an AST and return the top-level root segments of qualified type
/// references. Bare names resolve in the current document and built-in
/// scalar types are handled by Rumoca, so neither requires a source-root
/// admission.
pub fn scan_source_root_deps(ast: &StoredDefinition) -> HashSet<String> {
    let mut qualified_names = HashSet::new();
    for class in ast.classes.values() {
        walk_class_qualified_types(class, &mut qualified_names);
    }
    qualified_names
        .into_iter()
        .filter_map(|name| name.split('.').next().map(str::to_owned))
        .filter(|root| {
            !root.is_empty() && !lunco_modelica_ast::ast_extract::is_builtin_type_name(root)
        })
        .collect()
}

/// Scan source text for the source-root segments it references.
pub fn scan_source_root_deps_from_source(source: &str, uri: &str) -> HashSet<String> {
    lunco_modelica_ast::parse_to_ast(source, uri)
        .map(|ast| scan_source_root_deps(&ast))
        .unwrap_or_default()
}

fn walk_class_qualified_types(class: &ClassDef, out: &mut HashSet<String>) {
    lunco_modelica_ast::ast_extract::walk_class_type_names(class, &mut |name| {
        if name.contains('.') {
            out.insert(name.to_owned());
        }
    });
    for import in &class.imports {
        use rumoca_compile::parsing::ast::Import;
        let path = match import {
            Import::Qualified { path, .. }
            | Import::Renamed { path, .. }
            | Import::Unqualified { path, .. }
            | Import::Selective { path, .. } => path.to_string(),
        };
        if path.contains('.') {
            out.insert(path);
        }
    }
}
