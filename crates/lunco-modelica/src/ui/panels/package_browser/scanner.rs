//! Backend scanning logic for the Package Browser.

use bevy::prelude::*;
use std::path::{Path, PathBuf};
use crate::ui::state::ModelLibrary;
use super::types::{PackageNode, TwinNode};
use super::cache::TwinState;

// ─── Twin / Workspace Scanning ───────────────────────────────────────────────

pub fn scan_twin_folder(root: PathBuf) -> TwinState {
    let root_node = TwinNode {
        path: root.clone(),
        name: root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        children: scan_twin_children(&root),
        is_modelica: false,
    };
    TwinState {
        root,
        root_node,
    }
}

fn scan_twin_children(dir: &Path) -> Vec<TwinNode> {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in iter.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if should_skip(&name) {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let is_modelica = !is_dir
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("mo"))
                .unwrap_or(false);
        
        if is_dir {
            let children = scan_twin_children(&path);
            out.push(TwinNode {
                path,
                name,
                children,
                is_modelica: false,
            });
        } else if is_modelica {
            let display_name = name.strip_suffix(".mo").unwrap_or(&name).to_string();
            out.push(TwinNode {
                path,
                name: display_name,
                children: Vec::new(),
                is_modelica: true,
            });
        }
    }
    out.sort_by(|a, b| {
        b.is_modelica.cmp(&a.is_modelica).then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn should_skip(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target" | "shared_target" | "node_modules" | "__pycache__"
        )
}

// ─── MSL Scanning ────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub(crate) fn scan_msl_inmem(package_path: &str) -> Vec<PackageNode> {
    if crate::msl_remote::global_parsed_msl().is_none() {
        return Vec::new();
    }
    let tree = msl_inmem_index();
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
        let id = format!("msl_{}", qname.replace('.', "_"));
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
                library: ModelLibrary::MSL,
                class_kind: Some(*kind),
            });
        }
    }
    out.sort_by_key(omedit_sort_key);
    out
}

/// Third-party library roots present in the in-memory parsed bundle, minus
/// the MSL core and its required companions (those render under the dedicated
/// "Modelica Standard Library" root). Whatever remains is an extra library
/// shipped in the same bundle (`build_msl_assets --extra-root/--discover-extras`).
///
/// This is the web counterpart to [`discover_third_party_libs`]: on wasm there
/// is no filesystem cache to scan, so the palette derives its extra-lib roots
/// from the parsed AST that the bundle fetcher already installed.
#[cfg(target_arch = "wasm32")]
pub(crate) fn msl_inmem_top_level_libs() -> Vec<String> {
    const MSL_OWNED: &[&str] = &[
        "Modelica",
        "ModelicaServices",
        "Complex",
        "ModelicaReference",
        "ObsoleteModelica4",
    ];
    // Don't touch the `msl_inmem_index()` OnceLock before the parsed bundle is
    // resident — it would cache an empty tree permanently (same guard as
    // `scan_msl_inmem`).
    if crate::msl_remote::global_parsed_msl().is_none() {
        return Vec::new();
    }
    let tree = msl_inmem_index();
    let Some(top) = tree.get("") else {
        return Vec::new();
    };
    let mut libs: Vec<String> = top
        .iter()
        .map(|(short, _)| short.clone())
        .filter(|s| !MSL_OWNED.contains(&s.as_str()))
        .collect();
    libs.sort();
    libs.dedup();
    libs
}

#[cfg(target_arch = "wasm32")]
fn msl_inmem_index(
) -> &'static std::collections::HashMap<String, Vec<(String, crate::index::ClassKind)>> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        std::collections::HashMap<String, Vec<(String, crate::index::ClassKind)>>,
    > = OnceLock::new();
    CACHE.get_or_init(build_msl_inmem_index)
}

