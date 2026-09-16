//! Backend scanning logic for the Package Browser.

use bevy::prelude::*;
use lunco_modelica_index::package_tree::types::{ModelSource, PackageNode};
use std::path::Path;

/// Canonical tree-node id for a source-library / third-party class, keyed by its dotted
/// qualified name (`Modelica.Blocks.Examples.PID_Controller`).
///
/// SINGLE SOURCE OF TRUTH — every scanner site (native fs walk + web in-memory
/// walk) mints ids through here, and it MUST stay in sync with
/// [`crate::class_ref::ClassRef::parse_tree_id`] (the `library_path:` scheme it
/// reverses on click). The id is built straight from the dotted name; we never
/// substitute `.`→`_`. The lossy `library_<dots→underscores>` form
/// (`PID_Controller` ⇄ `PID.Controller`) is not emitted or parsed. Routing
/// both backends through one helper keeps them from diverging.
pub(crate) fn library_tree_id(qualified: &str) -> String {
    format!("library_path:{qualified}")
}

// ─── source library Scanning ────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub(crate) fn scan_library_inmem(package_path: &str) -> Vec<PackageNode> {
    if crate::library_remote::global_parsed_source_bundle().is_none() {
        return Vec::new();
    }
    let tree = library_inmem_index();
    let Some(children) = tree.get(package_path) else {
        return Vec::new();
    };
    let mut out: Vec<PackageNode> = Vec::with_capacity(children.len());
    for (short, kind) in children {
        let qname = if package_path.is_empty() {
            short.clone()
        } else {
            format!("{package_path}.{short}")
        };
        let has_children = tree.get(&qname).map(|v| !v.is_empty()).unwrap_or(false);
        let id = library_tree_id(&qname);
        if has_children {
            out.push(PackageNode::Category {
                id,
                name: short.clone(),
                package_path: qname,
                fs_path: std::path::PathBuf::new(),
                children: None,
                is_loading: false,
            });
        } else {
            out.push(PackageNode::Model {
                id,
                name: short.clone(),
                library: ModelSource::Source,
                class_kind: Some(*kind),
            });
        }
    }
    out.sort_by_key(omedit_sort_key);
    out
}

/// Top-level source roots present in the in-memory parsed bundle.
///
/// The bundle is an ordinary source store: every authored top-level package is
/// exposed, with no built-in allow-list or companion-package filtering.
#[cfg(target_arch = "wasm32")]
pub(crate) fn library_inmem_top_level_libs() -> Vec<String> {
    // Don't touch the `library_inmem_index()` OnceLock before the parsed bundle is
    // resident — it would cache an empty tree permanently (same guard as
    // `scan_library_inmem`).
    if crate::library_remote::global_parsed_source_bundle().is_none() {
        return Vec::new();
    }
    let tree = library_inmem_index();
    let Some(top) = tree.get("") else {
        return Vec::new();
    };
    let mut libs: Vec<String> = top.iter().map(|(short, _)| short.clone()).collect();
    libs.sort();
    libs.dedup();
    libs
}

#[cfg(target_arch = "wasm32")]
fn library_inmem_index(
) -> &'static std::collections::HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>>
{
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        std::collections::HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>>,
    > = OnceLock::new();
    CACHE.get_or_init(build_library_inmem_index)
}

#[cfg(target_arch = "wasm32")]
fn build_library_inmem_index(
) -> std::collections::HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>> {
    use std::collections::HashMap;
    let mut tree: HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>> =
        HashMap::new();
    let Some(parsed) = crate::library_remote::global_parsed_source_bundle() else {
        return tree;
    };

    fn walk(
        parent_qname: &str,
        short_name: &str,
        def: &rumoca_compile::parsing::ast::ClassDef,
        tree: &mut std::collections::HashMap<
            String,
            Vec<(String, lunco_modelica_index::index::ClassKind)>,
        >,
    ) {
        let qname = if parent_qname.is_empty() {
            short_name.to_string()
        } else {
            format!("{parent_qname}.{short_name}")
        };
        let kind = lunco_modelica_index::index::map_class_type(&def.class_type);
        tree.entry(parent_qname.to_string())
            .or_default()
            .push((short_name.to_string(), kind));
        for (child_short, child_def) in &def.classes {
            walk(&qname, child_short, child_def, tree);
        }
    }

    for (_uri, def) in parsed.iter() {
        let parent = def
            .within
            .as_ref()
            .map(|w| w.to_string())
            .unwrap_or_default();
        for (short, cdef) in &def.classes {
            walk(&parent, short, cdef, &mut tree);
        }
    }

    for entries in tree.values_mut() {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
    }
    tree
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn scan_library_dir_native(dir: &Path, package_path: String) -> Vec<PackageNode> {
    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                if name.starts_with('.') || name == "__MACOSX" {
                    continue;
                }
                let sub_path = format!("{}.{}", package_path, name);
                let id = library_tree_id(&sub_path);
                results.push(PackageNode::Category {
                    id,
                    name,
                    package_path: sub_path,
                    fs_path: path,
                    children: None, // Lazy load
                    is_loading: false,
                });
            } else if path.extension().map(|e| e == "mo").unwrap_or(false) {
                if name == "package.mo" {
                    continue;
                }
                let display_name = name.strip_suffix(".mo").unwrap_or(&name).to_string();
                let qualified = format!("{}.{}", package_path, display_name);
                results.push(node_from_modelica_file(&path, &qualified, &display_name));
            }
        }
    }

    let pkg_mo = dir.join("package.mo");
    if pkg_mo.is_file() {
        if let Ok(source) = std::fs::read_to_string(&pkg_mo) {
            let ast =
                lunco_modelica_ast::parse_to_recovered_ast(&source, &pkg_mo.display().to_string());
            if let Some((_, top_class)) = ast.classes.iter().next() {
                let existing_names: std::collections::HashSet<String> =
                    results.iter().map(|n| n.name().to_string()).collect();
                for (child_short, child_def) in &top_class.classes {
                    if existing_names.contains(child_short) {
                        continue;
                    }
                    let child_qualified = format!("{}.{}", package_path, child_short);
                    results.push(class_def_to_node(
                        &pkg_mo,
                        &child_qualified,
                        child_short,
                        child_def,
                    ));
                }
            }
        }
    }

    results.sort_by_key(omedit_sort_key);
    results
}

