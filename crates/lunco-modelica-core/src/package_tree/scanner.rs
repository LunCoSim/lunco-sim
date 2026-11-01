//! Backend scanning logic for the Package Browser.

use lunco_modelica_index::package_tree::types::{ModelSource, PackageNode};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

/// Canonical source-library node identity, shared by inventory and parsed-bundle
/// projections and consumed by [`crate::class_ref::ClassRef::parse_tree_id`].
pub(crate) fn library_tree_id(qualified: &str) -> String {
    format!("library_path:{qualified}")
}

// ─── source library Scanning ────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub(crate) fn scan_library_inmem(package_path: &str) -> Result<Vec<PackageNode>, String> {
    if lunco_modelica_library::source_library::global_parsed_source_bundle().is_none() {
        return Err("source library is not ready".into());
    }
    let tree = library_inmem_index();
    let children = tree
        .get(package_path)
        .ok_or_else(|| format!("unknown source package `{package_path}`"))?;
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
    Ok(out)
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
    if lunco_modelica_library::source_library::global_parsed_source_bundle().is_none() {
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
fn library_inmem_index()
-> &'static std::collections::HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>>
{
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        std::collections::HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>>,
    > = OnceLock::new();
    CACHE.get_or_init(build_library_inmem_index)
}

#[cfg(target_arch = "wasm32")]
fn build_library_inmem_index()
-> std::collections::HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>> {
    use std::collections::HashMap;
    let mut tree: HashMap<String, Vec<(String, lunco_modelica_index::index::ClassKind)>> =
        HashMap::new();
    let Some(parsed) = lunco_modelica_library::source_library::global_parsed_source_bundle() else {
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
        tree.entry(qname.clone()).or_default();
        tree.entry(parent_qname.to_string())
            .or_default()
            .push((short_name.to_string(), kind));
        for (child_short, child_def) in &def.classes {
            walk(&qname, child_short, child_def, tree);
        }
    }

    for document in parsed.iter() {
        let def = document.definition();
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
pub(crate) fn scan_library_source(
    source: &lunco_assets_runtime::library::LibrarySource,
    directory: &Path,
    package_path: &str,
) -> Result<Vec<PackageNode>, String> {
    let mut results = Vec::new();
    let mut directories = std::collections::BTreeSet::new();
    let mut members: Vec<_> = source.files().collect();
    members.sort();
    for file in members {
        let Ok(relative) = file.strip_prefix(directory) else {
            continue;
        };
        let Some(first) = relative.components().next() else {
            continue;
        };
        let name = first
            .as_os_str()
            .to_str()
            .ok_or("source identity is not UTF-8")?;
        if relative.components().count() > 1 {
            if name.starts_with('.') || name == "__MACOSX" {
                continue;
            }
            let child = directory.join(name);
            if source.contains(&source.base().join(&child).join("package.mo")) {
                directories.insert(name.to_owned());
            }
        } else if file.extension().is_some_and(|extension| extension == "mo")
            && name != "package.mo"
        {
            let display_name = file
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or("source filename is not UTF-8")?;
            let qualified = lunco_modelica_ast::ast_extract::qualify(package_path, display_name);
            results.push(node_from_source_file(
                source,
                file,
                &qualified,
                display_name,
            )?);
        }
    }
    for name in directories {
        let qualified = lunco_modelica_ast::ast_extract::qualify(package_path, &name);
        results.push(PackageNode::Category {
            id: library_tree_id(&qualified),
            name: name.clone(),
            package_path: qualified,
            children: None,
            is_loading: false,
        });
    }
    let package_file = directory.join("package.mo");
    let parsed = parse_source_file(source, &package_file)?;
    let top = source_class(&parsed, package_path)?;
    let existing: std::collections::HashSet<String> =
        results.iter().map(|node| node.name().to_owned()).collect();
    for (name, definition) in &top.classes {
        if !existing.contains(name) {
            results.push(class_def_to_node(
                &lunco_modelica_ast::ast_extract::qualify(package_path, name),
                name,
                definition,
            ));
        }
    }
    results.sort_by_key(omedit_sort_key);
    Ok(results)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_source_file(
    source: &lunco_assets_runtime::library::LibrarySource,
    relative: &Path,
) -> Result<rumoca_compile::compile::ParsedSourceDocument, String> {
    let bytes = source
        .read(relative)
        .map_err(|error| format!("{}: {error}", relative.display()))?
        .ok_or_else(|| format!("source member disappeared: {}", relative.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|error| format!("{}: {error}", relative.display()))?;
    let uri = relative.to_str().ok_or("source identity is not UTF-8")?;
    rumoca_compile::compile::ParsedSourceDocument::parse(uri.to_owned(), text)
        .map_err(|error| format!("{uri}: {error:?}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn source_class<'a>(
    parsed: &'a rumoca_compile::compile::ParsedSourceDocument,
    qualified: &str,
) -> Result<&'a rumoca_compile::parsing::ast::ClassDef, String> {
    let definition = parsed.definition();
    if !definition
        .declared_class_names()
        .iter()
        .any(|name| name == qualified)
    {
        return Err(format!("source does not declare `{qualified}`"));
    }
    let short = rumoca_core::qualified_name_segments(qualified)
        .last()
        .ok_or("empty class identity")?;
    definition
        .classes
        .get(short)
        .ok_or_else(|| format!("source does not contain top-level `{qualified}`"))
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

/// Project an admitted source member into its package-tree node.
#[cfg(not(target_arch = "wasm32"))]
fn node_from_source_file(
    source: &lunco_assets_runtime::library::LibrarySource,
    path: &Path,
    qualified: &str,
    display_name: &str,
) -> Result<PackageNode, String> {
    let parsed = parse_source_file(source, path)?;
    let class = source_class(&parsed, qualified)?;
    Ok(class_def_to_node(qualified, display_name, class))
}

pub fn peek_class_kind_from_source(src: &str) -> Option<lunco_modelica_index::index::ClassKind> {
    let ast = rumoca_phase_parse::parse_to_recovered_ast(src, "");
    ast.classes
        .iter()
        .next()
        .map(|(_, def)| lunco_modelica_index::index::map_class_type(&def.class_type))
}

/// Native source-store projection; browser nodes come from the admitted parsed bundle.
#[cfg(not(target_arch = "wasm32"))]
fn class_def_to_node(
    qualified: &str,
    short_name: &str,
    def: &rumoca_compile::parsing::ast::ClassDef,
) -> PackageNode {
    use lunco_modelica_ast::ClassType;
    let is_package = matches!(def.class_type, ClassType::Package);
    if is_package && !def.classes.is_empty() {
        let mut children: Vec<PackageNode> = def
            .classes
            .iter()
            .map(|(n, c)| {
                class_def_to_node(
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_assets_runtime::library::{InMemoryLibrary, LibrarySource};
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

    #[test]
    fn source_tree_reads_admitted_members_and_rejects_invalid_source() {
        let mut files: HashMap<PathBuf, Vec<u8>> = [
            (
                "Probe/package.mo",
                "package Probe model Inline end Inline; end Probe;",
            ),
            ("Probe/A.mo", "within Probe; model A end A;"),
            (
                "Probe/Child/package.mo",
                "within Probe; package Child end Child;",
            ),
            ("Probe/Resources/notes.txt", "resource"),
        ]
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
        let store = |files| LibrarySource::InMemory(Arc::new(InMemoryLibrary { files }));
        let nodes =
            scan_library_source(&store(files.clone()), Path::new("Probe"), "Probe").unwrap();
        let names: std::collections::BTreeSet<_> = nodes.iter().map(|node| node.name()).collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["A", "Child", "Inline"])
        );
        files.insert(PathBuf::from("Probe/A.mo"), vec![0xff]);
        let error = scan_library_source(&store(files.clone()), Path::new("Probe"), "Probe")
            .err()
            .expect("invalid UTF-8 must remain a diagnostic");
        assert!(error.contains("A.mo"), "{error}");
        files.insert(PathBuf::from("Probe/A.mo"), b"model A".to_vec());
        assert!(scan_library_source(&store(files.clone()), Path::new("Probe"), "Probe").is_err());
        files.insert(
            PathBuf::from("Probe/A.mo"),
            b"within Other; model A end A;".to_vec(),
        );
        assert!(scan_library_source(&store(files.clone()), Path::new("Probe"), "Probe").is_err());
        files.insert(
            PathBuf::from("Probe/A.mo"),
            b"within Probe; model A end A;".to_vec(),
        );
        files.remove(Path::new("Probe/package.mo"));
        let error = scan_library_source(&store(files), Path::new("Probe"), "Probe")
            .err()
            .expect("missing package marker must fail");
        assert!(error.contains("package.mo"), "{error}");
    }
}
