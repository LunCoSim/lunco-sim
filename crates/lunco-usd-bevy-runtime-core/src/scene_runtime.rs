//! Scene and Twin runtime orchestration for the USD projection stack.
//!
//! Document commands stay in lunco-usd-commands; this module owns scene
//! admission, Twin-backed loading, runtime overlays, and live-stage projection.

use std::path::Path;

use crate::scene::{
    ClearScene, LoadScene, SceneEntities, SceneLoadInFlight, SceneStageAssetOutcome,
    clear_scene_entities, resolve_root_prim, spawn_scene_root_world, validate_scene_address,
};
use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Document as _, OpenOutcome};
use lunco_doc_bevy::{DocumentChanged, DocumentRegistry, OpenFile};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_bevy_stage::{UsdStageAsset, source::UsdSourceBytes};
use lunco_usd_core::commands::{EmptyViewportReason, is_usd_path};
use lunco_usd_document::document::UsdDocument;
use lunco_workspace::open::{PendingTwinOpens, TwinOpenMode, spawn_twin_scan};
use lunco_workspace::{TwinClosed, WorkspaceResource};

/// Telemetry mnemonic for a default Twin scene whose authoritative source did
/// not become available. This is a scene-load failure, not a simulation fault:
/// the viewport remains empty and a later Twin replacement is still admitted.
pub(crate) const TWIN_SCENE_LOAD_FAILED: &str = "TWIN_SCENE_LOAD_FAILED";

#[derive(Clone)]
struct FailedTwinSceneRequest {
    doc: lunco_doc::DocumentId,
    path: String,
    root_prim: String,
    generation: u64,
}

/// Retains a failed Twin scene request until a new explicit scene transition.
#[derive(Resource, Default)]
pub(crate) struct FailedTwinSceneRequestState(Option<FailedTwinSceneRequest>);

/// User action available after an accepted source replacement repairs a
/// previously failed scene document.
#[derive(Event, Clone, Debug)]
pub struct SceneSourceRepairReady {
    /// The exact scene address to load after the user chooses to continue.
    pub path: String,
    /// The original root override, preserving the failed request semantics.
    pub root_prim: String,
}

pub(crate) fn remember_failed_twin_scene_request(
    trigger: On<lunco_core::SceneTransitionFailed>,
    registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    backed: Option<Res<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    mut failed: ResMut<FailedTwinSceneRequestState>,
) {
    failed.0 = None;
    let lunco_core::SceneTransition::Load { path, root_prim } = &trigger.event().transition else {
        return;
    };
    let Some((name, rel)) = lunco_assets_core::parse_twin_uri(path) else {
        return;
    };
    let (Some(registry), Some(backed)) = (registry, backed) else {
        return;
    };
    let Some(doc) = backed.doc_for(name, rel) else {
        return;
    };
    let Some(generation) = registry.host(doc).map(|host| host.document().generation()) else {
        return;
    };
    failed.0 = Some(FailedTwinSceneRequest {
        doc,
        path: path.clone(),
        root_prim: root_prim.clone(),
        generation,
    });
}

pub(crate) fn clear_failed_twin_scene_request(
    _trigger: On<lunco_core::SceneTransitionStarted>,
    mut failed: ResMut<FailedTwinSceneRequestState>,
) {
    failed.0 = None;
}

/// Announce a validated source replacement without resuming the failed scene.
pub(crate) fn announce_repaired_twin_scene(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    failed: Res<FailedTwinSceneRequestState>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    let Some(failed) = failed.0.as_ref().filter(|failed| failed.doc == doc) else {
        return;
    };
    let Some(host) = registry.host(doc) else {
        return;
    };
    let document = host.document();
    let generation = document.generation();
    if generation <= failed.generation || document.parse_error().is_some() {
        return;
    }
    let Some(ops) = document.ops_since(failed.generation) else {
        return;
    };
    if !ops.iter().any(|op| {
        matches!(
            op,
            lunco_usd_document::document::UsdOp::ReplaceSource { edit_target, .. }
                if edit_target.is_root()
        )
    }) || document.authoring_recipe().is_none()
    {
        return;
    }
    commands.trigger(SceneSourceRepairReady {
        path: failed.path.clone(),
        root_prim: failed.root_prim.clone(),
    });
}

