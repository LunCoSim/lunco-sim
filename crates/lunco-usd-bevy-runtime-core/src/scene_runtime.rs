//! Scene and Twin runtime orchestration for the USD projection stack.
//!
//! Document commands stay in lunco-usd-commands; this module owns scene
//! admission, Twin-backed loading, runtime overlays, and live-stage projection.

use std::path::Path;

use crate::scene::{
    ClearScene, LoadScene, SceneEntities, SceneLoadInFlight, SceneStageAssetOutcome,
    clear_scene_entities, resolve_root_prim, spawn_scene_root_with_stage, spawn_scene_root_world,
    validate_scene_address,
};
use bevy::asset::AssetPath;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Document as _, OpenOutcome};
use lunco_doc_bevy::{DocumentChanged, DocumentRegistry, OpenFile};
use lunco_storage::Storage;
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_bevy_stage::{UsdStageAsset, canonical::CanonicalStages, source::UsdSourceBytes};
use lunco_usd_core::commands::{EmptyViewportReason, is_usd_path};
use lunco_usd_document::document::{PreparedUsdSource, UsdDocument};
use lunco_workspace::open::{PendingTwinOpens, TwinOpenMode, spawn_twin_scan};
use lunco_workspace::{TwinClosed, WorkspaceResource};

#[derive(Resource, Default)]
pub(crate) struct PendingSceneRestart(Option<PendingSceneRestartWork>);

struct PendingSceneRestartWork {
    id: lunco_core::SceneTransitionId,
    transition: lunco_core::SceneTransition,
    previous_generation: u64,
    root: Entity,
    stage: Handle<UsdStageAsset>,
    asset_path: AssetPath<'static>,
    label: String,
    root_prim: String,
    document: Option<RestartDocumentRefresh>,
}

struct RestartDocumentRefresh {
    doc: lunco_doc::DocumentId,
    authority: String,
    relative_path: String,
    path: Option<std::path::PathBuf>,
    generation: u64,
    mount_revision: u64,
    reset_document: bool,
    task: Task<Result<PreparedRestartDocument, String>>,
}

struct RestartDocumentSnapshot {
    doc: lunco_doc::DocumentId,
    authority: String,
    relative_path: String,
    document: UsdDocument,
}

struct PreparedRestartDocument {
    source: Option<PreparedUsdSource>,
    overlays: Vec<PreparedRestartOverlay>,
}

