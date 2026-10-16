//! Resolver admission for atomic document commands. Workers only read immutable
//! layer snapshots. The command owner validates the exact lease before commit.

use super::*;
use lunco_core::{AppSceneExt, SceneResource, SceneTeardown};
use lunco_core_runtime::{
    AsyncWorkAdmission, AsyncWorkAdmissionPlugin, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority,
    SimulationProgress, SimulationProgressKey, SimulationProgressOwner,
};
use lunco_usd_compose::recipe::StageRecipe;
use lunco_workspace::PinnedDocumentRuntimeOwner;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

type Commit = Box<dyn FnOnce(&mut World) -> Result<Ack, String> + Send + Sync>;

pub(super) struct DocumentLease {
    owner: PinnedDocumentRuntimeOwner,
    generation: u64,
    revisions: (u64, u64, u64),
    origin: DocumentOrigin,
    stage: Option<bevy::asset::AssetId<UsdStageAsset>>,
    stage_identity: Option<u64>,
    recipe_revision: Option<(u64, u64)>,
}

impl DocumentLease {
    pub(super) fn capture(world: &World, doc: DocumentId) -> Result<Self, String> {
        let host = world
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(doc)
            .ok_or_else(|| format!("USD document {doc} is not open"))?;
        let document = host.document();
        let stage = world
            .get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .and_then(|backed| lunco_usd_bevy_twin::stage_asset_for_document(backed, doc));
        let canonical = lunco_usd_bevy_twin::canonical_stage_for_document(world, doc);
        Ok(Self {
            owner: PinnedDocumentRuntimeOwner::for_document(
                doc,
                world
                    .get_resource::<WorkspaceResource>()
                    .map(|workspace| &workspace.0),
            )?,
            generation: host.generation(),
            revisions: (
                document.base_revision(),
                document.runtime_revision(),
                document.view_revision(),
            ),
            origin: document.origin().clone(),
            stage,
            stage_identity: canonical.map(|stage| stage.identity()),
            recipe_revision: canonical.map(|stage| stage.layer_bytes_revision()),
        })
    }

    pub(super) fn document(&self) -> DocumentId {
        self.owner.document
    }

    pub(super) fn is_current(&self, world: &World) -> bool {
        let Some(host) = world
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(self.owner.document)
        else {
            return false;
        };
        let document = host.document();
        let replication = lunco_core_session::current_replication_owner_in(world);
        let canonical =
            lunco_usd_bevy_twin::canonical_stage_for_document(world, self.owner.document);
        self.owner.is_current(
            world
                .get_resource::<WorkspaceResource>()
                .map(|workspace| &workspace.0),
            replication.as_ref(),
        ) && host.generation() == self.generation
            && document.origin() == &self.origin
            && (
                document.base_revision(),
                document.runtime_revision(),
                document.view_revision(),
            ) == self.revisions
            && world
                .get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
                .and_then(|backed| {
                    lunco_usd_bevy_twin::stage_asset_for_document(backed, self.owner.document)
                })
                == self.stage
            && canonical.map(|stage| stage.layer_bytes_revision()) == self.recipe_revision
            && canonical.map(|stage| stage.identity()) == self.stage_identity
    }
}

struct PendingEdit {
    lease: DocumentLease,
    operation: u64,
    key: AsyncWorkKey,
    command_id: Option<u64>,
    correlation_id: Option<u64>,
    commit: Commit,
    snapshot: UsdDocument,
    completion: Arc<Mutex<Option<Result<StageRecipe, String>>>>,
}

#[derive(Default)]
struct Completed {
    ready: AtomicBool,
}

#[derive(Resource, Default)]
struct PendingAuthoring {
    next: u64,
    edits: BTreeMap<u64, PendingEdit>,
    completed: Arc<Completed>,
}

