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
pub mod resolved_interface;
pub mod rumoca_adapter;
pub mod source_dependencies;
pub mod source_memo;

/// Rumoca's parsed declaration classifications, exposed by the Modelica AST
/// boundary for runtime and authoring projections.
pub use rumoca_core::{
    Causality, ClassType, OpBinary, OpUnary, PredefinedComponentType, Span, Token, Variability,
};
/// Rumoca's parsed definition tree consumed by AST/fact extractors.
pub use rumoca_ir_ast::StoredDefinition;

use rumoca_ir_ast::ClassDef;

/// Structural conveniences for Rumoca's parsed definition tree that LunCo's
/// authoring and document layers share.
///
/// These methods navigate declarations already present in one source file;
/// Rumoca's compiler session remains responsible for semantic name resolution.
pub trait ModelicaDefinitionExt {
    /// Return a class declared in this definition by its qualified source path.
    fn class_by_path(&self, qualified_path: &str) -> Option<&ClassDef>;

    /// Return a mutable class declared in this definition by its qualified source path.
    fn class_by_path_mut(&mut self, qualified_path: &str) -> Option<&mut ClassDef>;

    /// List every declared class path, including nested declarations and `within`.
    fn class_paths(&self) -> Vec<String>;

    /// Resolve an exact or uniquely named class path within this definition.
    fn qualified_class_name(&self, name: &str) -> Option<String>;

    /// Remove this definition's `within` prefix from a qualified class path.
    fn relative_class_path(&self, qualified_path: &str) -> String;
}

impl ModelicaDefinitionExt for StoredDefinition {
    fn class_by_path(&self, qualified_path: &str) -> Option<&ClassDef> {
        let path = relative_path(self, qualified_path);
        class_at_path(&self.classes, &path)
    }

    fn class_by_path_mut(&mut self, qualified_path: &str) -> Option<&mut ClassDef> {
        let path = relative_path(self, qualified_path);
        class_at_path_mut(&mut self.classes, &path)
    }

    fn class_paths(&self) -> Vec<String> {
        let prefix = self
            .within
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        let prefix = rumoca_core::ComponentPath::from_flat_path(&prefix);
        let mut paths = Vec::new();
        collect_class_paths(&self.classes, &prefix, &mut paths);
        paths
    }

    fn qualified_class_name(&self, name: &str) -> Option<String> {
        let paths = self.class_paths();
        let exact = paths.iter().filter(|path| path.as_str() == name).count();
        if exact == 1 {
            return Some(name.to_owned());
        }

        let relative = paths
            .iter()
            .filter(|path| self.relative_class_path(path) == name)
            .collect::<Vec<_>>();
        if relative.len() == 1 {
            return relative.first().map(|path| (*path).clone());
        }

        let requested = rumoca_core::ComponentPath::from_flat_path(name);
        if requested.parts().len() != 1 {
            return None;
        }
        let short_name = requested.parts().first()?;
        let suffix = paths
            .iter()
            .filter(|path| {
                rumoca_core::ComponentPath::from_flat_path(path)
                    .parts()
                    .last()
                    == Some(short_name)
            })
            .collect::<Vec<_>>();
        (suffix.len() == 1).then(|| (*suffix[0]).clone())
    }

    fn relative_class_path(&self, qualified_path: &str) -> String {
        relative_path(self, qualified_path).to_flat_string()
    }
}

fn relative_path(
    definition: &StoredDefinition,
    qualified_path: &str,
) -> rumoca_core::ComponentPath {
    let path = rumoca_core::ComponentPath::from_flat_path(qualified_path);
    let prefix = definition
        .within
        .as_ref()
        .map(ToString::to_string)
        .map(|prefix| rumoca_core::ComponentPath::from_flat_path(&prefix))
        .unwrap_or_default();
    path.strip_prefix(&prefix).unwrap_or(path)
}

fn class_at_path<'a>(
    classes: &'a rumoca_ir_ast::AstIndexMap<String, ClassDef>,
    path: &rumoca_core::ComponentPath,
) -> Option<&'a ClassDef> {
    let mut parts = path.parts().iter();
    let mut class = classes.get(parts.next()?)?;
    for part in parts {
        class = class.classes.get(part)?;
    }
    Some(class)
}

fn class_at_path_mut<'a>(
    classes: &'a mut rumoca_ir_ast::AstIndexMap<String, ClassDef>,
    path: &rumoca_core::ComponentPath,
) -> Option<&'a mut ClassDef> {
    let mut parts = path.parts().iter();
    let mut class = classes.get_mut(parts.next()?)?;
    for part in parts {
        class = class.classes.get_mut(part)?;
    }
    Some(class)
}

fn collect_class_paths(
    classes: &rumoca_ir_ast::AstIndexMap<String, ClassDef>,
    prefix: &rumoca_core::ComponentPath,
    paths: &mut Vec<String>,
) {
    for (name, class) in classes {
        let path = prefix.join(&rumoca_core::ComponentPath::from_parts([name.clone()]));
        paths.push(path.to_flat_string());
        collect_class_paths(&class.classes, &path, paths);
    }
}

/// Split a Modelica path while preserving quoted identifiers and subscripts.
/// Rumoca validates the Modelica syntax; LunCo uses this shared segmentation
/// at boundaries that receive paths as strings.
pub fn modelica_path_parts(path: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut bracket_depth = 0usize;
    let mut quoted = false;
    for (index, byte) in path.bytes().enumerate() {
        match byte {
            b'\'' => quoted = !quoted,
            b'[' if !quoted => bracket_depth += 1,
            b']' if !quoted => bracket_depth = bracket_depth.saturating_sub(1),
            b'.' if !quoted && bracket_depth == 0 => {
                parts.push(path[start..index].to_owned());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(path[start..].to_owned());
    parts
}

pub mod resource_uri;

#[cfg(test)]
mod definition_ext_tests {
    use super::ModelicaDefinitionExt;

    #[test]
    fn definition_paths_include_within_and_nested_classes() {
        let definition = rumoca_phase_parse::parse_to_ast(
            "within Demo.Package; package Outer model Inner end Inner; end Outer;",
            "paths.mo",
        )
        .unwrap();

        assert_eq!(
            definition.class_paths(),
            ["Demo.Package.Outer", "Demo.Package.Outer.Inner"]
        );
        assert!(
            definition
                .class_by_path("Demo.Package.Outer.Inner")
                .is_some()
        );
        assert!(definition.class_by_path("Outer.Inner").is_some());
        assert_eq!(
            definition.relative_class_path("Demo.Package.Outer.Inner"),
            "Outer.Inner"
        );
    }

    #[test]
    fn mutable_class_paths_select_only_the_requested_declaration() {
        let mut definition = rumoca_phase_parse::parse_to_ast(
            "package P model A end A; model B end B; end P;",
            "mut-paths.mo",
        )
        .unwrap();
        let class = definition.class_by_path_mut("P.B").unwrap();
        class.partial = true;

        assert!(!definition.class_by_path("P.A").unwrap().partial);
        assert!(definition.class_by_path("P.B").unwrap().partial);
    }
}