fn omedit_sort_key(n: &PackageNode) -> (SortGroup, String) {
    let group = match n.name() {
        "UsersGuide" => SortGroup::UsersGuide,
        "Examples" => SortGroup::Examples,
        _ => match n {
            PackageNode::Category { .. } => SortGroup::SubPackage,
            PackageNode::Model { class_kind, .. } => {
                SortGroup::Leaf(LeafKind::from_kind(*class_kind))
            }
        },
    };
    (group, n.name().to_lowercase())
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum SortGroup {
    UsersGuide,
    Examples,
    SubPackage,
    Leaf(LeafKind),
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum LeafKind {
    Model,
    Block,
    Connector,
    Record,
    Function,
    Type,
    Other,
}

impl LeafKind {
    fn from_kind(kind: Option<lunco_modelica_index::index::ClassKind>) -> Self {
        use lunco_modelica_index::index::ClassKind;
        match kind {
            Some(ClassKind::Model) => Self::Model,
            Some(ClassKind::Block) => Self::Block,
            Some(ClassKind::Connector) | Some(ClassKind::ExpandableConnector) => Self::Connector,
            Some(ClassKind::Record) | Some(ClassKind::OperatorRecord) => Self::Record,
            Some(ClassKind::Function) => Self::Function,
            Some(ClassKind::Type) => Self::Type,
            _ => Self::Other,
        }
    }
}

/// Build a tree node from a single `.mo` file using rumoca's AST —
/// no line-scanning heuristics. Single-class files become a
/// [`PackageNode::Model`] leaf with the parsed kind on the badge;
/// inline-package files (`package Foo … model X … end Foo;`) become
/// a [`PackageNode::Category`] whose children mirror the nested
/// classes, so the user can drill into individual entries (the
/// source library `Modelica.Blocks.Continuous` case).
///
/// Native-only, like its single caller [`scan_library_dir_native`]: it reads a `.mo`
/// off the on-disk source library tree. The web has no such tree — the Package Browser
/// there is built by [`scan_library_inmem`] from the parsed bundle
/// (`InMemoryLibraryTree`), which needs no file reads at all.
#[cfg(not(target_arch = "wasm32"))]
fn node_from_modelica_file(path: &Path, qualified: &str, display_name: &str) -> PackageNode {
    let leaf_unknown = || PackageNode::Model {
        id: library_tree_id(qualified),
        name: display_name.to_string(),
        library: ModelSource::Source,
        class_kind: None,
    };
    let Ok(source) = std::fs::read_to_string(path) else {
        return leaf_unknown();
    };
    let ast = lunco_modelica_ast::parse_to_recovered_ast(&source, &path.display().to_string());
    let Some((_, top_class)) = ast.classes.iter().next() else {
        return leaf_unknown();
    };
    class_def_to_node(path, qualified, display_name, top_class)
}

pub fn peek_class_kind_from_source(src: &str) -> Option<lunco_modelica_index::index::ClassKind> {
    let ast = lunco_modelica_ast::parse_to_recovered_ast(src, "");
    ast.classes
        .iter()
        .next()
        .map(|(_, def)| lunco_modelica_index::index::map_class_type(&def.class_type))
}

/// Native-only: both callers ([`scan_library_dir_native`], [`node_from_modelica_file`])
/// are. The web builds its nodes from the parsed bundle in [`scan_library_inmem`].
#[cfg(not(target_arch = "wasm32"))]
fn class_def_to_node(
    path: &Path,
    qualified: &str,
    short_name: &str,
    def: &rumoca_compile::parsing::ast::ClassDef,
) -> PackageNode {
    use rumoca_compile::parsing::ClassType;
    let is_package = matches!(def.class_type, ClassType::Package);
    if is_package && !def.classes.is_empty() {
        let mut children: Vec<PackageNode> = def
            .classes
            .iter()
            .map(|(n, c)| {
                class_def_to_node(
                    path,
                    &lunco_modelica_ast::ast_extract::qualify(qualified, n),
                    n,
                    c,
                )
            })
            .collect();
        children.sort_by_key(omedit_sort_key);
        PackageNode::Category {
            id: library_tree_id(qualified),
            name: short_name.to_string(),
            package_path: qualified.to_string(),
            fs_path: path.to_path_buf(),
            children: Some(children),
            is_loading: false,
        }
    } else {
        PackageNode::Model {
            id: library_tree_id(qualified),
            name: short_name.to_string(),
            library: ModelSource::Source,
            class_kind: Some(lunco_modelica_index::index::map_class_type(&def.class_type)),
        }
    }
}
