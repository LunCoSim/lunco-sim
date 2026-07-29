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

use anyhow::{Result, anyhow};
use openusd::ar::ResolvedPath;
use openusd::sdf::Data;
use openusd::usd::Stage;
use openusd::usda;

pub use resolver::{LuncoUsdResolver, SharedLayerBytes, canonicalize_at, is_binary_asset};

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
    let root_id = match assets_root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(rel) => lunco_assets::engine_asset_uri(&lunco_assets::asset_path::slashed(rel)),
        None => lunco_assets::asset_path::canonicalize_root(&path.to_string_lossy()),
    };
    let root_bytes = lunco_assets::read_asset_file_bytes(path)
        .map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
    let mut bytes = HashMap::from([(root_id.clone(), root_bytes)]);
    let mut queue = vec![root_id.clone()];
    while let Some(id) = queue.pop() {
        let raw = bytes.get(&id).cloned().expect("queued USD layer is present");
        for child_id in child_layer_ids(&id, &raw)? {
            if bytes.contains_key(&child_id) {
                continue;
            }
            let child = lunco_assets::read_asset_bytes(&child_id, assets_root).map_err(|e| {
                anyhow!("failed to fetch sublayer {child_id} for {}: {e}", path.display())
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
        let highlight = parser.last_error_highlight()
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