impl SceneResource for PendingAuthoring {
    fn clear_scene(&mut self) {
        self.edits.clear();
        self.completed = Arc::default();
        // Operation identity belongs to the application, including workers
        // that may finish after this scene has retired.
    }
}

pub(super) fn install(app: &mut App) {
    if !app.is_plugin_added::<AsyncWorkAdmissionPlugin>() {
        app.add_plugins(AsyncWorkAdmissionPlugin);
    }
    app.init_scene_resource::<PendingAuthoring>()
        .add_systems(Update, drain)
        .add_systems(SceneTeardown, retire)
        .add_observer(close_document);
}

fn release(world: &mut World, operation: u64) {
    if let Some(mut progress) = world.get_resource_mut::<SimulationProgress>() {
        progress.release(SimulationProgressKey {
            owner: SimulationProgressOwner::UsdDocumentAuthoring,
            operation_id: operation,
        });
    }
}

fn finish(world: &mut World, edit: PendingEdit, result: Result<Ack, String>) {
    release(world, edit.operation);
    if let Err(error) = &result {
        warn!("[UsdAuthoringPreparation] {error}");
    }
    finish_command_result(
        world,
        edit.command_id,
        edit.correlation_id,
        result,
        ApiErrorCode::CommandRejected,
    );
}

fn cancel(world: &mut World, edit: PendingEdit, reason: &str) {
    world
        .resource_mut::<AsyncWorkAdmission>()
        .cancel_queued(edit.key);
    world
        .resource::<PendingAuthoring>()
        .completed
        .ready
        .store(true, Ordering::Release);
    finish(world, edit, Err(reason.to_owned()));
}

fn retire(world: &mut World) {
    let edits = std::mem::take(&mut world.resource_mut::<PendingAuthoring>().edits);
    for (_, edit) in edits {
        cancel(world, edit, "USD edit cancelled because its scene retired");
    }
}

fn close_document(trigger: On<DocumentClosed>, mut commands: Commands) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let operations = world
            .resource::<PendingAuthoring>()
            .edits
            .iter()
            .filter_map(|(operation, edit)| (edit.lease.document() == doc).then_some(*operation))
            .collect::<Vec<_>>();
        for operation in operations {
            let edit = world
                .resource_mut::<PendingAuthoring>()
                .edits
                .remove(&operation)
                .expect("collected pending edit");
            cancel(
                world,
                edit,
                "USD edit cancelled because its document closed",
            );
        }
        retire_pending_usd_discards(world, Some(doc));
    });
}

fn drain(world: &mut World) {
    let completed = world.resource::<PendingAuthoring>().completed.clone();
    if !completed.ready.swap(false, Ordering::AcqRel) {
        return;
    }
    loop {
        let Some(operation) = world
            .resource::<PendingAuthoring>()
            .edits
            .keys()
            .next()
            .copied()
        else {
            break;
        };
        let completion = world.resource::<PendingAuthoring>().edits[&operation]
            .completion
            .clone();
        let Some(result) = completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            break;
        };
        let edit = world
            .resource_mut::<PendingAuthoring>()
            .edits
            .remove(&operation)
            .expect("front edit exists");
        if !edit.lease.is_current(world) {
            finish(
                world,
                edit,
                Err("USD edit preparation has a retired or changed document/stage lease".into()),
            );
            continue;
        }
        let recipe = match result {
            Ok(recipe) => recipe,
            Err(error) => {
                finish(world, edit, Err(error));
                continue;
            }
        };
        world
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .host_mut(edit.lease.document())
            .expect("validated document lease")
            .document_mut()
            .set_authoring_recipe(edit.lease.recipe_revision, Some(recipe));
        let PendingEdit {
            lease,
            command_id,
            correlation_id,
            commit,
            operation,
            snapshot,
            ..
        } = edit;
        let result = commit(world);
        if result.is_err() {
            if let Some(host) = world
                .resource_mut::<DocumentRegistry<UsdDocument>>()
                .host_mut(lease.document())
            {
                host.document_mut().set_authoring_recipe(
                    snapshot.authoring_recipe_revision(),
                    snapshot.authoring_recipe().cloned(),
                );
            }
        }
        release(world, operation);
        if let Err(error) = &result {
            warn!("[UsdAuthoringPreparation] {error}");
        }
        finish_command_result(
            world,
            command_id,
            correlation_id,
            result,
            ApiErrorCode::CommandRejected,
        );
    }
}

