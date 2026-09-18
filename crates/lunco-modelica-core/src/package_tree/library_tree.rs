//! Source-tree provider — one cfg-free API for browsing explicitly installed
//! Modelica source roots, with the native/web split hidden behind a trait.
//!
//! The provider never invents a library identity. Native roots come from the
//! asset source registry; web roots come from the parsed source bundle. Both
//! backends expose the same package-tree queries.

use lunco_modelica_index::package_tree::types::PackageNode;

/// A browsable Modelica source tree.
pub trait LibraryTree {
    /// Top-level package names from explicitly installed source roots.
    fn library_roots(&self) -> Vec<String>;

    /// Immediate child nodes of a package path, ready to render.
    fn children(&self, package_path: &str) -> Vec<PackageNode>;
}

/// The process-wide provider, selected once per target.
pub fn library_tree() -> &'static dyn LibraryTree {
    #[cfg(target_arch = "wasm32")]
    {
        &InMemoryLibraryTree
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        &FsLibraryTree
    }
}

#[cfg(target_arch = "wasm32")]
pub struct InMemoryLibraryTree;

#[cfg(target_arch = "wasm32")]
impl LibraryTree for InMemoryLibraryTree {
    fn library_roots(&self) -> Vec<String> {
        super::scanner::library_inmem_top_level_libs()
    }

    fn children(&self, package_path: &str) -> Vec<PackageNode> {
        super::scanner::scan_library_inmem(package_path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FsLibraryTree;

#[cfg(not(target_arch = "wasm32"))]
impl LibraryTree for FsLibraryTree {
    fn library_roots(&self) -> Vec<String> {
        let mut roots = Vec::new();
        for source in lunco_assets_runtime::library::global_library_sources() {
            let base = source.base();
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path.join("package.mo").is_file()
                    && entry.file_name().to_str().is_some()
                {
                    roots.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }

    fn children(&self, package_path: &str) -> Vec<PackageNode> {
        super::scanner::scan_library_dir_native(
            &fs_root_for(package_path),
            package_path.to_string(),
        )
    }
}

/// Map a qualified package path to the explicitly installed filesystem root
/// that owns its top-level package. An unknown root yields an empty path so a
/// scan produces no fabricated children.
#[cfg(not(target_arch = "wasm32"))]
fn fs_root_for(package_path: &str) -> std::path::PathBuf {
    let top = package_path.split('.').next().unwrap_or(package_path);
    let rel: std::path::PathBuf = package_path.split('.').collect();
    for source in lunco_assets_runtime::library::global_library_sources() {
        let candidate = source.base().join(top);
        if candidate.is_dir() {
            return source.base().join(&rel);
        }
    }
    std::path::PathBuf::new()
}

/// Build a palette root for one top-level source package.
pub fn library_root_node(lib: &str) -> PackageNode {
    PackageNode::Category {
        id: format!("{lib}_root"),
        name: lib.to_string(),
        package_path: lib.to_string(),
        fs_path: std::path::PathBuf::new(),
        children: None,
        is_loading: false,
    }
}