/// Open one Twin-relative scene selected by the active Rhai loading policy.
#[Command(default)]
pub struct OpenTwinScene {
    /// Workspace identity of the Twin whose source authority was mounted.
    pub twin_id: u64,
    /// Exact `twin://` authority returned by the asset owner.
    pub name: String,
    /// Indexed path relative to the Twin root.
    pub relative_path: String,
}

/// Set the explanation displayed while the viewport has no scene.
#[Command(default)]
pub struct SetEmptyViewportReason {
    /// Authored reason to show, or an empty string to clear it.
    pub reason: String,
}

/// Closing the active Twin requests mounted-scene teardown through the scene
/// coordinator. Closing an inactive Twin must not clear the active Twin's scene.
pub(crate) fn clear_scene_on_twin_closed(
    trigger: On<TwinClosed>,
    mut pending_twin: ResMut<crate::twin_projection::PendingTwinDocs>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
    mut backed: ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<WorkspaceResource>>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
    mut commands: Commands,
) {
    let twin = trigger.event().twin;
    for key in pending_twin.release_twin(twin) {
        admission.cancel_queued(key);
    }
    for doc in backed.release_twin(twin) {
        if workspace.as_deref().is_some_and(|workspace| {
            workspace.document(doc).is_some_and(|entry| {
                entry.runtime_context == lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin)
            })
        }) {
            registry.remove(doc);
        }
    }
    if !trigger.event().was_active {
        return;
    }
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }
    commands.remove_resource::<SceneLoadInFlight>();
    commands.remove_resource::<lunco_usd_bevy_scene::FailedSceneLoad>();
    if let Some(id) = coordinator.lifecycle_id() {
        commands.trigger(lunco_core::SceneOwnerRetired { id });
    }
    if let Some((id, transition)) = coordinator.retire_current_owner() {
        commands.trigger(lunco_core::SceneTransitionFailed {
            id,
            transition,
            error: format!("scene owner Twin {} closed", trigger.event().twin.raw()),
        });
    }
    if trigger.event().was_active {
        commands.trigger(ClearScene {});
    }
}

/// Open one scene selected by the active Twin loading policy. The command
/// owns the typed path checks and the existing document-first USD load; Rhai
/// owns whether this Twin has a scene to open.
#[on_command(OpenTwinScene)]
fn on_open_twin_scene(
    trigger: On<OpenTwinScene>,
    workspace: Res<WorkspaceResource>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    asset_server: Option<Res<AssetServer>>,
    usd_sources: Option<Res<Assets<UsdSourceBytes>>>,
    mut pending_twin: ResMut<crate::twin_projection::PendingTwinDocs>,
) -> Result<Ack, String> {
    let request = trigger.event();
    let twin_id = lunco_workspace::TwinId::new(request.twin_id);
    let twin = workspace
        .twin(twin_id)
        .ok_or_else(|| format!("Twin {} is not open", request.twin_id))?;
    if workspace.active_twin != Some(twin_id) {
        return Err(format!("Twin {} is not active", request.twin_id));
    }
    let relative = Path::new(&request.relative_path);
    if !lunco_assets_path::is_safe_relative_path(&request.relative_path)
        || relative == Path::new(".")
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        || !is_usd_path(&request.relative_path)
    {
        return Err(format!(
            "Twin scene path `{}` must be a safe Twin-relative USD file",
            request.relative_path
        ));
    }
    if !twin
        .files()
        .iter()
        .any(|entry| entry.relative_path == relative)
    {
        return Err(format!(
            "Twin scene path `{}` is not indexed",
            request.relative_path
        ));
    }
    let mounted_root = roots
        .as_deref()
        .ok_or_else(|| "TwinRoots is not installed".to_owned())?
        .root_for(&request.name)
        .map_err(|error| error.to_string())?;
    if mounted_root.as_deref() != Some(twin.root.as_path()) {
        return Err(format!(
            "Twin asset authority `{}` does not belong to Twin {}",
            request.name, request.twin_id
        ));
    }
    let (Some(asset_server), Some(usd_sources)) = (asset_server, usd_sources) else {
        return Err("the USD asset pipeline is not installed".to_owned());
    };

    let scene_uri = lunco_assets_core::twin_uri(&request.name, &request.relative_path);
    info!(
        "[twin] doc-backing starting scene `{scene_uri}` (twin `{}`) — mount follows",
        twin.root.display()
    );
    let source_path = lunco_assets_core::asset_path::load_asset_path(&scene_uri, None, None, None)
        .map_err(|error| format!("invalid Twin scene source `{scene_uri}`: {error}"))?;
    let handle = asset_server.load::<UsdSourceBytes>(source_path);
    let source_ready = usd_sources.get(handle.id()).is_some();
    let source_failed = asset_server
        .get_load_state(handle.id())
        .is_some_and(|state| state.is_failed());
    let source_id = handle.id();
    pending_twin.push(
        handle,
        twin_id,
        source_ready,
        request.name.clone(),
        request.relative_path.clone(),
        twin.root.join(relative),
        twin.root.clone(),
    );
    if source_failed {
        pending_twin.mark_failed(
            source_id,
            "the source asset had already failed to load".into(),
        );
    }
    Ok(Ack::new(OpId::new()))
}