#[cfg(target_arch = "wasm32")]
fn build_msl_inmem_index(
) -> std::collections::HashMap<String, Vec<(String, crate::index::ClassKind)>> {
    use std::collections::HashMap;
    let mut tree: HashMap<String, Vec<(String, crate::index::ClassKind)>> = HashMap::new();
    let Some(parsed) = crate::msl_remote::global_parsed_msl() else {
        return tree;
    };

    fn walk(
        parent_qname: &str,
        short_name: &str,
        def: &rumoca_compile::parsing::ast::ClassDef,
        tree: &mut std::collections::HashMap<String, Vec<(String, crate::index::ClassKind)>>,
    ) {
        let qname = if parent_qname.is_empty() {
            short_name.to_string()
        } else {
            format!("{parent_qname}.{short_name}")
        };
        let kind = crate::index::map_class_type(&def.class_type);
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
pub(crate) fn scan_msl_dir_native(dir: &Path, package_path: String) -> Vec<PackageNode> {
    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                if name.starts_with('.') || name == "__MACOSX" { continue; }
                let sub_path = format!("{}.{}", package_path, name);
                let id = format!("msl_{}", sub_path.replace('.', "_").replace('/', "_"));
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
            let ast = rumoca_phase_parse::parse_to_recovered_ast(
                &source,
                &pkg_mo.display().to_string(),
            );
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
    Model, Block, Connector, Record, Function, Type, Other,
}

impl LeafKind {
    fn from_kind(kind: Option<crate::index::ClassKind>) -> Self {
        use crate::index::ClassKind;
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
/// MSL `Modelica.Blocks.Continuous` case).
fn node_from_modelica_file(path: &Path, qualified: &str, display_name: &str) -> PackageNode {
    let leaf_unknown = || PackageNode::Model {
        id: format!("msl_path:{}", qualified),
        name: display_name.to_string(),
        library: ModelLibrary::MSL,
        class_kind: None,
    };
    let Ok(source) = std::fs::read_to_string(path) else { return leaf_unknown(); };
    let ast = rumoca_phase_parse::parse_to_recovered_ast(&source, &path.display().to_string());
    let Some((_, top_class)) = ast.classes.iter().next() else { return leaf_unknown(); };
    class_def_to_node(path, qualified, display_name, top_class)
}

pub fn peek_class_kind_from_source(src: &str) -> Option<crate::index::ClassKind> {
    let ast = rumoca_phase_parse::parse_to_recovered_ast(src, "");
    ast.classes
        .iter()
        .next()
        .map(|(_, def)| crate::index::map_class_type(&def.class_type))
}

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
            .map(|(n, c)| class_def_to_node(path, &crate::ast_extract::qualify(qualified, n), n, c))
            .collect();
        children.sort_by_key(omedit_sort_key);
        PackageNode::Category {
            id: format!("msl_path:{}", qualified),
            name: short_name.to_string(),
            package_path: qualified.to_string(),
            fs_path: path.to_path_buf(),
            children: Some(children),
            is_loading: false,
        }
    } else {
        PackageNode::Model {
            id: format!("msl_path:{}", qualified),
            name: short_name.to_string(),
            library: ModelLibrary::MSL,
            class_kind: Some(crate::index::map_class_type(&def.class_type)),
        }
    }
}

pub fn discover_third_party_libs() -> Vec<(String, String)> {
    let cache = lunco_assets::cache_dir();
    let Ok(entries) = std::fs::read_dir(&cache) else { return Vec::new(); };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let subdir = entry.file_name().to_string_lossy().into_owned();
        if subdir == "msl" || subdir.starts_with('.') {
            continue;
        }
        let Ok(inner) = std::fs::read_dir(&path) else { continue; };
        for inner_entry in inner.flatten() {
            let inner_path = inner_entry.path();
            if inner_path.is_dir() && inner_path.join("package.mo").is_file() {
                let pkg = inner_entry.file_name().to_string_lossy().into_owned();
                out.push((subdir.clone(), pkg));
                break;
            }
        }
    }
    out.sort();
    out
}
