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
use lunco_doc_bevy::{DocumentRegistry, OpenFile};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_bevy_stage::{UsdStageAsset, source::UsdSourceText};
use lunco_usd_core::commands::{EmptyViewportReason, is_usd_path};
use lunco_usd_document::document::UsdDocument;
use lunco_workspace::open::{PendingTwinOpens, TwinOpenMode, spawn_twin_scan};
use lunco_workspace::{TwinClosed, WorkspaceResource};

/// Telemetry mnemonic for a default Twin scene whose authoritative source did
/// not become available. This is a scene-load failure, not a simulation fault:
/// the viewport remains empty and a later Twin replacement is still admitted.
pub(crate) const TWIN_SCENE_LOAD_FAILED: &str = "TWIN_SCENE_LOAD_FAILED";

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

/// Workspace replacement owns the scene boundary. Closing the old Twin must
/// clear its mounted USD scene immediately, even when the replacement Twin's
/// asynchronous folder scan later fails or takes a long time.
pub(crate) fn clear_scene_on_twin_closed(
    trigger: On<TwinClosed>,
    mut pending_twin: ResMut<crate::twin_projection::PendingTwinDocs>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
    mut backed: ResMut<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
    mut commands: Commands,
) {
    let root = trigger.event().root.clone();
    for key in pending_twin.release_root(&root) {
        admission.cancel_queued(key);
    }
    for doc in backed.release_root(&root) {
        registry.remove(doc);
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
    commands.trigger(ClearScene {});
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
    usd_sources: Option<Res<Assets<UsdSourceText>>>,
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
    if !roots
        .as_deref()
        .and_then(|roots| roots.name_for_root(&twin.root).ok().flatten())
        .is_some_and(|name| name == request.name)
    {
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
    let handle = asset_server.load::<UsdSourceText>(scene_uri);
    let source_ready = usd_sources.get(handle.id()).is_some();
    let source_failed = asset_server
        .get_load_state(handle.id())
        .is_some_and(|state| state.is_failed());
    let source_id = handle.id();
    pending_twin.push(
        handle,
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
) {
    let (Some(_asset_server), Some(_stages)) = (asset_server, stages) else {
        return;
    };
    let Some(path) = validate_scene_address(&trigger.event().path) else {
        return;
    };
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
}

/// Execute the load request that won admission at the scene lifecycle boundary.
/// Public command observers never mutate scene state directly.
pub(crate) fn execute_admitted_load_scene(
    trigger: On<lunco_core::SceneTransitionAdmitted>,
    asset_server: Res<AssetServer>,
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
    let new_id = asset_server.load::<UsdStageAsset>(&path).id();
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
    let Some(stage_path) = q_usd
        .iter()
        .find(|(_, is_root)| *is_root)
        .and_then(|(prim, _)| asset_server.get_path(prim.stage_handle.id()))
        .map(|path| path.to_string())
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
    let Ok(source) = String::from_utf8(bytes) else {
        warn!(
            "[restart-scene] `{}` is not UTF-8 USDA; keeping the mounted source",
            path.display()
        );
        return;
    };
    let (_, outcome) = if *reset_document {
        registry.reset_file(path, source)
    } else {
        registry.open_file(path, source)
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
    if let Err(error) =
        publish_restart_document_overlays(&mut registry, backed, &twins, doc, &name, &dependencies)
    {
        warn!("[restart-scene] could not publish the refreshed Twin sources: {error}");
    }
}

fn publish_restart_document_overlays(
    registry: &mut DocumentRegistry<UsdDocument>,
    backed: &mut lunco_usd_bevy_twin::DocBackedTwinScenes,
    twins: &lunco_assets_core::twin_source::TwinRoots,
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
        sources.push((
            rel,
            doc,
            document.generation(),
            document
                .persistent_composed_source()
                .map_err(|error| error.to_string())?,
        ));
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    for (rel, doc, generation, source) in sources {
        twins
            .set_overlay(name, &rel, std::sync::Arc::new(source.into_bytes()))
            .map_err(|error| error.to_string())?;
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
    use lunco_doc::DocumentId;
    use lunco_usd_document::document::{LayerId, UsdOp};

    #[test]
    fn twin_close_retires_only_the_active_scene_owner() {
        for was_active in [false, true] {
            let mut app = App::new();
            app.init_resource::<crate::twin_projection::PendingTwinDocs>()
                .init_resource::<lunco_core_runtime::AsyncWorkAdmission>()
                .init_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
                .init_resource::<DocumentRegistry<UsdDocument>>()
                .init_resource::<lunco_core::SceneTransitionCoordinator>()
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

    #[test]
    fn restart_refreshes_referenced_documents_without_view_or_unrelated_buffers() {
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let mut backed = lunco_usd_bevy_twin::DocBackedTwinScenes::default();
        let twins = lunco_assets_core::twin_source::TwinRoots::default();
        let mut ids = Vec::<DocumentId>::new();
        for rel in ["scene.usda", "engine.usda", "unrelated.usda"] {
            let (doc, _) = registry.open_file(
                std::path::PathBuf::from("/tmp/restart-overlay-test").join(rel),
                "#usda 1.0\ndef Sphere \"Part\" { double radius = 2 }\n".into(),
            );
            backed.track_preview(doc, "fixture".into(), rel.into());
            twins
                .set_overlay("fixture", rel, std::sync::Arc::new(b"old source".to_vec()))
                .unwrap();
            ids.push(doc);
        }
        let engine = registry.host_mut(ids[1]).unwrap().document_mut();
        engine.mark_restored_dirty();
        engine
            .apply(UsdOp::SetAttribute {
                edit_target: LayerId::view(),
                path: "/Part".into(),
                name: "radius".into(),
                type_name: "double".into(),
                value: "99".into(),
            })
            .unwrap();
        publish_restart_document_overlays(
            &mut registry,
            &mut backed,
            &twins,
            ids[0],
            "fixture",
            &std::collections::HashSet::from(["twin://fixture/engine.usda".into()]),
        )
        .unwrap();
        let bytes = twins
            .overlay_bytes("fixture", Path::new("engine.usda"))
            .unwrap()
            .unwrap();
        let source = String::from_utf8(bytes.as_ref().clone()).unwrap();
        assert!(source.contains("radius = 2"));
        assert!(!source.contains("99"));
        assert!(registry.host(ids[1]).unwrap().document().is_dirty());
        assert!(
            !registry
                .host(ids[1])
                .unwrap()
                .document()
                .composed_source()
                .contains("99")
        );
        assert_eq!(backed.overlay_synced_generation(ids[1]), Some(2));
        assert_eq!(
            twins
                .overlay_bytes("fixture", Path::new("unrelated.usda"))
                .unwrap()
                .unwrap()
                .as_slice(),
            b"old source"
        );
    }
}
