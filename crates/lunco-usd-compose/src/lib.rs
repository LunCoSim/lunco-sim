//! Asset-backed OpenUSD composition.
//!
//! `lunco-assets` owns canonical asset identities and storage locations. This
//! crate owns the USD meaning of those bytes: sublayers, references, payloads,
//! variants, and OpenUSD stage assembly. It is deliberately below the Bevy
//! projector and the simulation umbrella, so tutorials and headless tools can
//! consume a composed stage without creating an upward dependency cycle.

mod resolver;

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};
use openusd::ar::ResolvedPath;
use openusd::sdf::Data;
use openusd::usd::Stage;
use openusd::usda;

pub use resolver::{canonicalize_at, is_binary_asset, LuncoUsdResolver, SharedLayerBytes};

/// True when `path` is a USD layer that can declare further asset dependencies.
pub fn is_usd_layer(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("usd" | "usda" | "usdc")
    )
}

/// Extract every raw dependency declared by a USDA layer, including
/// asset-valued attributes as well as composition arcs.
///
/// This is USD interpretation only. Traversal and storage access remain in
/// `lunco-assets`.
pub fn layer_dependency_arcs(text: &str) -> Option<Vec<String>> {
    let data = usda::parse(text).ok()?;
    Some(
        data.composition_asset_dependencies()
            .into_iter()
            .chain(data.asset_dependencies())
            .collect(),
    )
}

/// Compose a native USDA file using the canonical LunCo asset traversal.
///
/// The root is promoted to `lunco://` when it lives below an `assets/` root, so
/// all arcs in the closure use the same canonical identity space.
pub fn compose_file_to_stage(path: &Path) -> Result<Stage> {
    let assets_root = lunco_assets::shipped_asset_root(path);
    compose_file_to_stage_with_assets(path, assets_root)
}

/// Compose an authored file with an explicit shipped-asset root. Twin/campaign
/// callers use this when the scene itself is outside the library but its
/// `lunco://` arcs still target the engine asset source.
pub fn compose_file_to_stage_with_assets(path: &Path, assets_root: Option<&Path>) -> Result<Stage> {
    compose_file_to_stage_with_roots(path, assets_root, None)
}

/// Compose a document with the asset roots that belong to its provenance.
/// `twin_root` is used only for `twin://` arcs; the engine library remains
/// resolved through `assets_root`. Keeping both roots explicit prevents a
/// custom Twin from silently falling back to the process working directory.
pub fn compose_file_to_stage_with_roots(
    path: &Path,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<Stage> {
    let root_id = match assets_root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(rel) => lunco_assets::engine_asset_uri(&lunco_assets::asset_path::slashed(rel)),
        None => lunco_assets::asset_path::canonicalize_root(&path.to_string_lossy()),
    };
    let root_bytes = lunco_assets::read_asset_file_bytes(path)
        .map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
    let mut bytes = HashMap::from([(root_id.clone(), root_bytes)]);
    let mut queue = vec![root_id.clone()];
    while let Some(id) = queue.pop() {
        let raw = bytes
            .get(&id)
            .cloned()
            .expect("queued USD layer is present");
        for child_id in child_layer_ids(&id, &raw)? {
            if bytes.contains_key(&child_id) {
                continue;
            }
            let child =
                lunco_assets::read_asset_bytes_with_twin_root(&child_id, assets_root, twin_root)
                    .map_err(|e| {
                        anyhow!(
                            "failed to fetch sublayer {child_id} for {}: {e}",
                            path.display()
                        )
                    })?;
            bytes.insert(child_id.clone(), child);
            queue.push(child_id);
        }
    }
    Stage::builder()
        .resolver(LuncoUsdResolver::new(bytes))
        .open(&root_id)
        .map_err(|e| anyhow!("USD composition error: {e}"))
}

/// Discover a USDA layer's non-binary composition dependencies in canonical
/// asset identity space. Fetch adapters use this; they do not parse arcs.
pub fn child_layer_ids(id: &str, raw: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(raw).map_err(|e| anyhow!("layer {id} is not UTF-8: {e}"))?;
    let mut parser = usda::parser::Parser::new(text);
    let specs = parser.parse().map_err(|e| {
        let highlight = parser
            .last_error_highlight()
            .map(|h| format!("\n{}", h.render()))
            .unwrap_or_default();
        anyhow!("USD parse error in {id}: {e}{highlight}")
    })?;
    let data = Data::from_specs(specs);
    let anchor = ResolvedPath::new(id);
    Ok(data
        .composition_asset_dependencies()
        .into_iter()
        .filter(|arc| !is_binary_asset(arc))
        .map(|arc| canonicalize_at(&arc, Some(&anchor)))
        .collect())
}