/// Prepare newly introduced composition arcs, then execute the original command
/// once at its owner. An admitted dependency closure can commit immediately.
/// One in-flight edit per document holds its immutable generation lease.
pub(super) fn run(
    world: &mut World,
    doc: DocumentId,
    ops: &[UsdOp],
    command_id: Option<u64>,
    correlation_id: Option<u64>,
    commit: impl FnOnce(&mut World) -> Result<Ack, String> + Send + Sync + 'static,
) {
    let preparation = capture(world, doc, ops);
    let (document, root_id, missing) = match preparation {
        Ok(Some(capture)) => capture,
        Ok(None) => {
            let result = commit(world);
            if let Err(error) = &result {
                warn!("[UsdAuthoringPreparation] {error}");
            }
            finish_command_result(
                world,
                command_id,
                correlation_id,
                result,
                ApiErrorCode::CommandRejected,
            );
            return;
        }
        Err(error) => {
            warn!("[UsdAuthoringPreparation] {error}");
            finish_command_result(
                world,
                command_id,
                correlation_id,
                Err(error),
                ApiErrorCode::CommandRejected,
            );
            return;
        }
    };
    let result = submit(
        world,
        doc,
        document,
        root_id,
        missing,
        command_id,
        correlation_id,
        Box::new(commit),
    );
    if let Err(error) = result {
        warn!("[UsdAuthoringPreparation] {error}");
        finish_command_result(
            world,
            command_id,
            correlation_id,
            Err(error),
            ApiErrorCode::CommandRejected,
        );
    }
}

