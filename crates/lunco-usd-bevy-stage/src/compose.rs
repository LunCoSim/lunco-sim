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
//!     synchronous, so all async fetching happens here, up front; a missing
//!     transitive layer is retained as a recoverable diagnostic.
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

use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};
use bevy::asset::{AssetPath, LoadContext, ReadAssetBytesError, io::AssetReaderError};
use openusd::usd::Stage;

use lunco_assets_path::canonicalize_root;

use lunco_usd_compose::recipe::{StageClosureLimits, StageDependencyDiagnostic, StageRecipe};
use lunco_usd_compose::{
    LuncoUsdResolver, SharedLayerBytes, check_stage_closure_limits, child_layer_ids,
};

fn is_missing_asset_read(error: &ReadAssetBytesError) -> bool {
    match error {
        ReadAssetBytesError::AssetReaderError(AssetReaderError::NotFound(_)) => true,
        ReadAssetBytesError::AssetReaderError(AssetReaderError::Io(error)) => {
            error.kind() == std::io::ErrorKind::NotFound
        }
        ReadAssetBytesError::Io { source, .. } => source.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}

/// Async BFS that fetches the available transitive `.usda` layer closure into
/// an in-memory, `Send` [`StageRecipe`]. The loader composes this recipe and
/// builds the initial `UsdStageProjectionPlan` before publishing the asset. The
/// live `!Send` stage is opened later by the canonical-stage owner when
/// authoring or incremental projection needs it.
///
/// The runtime adapter opens the same recipe in its live canonical-stage owner.
pub async fn fetch_layer_closure(
    load_context: &mut LoadContext<'_>,
    root_asset_path: &str,
    root_bytes: Vec<u8>,
) -> Result<StageRecipe> {
    fetch_layer_closure_with_limits(
        load_context,
        root_asset_path,
        root_bytes,
        StageClosureLimits::default(),
    )
    .await
}

/// Fetch a USD layer closure with an explicit resource budget.
///
/// A missing transitive layer is a recoverable USD composition diagnostic: the
/// remaining siblings continue loading and OpenUSD drops only the unresolved
/// arc. Every other reader error remains terminal because treating malformed,
/// forbidden, or unavailable storage as a missing file would hide the actual
/// ownership failure.
pub async fn fetch_layer_closure_with_limits(
    load_context: &mut LoadContext<'_>,
    root_asset_path: &str,
    root_bytes: Vec<u8>,
    limits: StageClosureLimits,
) -> Result<StageRecipe> {
    let root_id = canonicalize_root(root_asset_path);
    check_stage_closure_limits(&limits, 1, 0, 0, root_bytes.len())?;

    // 1. Pre-fetch BFS — keyed by the SAME canonical id the resolver will use.
    let mut total_bytes = root_bytes.len();
    let mut bytes: HashMap<String, Vec<u8>> = HashMap::new();
    bytes.insert(root_id.clone(), root_bytes);
    let mut seen = HashSet::from([root_id.clone()]);
    let mut missing_ids = HashSet::new();
    let mut queue = vec![(root_id.clone(), 0_usize)];
    let mut dependency_diagnostics = Vec::new();

    while let Some((id, depth)) = queue.pop() {
        let raw = bytes.get(&id).expect("queued id is present in map");
        let child_ids = child_layer_ids(&id, raw)?;
        check_stage_closure_limits(&limits, seen.len(), depth, child_ids.len(), total_bytes)?;
        for child_id in child_ids {
            if !seen.insert(child_id.clone()) {
                if missing_ids.contains(&child_id) {
                    dependency_diagnostics.push(StageDependencyDiagnostic::missing(
                        id.clone(),
                        child_id.clone(),
                    ));
                }
                continue;
            }
            let child_depth = depth + 1;
            check_stage_closure_limits(&limits, seen.len(), child_depth, 0, total_bytes)?;
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
                Err(error) if is_missing_asset_read(&error) => {
                    missing_ids.insert(child_id.clone());
                    dependency_diagnostics.push(StageDependencyDiagnostic::missing(
                        id.clone(),
                        child_id.clone(),
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(anyhow!(
                        "USD composition dependency `{child_id}` referenced by `{id}` could not \
                         be fetched: {error}"
                    ));
                }
            };
            total_bytes = total_bytes
                .checked_add(fetched.len())
                .ok_or_else(|| anyhow!("USD layer closure byte count overflowed"))?;
            check_stage_closure_limits(&limits, seen.len(), child_depth, 0, total_bytes)?;
            bytes.insert(child_id.clone(), fetched);
            queue.push((child_id, child_depth));
        }
    }

    let mut recipe = StageRecipe::new(root_id, bytes);
    recipe.dependency_diagnostics = dependency_diagnostics;
    Ok(recipe)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_not_found_reads_are_recoverable() {
        let not_found = ReadAssetBytesError::Io {
            path: "vessels/markers/waypoint.usda".into(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        let denied = ReadAssetBytesError::Io {
            path: "vessels/markers/waypoint.usda".into(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };

        assert!(is_missing_asset_read(&not_found));
        assert!(!is_missing_asset_read(&denied));
    }
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