#[on_command(SetEmptyViewportReason)]
fn on_set_empty_viewport_reason(
    trigger: On<SetEmptyViewportReason>,
    mut reason: ResMut<EmptyViewportReason>,
) {
    let value = trigger.event().reason.trim();
    reason.0 = (!value.is_empty()).then(|| value.to_owned());
}

/// Mount a scene, resolving the requested path to its **document** first.
///
/// A scene that is backed by a registry document must mount that document's
/// composed `base ⊕ runtime` — the runtime layer carries placed waypoints,
/// runtime spawns and moved transforms, and it is published as the overlay on the
/// scene's `twin://` source. Mounting the raw file instead re-reads the base
/// `.usda` from disk and silently drops all of it, so a second `LoadScene` for an
/// already-open scene would wipe every
/// live edit. Asking the registry (rather than pattern-matching the path against
/// twin roots) makes that an authoritative answer: the mount diverts exactly when
/// a document exists to divert to.
///
/// The observer lives HERE, not in `lunco-usd-sim`, because
/// [`DocumentRegistry`] does — `lunco-usd-sim` sits one layer below and owns the
/// mount mechanics this drives ([`validate_scene_address`], [`resolve_root_prim`],
/// [`clear_scene_entities`], [`spawn_scene_root_world`]).
#[on_command(LoadScene)]
fn on_load_scene(
    trigger: On<LoadScene>,
    // Optional so document-only hosts can install the command surface without
    // also installing the asset pipeline. Mounting a scene is meaningless
    // without one, so the request ends at the visible prerequisite boundary.
    asset_server: Option<Res<AssetServer>>,
    stages: Option<Res<Assets<UsdStageAsset>>>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
) -> Result<Ack, String> {
    let path = validate_scene_address(&trigger.event().path).map_err(|error| {
        warn!("{error}");
        error
    })?;
    if asset_server.is_none() || stages.is_none() {
        let error = "LoadScene requires the USD asset pipeline".to_string();
        warn!("[load-scene] {error}");
        return Err(error);
    }
    let root_prim = resolve_root_prim(&path, &trigger.event().root_prim);

    let request = lunco_core::SceneTransitionRequest::load(path.clone(), root_prim);
    match coordinator.admit(request) {
        lunco_core::SceneTransitionAdmission::AlreadyActive => {
            info!("[load-scene] `{}` is already mounting — no-op", path);
        }
        lunco_core::SceneTransitionAdmission::Queued => {
            info!(
                "[load-scene] queued `{}` behind the active scene transaction",
                path
            );
        }
        lunco_core::SceneTransitionAdmission::Admitted => {
            info!(
                "[load-scene] admitted `{}` for the next scene lifecycle phase",
                path
            );
        }
    }
    Ok(Ack::new(OpId::new()))
}

