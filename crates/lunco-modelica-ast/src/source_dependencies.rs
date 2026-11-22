//! Source-root facts projected from parsed Modelica names.

use std::collections::{BTreeSet, HashMap};

use crate::ModelicaDefinitionExt;
use rumoca_ir_ast::{ClassDef, Import, Name, StoredDefinition};

/// External source roots referenced by declarations in one parsed source file.
///
/// Rumoca owns full name resolution. This source-level inventory only decides
/// which registered roots to admit before the compile session resolves those
/// names. Local declarations and import aliases are interpreted from the AST.
pub fn required_source_roots(definition: &StoredDefinition) -> BTreeSet<String> {
    let class_paths = definition.class_paths();
    let local_classes = class_paths.iter().cloned().collect::<BTreeSet<_>>();
    let mut roots = BTreeSet::new();
    for path in class_paths {
        if let Some(class) = definition.class_by_path(&path) {
            let scope = crate::modelica_path_parts(&path);
            collect_class_references(class, &scope, &local_classes, &mut roots);
        }
    }
    roots
}

fn collect_class_references(
    class: &ClassDef,
    scope: &[String],
    local_classes: &BTreeSet<String>,
    roots: &mut BTreeSet<String>,
) {
    let mut aliases = HashMap::<String, Vec<String>>::new();

    for import in &class.imports {
        let (path, global_scope) = match import {
            Import::Qualified {
                path, global_scope, ..
            }
            | Import::Renamed {
                path, global_scope, ..
            }
            | Import::Unqualified {
                path, global_scope, ..
            }
            | Import::Selective {
                path, global_scope, ..
            } => (name_parts(path), *global_scope),
        };
        add_root(&path, scope, global_scope, true, local_classes, roots);
        match import {
            Import::Qualified { .. } => {
                if let Some(alias) = path.last() {
                    aliases.insert(alias.clone(), path);
                }
            }
            Import::Renamed { alias, .. } => {
                aliases.insert(alias.text.to_string(), path);
            }
            Import::Selective { names, .. } => {
                for name in names {
                    let mut target = path.clone();
                    target.push(name.text.to_string());
                    aliases.insert(name.text.to_string(), target);
                }
            }
            Import::Unqualified { .. } => {}
        }
    }

    for extended in &class.extends {
        add_reference(&extended.base_name, scope, &aliases, local_classes, roots);
    }
    for component in class.components.values() {
        add_reference(&component.type_name, scope, &aliases, local_classes, roots);
        if let Some(constraint) = &component.constrainedby {
            add_reference(constraint, scope, &aliases, local_classes, roots);
        }
    }
    if let Some(constraint) = &class.constrainedby {
        add_reference(constraint, &scope, &aliases, local_classes, roots);
    }
}

fn add_reference(
    name: &Name,
    scope: &[String],
    aliases: &HashMap<String, Vec<String>>,
    local_classes: &BTreeSet<String>,
    roots: &mut BTreeSet<String>,
) {
    let path = name_parts(name);
    if path.len() < 2 || aliases.contains_key(&path[0]) {
        return;
    }
    add_root(&path, scope, false, false, local_classes, roots);
}

fn add_root(
    path: &[String],
    scope: &[String],
    global_scope: bool,
    include_unqualified: bool,
    local_classes: &BTreeSet<String>,
    roots: &mut BTreeSet<String>,
) {
    if path.is_empty() || (!include_unqualified && path.len() < 2) {
        return;
    }
    let root = &path[0];
    if rumoca_core::is_builtin_type(root)
        || resolves_locally(path, scope, global_scope, local_classes)
    {
        return;
    }
    roots.insert(root.clone());
}

fn resolves_locally(
    path: &[String],
    scope: &[String],
    global_scope: bool,
    local_classes: &BTreeSet<String>,
) -> bool {
    let first_scope = if global_scope { 0 } else { scope.len() };
    (0..=first_scope).rev().any(|length| {
        let mut candidate = scope[..length].to_vec();
        candidate.extend_from_slice(path);
        let candidate = candidate.join(".");
        local_classes.iter().any(|local| {
            local == &candidate
                || local.starts_with(&format!("{candidate}."))
                || candidate.starts_with(&format!("{local}."))
        })
    })
}

fn name_parts(name: &Name) -> Vec<String> {
    name.name
        .iter()
        .map(|token| token.text.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::required_source_roots;

    fn roots(source: &str) -> BTreeSet<String> {
        let definition = rumoca_phase_parse::parse_to_ast(source, "roots.mo").unwrap();
        required_source_roots(&definition)
    }

    #[test]
    fn external_imports_and_types_select_their_root_namespaces() {
        assert_eq!(
            roots(concat!(
                "within RunSpace; package P model M ",
                "import Units = Library.Units; ",
                "SampleLibrary.Blocks.Interfaces.RealInput x; ",
                "Units.Temperature t; ",
                "extends LunCo.Controls.Base; ",
                "end M; end P;"
            )),
            BTreeSet::from([
                "Library".to_owned(),
                "LunCo".to_owned(),
                "SampleLibrary".to_owned(),
            ])
        );
    }

    #[test]
    fn local_and_quoted_names_do_not_select_external_roots() {
        assert!(
            roots(concat!(
                "within RunSpace; package P type 'A.B' = Real; ",
                "model M P.'A.B' x; P.Missing y; end M; end P;"
            ))
            .is_empty()
        );
        assert!(roots("model M Real x; end M;").is_empty());
    }

    #[test]
    fn import_forms_keep_external_namespace_roots() {
        assert_eq!(
            roots(concat!(
                "model M import Bare; import A.B.*; ",
                "import C.D.{T,U}; import .Global.T; ",
                "Missing.Type x; end M;"
            )),
            BTreeSet::from(["A", "Bare", "C", "Global", "Missing"].map(str::to_owned))
        );
    }
}
