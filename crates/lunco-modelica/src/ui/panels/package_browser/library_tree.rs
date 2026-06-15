//! Library-tree provider — one cfg-free API for browsing the Modelica library
//! tree, with the native/web split hidden behind a trait.
//!
//! Callers ([`super::cache`], the lazy-expand tasks in [`super::mod`] /
//! [`super::render`]) ask the same two questions on both targets:
//!
//! - [`LibraryTree::library_roots`] — which top-level libraries exist
//!   (`"Modelica"` + any third-party libs), for the palette's root rows.
//! - [`LibraryTree::children`] — the immediate child nodes of a package path,
//!   for lazy expansion.
//!
//! Under the hood the backends keep their distinct mechanisms — exactly the
//! storage-layer pattern [`lunco_assets::msl::MslAssetSource`] uses for bytes,
//! lifted to structural queries (which need `ClassKind` + nesting, hence this
//! lives in `lunco-modelica`, not `lunco-assets`):
//!
//! - **web** ([`InMemoryLibraryTree`]) — walks the in-memory parsed bundle
//!   (`global_parsed_msl()`), no filesystem.
//! - **native** ([`FsLibraryTree`]) — lazy `std::fs` walk of `.cache/...`,
//!   parsing `package.mo` on expand. No eager bundle load, no new startup cost.

use super::types::PackageNode;

/// A browsable Modelica library. See module docs.
pub trait LibraryTree {
    /// Browsable top-level library package names — `"Modelica"` first, then any
    /// third-party libraries (sorted). MSL companion packages
    /// (`ModelicaServices`, `Complex`, …) are excluded; they belong under MSL,
    /// not as separate roots.
    ///
    /// On web this is empty-but-`["Modelica"]` until the bundle loads; the
    /// extras land once [`super::cache`] reconciles on `MslLoadState::Ready`.
    fn library_roots(&self) -> Vec<String>;

    /// Immediate child nodes of `package_path`, ready to render. Sub-packages
    /// come back as [`PackageNode::Category`] with `children: None` (lazy) and
    /// an empty `fs_path` — the provider, not the node, owns path mapping.
    fn children(&self, package_path: &str) -> Vec<PackageNode>;
}

/// MSL core + its required companions. These render under the single
/// "Modelica Standard Library" root, never as their own top-level libraries.
/// (Native-only: the web backend has its own copy in `scanner`.)
#[cfg(not(target_arch = "wasm32"))]
const MSL_OWNED: &[&str] = &[
    "Modelica",
    "ModelicaServices",
    "Complex",
    "ModelicaReference",
    "ObsoleteModelica4",
];

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

// ─── web: in-memory parsed bundle ────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub struct InMemoryLibraryTree;

#[cfg(target_arch = "wasm32")]
impl LibraryTree for InMemoryLibraryTree {
    fn library_roots(&self) -> Vec<String> {
        let mut roots = vec!["Modelica".to_string()];
        roots.extend(super::scanner::msl_inmem_top_level_libs());
        roots
    }

    fn children(&self, package_path: &str) -> Vec<PackageNode> {
        super::scanner::scan_msl_inmem(package_path)
    }
}

// ─── native: lazy filesystem walk ────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
pub struct FsLibraryTree;

#[cfg(not(target_arch = "wasm32"))]
impl LibraryTree for FsLibraryTree {
    fn library_roots(&self) -> Vec<String> {
        let mut roots = vec!["Modelica".to_string()];
        for (_subdir, pkg) in super::scanner::discover_third_party_libs() {
            roots.push(pkg);
        }
        roots
    }

    fn children(&self, package_path: &str) -> Vec<PackageNode> {
        super::scanner::scan_msl_dir_native(&fs_root_for(package_path), package_path.to_string())
    }
}

/// Map a qualified package path to its on-disk directory — what each node's
/// `fs_path` used to carry, now derived centrally so library nodes don't store
/// it. Lazy expansion only ever asks for directory-backed packages (inline-
/// package files are populated eagerly during their parent's scan), so a plain
/// `package.replace('.', "/")` join is sufficient.
#[cfg(not(target_arch = "wasm32"))]
fn fs_root_for(package_path: &str) -> std::path::PathBuf {
    let top = package_path.split('.').next().unwrap_or(package_path);
    let rel: std::path::PathBuf = package_path.split('.').collect();
    if MSL_OWNED.contains(&top) {
        // `msl_dir()` is the parent of `Modelica/`, so joining the full
        // qualified path (which starts with `Modelica`) lands correctly.
        return lunco_assets::msl_dir().join(&rel);
    }
    // Third-party lib: its cache subdir is the parent of `<Pkg>/`, so join the
    // full qualified path (which starts with `<Pkg>`) onto it.
    for (subdir, pkg) in super::scanner::discover_third_party_libs() {
        if pkg == top {
            return lunco_assets::cache_dir().join(subdir).join(&rel);
        }
    }
    // Unknown top-level — fall back under the MSL dir (scan returns empty if
    // absent, matching the old missing-fs_path behaviour).
    lunco_assets::msl_dir().join(&rel)
}

/// Build a palette root [`PackageNode::Category`] for a top-level library,
/// decorating MSL with its display name + stable id. Shared by
/// [`super::cache::PackageTreeCache::new`] and the on-ready reconcile so both
/// produce identical nodes.
pub fn library_root_node(lib: &str) -> PackageNode {
    let (id, name) = if lib == "Modelica" {
        ("msl_root".to_string(), "📚 Modelica Standard Library".to_string())
    } else {
        (format!("{lib}_root"), lib.to_string())
    };
    PackageNode::Category {
        id,
        name,
        package_path: lib.to_string(),
        fs_path: std::path::PathBuf::new(),
        children: None,
        is_loading: false,
    }
}