fn capture(
    world: &mut World,
    doc: DocumentId,
    ops: &[UsdOp],
) -> Result<Option<(UsdDocument, String, BTreeSet<String>)>, String> {
    if world
        .resource::<PendingAuthoring>()
        .edits
        .values()
        .any(|edit| edit.lease.document() == doc)
    {
        return Err(format!(
            "USD document {doc} already has an edit awaiting dependency preparation"
        ));
    }
    refresh_authoring_recipe(world, doc)?;
    let document = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .ok_or_else(|| format!("USD document {doc} is not open"))?
        .document();
    let recipe = document.authoring_recipe();
    let (root_id, needs_closure) = match document.origin() {
        _ if document.authoring_recipe_revision().is_some() => {
            let recipe = recipe.ok_or("mounted USD authoring revision has no dependency recipe")?;
            (recipe.root_id.clone(), false)
        }
        DocumentOrigin::File { path, .. } => {
            let (root_id, _, _) = document_composition_context(world, doc, path)?;
            let needs_closure = if let Some(recipe) = recipe {
                recipe.root_id != root_id
            } else {
                let mut has_layers = false;
                for layer in [
                    document.data(),
                    document.runtime_data(),
                    document.view_data(),
                ] {
                    let paths = lunco_usd_compose::layer_dependency_paths(layer)
                        .map_err(|error| error.to_string())?;
                    has_layers |= paths
                        .iter()
                        .any(|path| lunco_usd_compose::is_usd_layer(Path::new(path)));
                }
                has_layers
            };
            (root_id, needs_closure)
        }
        _ => {
            let root_id = if let Some(recipe) = recipe {
                recipe.root_id.clone()
            } else {
                lunco_usd_authoring::author::document_stage_recipe(
                    document.data(),
                    document.runtime_data(),
                    document.view_data(),
                    None,
                )
                .map_err(|error| error.to_string())?
                .root_id
            };
            (root_id, false)
        }
    };
    let mut missing = BTreeSet::new();
    for op in ops {
        for id in op
            .composition_dependencies(&root_id)
            .map_err(|error| error.to_string())?
        {
            if recipe.is_none_or(|recipe| !recipe.bytes.contains_key(&id)) {
                missing.insert(id);
            }
        }
    }
    if missing.is_empty() && !needs_closure {
        Ok(None)
    } else {
        Ok(Some((document.clone(), root_id, missing)))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn submit(
    world: &mut World,
    doc: DocumentId,
    document: UsdDocument,
    root_id: String,
    missing: BTreeSet<String>,
    command_id: Option<u64>,
    correlation_id: Option<u64>,
    commit: Commit,
) -> Result<(), String> {
    let (assets_root, twin_root) = match document.origin() {
        DocumentOrigin::File { path, .. } => {
            let (_, assets, twin) = document_composition_context(world, doc, path)?;
            (assets, twin)
        }
        _ => (
            Some(lunco_assets_core::assets_dir_abs().map_err(|error| error.to_string())?),
            None,
        ),
    };
    let lease = DocumentLease::capture(world, doc)?;
    let worker_document = document.clone();
    let generation = lease.generation;
    let authoritative = lease
        .stage
        .is_some_and(|stage| lunco_usd_bevy_scene::is_authoritative_scene_stage(world, stage));
    let priority = if authoritative {
        AsyncWorkPriority::SimulationRequired
    } else {
        AsyncWorkPriority::Interactive
    };
    let mut pending = world.resource_mut::<PendingAuthoring>();
    let operation = pending
        .next
        .checked_add(1)
        .ok_or("USD preparation operation identity exhausted")?;
    pending.next = operation;
    let completed = Arc::downgrade(&pending.completed);
    let completion = Arc::new(Mutex::new(None));
    let result_slot = Arc::downgrade(&completion);
    let key = AsyncWorkKey::new(
        AsyncWorkKind::UsdDocumentAuthoring,
        0,
        u128::from(doc.raw()),
        generation,
        operation,
    );
    world
        .resource_mut::<AsyncWorkAdmission>()
        .submit_ordered(priority, key, operation, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let recipe = worker_document
                    .stage_recipe(&root_id, true)
                    .map_err(|error| error.to_string())?;
                prepare(
                    recipe,
                    missing,
                    assets_root.as_deref(),
                    twin_root.as_deref(),
                )
            }))
            .unwrap_or_else(|_| {
                Err("USD edit dependency preparation panicked; the document was not changed".into())
            });
            if let Some(slot) = result_slot.upgrade() {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
                if let Some(completed) = completed.upgrade() {
                    completed.ready.store(true, Ordering::Release);
                }
            }
        })
        .map_err(|error| format!("USD edit dependency preparation rejected: {error:?}"))?;
    world.resource_mut::<PendingAuthoring>().edits.insert(
        operation,
        PendingEdit {
            lease,
            operation,
            key,
            command_id,
            correlation_id,
            commit,
            snapshot: document,
            completion,
        },
    );
    if authoritative {
        if let Some(mut progress) = world.get_resource_mut::<SimulationProgress>() {
            progress.acquire(
                SimulationProgressKey {
                    owner: SimulationProgressOwner::UsdDocumentAuthoring,
                    operation_id: operation,
                },
                format!("Preparing USD edit dependencies for {doc}"),
            );
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn submit(
    _world: &mut World,
    _doc: DocumentId,
    _document: UsdDocument,
    _root_id: String,
    _missing: BTreeSet<String>,
    _command_id: Option<u64>,
    _correlation_id: Option<u64>,
    _commit: Commit,
) -> Result<(), String> {
    Err("USD dependency preparation requires the native worker transport".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare(
    mut recipe: StageRecipe,
    missing: BTreeSet<String>,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<StageRecipe, String> {
    let limits = lunco_usd_compose::recipe::StageClosureLimits::default();
    if missing.len() > limits.max_dependencies_per_layer {
        return Err("USD edit dependency count exceeds the composition budget".into());
    }
    let mut retained = recipe
        .bytes
        .values()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()))
        .ok_or("USD edit dependency byte count overflowed")?;
    for id in missing {
        if recipe.bytes.len() >= limits.max_layers {
            return Err("USD edit dependency layer budget exceeded".into());
        }
        let bytes = lunco_assets_core::read_asset_bytes_bounded_with_twin_root(
            &id,
            assets_root,
            twin_root,
            limits.max_bytes.saturating_sub(retained),
        )
        .map_err(|error| format!("USD edit dependency {id}: {error}"))?;
        retained = retained
            .checked_add(bytes.len())
            .ok_or("USD edit dependency byte count overflowed")?;
        recipe.bytes.insert(id, bytes);
    }
    let recipe = lunco_usd_compose::fetch_recipe_with_roots(recipe, assets_root, twin_root, limits)
        .map_err(|error| error.to_string())?;
    if !recipe.dependency_diagnostics.is_empty() {
        return Err(recipe
            .dependency_diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "));
    }
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_rejects_pending_edits_and_releases_their_owned_values() {
        let mut app = App::new();
        app.init_resource::<AsyncWorkAdmission>()
            .init_resource::<CommandResults>()
            .init_resource::<SimulationProgress>()
            .init_scene_resource::<PendingAuthoring>()
            .add_systems(SceneTeardown, retire);
        let captured = Arc::new(());
        let captured_weak = Arc::downgrade(&captured);
        let completion = Arc::new(Mutex::new(None));
        let completion_weak = Arc::downgrade(&completion);
        let doc = DocumentId::new(17);
        let operation = 9;
        let key = AsyncWorkKey::new(
            AsyncWorkKind::UsdDocumentAuthoring,
            0,
            u128::from(doc.raw()),
            0,
            operation,
        );
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .acquire(
                SimulationProgressKey {
                    owner: SimulationProgressOwner::UsdDocumentAuthoring,
                    operation_id: operation,
                },
                "pending edit",
            );
        let mut pending = app.world_mut().resource_mut::<PendingAuthoring>();
        pending.next = operation;
        pending.edits.insert(
            operation,
            PendingEdit {
                lease: DocumentLease {
                    owner: PinnedDocumentRuntimeOwner {
                        document: doc,
                        runtime: DocumentRuntimeOwner::Application,
                    },
                    generation: 0,
                    revisions: (0, 0, 0),
                    origin: DocumentOrigin::Untitled {
                        name: "edit".into(),
                    },
                    stage: None,
                    stage_identity: None,
                    recipe_revision: None,
                },
                operation,
                key,
                command_id: Some(23),
                correlation_id: None,
                completion,
                snapshot: UsdDocument::new(doc, "#usda 1.0\n"),
                commit: Box::new(move |_| {
                    drop(captured);
                    panic!("retired edit must never commit")
                }),
            },
        );
        lunco_core::run_scene_teardown(app.world_mut());
        assert!(
            captured_weak.upgrade().is_none(),
            "teardown must release captured command resources"
        );
        assert!(
            completion_weak.upgrade().is_none(),
            "late workers cannot retain a publication destination"
        );
        assert!(matches!(
            app.world().resource::<CommandResults>().get(23),
            Some(lunco_core::CommandOutcome::Rejected(_))
        ));
        assert_eq!(
            app.world()
                .resource::<SimulationProgress>()
                .blockers()
                .count(),
            0
        );
        let pending = app.world().resource::<PendingAuthoring>();
        assert!(pending.edits.is_empty());
        assert_eq!(
            pending.next, operation,
            "operation identities must survive scene replacement"
        );
    }
}
