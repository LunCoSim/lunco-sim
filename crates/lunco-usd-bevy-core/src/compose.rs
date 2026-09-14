//! Compose USD from an in-memory layer closure with OpenUSD's composition
//! engine. The asset loader uses this path on its async boundary to prepare the
//! initial [`UsdStageProjectionPlan`](crate::UsdStageProjectionPlan); the live
//! canonical stage is built separately on the main thread by the runtime
//! adapter for authoring and incremental edits. Neither representation
//! is flattened.
//!
//! Pipeline:
//!  1. **Pre-fetch BFS** ([`fetch_layer_closure`]) — discover every
//!     transitively-referenced `.usda` and fetch its bytes via
//!     `LoadContext::read_asset_bytes` (native + wasm, routed through Bevy's
//!     `AssetServer` + our registered sources). openusd's resolver is
//!     synchronous, so all async fetching happens here, up front.
//!  2. **Prepare** ([`UsdStageProjectionPlan::from_recipe`]) — compose the
//!     fetched closure with the same PCP engine and snapshot the composed
//!     hierarchy, default-time values, transforms, material bindings, and
//!     animation topology into owned `Send` data for initial projection.
//!  3. **Live composition** ([`build_stage_with_resolver`]) — when the runtime
//!     needs an editable `!Send` stage, the same recipe is opened on the main
//!     thread. `StageView` then serves authored edits and incremental reads.
//!
//! Binary assets (`.glb`/`.gltf`/…) are not USD layers: the resolver routes them
//! to an empty composition stub, while the render projection reads the
//! authored binary arc directly from the live prim stack. That keeps the USD
//! payload/reference as the only asset identity and means live authoring and
//! referenced wrappers use the same path (openusd has no `SdfFileFormat` plugin
//! system).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bevy::asset::{AssetPath, LoadContext};
use openusd::usd::Stage;

use lunco_assets_core::asset_path::canonicalize_root;

use lunco_usd_compose::{child_layer_ids, LuncoUsdResolver, SharedLayerBytes};
use lunco_usd_core::StageRecipe;

/// Async BFS that fetches the full transitive `.usda` layer closure into an
/// in-memory, `Send` [`StageRecipe`]. The loader composes this recipe and builds
/// the initial `UsdStageProjectionPlan` before publishing the asset. The live
/// `!Send` stage is opened later by the canonical-stage owner when authoring or
/// incremental projection needs it.
///
/// The runtime adapter opens the same recipe in its live canonical-stage owner.
pub async fn fetch_layer_closure(
    load_context: &mut LoadContext<'_>,
    root_asset_path: &str,
    root_bytes: Vec<u8>,
) -> Result<StageRecipe> {
    let root_id = canonicalize_root(root_asset_path);

    // 1. Pre-fetch BFS — keyed by the SAME canonical id the resolver will use.
    let mut bytes: HashMap<String, Vec<u8>> = HashMap::new();
    bytes.insert(root_id.clone(), root_bytes);
    let mut queue = vec![root_id.clone()];

    while let Some(id) = queue.pop() {
        let raw = bytes
            .get(&id)
            .cloned()
            .expect("queued id is present in map");
        for child_id in child_layer_ids(&id, &raw)? {
            if bytes.contains_key(&child_id) {
                continue;
            }
            // Parse `child_id` as an `AssetPath` (NOT a `PathBuf`): only the
            // string form parses a `source://` scheme into an asset source.
            // `PathBuf::from("lunco://vessels/…")` keeps the whole string as a
            // default-source relative path → `assets/lunco://vessels/…` →
            // "Path not found". `AssetPath::parse` routes `lunco://…` to the
            // registered `lunco` source; plain relative ids stay default-source.
            let fetched = match load_context
                .read_asset_bytes(AssetPath::parse(&child_id).into_owned())
                .await
            {
                Ok(fetched) => fetched,
                Err(e) => {
                    return Err(anyhow!(
                        "USD composition dependency `{child_id}` referenced by `{id}` could not \
                         be fetched: {e}"
                    ));
                }
            };
            bytes.insert(child_id.clone(), fetched);
            queue.push(child_id);
        }
    }

    Ok(StageRecipe { root_id, bytes })
}

/// Build an editable stage and return its resolver's
/// [`SharedLayerBytes`] handle so a live-stage owner can inject additional layer
/// closures at runtime — the substrate for authoring a
/// **referenced spawn** onto a live stage: add the spawned asset's bytes here,
/// then author the `references` arc, and PCP composes the subtree on the next
/// read (demand-driven resolution).
///
/// The runtime adapter owns the live canonical stage.
pub fn build_stage_with_resolver(recipe: &StageRecipe) -> Result<(Stage, SharedLayerBytes)> {
    let resolver = LuncoUsdResolver::new(recipe.bytes.clone());
    let shared = resolver.shared();
    let stage = Stage::builder()
        .resolver(resolver)
        .open(&recipe.root_id)
        .map_err(|e| anyhow!("USD composition error: {e}"))?;
    Ok((stage, shared))
}

/// Compose a USD layer from disk into a **live** [`Stage`] (read through
/// [`StageView`](crate::view::StageView), the production read path). Native +
/// synchronous, backed by [`openusd::ar::DefaultResolver`] — for tests and tools
/// that load a real on-disk `.usda` with every reference resolved, distinct from
/// the async `AssetServer`-driven loader (the storage-based recipe path).
/// `DefaultResolver` anchors each relative reference to its own layer's
/// directory, so the on-disk reference tree resolves exactly as authored.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file_to_stage(path: &std::path::Path) -> Result<Stage> {
    lunco_usd_compose::compose_file_to_stage(path)
}

/// Compose an on-disk USD layer while resolving `lunco://` references against
/// an explicitly supplied shipped-asset root.
///
/// External Twin and campaign files do not live below the engine's `assets/`
/// directory, so their path cannot reveal where `lunco://` is mounted. Runtime
/// gets that mount from `AssetServer`; parse-only tools pass the same root here.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file_to_stage_with_assets(
    path: &std::path::Path,
    assets_root: Option<&std::path::Path>,
) -> Result<Stage> {
    lunco_usd_compose::compose_file_to_stage_with_assets(path, assets_root)
}
