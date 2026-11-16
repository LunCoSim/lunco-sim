//! Source-tree provider — one cfg-free API for browsing installed Modelica
//! source roots, with the native/web split hidden behind a trait.
//!
//! The provider never invents a library identity. Native roots come from the
//! shared asset source store; web roots come from the parsed source bundle.

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use lunco_modelica_index::package_tree::types::PackageNode;

/// A browsable Modelica source tree.
pub trait LibraryTree {
    /// Top-level package names from explicitly installed source roots.
    fn library_roots(&self) -> Vec<String>;

    /// Immediate child nodes of a package path, ready to render.
    fn children(&self, package_path: &str) -> Result<Vec<PackageNode>, String>;

    /// Complete one root tree for an active search. Implementations expose
    /// immediate children; this default recursively resolves every unscanned
    /// package through the same provider used for ordinary browsing.
    fn complete_tree(&self, package_path: &str) -> Result<Vec<PackageNode>, String> {
        fn complete_nodes(
            provider: &(impl LibraryTree + ?Sized),
            nodes: &mut [PackageNode],
        ) -> Result<(), String> {
            for node in nodes {
                let PackageNode::Category {
                    package_path,
                    children,
                    ..
                } = node
                else {
                    continue;
                };
                if let Some(children) = children {
                    complete_nodes(provider, children)?;
                } else {
                    let mut descendants = provider.children(package_path)?;
                    complete_nodes(provider, &mut descendants)?;
                    *children = Some(descendants);
                }
            }
            Ok(())
        }

        let mut children = self.children(package_path)?;
        complete_nodes(self, &mut children)?;
        Ok(children)
    }
}

/// The process-wide provider, selected once per target.
pub fn library_tree() -> &'static dyn LibraryTree {
    #[cfg(target_arch = "wasm32")]
    {
        &InMemoryLibraryTree
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        &InstalledLibraryTree
    }
}

#[cfg(target_arch = "wasm32")]
pub struct InMemoryLibraryTree;

#[cfg(target_arch = "wasm32")]
impl LibraryTree for InMemoryLibraryTree {
    fn library_roots(&self) -> Vec<String> {
        super::scanner::library_inmem_top_level_libs()
    }

    fn children(&self, package_path: &str) -> Result<Vec<PackageNode>, String> {
        super::scanner::scan_library_inmem(package_path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct InstalledLibraryTree;

#[cfg(not(target_arch = "wasm32"))]
impl LibraryTree for InstalledLibraryTree {
    fn library_roots(&self) -> Vec<String> {
        let mut roots = Vec::new();
        for source in lunco_assets_runtime::library::global_library_sources() {
            for file in source.files() {
                if file.file_name().is_some_and(|name| name == "package.mo")
                    && file.components().count() == 2
                {
                    if let Some(name) = file
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                    {
                        roots.push(name.to_owned());
                    }
                }
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }

    fn children(&self, package_path: &str) -> Result<Vec<PackageNode>, String> {
        let file = lunco_modelica_library::package_paths::locate_library_file(package_path)
            .ok_or_else(|| format!("unknown source package `{package_path}`"))?;
        let relative: std::path::PathBuf =
            rumoca_core::qualified_name_segments(package_path).collect();
        for source in lunco_assets_runtime::library::global_library_sources() {
            if source.contains(&file) && source.base().join(&relative).join("package.mo") == file {
                return super::scanner::scan_library_source(source, &relative, package_path);
            }
        }
        Err(format!(
            "source package `{package_path}` has no admitted directory member"
        ))
    }
}

/// Build a palette root for one top-level source package.
pub fn library_root_node(lib: &str) -> PackageNode {
    PackageNode::Category {
        id: format!("{lib}_root"),
        name: lib.to_string(),
        package_path: lib.to_string(),
        children: None,
        is_loading: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_modelica_index::package_tree::types::ModelSource;
    use std::collections::HashMap;

    struct FixtureTree(HashMap<String, Vec<PackageNode>>);

    impl LibraryTree for FixtureTree {
        fn library_roots(&self) -> Vec<String> {
            vec!["Root".into()]
        }

        fn children(&self, package_path: &str) -> Result<Vec<PackageNode>, String> {
            self.0
                .get(package_path)
                .cloned()
                .ok_or_else(|| format!("unknown package `{package_path}`"))
        }
    }

    #[test]
    fn complete_tree_resolves_unscanned_packages_recursively() {
        let provider = FixtureTree(HashMap::from([
            (
                "Root".into(),
                vec![PackageNode::Category {
                    id: "root_one".into(),
                    name: "One".into(),
                    package_path: "Root.One".into(),
                    children: None,
                    is_loading: false,
                }],
            ),
            (
                "Root.One".into(),
                vec![PackageNode::Category {
                    id: "root_one_two".into(),
                    name: "Two".into(),
                    package_path: "Root.One.Two".into(),
                    children: None,
                    is_loading: false,
                }],
            ),
            (
                "Root.One.Two".into(),
                vec![PackageNode::Model {
                    id: "library_path:Root.One.Two.Leaf".into(),
                    name: "Leaf".into(),
                    library: ModelSource::Source,
                    class_kind: None,
                }],
            ),
        ]));

        let tree = provider.complete_tree("Root").unwrap();
        let PackageNode::Category {
            children: Some(one),
            ..
        } = &tree[0]
        else {
            panic!("the first package level was not completed");
        };
        let PackageNode::Category {
            children: Some(two),
            ..
        } = &one[0]
        else {
            panic!("the nested package level was not completed");
        };
        assert!(matches!(two[0], PackageNode::Model { ref name, .. } if name == "Leaf"));
    }
}
