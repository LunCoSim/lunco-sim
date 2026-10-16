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
//! Foreign binary formats remain unresolved by the native USD reader. The render
//! projection reads the authored binary arc from its prim stack and imports it
//! through Bevy's asset pipeline. Native composition diagnostics remain visible.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    task::Poll,
};

use anyhow::{Result, anyhow, ensure};
use bevy::asset::{
    AssetPath, AssetServer, Handle, LoadContext, ReadAssetBytesError, io::AssetReaderError,
};
use lunco_assets_core::registered_read::read_registered_asset_bytes;
use openusd::usd::Stage;

use crate::asset::UsdLayerReadReceipt;
use lunco_usd_compose::recipe::{StageClosureLimits, StageDependencyDiagnostic, StageRecipe};
use lunco_usd_compose::{SharedLayerBytes, check_stage_closure_limits, child_layer_ids};

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
    input: StageRecipe,
    roots: Option<&lunco_assets_core::TwinRoots>,
) -> Result<FetchedStageClosure> {
    fetch_layer_closure_with_limits(load_context, input, StageClosureLimits::default(), roots).await
}

/// Fetch captured native layers and transitive sources within one resource budget.
/// Root/session inputs are required; missing transitive arcs remain diagnostics.
pub async fn fetch_layer_closure_with_limits(
    load_context: &mut LoadContext<'_>,
    input: StageRecipe,
    limits: StageClosureLimits,
    roots: Option<&lunco_assets_core::TwinRoots>,
) -> Result<FetchedStageClosure> {
    let origin = load_context.path().clone().into_owned();
    fetch_layer_closure_inner(
        &mut ClosureReader::Loader(load_context),
        input,
        limits,
        roots,
        origin,
    )
    .await
}

/// Prepare a fresh closure through the registered readers used by the loader.
/// One-shot queries retain their own result without installing a cached stage.
pub async fn fetch_layer_closure_from_asset_reader(
    asset_server: &AssetServer,
    origin: AssetPath<'static>,
    root_bytes: Vec<u8>,
    roots: Option<&lunco_assets_core::TwinRoots>,
    limits: StageClosureLimits,
) -> Result<StageRecipe> {
    let root_id = lunco_assets_core::asset_path::anchor_of(&origin);
    let input = StageRecipe::new(root_id.clone(), HashMap::from([(root_id, root_bytes)]));
    Ok(fetch_layer_closure_inner(
        &mut ClosureReader::Registered(asset_server),
        input,
        limits,
        roots,
        origin,
    )
    .await?
    .recipe)
}

