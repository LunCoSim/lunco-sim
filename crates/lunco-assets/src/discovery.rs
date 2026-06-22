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
//! `twin://` / `lunco://` schemes. Native walks the filesystem; the web build
//! (no filesystem) enumerates the engine library from a compile-time manifest
//! baked by `build.rs`, so the spawn/shader catalogs populate on wasm too.

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
/// dot (`"usda"`, `"wgsl"`). Native walks the disk; the wasm twin below reads
/// the baked manifest (engine library only).
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

/// Compile-time manifest of the engine asset library, baked by `build.rs` (the
/// browser has no filesystem to walk). Only `usda`/`wgsl` are baked.
#[cfg(target_arch = "wasm32")]
mod baked {
    include!(concat!(env!("OUT_DIR"), "/baked_asset_manifest.rs"));
}

/// Web: enumerate the engine library from the baked manifest. Twin roots are
/// http-served (TODO) so they contribute nothing yet — engine `assets/` only,
/// which is what the spawn/shader catalogs need to resolve replicated spawns
/// and built-in props. `abs_path` is the bare relative path: web consumers
/// can't read file contents (e.g. `read_spawn_meta` falls back to defaults).
#[cfg(target_arch = "wasm32")]
pub fn list_assets(_roots: &TwinRoots, ext: &str) -> Vec<AssetFile> {
    let suffix = format!(".{ext}");
    baked::BAKED_ASSET_RELS
        .iter()
        .filter(|r| r.ends_with(&suffix))
        .map(|&r| {
            let rel = r.to_string();
            AssetFile {
                asset_path: rel.clone(),
                stem: stem_of(&rel),
                abs_path: std::path::PathBuf::from(&rel),
                twin: None,
                rel,
            }
        })
        .collect()
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

fn stem_of(rel: &str) -> String {
    Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}