/// Execute the load request that won admission at the scene lifecycle boundary.
/// Public command observers never mutate scene state directly.
pub(crate) fn execute_admitted_load_scene(
    trigger: On<lunco_core::SceneTransitionAdmitted>,
    asset_server: Res<AssetServer>,
    registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    mut backed: Option<ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath, Has<UsdSceneRoot>)>,
    scene: SceneEntities,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
    // A real scene is mounting — clear any empty-viewport reason recorded by a
    // prior clear/folder-open, so it can't haunt the placeholder once this load
    // despawns/resolves. Done HERE (not in the UI placeholder updater) so a
    // freshly-set reason is not wiped on the same frame by stale `UsdPrimPath`
    // entities from the scene being cleared (their despawn is deferred, so the
    // query would still read non-empty and clobber the reason mid-open).
    mut empty_reason: ResMut<EmptyViewportReason>,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
) {
    let lunco_core::SceneTransitionRequest::Load { path, root_prim } = &trigger.event().request
    else {
        return;
    };
    let path = path.clone();
    let root_prim = root_prim.clone();

    let transition = lunco_core::SceneTransition::load(path.clone(), root_prim.clone());
    let transition_id = coordinator.start(transition.clone());
    let asset_path = match lunco_assets_core::asset_path::load_asset_path(&path, None, None, None) {
        Ok(path) => path,
        Err(error) => {
            let error = format!("invalid USD scene address `{path}`: {error}");
            warn!("{error}");
            commands.trigger(lunco_core::SceneTransitionFailed {
                id: transition_id,
                transition,
                error,
            });
            return;
        }
    };
    if let Err(error) = refresh_doc_backed_scene_source(
        &path,
        registry.as_deref(),
        backed.as_deref_mut(),
        roots.as_deref(),
        &asset_server,
    ) {
        warn!("[load-scene] {error}");
        commands.trigger(lunco_core::SceneTransitionFailed {
            id: transition_id,
            transition,
            error,
        });
        return;
    }
    // Admission is the commit point. Only now does this request own scene state;
    // a request queued behind another transaction must not mutate the active
    // transaction's diagnostics or viewport reason.
    commands.remove_resource::<lunco_usd_bevy_scene::FailedSceneLoad>();
    empty_reason.0 = None;

    // Blender-style no-op: same stage, same root prim, already mounted.
    //
    // The identity is the PAIR `(stage asset, root prim)`, but the two halves of
    // the root prim are asked differently because an empty `root_prim` is
    // `resolve_root_prim`'s deferred sentinel, NOT a path:
    //
    // - sentinel (the ordinary load) means "mount the stage's `defaultPrim`". It
    //   cannot be compared as a string: `instantiate_usd_prim` resolves it and
    //   writes the concrete path back onto the scene root, so the mounted root
    //   represents the sentinel semantically rather than as an empty string.
    //   What the sentinel denotes is the stage's default mount, and that mount is
    //   exactly the `UsdSceneRoot`, so ask for that instead.
    // - an explicit override names a real prim path, so compare it as one.
    //
    // Deliberately NOT "any prim from this stage": the active simulation owns
    // one scene root. The editor preview, when present, uses `UsdPreviewOnly`
    // and is outside this simulation mount identity.
    let new_id = asset_server.load::<UsdStageAsset>(asset_path).id();
    let stage_already_loaded = asset_server.load_state(new_id).is_loaded();
    if q_usd.iter().any(|(entity, upp, is_scene_root)| {
        let current_mount_is_live = mount_state.as_deref().is_none_or(|state| {
            // A replacement invalidates the old root synchronously, while its
            // deferred despawn is still visible to this query. Never let that
            // stale entity satisfy the idempotent-load guard.
            state.contains_root(entity)
        });
        upp.stage_handle.id() == new_id
            && current_mount_is_live
            && if root_prim.is_empty() {
                is_scene_root
            } else {
                upp.path == root_prim
            }
    }) {
        info!(
            "[load-scene] `{}` @ `{}` already loaded — no-op",
            path, root_prim
        );
        commands.trigger(lunco_core::SceneTransitionCompleted {
            id: transition_id,
            transition,
        });
        return;
    }

    info!("[load-scene] reload path=`{}` root=`{}`", path, root_prim);

    // Invalidate outgoing roots NOW, before Bevy applies the deferred
    // despawns below.  The visual sync system may still query those entities
    // during this boundary frame; the mount state is its authoritative
    // ownership fence.
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }

    commands.insert_resource(SceneLoadInFlight {
        transition_id,
        path: path.clone(),
        stage_id: new_id,
    });
    commands.trigger(lunco_core::SceneTransitionStarted {
        id: transition_id,
        transition,
    });

    // Despawn the old scene + free worker-side state (shared with `ClearScene`).
    clear_scene_entities(&mut commands, &scene);

    // Spawn via shared helper, deferred so despawns flush first.
    commands.queue(move |world: &mut World| {
        spawn_scene_root_world(world, &path, &root_prim);
        world
            .resource_mut::<lunco_usd_bevy_twin::TwinProjectionWake>()
            .wake();
        if stage_already_loaded {
            world.write_message(SceneStageAssetOutcome::Loaded {
                transition_id,
                stage_id: new_id,
            });
        }
    });
}