enum ClosureReader<'a, 'b> {
    Loader(&'a mut LoadContext<'b>),
    Registered(&'a AssetServer),
}

async fn fetch_layer_closure_inner(
    transport: &mut ClosureReader<'_, '_>,
    input: StageRecipe,
    limits: StageClosureLimits,
    roots: Option<&lunco_assets_core::TwinRoots>,
    origin: AssetPath<'static>,
) -> Result<FetchedStageClosure> {
    ensure!(
        limits.max_parallel_reads > 0,
        "USD layer closure parallel read limit must be greater than zero"
    );
    let root_id = lunco_usd_compose::canonicalize_at(&input.root_id, None)?;
    ensure!(
        input.bytes.contains_key(&root_id),
        "required USD root `{root_id}` has no captured bytes"
    );
    let mut bytes = input.bytes;
    let mut total_bytes = bytes
        .values()
        .try_fold(0usize, |total, raw| total.checked_add(raw.len()))
        .ok_or_else(|| anyhow!("USD layer closure byte count overflowed"))?;
    let session_layer = input
        .session_layer
        .map(|id| lunco_usd_compose::canonicalize_at(&id, None))
        .transpose()?;
    let mut source_dependencies = Vec::new();
    if let Some(identifier) = &session_layer {
        ensure!(
            identifier != &root_id,
            "USD root and session must have distinct layer identities"
        );
        if !bytes.contains_key(identifier) {
            let path = lunco_assets_core::asset_path::load_asset_path(
                identifier,
                Some(&origin),
                roots,
                None,
            )
            .map_err(|error| anyhow!("invalid USD session load address `{identifier}`: {error}"))?;
            let raw = match transport {
                ClosureReader::Loader(context) => {
                    let mut child = context.begin_labeled_asset();
                    let raw = child.read_asset_bytes(path).await.map_err(|error| {
                        anyhow!("required USD session `{identifier}` could not be read: {error}")
                    })?;
                    let receipt = child.finish(UsdLayerReadReceipt);
                    source_dependencies.push(
                        context
                            .add_loaded_labeled_asset("usd-session-layer-read".to_owned(), receipt),
                    );
                    raw
                }
                ClosureReader::Registered(server) => read_registered_asset_bytes(
                    server,
                    &path,
                    limits.max_bytes.saturating_sub(total_bytes),
                )
                .await
                .map_err(|error| {
                    anyhow!("required USD session `{identifier}` could not be read: {error}")
                })?,
            };
            total_bytes = total_bytes
                .checked_add(raw.len())
                .ok_or_else(|| anyhow!("USD layer closure byte count overflowed"))?;
            bytes.insert(identifier.clone(), raw);
        }
    }
    check_stage_closure_limits(&limits, bytes.len(), 0, 0, total_bytes)?;
    let mut seen = bytes.keys().cloned().collect::<HashSet<_>>();
    let mut missing_ids = HashSet::new();
    let mut frontier = bytes
        .keys()
        .cloned()
        .map(|id| (id, 0usize))
        .collect::<Vec<_>>();
    frontier.sort();
    let mut dependency_diagnostics = Vec::new();
    let mut next_dependency_order = 0_usize;
    let mut source_label = 0_usize;

    while !frontier.is_empty() {
        let mut requests = Vec::new();
        let mut duplicate_referrers: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        for (id, depth) in frontier {
            let raw = bytes.get(&id).expect("frontier id is present in map");
            let child_ids = child_layer_ids(&id, raw)?;
            check_stage_closure_limits(&limits, seen.len(), depth, child_ids.len(), total_bytes)?;
            for child_id in child_ids {
                let dependency_order = next_dependency_order;
                next_dependency_order += 1;
                if !seen.insert(child_id.clone()) {
                    if missing_ids.contains(&child_id) {
                        dependency_diagnostics.push((
                            dependency_order,
                            StageDependencyDiagnostic::missing(id.clone(), child_id.clone()),
                        ));
                    } else if !bytes.contains_key(&child_id) {
                        duplicate_referrers
                            .entry(child_id)
                            .or_default()
                            .push((dependency_order, id.clone()));
                    }
                    continue;
                }
                let child_depth = depth + 1;
                check_stage_closure_limits(&limits, seen.len(), child_depth, 0, total_bytes)?;
                // The asset owner reconstructs source and filesystem path
                // separately, preserving literal filename characters as data.
                // AssetLoader executes on the I/O pool. Keep native lookup at
                // that boundary, while the recipe retains the authored USD id.
                let prepared = child_id.starts_with("file:").then(|| {
                    lunco_assets_core::asset_path::PreparedAssetPaths::prepare_on_worker(
                        [child_id.clone()],
                        Some(origin.clone()),
                        roots,
                    )
                });
                let child_path = lunco_assets_core::asset_path::load_asset_path(&child_id, Some(&origin), roots, prepared.as_ref())
                    .map_err(|error| anyhow!("USD dependency `{child_id}` referenced by `{id}` has an invalid load address: {error}"))?;
                requests.push((
                    id.clone(),
                    child_id,
                    child_path,
                    child_depth,
                    dependency_order,
                ));
            }
        }

        let mut next_frontier = Vec::new();
        for request_batch in requests.chunks(limits.max_parallel_reads) {
            let read_results = match transport {
                ClosureReader::Loader(load_context) => {
                    let reads = request_batch.iter().map(
                        |(referring_id, child_id, child_path, child_depth, dependency_order)| {
                            let mut child_context = load_context.begin_labeled_asset();
                            let referring_id = referring_id.clone();
                            let child_id = child_id.clone();
                            let child_path = child_path.clone();
                            let child_depth = *child_depth;
                            let dependency_order = *dependency_order;
                            async move {
                                let fetched = child_context.read_asset_bytes(child_path).await;
                                (
                                    dependency_order,
                                    referring_id,
                                    child_id,
                                    child_depth,
                                    Some(child_context),
                                    fetched,
                                )
                            }
                        },
                    );
                    join_all_ordered(reads).await
                }
                ClosureReader::Registered(asset_server) => {
                    let reads =
                        request_batch.iter().map(
                            |(
                                referring_id,
                                child_id,
                                child_path,
                                child_depth,
                                dependency_order,
                            )| async {
                                (
                                    *dependency_order,
                                    referring_id.clone(),
                                    child_id.clone(),
                                    *child_depth,
                                    None,
                                    read_registered_asset_bytes(
                                        asset_server,
                                        child_path,
                                        limits.max_bytes.saturating_sub(total_bytes),
                                    )
                                    .await,
                                )
                            },
                        );
                    join_all_ordered(reads).await
                }
            };
            let mut completed = Vec::with_capacity(read_results.len());

            for (dependency_order, referring_id, child_id, child_depth, child_context, result) in
                read_results
            {
                match result {
                    Ok(fetched) => {
                        total_bytes = total_bytes
                            .checked_add(fetched.len())
                            .ok_or_else(|| anyhow!("USD layer closure byte count overflowed"))?;
                        check_stage_closure_limits(
                            &limits,
                            seen.len(),
                            child_depth,
                            0,
                            total_bytes,
                        )?;
                        let label = format!("usd-layer-read-{source_label}");
                        source_label += 1;
                        completed.push((
                            label,
                            child_id,
                            child_depth,
                            fetched,
                            child_context.map(|context| context.finish(UsdLayerReadReceipt)),
                        ));
                    }
                    Err(error) if is_missing_asset_read(&error) => {
                        missing_ids.insert(child_id.clone());
                        dependency_diagnostics.push((
                            dependency_order,
                            StageDependencyDiagnostic::missing(referring_id, child_id.clone()),
                        ));
                        if let Some(referrers) = duplicate_referrers.remove(&child_id) {
                            dependency_diagnostics.extend(referrers.into_iter().map(
                                |(order, referring)| {
                                    (
                                        order,
                                        StageDependencyDiagnostic::missing(
                                            referring,
                                            child_id.clone(),
                                        ),
                                    )
                                },
                            ));
                        }
                    }
                    Err(error) => {
                        return Err(anyhow!(
                            "USD composition dependency `{child_id}` referenced by \
                             `{referring_id}` could not be fetched: {error}"
                        ));
                    }
                }
            }

            // Finish every child context before mutably registering its labeled
            // receipt with the parent LoadContext.
            for (label, child_id, child_depth, fetched, loaded_receipt) in completed {
                if let (ClosureReader::Loader(load_context), Some(receipt)) =
                    (&mut *transport, loaded_receipt)
                {
                    source_dependencies.push(load_context.add_loaded_labeled_asset(label, receipt));
                }
                bytes.insert(child_id.clone(), fetched);
                next_frontier.push((child_id, child_depth));
            }
        }
        frontier = next_frontier;
    }

    dependency_diagnostics.sort_by_key(|(order, _)| *order);
    let mut recipe = StageRecipe::new(root_id, bytes);
    recipe.session_layer = session_layer;
    recipe.dependency_diagnostics = dependency_diagnostics
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect();
    Ok(FetchedStageClosure {
        recipe,
        source_dependencies,
    })
}

/// Fetched stage data plus Bevy handles that retain the loader dependency graph
/// for the transitive source layers.
pub struct FetchedStageClosure {
    /// Send-safe bytes and diagnostics for the composed stage.
    pub recipe: StageRecipe,
    pub(crate) source_dependencies: Vec<Handle<UsdLayerReadReceipt>>,
}

async fn join_all_ordered<F>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output>
where
    F: Future,
{
    let mut futures = futures
        .into_iter()
        .map(|future| Some(Box::pin(future) as Pin<Box<F>>))
        .collect::<Vec<_>>();
    let mut outputs = std::iter::repeat_with(|| None)
        .take(futures.len())
        .collect::<Vec<Option<F::Output>>>();
    let mut remaining = futures.len();

    std::future::poll_fn(|context| {
        for index in 0..futures.len() {
            let output = match futures[index].as_mut() {
                Some(future) => match future.as_mut().poll(context) {
                    Poll::Ready(output) => Some(output),
                    Poll::Pending => None,
                },
                None => None,
            };
            if let Some(output) = output {
                outputs[index] = Some(output);
                futures[index] = None;
                remaining -= 1;
            }
        }
        if remaining == 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;

    outputs
        .into_iter()
        .map(|output| output.expect("all dependency reads completed"))
        .collect()
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
    let _span = bevy::log::info_span!("usd_live_open_stage").entered();
    lunco_usd_compose::open_recipe(
        recipe.clone(),
        lunco_usd_authoring::schema::native_registry(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc, task::Poll};

    fn controlled_read(
        request_order: usize,
        pending_polls: usize,
        completion_order: Rc<RefCell<Vec<usize>>>,
    ) -> impl Future<Output = (usize, String, Vec<u8>)> {
        let mut pending_polls = pending_polls;
        let layer_id = format!("root/branch-{request_order}.usda");
        let bytes = vec![request_order as u8, 0x5a];
        std::future::poll_fn(move |context| {
            if pending_polls == 0 {
                completion_order.borrow_mut().push(request_order);
                Poll::Ready((request_order, layer_id.clone(), bytes.clone()))
            } else {
                pending_polls -= 1;
                context.waker().wake_by_ref();
                Poll::Pending
            }
        })
    }

    fn closure_after_controlled_reads(
        delays: [usize; 4],
    ) -> (
        Vec<usize>,
        Vec<usize>,
        lunco_usd_compose::recipe::StageContentClosure,
    ) {
        let completion_order = Rc::new(RefCell::new(Vec::new()));
        let reads = delays
            .into_iter()
            .enumerate()
            .map(|(order, delay)| controlled_read(order, delay, completion_order.clone()));
        let ordered_reads = bevy::tasks::futures_lite::future::block_on(join_all_ordered(reads));
        let applied_order = ordered_reads
            .iter()
            .map(|(request_order, _, _)| *request_order)
            .collect();
        let mut bytes = HashMap::from([("root.usda".to_owned(), b"root".to_vec())]);
        for (_, layer_id, layer_bytes) in ordered_reads {
            bytes.insert(layer_id, layer_bytes);
        }
        let closure = StageRecipe::new("root.usda", bytes)
            .content_closure()
            .expect("the controlled reads contain the complete layer closure");
        (completion_order.borrow().clone(), applied_order, closure)
    }

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

    #[test]
    fn reverse_async_completion_keeps_authored_recipe_order_and_identity() {
        let (reverse_completion, reverse_application, reverse_closure) =
            closure_after_controlled_reads([3, 2, 1, 0]);
        let (authored_completion, authored_application, authored_closure) =
            closure_after_controlled_reads([0, 1, 2, 3]);

        assert_eq!(reverse_completion, [3, 2, 1, 0]);
        assert_eq!(authored_completion, [0, 1, 2, 3]);
        assert_eq!(reverse_application, [0, 1, 2, 3]);
        assert_eq!(authored_application, [0, 1, 2, 3]);
        assert_eq!(reverse_closure, authored_closure);
    }
}

/// Compose a USD layer from disk into a **live** [`Stage`] (read through
/// [`StageView`](crate::view::StageView), the production read path). Native +
/// synchronous, using the shared storage-based closure and explicit compiled
/// schema registry. Relative references retain their authored layer anchors.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file_to_stage(path: &std::path::Path) -> Result<Stage> {
    lunco_usd_compose::compose_file_to_stage(path, lunco_usd_authoring::schema::native_registry())
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
    lunco_usd_compose::compose_file_to_stage_with_assets(
        path,
        assets_root,
        lunco_usd_authoring::schema::native_registry(),
    )
}