struct PreparedRestartOverlay {
    authority: String,
    relative_path: String,
    doc: lunco_doc::DocumentId,
    source_generation: u64,
    generation: u64,
    authoring_recipe_revision: Option<(u64, u64)>,
    authoring_recipe: lunco_usd_compose::recipe::StageRecipe,
    input: lunco_usd_document::document::DocumentStageInput,
}

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
    /// Workspace identity of the Twin whose source owns the scene.
    pub twin_id: u64,
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
    let roots = roots
        .as_deref()
        .ok_or_else(|| "TwinRoots is not installed".to_owned())?;
    let authority = roots
        .authority_for_root(&twin.root)
        .map_err(|_| "Twin asset source is unavailable".to_owned())?
        .ok_or_else(|| "Twin asset source is unavailable".to_owned())?;
    let (Some(asset_server), Some(usd_sources)) = (asset_server, usd_sources) else {
        return Err("the USD asset pipeline is not installed".to_owned());
    };

    let internal_scene_uri = lunco_assets_core::twin_uri(&authority, &request.relative_path);
    let scene_uri =
        lunco_assets_core::asset_path::source_uri_for_address(&internal_scene_uri, Some(roots))
            .map_err(|_| "Twin scene source is unavailable".to_owned())?;
    info!(
        "[twin] doc-backing starting scene `{scene_uri}` (twin `{}`) — mount follows",
        twin.root.display()
    );
    let source_path =
        lunco_assets_core::asset_path::load_asset_path(&internal_scene_uri, None, None, None)
            .map_err(|_| "Twin scene source is unavailable".to_owned())?;
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
        authority,
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
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    mut pending_twin: Option<ResMut<crate::twin_projection::PendingTwinDocs>>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
) -> Result<Ack, String> {
    let public_path = validate_scene_address(&trigger.event().path).map_err(|error| {
        warn!("{error}");
        error
    })?;
    if asset_server.is_none() || stages.is_none() {
        let error = "LoadScene requires the USD asset pipeline".to_string();
        warn!("[load-scene] {error}");
        return Err(error);
    }
    let path = lunco_assets_core::asset_path::load_source_uri(&public_path, roots.as_deref())
        .map(|path| lunco_assets_core::asset_path::anchor_of(&path))
        .map_err(|error| {
            warn!("[load-scene] source URI could not be admitted: {error}");
            error.to_string()
        })?;
    let root_prim = resolve_root_prim(&path, &trigger.event().root_prim);

    if pending_twin.as_deref_mut().is_some_and(|pending| {
        pending.defer_scene_load(&path, public_path.clone(), root_prim.clone())
    }) {
        info!("[load-scene] queued Twin scene until its document source is ready");
        return Ok(Ack::new(OpId::new()));
    }

    let request = lunco_core::SceneTransitionRequest::load(path.clone(), root_prim);
    match coordinator.admit(request) {
        lunco_core::SceneTransitionAdmission::AlreadyActive => {
            info!("[load-scene] `{public_path}` is already mounting — no-op");
        }
        lunco_core::SceneTransitionAdmission::Queued => {
            info!("[load-scene] queued `{public_path}` behind the active scene transaction");
        }
        lunco_core::SceneTransitionAdmission::Admitted => {
            info!("[load-scene] admitted `{public_path}` for the next scene lifecycle phase");
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
    let display_path = crate::scene::stable_source_address(&path, roots.as_deref())
        .unwrap_or_else(|_| "Twin scene".to_owned());

    let transition = lunco_core::SceneTransition::load(path.clone(), root_prim.clone());
    let transition_id = coordinator.start(transition.clone());
    let asset_path = match lunco_assets_core::asset_path::load_asset_path(&path, None, None, None) {
        Ok(path) => path,
        Err(error) => {
            let error = if lunco_assets_core::parse_twin_uri(&path).is_some() {
                format!("Twin scene source `{display_path}` is unavailable")
            } else {
                format!("invalid USD scene address `{display_path}`: {error}")
            };
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
        let error = if lunco_assets_core::parse_twin_uri(&path).is_some() {
            format!("Twin scene source `{display_path}` could not be prepared")
        } else {
            error
        };
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
            display_path, root_prim
        );
        commands.trigger(lunco_core::SceneTransitionCompleted {
            id: transition_id,
            transition,
        });
        return;
    }

    info!(
        "[load-scene] reload path=`{}` root=`{}`",
        display_path, root_prim
    );

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

/// Admit a source reload while keeping the currently committed scene mounted
/// until its authoritative document source has been read and parsed.
pub(crate) fn execute_admitted_restart_scene(
    trigger: On<lunco_core::SceneTransitionAdmitted>,
    asset_server: Res<AssetServer>,
    admission: Res<lunco_core_runtime::AsyncWorkAdmission>,
    registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    backed: Option<Res<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    twins: Option<Res<lunco_assets_core::TwinRoots>>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
    mut pending: ResMut<PendingSceneRestart>,
    q_usd: Query<(Entity, &UsdPrimPath, Has<UsdSceneRoot>)>,
    mount_state: Option<Res<lunco_core::SceneMountState>>,
    role: Option<Res<lunco_core_session::NetworkRole>>,
    mut commands: Commands,
) {
    let lunco_core::SceneTransitionRequest::Restart { reset_document } = &trigger.event().request
    else {
        return;
    };
    if pending.0.is_some() {
        fail_restart_before_start(
            &mut commands,
            "a scene source reload is already being prepared".to_owned(),
        );
        return;
    }
    let Some(previous_generation) = coordinator.completed_generation() else {
        coordinator.finish_noop();
        warn!("[restart-scene] no committed scene is loaded — nothing to reload");
        return;
    };
    let root = mount_state
        .as_deref()
        .and_then(lunco_core::SceneMountState::active_root)
        .and_then(|root| q_usd.get(root).ok())
        .filter(|(_, _, is_root)| *is_root);
    let Some((root, prim, _)) = root else {
        coordinator.finish_noop();
        fail_restart_before_start(
            &mut commands,
            "RestartScene requires the committed primary scene root".to_owned(),
        );
        return;
    };

    let stage = prim.stage_handle.clone();
    let root_prim = prim.path.clone();
    let Some(asset_path) = asset_server
        .get_path(stage.id())
        .map(|path| path.into_owned())
    else {
        coordinator.finish_noop();
        fail_restart_before_start(
            &mut commands,
            "RestartScene requires a reloadable asset source; use ResetSceneRuntime for an in-memory stage"
                .to_owned(),
        );
        return;
    };
    let internal_label = asset_path.to_string();
    let label = match crate::scene::stable_source_address(&internal_label, twins.as_deref()) {
        Ok(label) => label,
        Err(error) => {
            coordinator.finish_noop();
            fail_restart_before_start(&mut commands, error);
            return;
        }
    };
    if *reset_document && role.as_deref().is_some_and(|role| !role.is_authoritative()) {
        coordinator.finish_noop();
        fail_restart_before_start(
            &mut commands,
            "full document reset is owned by the authoritative session".to_owned(),
        );
        return;
    }
    let transition = lunco_core::SceneTransition::Restart {
        path: label.clone(),
        root_prim: root_prim.clone(),
        reset_document: *reset_document,
    };
    let id = coordinator.start(transition.clone());

    let document = if role.as_deref().is_some_and(|role| !role.is_authoritative()) {
        Ok(None)
    } else {
        prepare_restart_document_refresh(
            Some(&internal_label),
            *reset_document,
            previous_generation,
            id,
            registry.as_deref(),
            backed.as_deref(),
            twins.as_deref(),
            &asset_server,
            &admission,
        )
    };
    let document = match document {
        Ok(document) => document,
        Err(error) => {
            reject_active_restart(id, &error, &mut coordinator, &mut commands);
            return;
        }
    };

    info!("[restart-scene] preparing source reload for `{label}`");
    pending.0 = Some(PendingSceneRestartWork {
        id,
        transition,
        previous_generation,
        root,
        stage,
        asset_path,
        label,
        root_prim,
        document,
    });
}

fn prepare_restart_document_refresh(
    stage_path: Option<&str>,
    reset_document: bool,
    scope_generation: u64,
    transition_id: lunco_core::SceneTransitionId,
    registry: Option<&DocumentRegistry<UsdDocument>>,
    backed: Option<&lunco_usd_bevy_twin::DocBackedTwinScenes>,
    twins: Option<&lunco_assets_core::TwinRoots>,
    asset_server: &AssetServer,
    admission: &lunco_core_runtime::AsyncWorkAdmission,
) -> Result<Option<RestartDocumentRefresh>, String> {
    let Some((authority, relative_path)) = stage_path.and_then(lunco_assets_core::parse_twin_uri)
    else {
        return if reset_document {
            Err("full document reset requires an open Twin-backed USD document".to_owned())
        } else {
            Ok(None)
        };
    };
    let Some(backed) = backed else {
        return if reset_document {
            Err("full document reset requires the active Twin document projection".to_owned())
        } else {
            Ok(None)
        };
    };
    let Some(doc) = backed.doc_for(authority, relative_path) else {
        return if reset_document {
            Err("full document reset requires an open active Twin USD document".to_owned())
        } else {
            Ok(None)
        };
    };
    let registry = registry.ok_or("the active Twin USD document registry is unavailable")?;
    let twins = twins.ok_or("the active Twin asset registry is unavailable")?;
    if twins
        .root_for(authority)
        .map_err(|_| "Twin asset registry is unavailable".to_owned())?
        .is_none()
    {
        return Err("Twin asset source retired during reload admission".to_owned());
    }
    let host = registry
        .host(doc)
        .ok_or_else(|| format!("active Twin USD document {doc} is unavailable"))?;
    let document = host.document();
    if document.parse_error().is_some() {
        return Err(format!(
            "active Twin USD document {doc} has invalid authored source"
        ));
    }
    let generation = document.generation();
    let path = document
        .origin()
        .canonical_path()
        .map(std::path::Path::to_owned);
    if reset_document && path.is_none() {
        return Err("full document reset requires a file-backed USD source".to_owned());
    }
    let should_read_source = reset_document || !document.is_dirty();
    let mount_revision = twins.revision().map_err(|error| error.to_string())?;
    let pool =
        AsyncComputeTaskPool::try_get().ok_or("USD reload preparation task pool is unavailable")?;
    let permit = admission
        .admit_external(
            lunco_core_runtime::AsyncWorkPriority::Interactive,
            lunco_core_runtime::AsyncWorkKey::new(
                lunco_core_runtime::AsyncWorkKind::UsdPreparation,
                scope_generation,
                u128::from(doc.raw()),
                generation,
                transition_id.get(),
            ),
        )
        .map_err(|error| format!("USD reload preparation admission rejected: {error:?}"))?;
    let task_path = path.clone();
    let root_id = lunco_assets_core::twin_uri(authority, relative_path);
    let task_authority = authority.to_owned();
    let task_relative_path = relative_path.to_owned();
    let candidate = document.clone();
    let mut referenced_documents = Vec::new();
    for (referenced_doc, referenced_authority, referenced_path, _, _, _) in backed.entries() {
        if referenced_doc == doc {
            continue;
        }
        if let Some(host) = registry.host(referenced_doc) {
            referenced_documents.push(RestartDocumentSnapshot {
                doc: referenced_doc,
                authority: referenced_authority,
                relative_path: referenced_path,
                document: host.document().clone(),
            });
        }
    }
    let source_roots = twins.clone();
    let source_server = asset_server.clone();
    let task = pool.spawn(async move {
        let result = async {
            let source = if should_read_source {
                if let Some(path) = task_path.as_ref() {
                    let storage = lunco_storage::FileStorage::new();
                    let handle = lunco_storage::StorageHandle::File(path.clone());
                    let bytes = storage
                        .read(&handle)
                        .await
                        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
                    Some(PreparedUsdSource::from_bytes(bytes)?)
                } else {
                    None
                }
            } else {
                None
            };
            if reset_document && source.is_none() {
                return Err("full document reset completed without prepared file source".to_owned());
            }

            let mut candidate = candidate;
            if let Some(source) = source.as_ref() {
                let accepted = if reset_document {
                    <UsdDocument as lunco_doc::PreparedFileBacked>::reset_prepared_source(
                        &mut candidate,
                        source.clone(),
                    )
                } else {
                    <UsdDocument as lunco_doc::PreparedFileBacked>::reload_prepared_source(
                        &mut candidate,
                        source.clone(),
                    )
                };
                if !accepted {
                    return Err(
                        "prepared USD source could not be applied to its candidate document"
                            .to_owned(),
                    );
                }
            }

            let initial = candidate
                .capture_stage_input(&root_id, false)
                .map_err(|error| error.to_string())?;
            let seed = restart_source_seed(&initial)?;
            let mut source_overrides = std::collections::HashMap::from([(
                root_id.clone(),
                initial.root_bytes.as_ref().clone(),
            )]);
            let mut referenced_inputs = Vec::new();
            for referenced in referenced_documents {
                let uri =
                    lunco_assets_core::twin_uri(&referenced.authority, &referenced.relative_path);
                let input = referenced
                    .document
                    .capture_stage_input(&uri, false)
                    .map_err(|error| error.to_string());
                if let Ok(input) = &input {
                    source_overrides.insert(uri.clone(), input.root_bytes.as_ref().clone());
                }
                referenced_inputs.push((referenced, uri, input));
            }
            let origin = lunco_assets_core::asset_path::load_asset_path(&root_id, None, None, None)
                .map_err(|error| format!("invalid USD reload source `{root_id}`: {error}"))?;
            let authoring_recipe =
                lunco_usd_bevy_stage::compose::fetch_stage_recipe_from_asset_reader(
                    &source_server,
                    origin,
                    seed,
                    &source_overrides,
                    Some(&source_roots),
                    lunco_usd_compose::recipe::StageClosureLimits::default(),
                )
                .await
                .map_err(|error| format!("could not refresh the USD source closure: {error}"))?;
            // The old canonical stage's layer revision does not identify this
            // freshly fetched closure. Leave the revision unset until the new
            // stage becomes the authoring authority.
            let authoring_recipe_revision = None;
            candidate
                .set_authoring_recipe(authoring_recipe_revision, Some(authoring_recipe.clone()));
            let candidate_input = candidate
                .capture_stage_input(&root_id, false)
                .map_err(|error| error.to_string())?;
            let dependencies: std::collections::HashSet<_> =
                authoring_recipe.bytes.keys().cloned().collect();
            let mut overlays = vec![PreparedRestartOverlay {
                authority: task_authority.clone(),
                relative_path: task_relative_path.clone(),
                doc,
                source_generation: generation,
                generation: candidate.generation(),
                authoring_recipe_revision,
                authoring_recipe: authoring_recipe.clone(),
                input: candidate_input,
            }];
            for (mut referenced, uri, input) in referenced_inputs {
                if !dependencies.contains(&uri) {
                    continue;
                }
                let input = input.map_err(|error| {
                    format!(
                        "dependent USD document {} cannot be prepared for `{uri}`: {error}",
                        referenced.doc
                    )
                })?;
                let seed = restart_source_seed(&input)?;
                let origin = lunco_assets_core::asset_path::load_asset_path(&uri, None, None, None)
                    .map_err(|error| format!("invalid dependent USD source `{uri}`: {error}"))?;
                let authoring_recipe =
                    lunco_usd_bevy_stage::compose::fetch_stage_recipe_from_asset_reader(
                        &source_server,
                        origin,
                        seed,
                        &source_overrides,
                        Some(&source_roots),
                        lunco_usd_compose::recipe::StageClosureLimits::default(),
                    )
                    .await
                    .map_err(|error| {
                        format!("could not refresh dependent USD source `{uri}`: {error}")
                    })?;
                let authoring_recipe_revision = None;
                referenced.document.set_authoring_recipe(
                    authoring_recipe_revision,
                    Some(authoring_recipe.clone()),
                );
                let generation = referenced.document.generation();
                let input = referenced
                    .document
                    .capture_stage_input(&uri, false)
                    .map_err(|error| error.to_string())?;
                overlays.push(PreparedRestartOverlay {
                    authority: referenced.authority,
                    relative_path: referenced.relative_path,
                    doc: referenced.doc,
                    source_generation: generation,
                    generation,
                    authoring_recipe_revision,
                    authoring_recipe,
                    input,
                });
            }
            overlays.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
            Ok(PreparedRestartDocument { source, overlays })
        }
        .await;
        drop(permit);
        result
    });
    Ok(Some(RestartDocumentRefresh {
        doc,
        authority: authority.to_owned(),
        relative_path: relative_path.to_owned(),
        path,
        generation,
        mount_revision,
        reset_document,
        task,
    }))
}

fn restart_source_seed(
    input: &lunco_usd_document::document::DocumentStageInput,
) -> Result<lunco_usd_compose::recipe::StageRecipe, String> {
    let root_id = input.recipe.root_id.clone();
    let root_bytes = input
        .recipe
        .bytes
        .get(&root_id)
        .cloned()
        .ok_or_else(|| format!("candidate source recipe has no root bytes for {root_id}"))?;
    let mut seed = lunco_usd_compose::recipe::StageRecipe::new(
        root_id,
        std::collections::HashMap::from([(input.recipe.root_id.clone(), root_bytes)]),
    );
    seed.session_layer = input.recipe.session_layer.clone();
    if let Some(session) = seed.session_layer.as_ref() {
        let bytes = input.recipe.bytes.get(session).cloned().ok_or_else(|| {
            format!("candidate source recipe has no session-layer bytes for {session}")
        })?;
        seed.bytes.insert(session.clone(), bytes);
    }
    Ok(seed)
}

/// Commit a prepared source reload at the lifecycle boundary. Every identity
/// and source lease is checked again before document or mount state changes.
pub(crate) fn drain_pending_scene_restart(
    mut pending: ResMut<PendingSceneRestart>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
    asset_server: Res<AssetServer>,
    mut registry: Option<ResMut<DocumentRegistry<UsdDocument>>>,
    mut backed: Option<ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>>,
    twins: Option<Res<lunco_assets_core::TwinRoots>>,
    mut canonical: Option<NonSendMut<CanonicalStages>>,
    q_usd: Query<(Entity, &UsdPrimPath, Has<UsdSceneRoot>)>,
    scene: SceneEntities,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
    mut commands: Commands,
) {
    let polled = match pending.0.as_mut().and_then(|work| work.document.as_mut()) {
        Some(document) => bevy::tasks::block_on(bevy::tasks::futures_lite::future::poll_once(
            &mut document.task,
        ))
        .map(|result| result.map(Some)),
        None => Some(Ok(None)),
    };
    let Some(prepared) = polled else {
        return;
    };
    let Some(work) = pending.0.take() else {
        return;
    };
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            reject_active_restart(work.id, &error, &mut coordinator, &mut commands);
            return;
        }
    };
    if coordinator.active_id() != Some(work.id)
        || coordinator.active() != Some(&work.transition)
        || coordinator.completed_generation() != Some(work.previous_generation)
        || mount_state
            .as_deref()
            .and_then(lunco_core::SceneMountState::active_root)
            != Some(work.root)
        || !q_usd.get(work.root).is_ok_and(|(_, prim, is_root)| {
            is_root && prim.stage_handle.id() == work.stage.id() && prim.path == work.root_prim
        })
    {
        reject_active_restart(
            work.id,
            "the mounted scene changed while its source reload was preparing",
            &mut coordinator,
            &mut commands,
        );
        return;
    }

    if let Some(refresh) = work.document.as_ref() {
        let Some(twins) = twins.as_deref() else {
            reject_active_restart(
                work.id,
                "the active Twin asset registry retired during reload preparation",
                &mut coordinator,
                &mut commands,
            );
            return;
        };
        let (Some(registry_ref), Some(backed_ref)) = (registry.as_deref(), backed.as_deref())
        else {
            reject_active_restart(
                work.id,
                "the active Twin document projection retired during reload preparation",
                &mut coordinator,
                &mut commands,
            );
            return;
        };
        if twins.revision().ok() != Some(refresh.mount_revision)
            || twins.root_for(&refresh.authority).ok().flatten().is_none()
            || backed_ref.doc_for(&refresh.authority, &refresh.relative_path) != Some(refresh.doc)
            || registry_ref
                .host(refresh.doc)
                .is_none_or(|host| host.document().generation() != refresh.generation)
        {
            reject_active_restart(
                work.id,
                "the active Twin document or mount changed while its source reload was preparing",
                &mut coordinator,
                &mut commands,
            );
            return;
        }

        let Some(prepared_document) = prepared else {
            reject_active_restart(
                work.id,
                "USD reload preparation completed without its document candidate",
                &mut coordinator,
                &mut commands,
            );
            return;
        };
        let Some(active_overlay) = prepared_document
            .overlays
            .iter()
            .find(|overlay| overlay.doc == refresh.doc)
        else {
            reject_active_restart(
                work.id,
                format!(
                    "active USD document {} has no prepared source input",
                    refresh.doc
                ),
                &mut coordinator,
                &mut commands,
            );
            return;
        };
        let dependencies: std::collections::HashSet<_> = active_overlay
            .authoring_recipe
            .bytes
            .keys()
            .cloned()
            .collect();
        let mut prepared_docs = std::collections::HashSet::new();
        for overlay in &prepared_document.overlays {
            if !prepared_docs.insert(overlay.doc)
                || twins.root_for(&overlay.authority).ok().flatten().is_none()
                || backed_ref.doc_for(&overlay.authority, &overlay.relative_path)
                    != Some(overlay.doc)
                || backed_ref.coords_of(overlay.doc)
                    != Some((overlay.authority.clone(), overlay.relative_path.clone()))
                || registry_ref
                    .host(overlay.doc)
                    .is_none_or(|host| host.document().generation() != overlay.source_generation)
            {
                reject_active_restart(
                    work.id,
                    format!(
                        "Twin document {} changed or retired while its source reload was preparing",
                        overlay.doc
                    ),
                    &mut coordinator,
                    &mut commands,
                );
                return;
            }
        }
        for (doc, authority, relative_path, _, _, _) in backed_ref.entries() {
            if dependencies.contains(&lunco_assets_core::twin_uri(&authority, &relative_path))
                && !prepared_docs.contains(&doc)
            {
                reject_active_restart(
                    work.id,
                    format!(
                        "dependent Twin document {doc} opened while its source reload was preparing"
                    ),
                    &mut coordinator,
                    &mut commands,
                );
                return;
            }
        }

        if let Some(prepared_source) = prepared_document.source {
            let Some(registry) = registry.as_deref_mut() else {
                reject_active_restart(
                    work.id,
                    "the active Twin USD document registry retired during reload preparation",
                    &mut coordinator,
                    &mut commands,
                );
                return;
            };
            let Some(path) = refresh.path.as_ref() else {
                reject_active_restart(
                    work.id,
                    "prepared USD source has no file identity",
                    &mut coordinator,
                    &mut commands,
                );
                return;
            };
            let (doc, outcome) = if refresh.reset_document {
                registry.reset_prepared_file(path.clone(), prepared_source)
            } else {
                registry.open_prepared_file(path.clone(), prepared_source, true)
            };
            if doc != refresh.doc || outcome != OpenOutcome::Refreshed {
                reject_active_restart(
                    work.id,
                    format!("USD source refresh was rejected: {outcome:?}"),
                    &mut coordinator,
                    &mut commands,
                );
                return;
            }
        }
        let Some(registry) = registry.as_deref_mut() else {
            reject_active_restart(
                work.id,
                "the active Twin USD document registry retired during reload preparation",
                &mut coordinator,
                &mut commands,
            );
            return;
        };
        for overlay in &prepared_document.overlays {
            if registry
                .host(overlay.doc)
                .is_none_or(|host| host.document().generation() != overlay.generation)
            {
                reject_active_restart(
                    work.id,
                    format!(
                        "committed Twin document {} does not match its prepared stage input",
                        overlay.doc
                    ),
                    &mut coordinator,
                    &mut commands,
                );
                return;
            }
        }
        for overlay in &prepared_document.overlays {
            let Some(host) = registry.host_mut(overlay.doc) else {
                reject_active_restart(
                    work.id,
                    format!(
                        "prepared Twin document {} retired before overlay commit",
                        overlay.doc
                    ),
                    &mut coordinator,
                    &mut commands,
                );
                return;
            };
            host.document_mut().set_authoring_recipe(
                overlay.authoring_recipe_revision,
                Some(overlay.authoring_recipe.clone()),
            );
        }
        let Some(backed) = backed.as_deref_mut() else {
            reject_active_restart(
                work.id,
                "the active Twin document projection retired during reload preparation",
                &mut coordinator,
                &mut commands,
            );
            return;
        };
        if let Err(error) = publish_restart_document_overlays(
            backed,
            twins,
            &asset_server,
            prepared_document.overlays,
        ) {
            reject_active_restart(work.id, error, &mut coordinator, &mut commands);
            return;
        }
    }

    if let Some(stages) = canonical.as_deref_mut() {
        stages.retire(work.stage.id());
    }
    info!("[restart-scene] reloading `{}` from its source", work.label);
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }
    commands.remove_resource::<SceneLoadInFlight>();
    commands.remove_resource::<lunco_usd_bevy_scene::FailedSceneLoad>();
    commands.insert_resource(SceneLoadInFlight {
        transition_id: work.id,
        path: work.label.clone(),
        stage_id: work.stage.id(),
    });
    commands.trigger(lunco_core::SceneTransitionStarted {
        id: work.id,
        transition: work.transition.clone(),
    });
    clear_scene_entities(&mut commands, &scene);
    let PendingSceneRestartWork {
        stage,
        asset_path,
        label,
        root_prim,
        ..
    } = work;
    commands.queue(move |world: &mut World| {
        world.resource::<AssetServer>().reload(asset_path);
        spawn_scene_root_with_stage(world, &label, &root_prim, stage.clone());
    });
}