/// Publish the current authored document into a Twin stage only after the
/// coordinator admits its scene transition. Queued or duplicate requests do
/// not mutate the source used by a mounted scene.
fn refresh_doc_backed_scene_source(
    path: &str,
    registry: Option<&DocumentRegistry<UsdDocument>>,
    mut backed: Option<&mut lunco_usd_bevy_twin::DocBackedTwinScenes>,
    roots: Option<&lunco_assets_core::twin_source::TwinRoots>,
    asset_server: &AssetServer,
) -> Result<(), String> {
    let Some((name, rel)) = lunco_assets_core::parse_twin_uri(path) else {
        return Ok(());
    };
    let Some(backed) = backed.as_deref_mut() else {
        return Ok(());
    };
    let Some(doc) = backed.doc_for(name, rel) else {
        return Ok(());
    };
    let registry =
        registry.ok_or_else(|| "the doc-backed USD registry is unavailable".to_owned())?;
    let document = registry
        .host(doc)
        .ok_or_else(|| format!("doc-backed USD document {doc} is unavailable"))?
        .document();
    if document.parse_error().is_some() {
        return Err(format!(
            "USD source for `{path}` is invalid; replace it with a valid source before loading"
        ));
    }
    let Some(recipe) = document.authoring_recipe().cloned() else {
        return Ok(());
    };
    if recipe.root_id != path {
        return Err(format!(
            "prepared source recipe `{}` does not match scene `{path}`",
            recipe.root_id
        ));
    }
    let root_bytes = recipe
        .bytes
        .get(path)
        .cloned()
        .ok_or_else(|| format!("prepared source recipe for `{path}` has no root bytes"))?;
    let generation = document.generation();
    let roots = roots
        .ok_or_else(|| "Twin asset roots are unavailable for the doc-backed scene".to_owned())?;
    backed
        .publish_stage_input(
            doc,
            roots,
            asset_server,
            lunco_usd_document::document::DocumentStageInput {
                recipe,
                root_bytes: std::sync::Arc::new(root_bytes),
            },
        )
        .map_err(|error| format!("could not prepare current USD source for `{path}`: {error}"))?;
    backed.mark_overlay_synced(doc, generation);
    Ok(())
}

