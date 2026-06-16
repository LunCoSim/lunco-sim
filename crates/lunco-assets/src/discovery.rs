//! Project-wide USD asset discovery.
//!
//! One DRY source of truth for "what `*.usda` files exist in the project" —
//! the engine asset *library* (`<cwd>/assets`, the default/`lunco://` source)
//! plus every open Twin root (`twin://<name>/…`). Consumers (the spawn
//! catalog, pickers, the API) call [`list_usd_assets`] instead of each
//! re-walking the disk with their own bespoke scan.
//!
//! Lives in `lunco-assets` because this crate already owns *where assets live*
//! — the [`TwinRoots`](crate::twin_source::TwinRoots) registry and the
//! `twin://` / `lunco://` schemes. Native-only (the web build has no
//! filesystem); returns an empty list on wasm so callers compile uniformly.

use std::path::Path;

use crate::twin_source::TwinRoots;

/// A USD file discovered somewhere in the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsdAsset {
    /// Loadable Bevy asset path. Engine-relative (`vessels/rovers/skid_rover.usda`,
    /// served by the default source) or Twin-scoped (`twin://moonbase/structures/habitat_fsh.usda`).
    pub asset_path: String,
    /// File stem (`skid_rover`, `habitat_fsh`) — a stable id for catalogs.
    pub stem: String,
    /// Path relative to its own root (`vessels/rovers/skid_rover.usda`,
    /// `structures/habitat_fsh.usda`). Use for category heuristics.
    pub rel: String,
    /// Absolute on-disk path — for consumers that read the file's USD metadata
    /// (e.g. a per-asset spawn-height attribute) without re-resolving the
    /// asset path. Native-only; on wasm there is no list to populate.
    pub abs_path: std::path::PathBuf,
    /// Open-Twin name this came from, or `None` for the engine library.
    pub twin: Option<String>,
}

/// List every `*.usda` in the project: the engine `assets/` library first,
/// then each open Twin root (sorted by name). Recurses subdirectories,
/// skipping hidden dirs and `target/`. Native-only — empty on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_usd_assets(roots: &TwinRoots) -> Vec<UsdAsset> {
    let mut out = Vec::new();

    // Engine library under `<cwd>/assets`, addressed by the default source
    // (plain relative paths — matches how the catalog already loads rovers).
    let assets_dir = std::env::current_dir().unwrap_or_default().join("assets");
    walk_usda(&assets_dir, &assets_dir, &mut |rel| {
        out.push(UsdAsset {
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
            walk_usda(&root, &root, &mut |rel| {
                out.push(UsdAsset {
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
pub fn list_usd_assets(_roots: &TwinRoots) -> Vec<UsdAsset> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn walk_usda(base: &Path, dir: &Path, f: &mut impl FnMut(String)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            match p.file_name().and_then(|s| s.to_str()) {
                Some(n) if n.starts_with('.') || n == "target" => continue,
                _ => walk_usda(base, &p, f),
            }
        } else if p.extension().and_then(|s| s.to_str()) == Some("usda") {
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