fn reject_active_restart(
    id: lunco_core::SceneTransitionId,
    error: impl Into<String>,
    coordinator: &mut lunco_core::SceneTransitionCoordinator,
    commands: &mut Commands,
) {
    let error = error.into();
    if coordinator.fail(id) {
        warn!("[restart-scene] source reload stopped before teardown: {error}");
        lunco_core::trigger_runtime_error(commands, "scene-reload-preparation-failed", error);
    }
}

fn fail_restart_before_start(commands: &mut Commands, error: String) {
    warn!("[restart-scene] {error}");
    lunco_core::trigger_runtime_error(commands, "scene-reload-rejected", error);
}

fn publish_restart_document_overlays(
    backed: &mut lunco_usd_bevy_twin::DocBackedTwinScenes,
    twins: &lunco_assets_core::twin_source::TwinRoots,
    server: &AssetServer,
    overlays: Vec<PreparedRestartOverlay>,
) -> Result<(), String> {
    for overlay in overlays {
        if backed.coords_of(overlay.doc)
            != Some((overlay.authority.clone(), overlay.relative_path.clone()))
        {
            return Err(format!(
                "document {} projection lease changed while its source reload was preparing",
                overlay.doc
            ));
        }
        backed.publish_stage_input(overlay.doc, twins, server, overlay.input)?;
        backed.mark_overlay_synced(overlay.doc, overlay.generation);
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