/// Refresh the active doc-backed Twin from its source file before the shared
/// [`RestartScene`] lifecycle handler reloads its asset. The lower simulation
/// layer deliberately does not know documents; it queues its asset reload, which
/// gives this observer one synchronous place to update the composed Twin overlay.
///
/// A normal restart retains dirty documents; a full reset is a separately
/// confirmed intent which discards both their authored and runtime layers. The
/// document registry owns both policies so every file-backed domain keeps the
/// same identity and history invariants.
pub(crate) fn on_restart_scene_refresh_active_document(
    trigger: On<lunco_core::SceneTransitionStarted>,
    asset_server: Option<Res<AssetServer>>,
    q_usd: Query<(&UsdPrimPath, Has<UsdSceneRoot>)>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: Option<ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    twins: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    assets: Option<Res<Assets<UsdStageAsset>>>,
    mut canonical: Option<NonSendMut<lunco_usd_bevy_stage::canonical::CanonicalStages>>,
    role: Option<Res<lunco_core_session::NetworkRole>>,
) {
    let lunco_core::SceneTransition::Restart { reset_document, .. } = &trigger.event().transition
    else {
        return;
    };
    // The authoritative host/standalone process owns the source file. Clients
    // restart the currently replicated asset and must not invent a local base.
    if role.as_deref().is_some_and(|role| !role.is_authoritative()) {
        return;
    }
    let (Some(asset_server), Some(backed), Some(twins)) =
        (asset_server, backed.as_deref_mut(), twins)
    else {
        return;
    };
    let Some((stage_id, stage_path)) =
        q_usd
            .iter()
            .find(|(_, is_root)| *is_root)
            .and_then(|(prim, _)| {
                let id = prim.stage_handle.id();
                asset_server.get_path(id).map(|path| (id, path.to_string()))
            })
    else {
        return;
    };

    let active = registry.ids().find_map(|doc| {
        let (name, rel) = backed.coords_of(doc)?;
        (lunco_assets_core::twin_uri(&name, &rel) == stage_path).then_some((doc, name, rel))
    });
    let Some((doc, name, _rel)) = active else {
        return;
    };
    let Some(path) = registry
        .host(doc)
        .and_then(|host| host.document().origin().canonical_path())
        .map(std::path::Path::to_owned)
    else {
        return;
    };
    let Ok(bytes) = lunco_storage::read_file_sync(&path) else {
        warn!(
            "[restart-scene] cannot reread `{}`; keeping the mounted source",
            path.display()
        );
        return;
    };
    let prepared = match lunco_usd_document::document::PreparedUsdSource::from_bytes(bytes) {
        Ok(prepared) => prepared,
        Err(error) => {
            warn!(
                "[restart-scene] refusing invalid USD source `{}`: {error}",
                path.display()
            );
            return;
        }
    };
    let (_, outcome) = if *reset_document {
        registry.reset_prepared_file(path, prepared)
    } else {
        registry.open_prepared_file(path, prepared, true)
    };
    match outcome {
        OpenOutcome::Refreshed => {
            if *reset_document {
                info!("[restart-scene] fully reset active Twin from disk before remount")
            } else {
                info!("[restart-scene] refreshed active Twin source before remount")
            }
        }
        OpenOutcome::KeptDirty => {
            warn!(
                "[restart-scene] active Twin has unsaved edits; retaining them instead of overwriting from disk"
            );
        }
        OpenOutcome::KeptUnparsable => {
            warn!("[restart-scene] source did not parse as USDA; retaining the mounted document")
        }
        OpenOutcome::Allocated => {}
    }
    // A referenced Editor document may have advanced since its last mount.
    // Its resolver overlay must advance too before the replacement asset reads
    // the closure; otherwise reload consumes older component parameters.
    let Some(recipe) = q_usd
        .iter()
        .find(|(_, root)| *root)
        .and_then(|(prim, _)| assets.as_deref()?.get(prim.stage_handle.id()))
        .and_then(|asset| asset.recipe.as_ref())
    else {
        warn!("[restart-scene] mounted Twin has no source closure; cannot refresh its overlays");
        return;
    };
    let dependencies = recipe
        .bytes
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    if let Err(error) = publish_restart_document_overlays(
        &mut registry,
        backed,
        &twins,
        &asset_server,
        doc,
        &name,
        &dependencies,
    ) {
        warn!("[restart-scene] could not publish the refreshed Twin sources: {error}");
        return;
    }
    if let Some(stages) = canonical.as_deref_mut() {
        stages.retire(stage_id);
    }
}

