//! Project-wide asset discovery.
//!
//! One DRY scanner for "what files of extension `ext` exist in the project" —
//! the engine asset *library* (`<cwd>/assets`, the default/`lunco://` source)
//! plus every open Twin root (`twin://<name>/…`). Consumers (the spawn catalog
//! for `usda`, the shader catalog for `wgsl`, pickers, the API) call
//! [`list_assets`] instead of each re-walking the disk with their own scan.
//!
//! Lives in `lunco-assets` because this crate already owns *where assets live*
//! — the [`TwinRoots`](crate::twin_source::TwinRoots) registry and the
//! `twin://` / `lunco://` schemes. Native-only (the web build has no
//! filesystem); returns an empty list on wasm so callers compile uniformly.

use std::path::Path;

use crate::twin_source::TwinRoots;

/// A file discovered somewhere in the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetFile {
    /// Loadable Bevy asset path. Engine-relative (`vessels/rovers/skid_rover.usda`,
    /// served by the default source) or Twin-scoped (`twin://moonbase/structures/habitat_fsh.usda`).
    pub asset_path: String,
    /// File stem (`skid_rover`, `regolith`) — a stable id for catalogs.
    pub stem: String,
    /// Path relative to its own root (`vessels/rovers/skid_rover.usda`,
    /// `shaders/regolith.wgsl`). Use for category heuristics.
    pub rel: String,
    /// Absolute on-disk path — for consumers that read the file's contents
    /// (e.g. a per-asset USD attribute) without re-resolving the asset path.
    pub abs_path: std::path::PathBuf,
    /// Open-Twin name this came from, or `None` for the engine library.
    pub twin: Option<String>,
}

/// List every `*.<ext>` in the project: the engine `assets/` library first,
/// then each open Twin root (sorted by name). Recurses subdirectories,
/// skipping hidden dirs and `target/`. `ext` is the bare extension without the
/// dot (`"usda"`, `"wgsl"`). Native-only — empty on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_assets(roots: &TwinRoots, ext: &str) -> Vec<AssetFile> {
    let mut out = Vec::new();

    // Engine library under `<cwd>/assets`, addressed by the default source
    // (plain relative paths — matches how the catalog already loads rovers).
    let assets_dir = std::env::current_dir().unwrap_or_default().join("assets");
    walk(&assets_dir, &assets_dir, ext, &mut |rel| {
        out.push(AssetFile {
            asset_path: rel.clone(),
            stem: stem_of(&rel),
            abs_path: assets_dir.join(&rel),
            twin: None,
            rel,
        });
    });

    // Open Twins → `twin://<name>/<rel>` so the `twin://` reader resolves them.
    for name in roots.names() {
        if let Some(root) = roots.root_of(&name) {
            walk(&root, &root, ext, &mut |rel| {
                out.push(AssetFile {
                    asset_path: format!("twin://{name}/{rel}"),
                    stem: stem_of(&rel),
                    abs_path: root.join(&rel),
                    twin: Some(name.clone()),
                    rel,
                });
            });
        }
    }

    out
}

/// wasm stub — no filesystem to walk.
#[cfg(target_arch = "wasm32")]
pub fn list_assets(_roots: &TwinRoots, _ext: &str) -> Vec<AssetFile> {
    Vec::new()
}

/// Convenience: every `*.usda` in the project. Thin wrapper over [`list_assets`].
pub fn list_usd_assets(roots: &TwinRoots) -> Vec<AssetFile> {
    list_assets(roots, "usda")
}

#[cfg(not(target_arch = "wasm32"))]
fn walk(base: &Path, dir: &Path, ext: &str, f: &mut impl FnMut(String)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            match p.file_name().and_then(|s| s.to_str()) {
                Some(n) if n.starts_with('.') || n == "target" => continue,
                _ => walk(base, &p, ext, f),
            }
        } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
            if let Ok(rel) = p.strip_prefix(base) {
                if let Some(rel_s) = rel.to_str() {
                    f(rel_s.replace('\\', "/"));
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn stem_of(rel: &str) -> String {
    Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}