fn publish_restart_document_overlays(
    registry: &mut DocumentRegistry<UsdDocument>,
    backed: &mut lunco_usd_bevy_twin::DocBackedTwinScenes,
    twins: &lunco_assets_core::twin_source::TwinRoots,
    server: &AssetServer,
    active_doc: lunco_doc::DocumentId,
    name: &str,
    dependencies: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let mut sources = Vec::new();
    for (doc, twin, rel, _, _, _) in backed.entries() {
        if twin != name
            || (doc != active_doc
                && !dependencies.contains(&lunco_assets_core::twin_uri(&twin, &rel)))
        {
            continue;
        }
        let document = registry
            .host_mut(doc)
            .ok_or_else(|| format!("document {doc} is not open"))?
            .document_mut();
        document.clear_view();
        let input = document
            .capture_stage_input(&lunco_assets_core::twin_uri(&twin, &rel), false)
            .map_err(|error| error.to_string())?;
        sources.push((rel, doc, document.generation(), input));
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    for (_rel, doc, generation, source) in sources {
        backed.publish_stage_input(doc, twins, server, source)?;
        backed.mark_overlay_synced(doc, generation);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// OpenFile — gated on USD extensions
// ─────────────────────────────────────────────────────────────────────

// `OpenFile` for a USD path drives two independent halves, each its own
// observer so headless bins get both without the UI:
//
//   1. `on_open_file_for_usd` — document **registration**: async read via
//      `lunco-storage`, idempotent allocate into `DocumentRegistry<UsdDocument>`.
//   2. `on_open_file` (this one) — scene-root selection: external files open
//      their owning Twin and enter the doc-first mount; files inside the active
//      Twin remain document-only.
//
// Only the admitted typed `LoadScene` observer calls `spawn_scene_root_world`,
// so no OpenFile path can create a second raw stage beside the Twin mount.
#[on_command(OpenFile)]
fn on_open_file(
    trigger: On<OpenFile>,
    workspace: Option<Res<WorkspaceResource>>,
    mut pending: Option<ResMut<PendingTwinOpens>>,
    mut commands: Commands,
) {
    let raw_path = &trigger.event().path;
    let path = match lunco_storage::file_uri_to_path(raw_path) {
        Ok(Some(path)) => path,
        Ok(None) if lunco_assets_core::has_scheme(raw_path) => {
            // Asset schemes enter the shared typed scene transition; decoded
            // native file URIs keep their filesystem identity below.
            if is_usd_path(raw_path) {
                commands.trigger(LoadScene {
                    path: raw_path.clone(),
                    root_prim: String::new(),
                });
            }
            return;
        }
        Ok(None) => std::path::PathBuf::from(raw_path),
        Err(error) => {
            warn!("[OpenFile] {error}");
            return;
        }
    };
    if !is_usd_path(&path.to_string_lossy()) {
        return;
    }

    let Some(workspace) = workspace else {
        warn!(
            "[OpenFile] cannot open USD filesystem scene `{}`: WorkspacePlugin is not installed",
            path.display()
        );
        return;
    };
    let abs = match lunco_storage::canonicalize_file_path(&path) {
        Ok(abs) => abs,
        Err(error) => {
            warn!(
                "[OpenFile] cannot resolve USD filesystem scene `{}`: {error}",
                path.display()
            );
            return;
        }
    };
    // A USD file already inside the active Twin is an additive document open;
    // the Twin browser uses this to inspect reusable layers without replacing
    // the running world. External scenes replace the workspace at the root.
    if workspace
        .twins()
        .any(|(_, twin)| abs.starts_with(&twin.root))
    {
        return;
    }
    let Some(pending) = pending.as_deref_mut() else {
        warn!(
            "[OpenFile] cannot open USD filesystem scene `{}`: WorkspacePlugin is not installed",
            path.display()
        );
        return;
    };
    spawn_twin_from_scene(&abs, pending, "OpenFile");
}

/// Open the root that owns `scene` and select that scene.
///
/// This root-relative scan is owned by the USD scene domain so GUI and headless
/// `OpenFile` requests cannot mount one file through competing paths.
fn spawn_twin_from_scene(scene: &Path, pending: &mut PendingTwinOpens, log_tag: &str) {
    let abs = match lunco_storage::canonicalize_file_path(scene) {
        Ok(abs) => abs,
        Err(error) => {
            warn!(
                "[{log_tag}] cannot resolve USD filesystem scene `{}`: {error}",
                scene.display()
            );
            return;
        }
    };
    let root = lunco_twin::root_for_file(&abs);
    let rel = abs
        .strip_prefix(&root)
        .map(lunco_assets_path::slashed)
        .unwrap_or_else(|_| {
            abs.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    spawn_twin_scan(&root, pending, log_tag, Some(rel), TwinOpenMode::Replace);
}

register_commands!(
    on_load_scene,
    on_open_file,
    on_open_twin_scene,
    on_set_empty_viewport_reason
);

#[cfg(test)]
mod restart_overlay_tests {
    use super::*;

    #[test]
    fn twin_close_retires_only_the_active_scene_owner() {
        for was_active in [false, true] {
            let mut app = App::new();
            app.init_resource::<crate::twin_projection::PendingTwinDocs>()
                .init_resource::<lunco_core_runtime::AsyncWorkAdmission>()
                .init_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
                .init_resource::<DocumentRegistry<UsdDocument>>()
                .init_resource::<lunco_core::SceneTransitionCoordinator>()
                .init_resource::<lunco_core::RuntimeFaults>()
                .init_resource::<lunco_core::SceneMountState>()
                .add_observer(clear_scene_on_twin_closed)
                .add_observer(crate::scene::on_scene_transition_failed)
                .add_observer(|_: On<ClearScene>, mut state: ResMut<lunco_core::SceneTransitionCoordinator>| {
                    state.admit(lunco_core::SceneTransitionRequest::clear());
                });
            let id = app
                .world_mut()
                .resource_mut::<lunco_core::SceneTransitionCoordinator>()
                .start(lunco_core::SceneTransition::load(
                    "twin://old/scene.usda",
                    "/World",
                ));
            app.world_mut()
                .resource_mut::<lunco_core::SceneTransitionCoordinator>()
                .admit(lunco_core::SceneTransitionRequest::load(
                    "twin://old/queued.usda",
                    "/World",
                ));
            let root = app.world_mut().spawn_empty().id();
            app.world_mut()
                .resource_mut::<lunco_core::SceneMountState>()
                .register_root(root, true);
            app.insert_resource(SceneLoadInFlight {
                transition_id: id,
                path: "twin://old/scene.usda".into(),
                stage_id: Handle::<UsdStageAsset>::default().id(),
            });
            app.world_mut().trigger(TwinClosed {
                twin: lunco_workspace::TwinId::new(1),
                root: std::path::PathBuf::from("owner"),
                was_active,
            });
            app.world_mut().flush();
            let state = app
                .world()
                .resource::<lunco_core::SceneTransitionCoordinator>();
            if was_active {
                assert!(state.active_id().is_none());
                assert!(!app.world().contains_resource::<SceneLoadInFlight>());
                assert!(
                    app.world()
                        .resource::<lunco_core::SceneMountState>()
                        .active_root()
                        .is_none()
                );
                assert_eq!(
                    app.world_mut()
                        .resource_mut::<lunco_core::SceneTransitionCoordinator>()
                        .take_admitted(),
                    Some(lunco_core::SceneTransitionRequest::clear())
                );
            } else {
                assert_eq!(state.active_id(), Some(id));
                assert!(app.world().contains_resource::<SceneLoadInFlight>());
                assert_eq!(
                    app.world()
                        .resource::<lunco_core::SceneMountState>()
                        .active_root(),
                    Some(root)
                );
            }
        }
    }
}
