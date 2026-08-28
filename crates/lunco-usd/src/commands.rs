//! `UsdCommandsPlugin` — typed-command surface for USD documents.
//!
//! Plumbs USD into the shared workbench command bus described in
//! `AGENTS.md` §4.2:
//!
//! - **Open**: observes [`OpenFile`]
//!   and handles paths with a USD extension. Modelica observes the same
//!   command for `.mo`; future SysML / mission crates will join the
//!   chorus. Each observer is responsible for its own extension gate so
//!   an `OpenFile { path: "/foo.mo" }` doesn't end up parsed as USD.
//! - **New**: observes [`NewDocument`]
//!   gated on `kind == "usd"`. Lets File→New surface "USD Stage" once
//!   the kind is registered.
//! - **Save**: observes
//!   [`SaveDocument`] gated on
//!   [`DocumentRegistry::<UsdDocument>::contains`].
//! - **Notifications**: each frame drains the registry's pending rings
//!   into [`DocumentOpened`],
//!   [`lunco_doc_bevy::DocumentChanged`], and
//!   [`DocumentClosed`] so views
//!   subscribe through the canonical channels rather than polling the
//!   registry directly.
//!
//! Registers the `usd` document kind in
//! [`DocumentKindRegistry`] on build
//! so File menus, picker dialogs, and `twin.toml` parsers see USD
//! without any central edit.

use crate::document::UsdDocument;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use lunco_core::{on_command, register_commands, Command};
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_doc_bevy::{
    DocumentChanged, DocumentClosed, DocumentOpened, NewDocument, OpenFile, RedoDocument,
    SaveAsDocument, SaveDocument, UndoDocument,
};
use lunco_settings::AppSettingsExt;
use lunco_storage::Storage; // brings `write_sync` / `read_sync` into scope
use lunco_twin::{DocumentKindId, DocumentKindMeta, DocumentKindRegistry};
// The empty-viewport placeholder is a workbench (egui shell) concept; the
// document/file command surface below is headless-safe. Gate only this.
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::{UsdPrimPath, UsdSceneRoot};
#[cfg(feature = "ui")]
use lunco_workbench::ViewportPlaceholder;
use lunco_workspace::open::{spawn_twin_scan, PendingTwinOpens, TwinOpenMode};
use lunco_workspace::{TwinClosed, WorkspaceResource};

use crate::document::{LayerId, UsdOp};
use lunco_doc::OpenOutcome;
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_sim::cosim::{
    clear_scene_entities, resolve_root_prim, spawn_scene_root_world, validate_scene_address,
    ClearScene, LoadScene, SceneEntities, SceneLoadInFlight,
};

/// Stable id for the USD document kind in
/// [`DocumentKindRegistry`].
pub const USD_DOCUMENT_KIND: &str = "usd";

/// A *reason* the viewport is empty, set at the moment a scene-clearing
/// action knows one — e.g. opening a folder whose `twin.toml` declares no
/// `default_scene` (the usual cause: you opened the WRONG FOLDER, one level
/// too shallow, so the real twin's manifest is not where the engine looked).
///
/// Without this, [`update_viewport_placeholder`] only sees `scene.is_empty()`
/// and falls back to a generic "open a scene" hint — which tells you nothing
/// about *why* the scene you expected never appeared. This resource carries
/// that why through to the placeholder for as long as the viewport stays empty,
/// and is cleared the instant a real scene mounts. Headless-safe: it is a plain
/// `Resource` with no UI dependency, so test/`scene_test` bins pay nothing.
#[derive(Resource, Default)]
pub struct EmptyViewportReason(pub Option<String>);

/// Telemetry mnemonic for a default Twin scene whose authoritative source did
/// not become available. This is a scene-load failure, not a simulation fault:
/// the viewport remains empty and a later Twin replacement is still admitted.
pub(crate) const TWIN_SCENE_LOAD_FAILED: &str = "TWIN_SCENE_LOAD_FAILED";

impl EmptyViewportReason {
    /// Record a diagnostic message naming why the viewport was just emptied.
    fn set(&mut self, msg: impl Into<String>) {
        self.0 = Some(msg.into());
    }
}

/// Plugin that registers the USD document kind, the typed-command
/// observers, and the pending-event drain system.
///
/// **Layer 2 (domain).** No UI, no Bevy renderer touches — added by
/// [`UsdPlugins`](crate::UsdPlugins) so any binary that pulls in USD
/// gets the document surface, even headless / sandbox bins.
pub struct UsdCommandsPlugin;

/// Promote an authored document when the live twin projection is installed.
///
/// `apply_ops_as_change_set` is also the reusable headless document-editing
/// boundary, so a caller that only installs `DocumentRegistry` must not panic
/// merely because no live twin projection exists. In the production USD
/// command plugin this resource is always initialized; when it is absent there
/// is no scene lease to promote and no ownership event to publish.
fn claim_user_document_if_projected(world: &mut World, doc: DocumentId) {
    let claimed = world
        .get_resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
        .is_some_and(|mut backed| backed.claim_user(doc));
    if claimed {
        world.trigger(crate::twin_projection::UsdDocumentUserOwned { doc });
    }
}

/// Session restore is the one document-open path that does not carry an
/// explicit user-open command. A restored document is therefore promoted to a
/// user lease here, while an automatically opened Twin scene is already linked
/// to a scene lease and remains internal until the user acts on it.
fn claim_user_document_on_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<crate::twin_projection::DocBackedTwinScenes>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    if !registry.contains(doc) || backed.coords_of(doc).is_some() {
        return;
    }
    if backed.claim_user(doc) {
        commands.trigger(crate::twin_projection::UsdDocumentUserOwned { doc });
    }
}

/// Keep the shared Workspace document list in step with the USD registry.
///
/// This belongs to the headless-safe command/lifecycle plugin rather than the
/// editor UI: startup Twin scenes are opened by this plugin even when the
/// simulator runs without `UsdUiPlugin`, and API/Rhai document discovery must
/// see the same USD documents as the desktop shell.
fn sync_workspace_on_doc_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    if workspace.document(doc).is_some() {
        workspace.active_document = Some(doc);
        return;
    }
    let origin = host.document().origin().clone();
    let context_twin = origin
        .is_untitled()
        .then_some(workspace.active_twin)
        .flatten();
    workspace.add_document(lunco_workspace::DocumentEntry {
        id: doc,
        kind: DocumentKindId::new(USD_DOCUMENT_KIND),
        title: origin.display_name(),
        origin,
        context_twin,
    });
    workspace.active_document = Some(doc);
}

/// Reflect USD Save and Save-As origin changes into the shared Workspace.
fn sync_workspace_on_doc_saved(
    trigger: On<lunco_doc_bevy::DocumentSaved>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    let doc = trigger.event().doc;
    let Some(host) = registry.host(doc) else {
        return;
    };
    let origin = host.document().origin().clone();
    if let Some(path) = origin.canonical_path() {
        workspace.recents.push_loose(path.to_path_buf());
    }
    if let Some(entry) = workspace.document_mut(doc) {
        entry.title = origin.display_name();
        entry.origin = origin;
    }
}

/// Remove the shared Workspace entry when a USD registry document closes.
fn sync_workspace_on_doc_closed(
    trigger: On<DocumentClosed>,
    workspace: Option<ResMut<WorkspaceResource>>,
) {
    let Some(mut workspace) = workspace else {
        return;
    };
    workspace.close_document(trigger.event().doc);
}

/// Registry closure is the final lifetime edge for projection bookkeeping.
/// Remove both scene coordinates and user claims so a closed document cannot
/// be rediscovered by a later stage-path lookup.
fn forget_backed_document_on_closed(
    trigger: On<DocumentClosed>,
    mut backed: ResMut<crate::twin_projection::DocBackedTwinScenes>,
) {
    backed.forget_document(trigger.event().doc);
}

/// Workspace replacement owns the scene boundary. Closing the old Twin must
/// clear its mounted USD scene immediately, even when the replacement Twin's
/// asynchronous folder scan later fails or takes a long time.
fn clear_scene_on_twin_closed(
    trigger: On<TwinClosed>,
    mut pending_twin: ResMut<crate::twin_projection::PendingTwinDocs>,
    mut backed: ResMut<crate::twin_projection::DocBackedTwinScenes>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut commands: Commands,
) {
    let root = trigger.event().root.clone();
    pending_twin.release_root(&root);
    for doc in backed.release_root(&root) {
        registry.remove(doc);
    }
    commands.trigger(ClearScene {});
}

impl Plugin for UsdCommandsPlugin {
    fn build(&self, app: &mut App) {
        // Twin authority registration belongs to the asset boundary and is
        // shared by lunica and luncosim. Install it here only for minimal USD
        // hosts/tests that do not compose the normal asset-source root.
        if !app.is_plugin_added::<lunco_assets::TwinRootsPlugin>() {
            app.add_plugins(lunco_assets::TwinRootsPlugin);
        }
        app.register_settings_section::<crate::runtime_persistence::RuntimePersistenceSettings>();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<lunco_core::SceneTransitionCoordinator>();
        app.add_observer(clear_scene_on_twin_closed);
        app.add_systems(
            lunco_core::SceneTeardown,
            crate::twin_projection::reset_scene_projection_state,
        );

        // Self-register with the workbench's plugin-driven document
        // kind registry. `init_resource` defends against the case where
        // the workbench plugin hasn't been added yet — we still own
        // our entry, the workbench picks it up when it boots.
        app.init_resource::<DocumentKindRegistry>();
        app.world_mut()
            .resource_mut::<DocumentKindRegistry>()
            .register(
                DocumentKindId::new(USD_DOCUMENT_KIND),
                DocumentKindMeta {
                    display_name: "USD Stage".into(),
                    extensions: vec!["usda", "usdc", "usd"],
                    can_create_new: true,
                    default_filename: Some("NewStage.usda"),
                    uri_scheme: Some("usd"),
                    manifest_section: Some("usd"),
                },
            );

        // Document *open/load* pipeline (domain-layer, so it works in
        // headless / sandbox bins that don't add `UsdUiPlugin`). Reads
        // run on the `AsyncComputeTaskPool` through `lunco-storage` and
        // land in the registry via `drain_pending_usd_file_loads`. The
        // UI's `browser_dispatch` only translates browser-panel clicks
        // into calls on this pipeline.
        app.init_resource::<PendingUsdLoads>();
        app.add_systems(Update, drain_pending_usd_file_loads);

        app.add_systems(Update, drain_usd_pending_events);
        app.add_observer(sync_workspace_on_doc_opened);
        app.add_observer(sync_workspace_on_doc_saved);
        app.add_observer(sync_workspace_on_doc_closed);
        // A3 auto-bridge: when the journal appears, hand it to the registry
        // once (reactive — `resource_added`, not per-frame). Headless builds
        // without a journal never run it.
        app.add_systems(
            Update,
            wire_usd_journal_handle.run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );
        // Workbench-only: the empty-viewport placeholder lives in the egui
        // shell; headless / sandbox / server bins don't add it.
        #[cfg(feature = "ui")]
        app.add_systems(Update, update_viewport_placeholder);
        // Carries the *reason* a scene is empty through to the placeholder.
        // Always present (headless too) so the open path can record one without
        // a UI feature gate.
        app.init_resource::<EmptyViewportReason>();
        app.add_observer(open_usd_docs_on_twin_asset_mounted);
        app.add_observer(execute_admitted_load_scene);
        // Restart document refresh belongs to the admitted transaction, not the
        // raw request. A restart queued behind a load must refresh the document
        // that is active when that load reaches its terminal edge.
        app.add_observer(on_restart_scene_refresh_active_document);
        // A mount that died on a missing/unreadable stage leaves the same empty
        // viewport as a deliberate clear. `on_load_scene` cleared the reason on
        // the way in (a load was committed), so without this the placeholder
        // says nothing and the tester sees a blank window with the explanation
        // only in the log. The typed transition failure carries the cause.
        app.add_observer(
            |trigger: On<lunco_core::SceneTransitionFailed>,
             mut empty_reason: ResMut<EmptyViewportReason>| {
                let (lunco_core::SceneTransition::Load { path, .. }
                | lunco_core::SceneTransition::Restart { path, .. }) = &trigger.event().transition
                else {
                    return;
                };
                empty_reason.set(format!(
                    "`{path}` could not be loaded: {}",
                    trigger.event().error
                ));
            },
        );
        // C5-A: persist the runtime overlay (C4b spawns + moves) to
        // `<twin>/.lunco/runtime/<scene>.usda`, parallel to the journal. Loading
        // it is a separate opt-in setting, so corrupt `.lunco` state cannot block
        // authored scene loading.
        app.add_observer(crate::runtime_persistence::on_doc_opened_load_runtime);
        app.add_observer(crate::runtime_persistence::on_doc_changed_save_runtime);
        // E1b: make the default twin scene doc-backed by serving its composed
        // source as a `twin://` byte-overlay (web-ready via the async loader).
        app.init_resource::<crate::twin_projection::PendingTwinDocs>();
        app.init_resource::<crate::twin_projection::DocBackedTwinScenes>();
        app.add_observer(claim_user_document_on_opened);
        app.add_observer(forget_backed_document_on_closed);
        app.init_resource::<crate::live_consume::LiveTransformEditHints>();
        // Referenced spawns whose asset closure is still loading (fetched once,
        // then authored onto the live stage — no whole-scene reload).
        app.init_resource::<crate::twin_projection::PendingRefSpawns>();
        // Gated on the asset pipeline: these need `AssetServer` (to fetch a
        // referenced asset's closure) and the `Assets<UsdSourceText>` store
        // (UsdBevyPlugin's `init_asset`). Both are absent in headless
        // `MinimalPlugins` test apps — and a partial setup can have one without
        // the other — so require both. Chained before `project_stage_changes`
        // (below) so a spawn authored this frame projects the same frame.
        // `PreUpdate`, NOT `Update` — and this is structural, not a preference.
        //
        // `project_stage_changes` DESPAWNS and rebuilds the subtree of any prim
        // whose attributes changed. In `Update` it raced every system that queues
        // commands against those entities: the render binder reacts to
        // `Changed<PbrLook>` and queues `insert(MeshMaterial3d(..))`, the projector
        // then despawns the entity, and the buffered insert panics on apply
        // ("Entity despawned … its index now has generation 1"). Opening the
        // moonbase twin — which replays a runtime overlay, changing looks AND
        // rebuilding subtrees in one frame — hit exactly this.
        //
        // Ordering the projector before that ONE binder would have fixed that ONE
        // panic. But seven crates bind looks and several despawn USD entities, so a
        // per-binder `.before(..)` rule is a rule each of them must remember — i.e.
        // one that gets forgotten by the next system anyone adds. Running the
        // projector a schedule EARLIER makes the hazard unrepresentable instead:
        // every `Update` system, present and future, observes a world the projector
        // has already settled, and none of them can hold a command queued against
        // an entity it is about to despawn.
        //
        // Cost: an op authored during `Update` projects on the next frame's
        // `PreUpdate` rather than the same frame. That is one frame of latency on a
        // path that is already asynchronous (the gizmo writes `Transform`
        // optimistically; nothing reads back the projection within the frame).
        app.add_systems(
            PreUpdate,
            (
                crate::twin_projection::drain_pending_twin_docs,
                // Author doc deltas (translate / spawn / remove) onto the live
                // stage; queue referenced spawns needing a closure fetch.
                crate::twin_projection::sync_twin_overlays,
                // Complete referenced spawns whose closure has now loaded.
                crate::twin_projection::drain_ref_spawns,
                crate::live_consume::project_stage_changes,
            )
                .chain()
                .run_if(resource_exists::<AssetServer>)
                .run_if(resource_exists::<Assets<lunco_usd_bevy::UsdSourceText>>),
        );
        register_all_commands(app);
    }
}

/// Route the dependency-light in-process scene intent to the typed USD command
/// that owns path resolution and scene mounting. This is the single adapter
/// between higher-level domains and the USD command surface; it carries typed
/// data all the way through and never parses a command name or JSON payload.
/// Once the asset boundary has mounted a Twin authority, make the viewport
/// **reflect the opened Twin/folder**.
/// — clear-and-replace, so a previously loaded scene never lingers:
///
/// - **Has `[usd] default_scene`** → construct its `twin://` address and
///   [`LoadScene`] it. `LoadScene` clears the old scene, then mounts this
///   one as the single active stage; [`UsdSimPlugin`](lunco_usd_sim::UsdSimPlugin)
///   derives its native `connectionPaths` wiring from the composed prims.
/// - **No starting scene** (Twin without `default_scene`, or a plain
///   folder with no manifest — including one with **no `.usda` at all**)
///   → [`ClearScene`]: empty viewport. The folder's files are still
///   indexed and shown in the browser; the user picks a scene from there.
///
/// The Twin's other `.usda` files are an **asset library** — indexed but
/// not auto-loaded; composed into the active stage on demand via
/// `AddReference`. Full resolution rule in
/// `docs/architecture/21-domain-usd.md` § "Which stage opens".
///
/// Skips child Twins — they raise their own `TwinAdded` when the
/// workspace eagerly opens them, each resolving its own starting scene.
fn open_usd_docs_on_twin_asset_mounted(
    trigger: On<lunco_assets::TwinAssetMounted>,
    workspace: Res<WorkspaceResource>,
    // Optional because headless hosts may not install the asset pipeline. The
    // authoritative doc-backed mount below is the only production path; the
    // test-only branch preserves this observer's decision coverage in
    // MinimalPlugins apps without pretending to mount a scene there.
    asset_server: Option<Res<AssetServer>>,
    usd_sources: Option<Res<Assets<lunco_usd_bevy::UsdSourceText>>>,
    mut pending_twin: ResMut<crate::twin_projection::PendingTwinDocs>,
    mut empty_reason: ResMut<EmptyViewportReason>,
    mut commands: Commands,
) {
    let twin_id = trigger.event().twin;
    let Some(twin) = workspace.twin(twin_id) else {
        return;
    };
    let default_scene = twin
        .manifest
        .as_ref()
        .and_then(|m| m.usd.as_ref())
        .and_then(|u| u.default_scene.as_deref());
    // The asset boundary emitted this event only after registering the root.
    // Use the exact assigned authority from the event; do not rediscover it
    // through a second lookup whose timing could reintroduce the mount race.
    let twin_name = trigger.event().name.clone();
    match default_scene {
        Some(scene) => {
            let scene_uri = lunco_assets::twin_uri(&twin_name, scene);
            // Load the scene THROUGH the `twin://` source registered above —
            // never a bare absolute path. Works identically on native (fs) and
            // web (http), and keeps the scene's co-located relative refs
            // (terrain glb) resolving under `twin://`.
            //
            // E1b: open the scene as a document FIRST — the mount comes from
            // `drain_pending_twin_docs` once the document exists and its composed
            // (base ⊕ runtime) source is published as the twin overlay, so the
            // one and only stage build already carries persisted runtime
            // spawns/moves. Mounting eagerly here and doc-backing afterwards
            // built the stage from the raw base, then the open-time
            // `restore_runtime` forced a whole-scene rebuild ~70 ms later —
            // every prim (rovers included) spawned twice. Read the base text
            // THROUGH the twin source (web-ready) rather than `std::fs`.
            if let (Some(asset_server), Some(_)) = (&asset_server, &usd_sources) {
                info!(
                    "[twin] doc-backing starting scene `twin://{}/{}` (twin `{}`) — mount follows",
                    twin_name,
                    scene,
                    twin.root.display()
                );
                let handle = asset_server.load::<lunco_usd_bevy::UsdSourceText>(scene_uri.clone());
                pending_twin.push(
                    handle,
                    twin_name.clone(),
                    scene.to_string(),
                    twin.root.join(scene),
                    twin.root.clone(),
                );
            }
            #[cfg(test)]
            if asset_server.is_none() || usd_sources.is_none() {
                info!(
                    "[twin:test] recording starting scene `twin://{}/{}` (twin `{}`)",
                    twin_name,
                    scene,
                    twin.root.display()
                );
                commands.trigger(LoadScene {
                    path: scene_uri,
                    root_prim: String::new(),
                });
            }
            #[cfg(not(test))]
            if asset_server.is_none() || usd_sources.is_none() {
                let detail = format!(
                    "cannot load `{}`: the USD asset pipeline is not installed",
                    scene_uri
                );
                warn!("[twin] {detail}");
                empty_reason.set(detail.clone());
                lunco_core::trigger_error(&mut commands, TWIN_SCENE_LOAD_FAILED, detail);
            }
        }
        None => {
            // A folder with a `twin.toml` that names no `default_scene` is rare;
            // the usual cause of reaching here is that the folder has NO
            // `twin.toml` at all (opened as a plain folder), which most often
            // means the user opened the WRONG DIRECTORY — e.g. the wrapper that
            // *contains* the twin rather than the twin itself. Distinguish the
            // two so the placeholder can tell the user which it is, instead of
            // a generic "nothing to show".
            let has_manifest = twin.manifest.is_some();
            let reason = if has_manifest {
                format!(
                    "`{}` has a twin.toml but declares no default scene — nothing to load.",
                    twin.root.display()
                )
            } else {
                format!(
                    "`{}` has no twin.toml, so there is no scene to load. \
                     You may have opened the wrong folder — check that you opened the Twin \
                     root itself (the one containing twin.toml), not a folder above or beside it.",
                    twin.root.display()
                )
            };
            info!(
                "[twin] `{}` declares no starting scene — clearing viewport ({})",
                twin.root.display(),
                if has_manifest {
                    "manifest present, no default_scene"
                } else {
                    "no twin.toml"
                }
            );
            empty_reason.set(reason);
            commands.trigger(ClearScene {});
        }
    }
}

/// The generic hint shown when the viewport is empty and no specific cause
/// was recorded. Public so tests can assert against it without hardcoding the
/// string in two places.
pub const GENERIC_EMPTY_HINT: &str = "Nothing to show — open a scene or a Twin.";

/// Pure decision: given whether a scene is mounted and an optional recorded
/// [`EmptyViewportReason`], what (if anything) should the placeholder show?
///
/// - Scene present → `None` (render nothing; a real world is on screen).
/// - Empty WITH a recorded reason → `Some(reason)` (the diagnostic — e.g.
///   "opened folder has no twin.toml").
/// - Empty WITHOUT a reason → `Some(GENERIC_EMPTY_HINT)` (the fallback).
///
/// Extracted from [`update_viewport_placeholder`] so the precedence (reason
/// beats generic; scene beats both) is unit-testable without the `ui` feature
/// or a workbench resource.
#[cfg(any(feature = "ui", test))]
fn empty_viewport_message(scene_empty: bool, reason: Option<&str>) -> Option<String> {
    if !scene_empty {
        return None;
    }
    Some(reason.unwrap_or(GENERIC_EMPTY_HINT).to_string())
}

/// Keep the workbench's [`ViewportPlaceholder`] in sync with whether a
/// USD scene is loaded. With **no** `UsdPrimPath` entities — an empty
/// viewport, e.g. right after [`ClearScene`] from opening a scene-less
/// folder — show an empty-state hint; otherwise clear it so the message
/// vanishes the instant a scene mounts. No-op in headless binaries that
/// don't add the workbench (the resource is absent).
///
/// When [`EmptyViewportReason`] carries a *specific* reason the viewport was
/// emptied (e.g. "opened folder has no twin.toml"), prefer it over the generic
/// hint — the generic one tells you nothing about why the scene you expected
/// never appeared. The reason is dropped the moment a real scene mounts, so a
/// subsequent open that succeeds returns to the plain "nothing to show" only
/// when the viewport is next empty *without* a recorded cause.
#[cfg(feature = "ui")]
fn update_viewport_placeholder(
    scene: Query<(), With<UsdPrimPath>>,
    empty_reason: Res<EmptyViewportReason>,
    placeholder: Option<ResMut<ViewportPlaceholder>>,
) {
    let Some(mut placeholder) = placeholder else {
        return;
    };
    if !scene.is_empty() {
        // A real scene is on screen — render nothing. NOTE: the reason is NOT
        // cleared here. Entity despawns from a `ClearScene` are deferred, so on
        // the same frame a folder-open sets a reason and clears the scene, this
        // query can still read the OLD scene's `UsdPrimPath` entities as
        // non-empty — clearing the reason here would wipe the diagnostic the
        // open just recorded. The reason is cleared authoritatively in
        // `on_load_scene` when a NEW scene actually mounts.
        placeholder.message = None;
        return;
    }
    let want = empty_viewport_message(true, empty_reason.0.as_deref());
    if placeholder.message != want {
        placeholder.message = want;
    }
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
    // Optional: this observer is registered by `UsdCommandsPlugin`, which is
    // headless-safe and lands in apps that never build an asset pipeline (the
    // document-surface tests below are exactly that). Mounting a scene is
    // meaningless without one, so a missing asset pipeline is a no-op, not a
    // panic — a required `Res` here aborts the whole `Main` schedule.
    asset_server: Option<Res<AssetServer>>,
    stages: Option<Res<Assets<lunco_usd_bevy::UsdStageAsset>>>,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
) {
    let (Some(_asset_server), Some(_stages)) = (asset_server, stages) else {
        return;
    };
    let Some(path) = validate_scene_address(&cmd.path) else {
        return;
    };
    let root_prim = resolve_root_prim(&path, &cmd.root_prim);

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
fn execute_admitted_load_scene(
    trigger: On<lunco_core::SceneTransitionAdmitted>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath, Has<UsdSceneRoot>)>,
    scene: SceneEntities,
    mut coordinator: ResMut<lunco_core::SceneTransitionCoordinator>,
    // A real scene is mounting — clear any empty-viewport reason recorded by a
    // prior clear/folder-open, so it can't haunt the placeholder once this load
    // despawns/resolves. Done HERE (not in `update_viewport_placeholder`) so a
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
    coordinator.start(transition.clone());
    // Admission is the commit point. Only now does this request own scene state;
    // a request queued behind another transaction must not mutate the active
    // transaction's diagnostics or viewport reason.
    commands.remove_resource::<lunco_usd_bevy::FailedSceneLoad>();
    empty_reason.0 = None;

    // Blender-style no-op: same stage, same root prim, already mounted.
    //
    // The identity is the PAIR `(stage asset, root prim)`, but the two halves of
    // the root prim are asked differently because an empty `root_prim` is
    // `resolve_root_prim`'s deferred sentinel, NOT a path:
    //
    // - sentinel (the ordinary load) means "mount the stage's `defaultPrim`". It
    //   cannot be compared as a string: `instantiate_usd_prim` resolves it and
    //   writes the concrete path BACK onto the scene root, so once the stage has
    //   parsed no entity carries `""` and a string compare matches nothing —
    //   which is why a repeat load used to tear down and remount a live scene.
    //   What the sentinel denotes is the stage's default mount, and that mount is
    //   exactly the `UsdSceneRoot`, so ask for that instead.
    // - an explicit override names a real prim path, so compare it as one.
    //
    // Deliberately NOT "any prim from this stage": the active simulation owns
    // one scene root. The editor preview, when present, uses `UsdPreviewOnly`
    // and is outside this simulation mount identity.
    let new_id = asset_server
        .load::<lunco_usd_bevy::UsdStageAsset>(&path)
        .id();
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
        commands.trigger(lunco_core::SceneTransitionCompleted { transition });
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
        path: path.clone(),
        stage_id: new_id,
    });
    commands.trigger(lunco_core::SceneTransitionStarted { transition });

    // Despawn the old scene + free worker-side state (shared with `ClearScene`).
    clear_scene_entities(&mut commands, &scene);

    // Spawn via shared helper, deferred so despawns flush first.
    commands.queue(move |world: &mut World| {
        spawn_scene_root_world(world, &path, &root_prim);
        if stage_already_loaded {
            world.write_message(lunco_usd_sim::cosim::SceneStageAssetOutcome::Loaded {
                stage_id: new_id,
            });
        }
    });
}

register_commands!(
    on_load_scene,
    on_apply_usd_op,
    on_apply_usd_ops,
    // The USD half of the generic `UndoDocument`/`RedoDocument` verbs. Registering the
    // observers here (not in the editor) is what lets a headless binary undo.
    on_undo_usd_document,
    on_redo_usd_document,
    on_attach_component,
    on_attach_program,
    on_set_dome_light,
    on_new_document,
    on_open_file,
    on_open_file_for_usd,
    on_save_document,
    on_save_as_document,
);

/// Refresh the active doc-backed Twin from its source file before the shared
/// [`RestartScene`] lifecycle handler reloads its asset. The lower simulation
/// layer deliberately does not know documents; it queues its asset reload, which
/// gives this observer one synchronous place to update the composed Twin overlay.
///
/// A normal restart retains dirty documents; a full reset is a separately
/// confirmed intent which discards both their authored and runtime layers. The
/// document registry owns both policies so every file-backed domain keeps the
/// same identity and history invariants.
fn on_restart_scene_refresh_active_document(
    trigger: On<lunco_core::SceneTransitionStarted>,
    asset_server: Option<Res<AssetServer>>,
    q_usd: Query<(&UsdPrimPath, Has<UsdSceneRoot>)>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    backed: Option<Res<crate::twin_projection::DocBackedTwinScenes>>,
    twins: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    role: Option<Res<lunco_core::NetworkRole>>,
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
    let (Some(asset_server), Some(backed), Some(twins)) = (asset_server, backed.as_deref(), twins)
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
        (lunco_assets::twin_uri(&name, &rel) == stage_path).then_some((doc, name, rel))
    });
    let Some((doc, name, rel)) = active else {
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
            warn!("[restart-scene] active Twin has unsaved edits; retaining them instead of overwriting from disk");
        }
        OpenOutcome::KeptUnparsable => {
            warn!("[restart-scene] source did not parse as USDA; retaining the mounted document")
        }
        OpenOutcome::Allocated => {}
    }
    let Some(composed) = registry
        .host(doc)
        .map(|host| host.document().composed_source())
    else {
        return;
    };
    if let Err(error) = twins.set_overlay(&name, &rel, std::sync::Arc::new(composed.into_bytes())) {
        warn!("[restart-scene] could not publish the refreshed Twin source: {error}");
    }
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
    let raw_path = trigger.event().path.clone();
    let path = raw_path
        .strip_prefix("file://")
        .unwrap_or(&raw_path)
        .to_string();
    if !is_usd_path(&path) {
        return;
    }

    // A scheme already names its root. Send it through the typed scene
    // transition so it gets the same admission, teardown, and readiness path
    // as startup, tutorials, and Twin default scenes.
    if lunco_assets::has_scheme(&path) {
        commands.trigger(LoadScene {
            path,
            root_prim: String::new(),
        });
        return;
    }

    let Some(workspace) = workspace else {
        warn!(
            "[OpenFile] cannot open USD filesystem scene `{path}`: WorkspacePlugin is not installed"
        );
        return;
    };
    let abs = match lunco_storage::canonicalize_file_path(Path::new(&path)) {
        Ok(abs) => abs,
        Err(error) => {
            warn!("[OpenFile] cannot resolve USD filesystem scene `{path}`: {error}");
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
            "[OpenFile] cannot open USD filesystem scene `{path}`: WorkspacePlugin is not installed"
        );
        return;
    };
    spawn_twin_from_scene(&abs, pending, "OpenFile");
}

/// Open the root that owns `scene` and select that scene.
///
/// This is the same root-relative scan used by the workbench previously, now
/// owned by the USD scene domain so GUI and headless `OpenFile` requests cannot
/// mount one file through competing paths.
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
        .map(lunco_assets::asset_path::slashed)
        .unwrap_or_else(|_| {
            abs.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    spawn_twin_scan(&root, pending, log_tag, Some(rel), TwinOpenMode::Replace);
}

// ─────────────────────────────────────────────────────────────────────
// USD document open/load pipeline (domain layer)
//
// Moved here from `ui/browser_dispatch.rs` (2026-06-02): file I/O and the
// `OpenFile` command observer are document-lifecycle concerns, not UI.
// Living in `UsdCommandsPlugin` means HTTP API / MCP / `Open`-URI dispatch
// register USD documents even in headless / sandbox bins that never add
// `UsdUiPlugin`. The UI's `browser_dispatch` keeps only the browser-panel
// `BrowserAction` → `spawn_usd_load` translation.
// ─────────────────────────────────────────────────────────────────────

/// Pending file-read kicked off by [`spawn_usd_load`]. Polled by
/// [`drain_pending_usd_file_loads`] each frame until it completes; the
/// resulting source is allocated as a USD document and the viewport
/// picks it up via the standard `DocumentOpened` lifecycle observer.
struct PendingUsdLoad {
    path: PathBuf,
    task: Task<Result<String, String>>,
}

#[derive(Resource, Default)]
pub(crate) struct PendingUsdLoads {
    tasks: Vec<PendingUsdLoad>,
}

/// Observer for the workbench's typed [`OpenFile`] command. Picks up
/// `.usd*` paths so HTTP API / MCP / `Open` URI dispatch all route into
/// the same async-load pipeline the Twin browser uses. Modelica's
/// `on_open_file` ignores non-`.mo` paths, so the observers coexist.
#[on_command(OpenFile)]
fn on_open_file_for_usd(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    commands.queue(move |world: &mut World| {
        // `file://` is a filesystem spelling, not a registered asset source;
        // strip it before deciding whether this is an already-addressable
        // scene URI. Other schemes do not have a filesystem document to read;
        // `on_open_file` sends them through the typed scene transition.
        let stripped = path.strip_prefix("file://").unwrap_or(&path);
        if lunco_assets::has_scheme(stripped) {
            return;
        }
        if !is_usd_path(stripped) {
            return;
        }
        spawn_usd_load(world, PathBuf::from(stripped));
    });
}

/// Spawn the async file-read for `abs_path` and queue the result in
/// [`PendingUsdLoads`]. Callers should have already established that the
/// path looks like a USD file. Shared by the [`OpenFile`] observer and
/// the UI's `browser_dispatch::drain_browser_actions_for_usd`.
pub(crate) fn spawn_usd_load(world: &mut World, abs_path: PathBuf) {
    let pool = AsyncComputeTaskPool::get();
    let path_for_task = abs_path.clone();
    let task = pool.spawn(async move {
        // Read through the storage abstraction — `std::fs` is clippy-banned
        // in domain crates and absent on wasm; `lunco-storage` owns it.
        // `FileStorage`'s read future wraps synchronous fs, so awaiting on
        // the task thread parks no reactor.
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path_for_task.clone());
        match storage.read(&handle).await {
            Ok(bytes) => String::from_utf8(bytes)
                .map_err(|e| format!("invalid UTF-8 in {}: {e}", path_for_task.display())),
            Err(e) => Err(format!("failed to read {}: {e:?}", path_for_task.display())),
        }
    });
    world
        .resource_mut::<PendingUsdLoads>()
        .tasks
        .push(PendingUsdLoad {
            path: abs_path,
            task,
        });
}

/// Poll outstanding [`PendingUsdLoads`] and finish the open once each
/// file's bytes are in hand. Skips and warns on read errors — continuing
/// leaves no half-loaded document behind.
pub(crate) fn drain_pending_usd_file_loads(world: &mut World) {
    if world.resource::<PendingUsdLoads>().tasks.is_empty() {
        return;
    }

    let taken = std::mem::take(&mut world.resource_mut::<PendingUsdLoads>().tasks);
    let mut still_pending: Vec<PendingUsdLoad> = Vec::new();

    for mut load in taken {
        match block_on(future::poll_once(&mut load.task)) {
            None => still_pending.push(load),
            Some(Err(err)) => {
                bevy::log::warn!("[UsdOpenFile] {}", err);
            }
            Some(Ok(source)) => {
                // Idempotent re-open: one document per file, base refreshed from
                // the text we just read. This used to be a hand-rolled scan plus
                // `if existing.is_none() { allocate(source) }` — so re-opening an
                // already-open file threw `source` away and kept the stale
                // document, even though the read had just happened.
                let (doc, outcome) = world
                    .resource_mut::<DocumentRegistry<UsdDocument>>()
                    .open_file(load.path.clone(), source);
                claim_user_document_if_projected(world, doc);
                // A re-open that couldn't take the disk bytes is not an error,
                // but it IS a surprise the user should see — "I opened the file
                // and nothing happened" otherwise. `warn!` alone was invisible
                // in the app; also raise it on the status bus (UI builds only).
                let user_notice = match outcome {
                    OpenOutcome::KeptDirty => {
                        bevy::log::warn!(
                            "[UsdOpenFile] {} has unsaved edits — keeping them; disk NOT reloaded ({doc})",
                            load.path.display()
                        );
                        Some("has unsaved edits — kept them, did not reload from disk")
                    }
                    OpenOutcome::KeptUnparsable => {
                        bevy::log::warn!(
                            "[UsdOpenFile] {} does not parse as USDA — keeping the open document ({doc})",
                            load.path.display()
                        );
                        Some("does not parse as USDA — kept the open document")
                    }
                    OpenOutcome::Refreshed => {
                        bevy::log::info!(
                            "[UsdOpenFile] {} already open — refreshed from disk ({doc})",
                            load.path.display()
                        );
                        None
                    }
                    OpenOutcome::Allocated => None,
                };
                #[cfg(feature = "ui")]
                if let Some(msg) = user_notice {
                    if let Some(mut bus) =
                        world.get_resource_mut::<lunco_workbench::status_bus::StatusBus>()
                    {
                        let name = load
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| load.path.display().to_string());
                        bus.push(
                            "usd",
                            lunco_workbench::status_bus::StatusLevel::Warn,
                            format!("{name} {msg}"),
                        );
                    }
                }
                #[cfg(not(feature = "ui"))]
                let _ = user_notice;
            }
        }
    }

    world.resource_mut::<PendingUsdLoads>().tasks = still_pending;
}

// ─────────────────────────────────────────────────────────────────────
// NewDocument — File→New "USD Stage"
// ─────────────────────────────────────────────────────────────────────

#[on_command(NewDocument)]
fn on_new_document(trigger: On<NewDocument>, mut commands: Commands) {
    if trigger.event().kind != USD_DOCUMENT_KIND {
        return;
    }
    commands.queue(|world: &mut World| {
        let mut registry = world.resource_mut::<DocumentRegistry<UsdDocument>>();
        let next = registry.ids().count() + 1;
        let doc_id = registry.allocate(
            DEFAULT_USDA_SCAFFOLD.to_string(),
            lunco_doc::PathlessOrigin::untitled(format!("UntitledStage-{}.usda", next)),
        );
        drop(registry);
        claim_user_document_if_projected(world, doc_id);
        bevy::log::info!("[NewUsd] created untitled USD stage as {}", doc_id);
    });
}

/// Minimal valid `.usda` source for File→New. One empty `World` Xform
/// — enough that the parser is happy and the user has somewhere to
/// add prims.
const DEFAULT_USDA_SCAFFOLD: &str =
    "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n}\n";

// ─────────────────────────────────────────────────────────────────────
// SaveDocument — gated on registry membership
// ─────────────────────────────────────────────────────────────────────

#[on_command(SaveDocument)]
fn on_save_document(trigger: On<SaveDocument>, mut commands: Commands) {
    let doc_id = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let registry = world.resource::<DocumentRegistry<UsdDocument>>();
        let Some(host) = registry.host(doc_id) else {
            return;
        };
        let doc = host.document();
        let path = match doc.origin() {
            DocumentOrigin::File {
                path,
                writable: true,
            } => path.clone(),
            DocumentOrigin::File {
                writable: false, ..
            } => {
                bevy::log::warn!("[SaveUsd] {} is read-only", doc_id);
                return;
            }
            DocumentOrigin::Untitled { .. } => {
                bevy::log::warn!("[SaveUsd] {} is Untitled — Save-As required", doc_id);
                return;
            }
            DocumentOrigin::Bundled { .. } => {
                bevy::log::warn!("[SaveUsd] {} is a bundled example — read-only", doc_id);
                return;
            }
        };
        let source = doc.source();
        // Route through the storage abstraction instead of a direct
        // `std::fs::write` (clippy-banned in domain crates, wasm-broken).
        // `write_sync` blocks on `FileStorage`'s write future, which wraps
        // synchronous fs and is already `Ready` — no reactor, no hang.
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(e) = storage.write_sync(&handle, source.as_bytes()) {
            bevy::log::error!(
                "[SaveUsd] {} write to {} failed: {:?}",
                doc_id,
                path.display(),
                e
            );
            return;
        }
        // Borrow mut to mark saved. `host_mut` doesn't bump the
        // change ring because saving doesn't change the document — it
        // only resets the dirty marker.
        {
            let mut reg = world.resource_mut::<DocumentRegistry<UsdDocument>>();
            if let Some(host) = reg.host_mut(doc_id) {
                host.document_mut().mark_saved();
            }
            // Re-baseline the disk watermark: the bytes on disk are now ours, so
            // the staleness check must not flag this write as an external edit.
            reg.note_saved(doc_id);
        }
        bevy::log::info!("[SaveUsd] {} saved to {}", doc_id, path.display());
    });
}

/// Persist a USD document to a new path and rebind its canonical origin.
///
/// Untitled stages are real documents, so Save-As is the promotion edge that
/// makes their edits visible to the ordinary file/Twin workflow. The domain
/// owns the bytes and origin update; the workbench only supplies a path when a
/// dialog is needed.
#[on_command(SaveAsDocument)]
fn on_save_as_document(
    trigger: On<SaveAsDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    #[cfg(feature = "ui")] workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    let doc_id = trigger.event().doc;
    let target_path = trigger.event().path.clone();
    let Some(host) = registry.host(doc_id) else {
        bevy::log::warn!("[SaveAsUsd] unknown document {doc_id}");
        return;
    };
    let document = host.document();
    let source = document.source().to_string();
    #[cfg(feature = "ui")]
    let suggested_name = {
        let name = document.origin().display_name();
        if name.to_ascii_lowercase().ends_with(".usda")
            || name.to_ascii_lowercase().ends_with(".usdc")
            || name.to_ascii_lowercase().ends_with(".usd")
        {
            name
        } else {
            format!("{name}.usda")
        }
    };

    if target_path.is_empty() {
        #[cfg(feature = "ui")]
        {
            let start_dir = workspace
                .as_deref()
                .and_then(|ws| ws.active_twin)
                .and_then(|id| workspace.as_deref()?.twin(id))
                .map(|twin| lunco_storage::StorageHandle::File(twin.root.clone()));
            commands.trigger(lunco_workbench::picker::PickHandle {
                mode: lunco_workbench::picker::PickMode::SaveFile(
                    lunco_workbench::picker::SaveHint {
                        suggested_name: Some(suggested_name),
                        start_dir,
                        filters: vec![lunco_workbench::picker::OpenFilter::new(
                            "USD stages",
                            &["usda", "usdc", "usd"],
                        )],
                    },
                ),
                on_resolved: lunco_workbench::picker::PickFollowUp::SaveAs(doc_id),
            });
        }
        #[cfg(not(feature = "ui"))]
        bevy::log::warn!(
            "[SaveAsUsd] {doc_id} has no target path; a headless caller must provide one"
        );
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(feature = "ui")]
        {
            if let Err(error) = lunco_workbench::picker::download_file(&suggested_name, &source) {
                bevy::log::error!("[SaveAsUsd] {doc_id} download failed: {error:?}");
                return;
            }
            if let Some(host) = registry.host_mut(doc_id) {
                host.document_mut().mark_saved();
            }
            commands.trigger(lunco_doc_bevy::DocumentSaved::local(doc_id));
        }
        #[cfg(not(feature = "ui"))]
        bevy::log::warn!("[SaveAsUsd] {doc_id} cannot save without the browser UI backend");
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = std::path::PathBuf::from(target_path);
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(error) = storage.write_sync(&handle, source.as_bytes()) {
            bevy::log::error!(
                "[SaveAsUsd] {doc_id} write to {} failed: {error}",
                path.display()
            );
            return;
        }
        if let Some(host) = registry.host_mut(doc_id) {
            host.document_mut().set_origin(DocumentOrigin::File {
                path: path.clone(),
                writable: true,
            });
            host.document_mut().mark_saved();
        }
        bevy::log::info!("[SaveAsUsd] {doc_id} saved to {}", path.display());
        commands.trigger(lunco_doc_bevy::DocumentSaved::local(doc_id));
    }
}

// ─────────────────────────────────────────────────────────────────────
// ApplyUsdOp — typed entry for programmatic / UI-driven edits
// ─────────────────────────────────────────────────────────────────────

/// Apply a [`UsdOp`] to the named document via the typed-command bus.
///
/// Same shape as `lunco-modelica`'s op-dispatch commands: UI clicks,
/// HTTP API calls, and scripts all dispatch this; the observer
/// routes it through [`DocumentRegistry::<UsdDocument>::apply`] so undo/redo,
/// change notification, and read-only enforcement stay in one place.
#[Command(default)]
pub struct ApplyUsdOp {
    /// Target document.
    pub doc: DocumentId,
    /// Operation to apply.
    pub op: UsdOp,
}

/// Apply one authored intent that lowers to several USD operations.
///
/// This is the command boundary for program construction, component assembly,
/// and other compound edits: UI, Rhai and API callers all submit the same typed
/// operation list, which is journalled as one undo unit and observed by the live
/// projector only after the document reaches its complete shape.
#[Command(default)]
pub struct ApplyUsdOps {
    /// Target document.
    pub doc: DocumentId,
    /// Human-readable undo/journal label.
    pub label: String,
    /// Ordered primitive USD operations comprising the one intent.
    pub ops: Vec<UsdOp>,
}

#[on_command(ApplyUsdOps)]
fn on_apply_usd_ops(trigger: On<ApplyUsdOps>, mut commands: Commands) {
    let command = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let (applied, total) =
            apply_ops_as_change_set(world, command.doc, command.label, command.ops);
        if applied != total {
            bevy::log::warn!(
                "[ApplyUsdOps] {} applied {applied}/{total} operations",
                command.doc
            );
        }
    });
}

#[on_command(ApplyUsdOp)]
fn on_apply_usd_op(trigger: On<ApplyUsdOp>, mut commands: Commands) {
    let doc = trigger.event().doc;
    let op = trigger.event().op.clone();
    commands.queue(move |world: &mut World| {
        // Apply through the registry funnel. Journaling is automatic (A3):
        // the host carries a `JournalOpRecorder` installed by
        // `wire_usd_journal_recorders`, so a successful `apply` records the
        // lossless (forward, inverse) pair — no per-op recording code here,
        // and the same seam journals undo/redo too.
        let result = world
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .apply(doc, op);
        match result {
            Ok(ack) => {
                claim_user_document_if_projected(world, doc);
                bevy::log::debug!("[ApplyUsdOp] {} → gen {}", doc, ack.new_gen.unwrap_or(0));
            }
            Err(reject) => {
                bevy::log::warn!("[ApplyUsdOp] {} rejected: {:?}", doc, reject);
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// UndoDocument / RedoDocument — the ONE undo, per-domain
// ─────────────────────────────────────────────────────────────────────
//
// The VERB is generic and lives in `lunco-doc-bevy`; each domain observes it and acts
// only on documents its own registry owns (a Modelica document is handled by Modelica's
// observer in `lunco-modelica/src/ui/commands/doc.rs`). These are USD's half, and they
// live HERE — in the crate that owns `DocumentRegistry<UsdDocument>` — not in the editor, so a
// headless binary with documents but no 3D editor can still undo.
//
// There is no separate `UndoEdit`/`RedoEdit`: it was a second, USD-only pair of commands
// with a byte-for-byte identical body, which would have advertised four undo verbs on the
// API and silently done nothing on a Modelica document.

/// Per-domain [`UndoDocument`] handler for **USD** documents: pop the document's last op
/// and apply its typed inverse.
///
/// This is the **only** undo. Every authored edit — spawn, move, delete, terrain stroke,
/// waypoint, property — reaches the world as a [`UsdOp`] through [`ApplyUsdOp`], and
/// `UsdDocument::apply` hands back a typed inverse for each. So undo is a document
/// concern, not an editor one: pop the inverse, apply it, and the projection re-derives
/// the ECS ([`crate::live_consume`]). It journals (undo/redo record through the same
/// `OpRecorder` seam) and replicates like any other op.
///
/// An editor-side "remember the old Transform and write it back" stack cannot do this: it
/// does not know about the document, so an undone spawn stays in the layer and the
/// journal, and the two disagree. There used to be one; it is gone.
///
/// No-ops for a `doc` this registry doesn't own, per the `UndoDocument` ownership
/// convention.
#[on_command(UndoDocument)]
pub fn on_undo_usd_document(
    trigger: On<UndoDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<crate::twin_projection::DocBackedTwinScenes>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    let outcome = {
        let Some(host) = registry.host_mut(doc) else {
            return;
        };
        host.undo()
    };
    match outcome {
        Ok(true) => {
            // `host_mut` bypasses the registry's `apply` funnel, so the Changed
            // notification has to be raised by hand (documented on `host_mut`). The twin
            // projection then re-derives the scene.
            registry.mark_changed(doc);
            if backed.claim_user(doc) {
                commands.trigger(crate::twin_projection::UsdDocumentUserOwned { doc });
            }
            bevy::log::info!("[usd] undo applied on {doc}");
        }
        Ok(false) => bevy::log::info!("[usd] nothing to undo on {doc}"),
        Err(e) => bevy::log::warn!("[usd] undo failed on {doc}: {e:?}"),
    }
}

/// Per-domain [`RedoDocument`] handler for **USD** documents. The mirror of
/// [`on_undo_usd_document`]; same ownership rules.
#[on_command(RedoDocument)]
pub fn on_redo_usd_document(
    trigger: On<RedoDocument>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<crate::twin_projection::DocBackedTwinScenes>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    let outcome = {
        let Some(host) = registry.host_mut(doc) else {
            return;
        };
        host.redo()
    };
    match outcome {
        Ok(true) => {
            registry.mark_changed(doc);
            if backed.claim_user(doc) {
                commands.trigger(crate::twin_projection::UsdDocumentUserOwned { doc });
            }
            bevy::log::info!("[usd] redo applied on {doc}");
        }
        Ok(false) => bevy::log::info!("[usd] nothing to redo on {doc}"),
        Err(e) => bevy::log::warn!("[usd] redo failed on {doc}: {e:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// AttachComponent — build-from-parts (doc 48 §3.1)
// ─────────────────────────────────────────────────────────────────────

/// Apply a lowered [`UsdOp`] sequence to `doc` as **one journal change set** —
/// i.e. one undo unit (H10).
///
/// A command that lowers to many primitive ops (`AttachComponent` → 7 base ops,
/// plus optional socket/rotation/axis ops; `realign_component_ops` → 4) must not
/// journal them as N independent entries:
/// one undo would then peel off ONE op and leave the object half-attached. The
/// journal's change-set API exists for exactly this
/// ([`lunco_doc_bevy::JournalResource::change_set`] → `Journal::begin_change_set`),
/// and the auto-recorder on the registry appends with `change_set: None`, so every
/// entry recorded inside the closure inherits the ambient set with no per-op code.
/// `UndoManager::take_undo_group` then undoes the whole group.
///
/// **Every multi-op USD handler should route through this** — including the
/// `realign_component_ops` call sites in `lunco-luncosim-edit`.
///
/// The complete sequence is validated against a clone of the current USD document
/// before the live host is touched. A malformed multi-op intent therefore applies
/// zero operations; a valid intent is then committed as one journal change set.
/// Headless builds with no `JournalResource` still get the same all-or-nothing
/// validation, just without undo grouping.
///
/// Returns `(applied, total)`.
pub fn apply_ops_as_change_set(
    world: &mut World,
    doc: DocumentId,
    label: impl Into<String>,
    ops: Vec<UsdOp>,
) -> (usize, usize) {
    let total = ops.len();
    // `UsdDocument` is cloneable and its `Document::apply` path is the authoritative
    // validator. Run the whole intent against a private candidate first so a bad
    // attribute, missing prim, or read-only document cannot leave a half-authored
    // terrain/component assembly in the live host.
    let validation = {
        let registry = world.resource::<DocumentRegistry<UsdDocument>>();
        match registry.host(doc) {
            None => Err(lunco_doc::Reject::InvalidOp(format!("unknown doc {doc}"))),
            Some(host) => {
                let mut candidate = host.document().clone();
                let mut result = Ok(());
                for op in &ops {
                    if let Err(error) = candidate.apply(op.clone()) {
                        result = Err(match error {
                            lunco_doc::DocumentError::ReadOnly => lunco_doc::Reject::ReadOnly,
                            lunco_doc::DocumentError::ValidationFailed(message)
                            | lunco_doc::DocumentError::Internal(message) => {
                                lunco_doc::Reject::InvalidOp(message)
                            }
                            _ => lunco_doc::Reject::InvalidOp(format!("{error:?}")),
                        });
                        break;
                    }
                }
                result
            }
        }
    };
    if let Err(reject) = validation {
        bevy::log::warn!("[usd] {doc} compound operation rejected before apply: {reject:?}");
        return (0, total);
    }

    // Clone the handle FIRST: `registry.apply` takes `&mut World`'s registry, so
    // the journal resource can't stay borrowed across it.
    let journal = world
        .get_resource::<lunco_doc_bevy::JournalResource>()
        .cloned();

    let apply_all = |world: &mut World| {
        let mut registry = world.resource_mut::<DocumentRegistry<UsdDocument>>();
        let mut applied = 0usize;
        for op in ops {
            match registry.apply(doc, op) {
                Ok(_) => applied += 1,
                Err(reject) => bevy::log::warn!(
                    "[usd] {doc} op rejected ({applied}/{total} applied): {reject:?}"
                ),
            }
        }
        applied
    };

    let applied = match journal {
        Some(j) => j.change_set(label, || apply_all(world)),
        None => apply_all(world),
    };
    if applied != 0 {
        claim_user_document_if_projected(world, doc);
    }
    (applied, total)
}

/// Attach a component asset to a host body as a jointed child, deriving the
/// joint anchor from the placement so it is authored once, not twice. Lowers to
/// the primitive [`UsdOp`]s in [`crate::attach::attach_component_ops`].
///
/// The whole lowering is applied inside **one journal change set**
/// ([`apply_ops_as_change_set`]), so the attach is **one undo unit** — undo removes
/// the part, its placement, its joint and the joint's anchors together. (It used to
/// journal one entry per op: an undo peeled off a single op and left the object
/// half-attached.)
///
/// The generic compound boundary validates the complete lowered sequence before
/// touching the live document. Socket attaches additionally validate the selected
/// socket, its schema, kind, joint/axis, asset plug, occupancy, and child
/// identity here so a stale or incompatible request cannot author a bad mount.
fn authored_text(
    data: &openusd::sdf::Data,
    prim: &openusd::sdf::Path,
    name: &str,
) -> Option<String> {
    let attribute = prim.append_property(name).ok()?;
    data.field(&attribute, "default")?
        .as_str()
        .map(str::to_string)
}

fn is_descendant_or_self(path: &openusd::sdf::Path, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path.as_str() == root
        || path
            .as_str()
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_attach_component(
    world: &World,
    doc: DocumentId,
    spec: &crate::attach::AttachSpec,
) -> Result<(), String> {
    if spec.placement.iter().any(|value| !value.is_finite())
        || spec.rotate_deg.iter().any(|value| !value.is_finite())
    {
        return Err("attachment transform contains a non-finite value".into());
    }
    let host_root = spec.host_path.trim_end_matches('/');
    let registry = world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .ok_or_else(|| "USD document registry is unavailable".to_string())?;
    let host = registry
        .host(doc)
        .ok_or_else(|| format!("unknown USD document {doc}"))?;
    let composed = host.document().composed();

    let host_path = openusd::sdf::Path::new(host_root)
        .map_err(|error| format!("invalid host path {}: {error}", spec.host_path))?;
    if composed.spec(&host_path).is_none() {
        return Err(format!("host body {} does not exist", spec.host_path));
    }
    if !lunco_usd_bevy::has_api_schema(&composed, &host_path, "PhysicsRigidBodyAPI") {
        return Err(format!(
            "host {} does not apply PhysicsRigidBodyAPI",
            spec.host_path
        ));
    }

    let Some(socket_path) = spec.socket_path.as_deref() else {
        return Ok(());
    };
    let mount_prefix = format!("{host_root}/Mounts/");
    let Some(socket_name) = socket_path.strip_prefix(&mount_prefix) else {
        return Err(format!(
            "socket {socket_path} is not under host mount group {host_root}/Mounts"
        ));
    };
    if socket_name.is_empty() || socket_name.contains('/') {
        return Err(format!(
            "socket {socket_path} must be a direct child of {host_root}/Mounts"
        ));
    }
    let socket = openusd::sdf::Path::new(socket_path)
        .map_err(|error| format!("invalid socket path {socket_path}: {error}"))?;
    if composed.spec(&socket).is_none() {
        return Err(format!(
            "socket {socket_path} does not exist in document {doc}"
        ));
    }
    if !lunco_usd_bevy::has_api_schema(&composed, &socket, "LunCoMountSocketAPI") {
        return Err(format!(
            "socket {socket_path} does not apply LunCoMountSocketAPI"
        ));
    }
    let accepts = authored_text(&composed, &socket, "lunco:mount:socket")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("socket {socket_path} has no accepted plug kind"))?;
    let expected_joint = match &spec.joint {
        crate::attach::AttachJoint::Fixed => "fixed",
        crate::attach::AttachJoint::Revolute { .. } => "revolute",
        crate::attach::AttachJoint::Prismatic { .. } => "prismatic",
    };
    let actual_joint = authored_text(&composed, &socket, "lunco:mount:joint")
        .unwrap_or_else(|| "fixed".to_string());
    if actual_joint != expected_joint {
        return Err(format!(
            "socket {socket_path} requires joint {actual_joint}, request supplied {expected_joint}"
        ));
    }
    let requested_axis = match &spec.joint {
        crate::attach::AttachJoint::Fixed => None,
        crate::attach::AttachJoint::Revolute { axis }
        | crate::attach::AttachJoint::Prismatic { axis } => Some(match axis {
            crate::attach::Axis::X => "X",
            crate::attach::Axis::Y => "Y",
            crate::attach::Axis::Z => "Z",
        }),
    };
    let authored_axis =
        authored_text(&composed, &socket, "lunco:mount:axis").filter(|value| !value.is_empty());
    match (requested_axis, authored_axis.as_deref()) {
        (None, None) => {}
        (None, Some(axis)) => {
            return Err(format!(
                "fixed socket {socket_path} must not author axis {axis}"
            ));
        }
        (Some(expected), Some(actual)) if expected == actual => {}
        (Some(expected), actual) => {
            return Err(format!(
                "socket {socket_path} requires axis {actual:?}, request supplied {expected}"
            ));
        }
    }
    if let Some(existing) = lunco_usd_bevy::read_rel_target(&composed, &socket, "lunco:mount:part")
    {
        let existing_path = openusd::sdf::Path::new(&existing).map_err(|error| {
            format!("socket {socket_path} has invalid lunco:mount:part target {existing}: {error}")
        })?;
        if !is_descendant_or_self(&existing_path, host_root) {
            return Err(format!(
                "socket {socket_path} points outside host body {host_root}"
            ));
        }
        return Err(format!(
            "socket {socket_path} is already occupied by {existing}"
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let schemes = world
            .get_resource::<lunco_assets::SchemeRegistry>()
            .ok_or_else(|| "asset scheme registry is unavailable".to_string())?;
        let local_asset = schemes
            .local_path(&spec.asset)
            .map_err(|error| format!("could not resolve attachment asset {}: {error}", spec.asset))?
            .ok_or_else(|| format!("attachment asset {} has no local file", spec.asset))?;
        let plug = lunco_usd_bevy::mount::read_asset_plug(&local_asset)
            .ok_or_else(|| format!("attachment asset {} has no valid mount plug", spec.asset))?;
        if plug.kind != accepts {
            return Err(format!(
                "attachment asset {} advertises plug {}, socket accepts {}",
                spec.asset, plug.kind, accepts
            ));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = accepts;
        return Err("socket attachment asset validation is unavailable on wasm".into());
    }
    let child = format!("{host_root}/{}", spec.name);
    let child_path = openusd::sdf::Path::new(&child)
        .map_err(|error| format!("invalid attached child path {child}: {error}"))?;
    if composed.spec(&child_path).is_some() {
        return Err(format!("attached child {child} already exists"));
    }
    Ok(())
}

#[Command(default)]
pub struct AttachComponent {
    /// Target document.
    pub doc: DocumentId,
    /// The attachment to perform.
    pub spec: crate::attach::AttachSpec,
}

#[on_command(AttachComponent)]
fn on_attach_component(trigger: On<AttachComponent>, mut commands: Commands) {
    let doc = trigger.event().doc;
    let spec = trigger.event().spec.clone();
    commands.queue(move |world: &mut World| {
        if let Err(error) = validate_attach_component(world, doc, &spec) {
            bevy::log::warn!("[AttachComponent] attach rejected: {error}");
            return;
        }
        let ops = crate::attach::attach_component_ops(&spec);
        let label = format!("Attach {} to {}", spec.name, spec.host_path);
        let (applied, n) = apply_ops_as_change_set(world, doc, label, ops);
        bevy::log::info!(
            "[AttachComponent] {doc}: attached `{}` to `{}` ({applied}/{n} ops, one change set)",
            spec.name,
            spec.host_path
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// AttachProgram — source-backed simulation program authoring
// ─────────────────────────────────────────────────────────────────────

/// Attach one source-backed simulation program to an existing USD prim.
///
/// The command lowers the complete `LunCoProgramAPI` contract — source asset,
/// declared scalar ports, constants, connections, and realtime-safety promise —
/// to one journaled USD change set. The Models palette, Rhai, HTTP, and future
/// editor surfaces all use this command; none inserts ECS marker components.
///
/// An empty `inputs`/`outputs` contract is valid for an effects-only program,
/// but it is not a running scalar co-simulation participant. Add explicit ports
/// and connections when the source must exchange values with Rust or Modelica.
#[Command(default)]
pub struct AttachProgram {
    /// Target USD document.
    pub doc: DocumentId,
    /// Complete program attachment intent.
    pub spec: crate::program::ProgramAttachSpec,
}

#[on_command(AttachProgram)]
fn on_attach_program(trigger: On<AttachProgram>, mut commands: Commands) {
    let command = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let ops = match crate::program::program_attach_ops(&command.spec) {
            Ok(ops) => ops,
            Err(error) => {
                bevy::log::warn!(
                    "[AttachProgram] {} rejected before authoring: {}",
                    command.doc,
                    error
                );
                return;
            }
        };
        let label = format!(
            "Attach program {} to {}",
            command.spec.name, command.spec.host_path
        );
        let (applied, total) = apply_ops_as_change_set(world, command.doc, label, ops);
        if applied == total {
            bevy::log::info!(
                "[AttachProgram] {}: attached `{}` to `{}` ({total} ops, one change set)",
                command.doc,
                command.spec.name,
                command.spec.host_path
            );
        } else {
            bevy::log::warn!(
                "[AttachProgram] {} rejected during authoring: {applied}/{total} ops applied",
                command.doc
            );
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// SetDomeLight — HDRI environment, authored as a UsdLuxDomeLight
// ─────────────────────────────────────────────────────────────────────

/// Author the scene's HDRI environment: a `UsdLuxDomeLight` carrying
/// `inputs:texture:file`. Projected by `lunco_usd_bevy::dome` into a skybox +
/// image-based lighting.
///
/// **This is the only way to change the environment at runtime.** It lowers to
/// [`UsdOp`]s and goes through [`apply_ops_as_change_set`], so the edit saves,
/// journals, undoes as ONE unit, and replicates — exactly like any other USD
/// edit. Writing to the `Skybox`/`GeneratedEnvironmentMapLight` components
/// directly would light the local viewport and be invisible to all four of
/// those, which is the failure mode this command exists to prevent.
///
/// Idempotent: `AddPrim` is a `define_prim`, so re-issuing hot-replaces the
/// dome rather than stacking duplicates. Every field is `Option` — `None`
/// leaves the authored value alone, so a lighting tweak need not restate the
/// texture.
#[Command(default)]
pub struct SetDomeLight {
    /// Document to author into. `None` = the workspace's active document.
    pub doc: Option<DocumentId>,
    /// Prim path of the dome. `None` = `/World/Sky`.
    ///
    /// It must live **under the stage's `defaultPrim` subtree** (`/World` in
    /// every scene here) — a prim authored outside it composes into the layer
    /// but is never mounted, so the sky would silently not appear.
    pub path: Option<String>,
    /// `inputs:texture:file` — the HDRI, resolved relative to the stage layer
    /// (e.g. `../hdri/lunar_horizon_2k.hdr`). Equirectangular (`.hdr`, `.png`)
    /// or a `.ktx2` cubemap.
    pub texture: Option<String>,
    /// `inputs:intensity` — multiplier on the image (1.0 = as authored).
    pub intensity: Option<f32>,
    /// `inputs:exposure` — stops, applied as intensity × 2^exposure.
    pub exposure: Option<f32>,
    /// `inputs:color` — linear RGB tint multiplied into the image.
    pub color: Option<[f32; 3]>,
    /// `xformOp:rotateXYZ`, **degrees** — spins the environment. The usual case
    /// is yaw only (`[0, heading, 0]`).
    pub rotation: Option<[f32; 3]>,
    /// `lunco:dome:skybox` — `false` lights the scene from the HDRI but leaves
    /// the sky black. The lunar case: real bounce light, no visible sky.
    pub skybox: Option<bool>,
}

#[on_command(SetDomeLight)]
fn on_set_dome_light(
    trigger: On<SetDomeLight>,
    backed: Option<Res<crate::twin_projection::DocBackedTwinScenes>>,
    asset_server: Res<AssetServer>,
    roots: Query<&UsdPrimPath, With<UsdSceneRoot>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // The running scene's root is the single entity that knows both things this
    // command needs: which document to author into, and which prim to author
    // under. Ask it for both, rather than counting registry entries (the
    // registry also holds terrain and script documents, so "the only one" is not
    // a thing that exists) or hardcoding `/World` (the sandbox scene is rooted
    // at `/SandboxScene`, and a prim authored under a non-existent parent
    // composes into the layer and is then never mounted — an invisible sky).
    let root = match roots.iter().collect::<Vec<_>>()[..] {
        [root] => root,
        [] => {
            bevy::log::warn!("[SetDomeLight] no scene is loaded — nothing to author a dome onto");
            return;
        }
        _ => {
            bevy::log::warn!(
                "[SetDomeLight] several scenes are mounted — pass `doc` and `path` explicitly"
            );
            return;
        }
    };

    let doc = match cmd.doc {
        Some(doc) => doc,
        None => {
            let Some(doc) = backed.as_ref().and_then(|b| {
                crate::twin_projection::scene_document_for(b, &asset_server, root.stage_handle.id())
            }) else {
                bevy::log::warn!(
                    "[SetDomeLight] the running scene is a raw-file scene (not doc-backed), so it \
                     has no document to journal into — open it as a Twin, or pass `doc`"
                );
                return;
            };
            doc
        }
    };

    // Default the dome to a `Sky` prim directly under the scene's *mounted root*
    // — `/SandboxScene/Sky`, `/World/Sky`, … — which is inside the subtree the
    // stage actually mounts, and so is the one place a new prim is guaranteed to
    // compose AND appear.
    let path = cmd.path.clone().unwrap_or_else(|| {
        let root_path = root.path.trim_end_matches('/');
        format!("{root_path}/Sky")
    });

    // Split `/SandboxScene/Sky` → parent `/SandboxScene`, name `Sky`: `AddPrim`
    // takes them separately.
    let Some((parent, name)) = path.rsplit_once('/') else {
        bevy::log::warn!("[SetDomeLight] `{path}` is not an absolute prim path");
        return;
    };
    let parent = if parent.is_empty() { "/" } else { parent }.to_string();
    let name = name.to_string();
    if name.is_empty() {
        bevy::log::warn!("[SetDomeLight] `{path}` has no prim name");
        return;
    }

    let cmd = cmd.clone();
    commands.queue(move |world: &mut World| {
        let root = LayerId::root();
        let mut ops = vec![UsdOp::AddPrim {
            edit_target: root.clone(),
            parent_path: parent,
            name,
            type_name: Some("DomeLight".into()),
            reference: None,
        }];

        // `SetAttribute`'s non-string branch parses `value` as a USDA literal,
        // so an asset path is spelled with its `@…@` delimiters and a color3f
        // as `(r, g, b)`. See the op's docs — this is the one place the
        // encoding is decided, and no call site hand-escapes.
        let mut attr = |name: &str, ty: &str, value: String| {
            ops.push(UsdOp::SetAttribute {
                edit_target: root.clone(),
                path: path.clone(),
                name: name.into(),
                type_name: ty.into(),
                value,
            });
        };
        if let Some(t) = &cmd.texture {
            attr("inputs:texture:file", "asset", format!("@{t}@"));
            // Be explicit rather than leaning on USD's `automatic`: it makes the
            // authored intent legible in the .usda, and `automatic` is what a
            // reader has to *guess* at.
            attr("inputs:texture:format", "token", "\"latlong\"".into());
        }
        if let Some(i) = cmd.intensity {
            attr("inputs:intensity", "float", i.to_string());
        }
        if let Some(e) = cmd.exposure {
            attr("inputs:exposure", "float", e.to_string());
        }
        if let Some([r, g, b]) = cmd.color {
            attr("inputs:color", "color3f", format!("({r}, {g}, {b})"));
        }
        if let Some(s) = cmd.skybox {
            attr("lunco:dome:skybox", "bool", s.to_string());
        }
        // Rotation is an xformOp, not a plain attribute: `SetRotate` also
        // authors `xformOpOrder` when the prim has none, which a bare
        // `SetAttribute` would not — the sky would then simply not rotate.
        if let Some([x, y, z]) = cmd.rotation {
            ops.push(UsdOp::SetRotate {
                edit_target: root.clone(),
                path: path.clone(),
                value: [x as f64, y as f64, z as f64],
            });
        }

        let (applied, n) = apply_ops_as_change_set(world, doc, "Set dome light", ops);
        bevy::log::info!(
            "[SetDomeLight] {doc}: authored `{path}` ({applied}/{n} ops, one change set)"
        );
    });
}

/// A3 auto-bridge: hand the [`JournalResource`](lunco_doc_bevy::JournalResource)
/// to the USD registry the moment it appears, so it fits a
/// [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) onto existing and
/// future hosts. Edits — **including undo/redo** — then record losslessly with
/// no per-op code.
///
/// Reactive, not per-frame: gated by `resource_added`, so it runs once (the
/// frame the journal is installed) and never again. Headless builds without a
/// journal never run it.
fn wire_usd_journal_handle(
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    journal: Res<lunco_doc_bevy::JournalResource>,
) {
    registry.set_journal(journal.clone());
}

// ─────────────────────────────────────────────────────────────────────
// Pending-event drain — registry rings → trigger events
// ─────────────────────────────────────────────────────────────────────

/// Each frame, drain the registry's pending-event rings into the
/// canonical [`lunco_doc_bevy`] notification triggers.
///
/// Mirrors the publish-events system in `lunco-modelica`. Cheap
/// no-op when nothing is pending; gated implicitly by the
/// `Vec::is_empty` checks inside `drain_pending`.
fn drain_usd_pending_events(
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut commands: Commands,
) {
    let pending = registry.drain_pending();
    if pending.opened.is_empty() && pending.changed.is_empty() && pending.closed.is_empty() {
        return;
    }
    for doc in pending.opened {
        commands.trigger(DocumentOpened::local(doc));
    }
    for doc in pending.changed {
        commands.trigger(DocumentChanged::local(doc));
    }
    for doc in pending.closed {
        commands.trigger(DocumentClosed::local(doc));
    }
}

// ─────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────

/// True if `path`'s extension is one of `usda` / `usdc` / `usd`.
/// Used by the `OpenFile` observer to skip non-USD paths.
pub fn is_usd_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        std::path::Path::new(&lower)
            .extension()
            .and_then(|s| s.to_str()),
        Some("usda") | Some("usdc") | Some("usd")
    )
}

#[cfg(test)]
mod change_set_tests {
    //! **H10** — a multi-op command undoes as ONE unit.
    //!
    //! `AttachComponent` lowers to 7 base `UsdOp`s. Each journals its own lossless
    //! `(forward, inverse)` entry, so before this fix one undo peeled off ONE op
    //! and left the part attached-but-unplaced (or jointed to nothing).
    //! `ChangeSetId` was designed for exactly this and used by nobody.
    use super::*;
    use crate::attach::{attach_component_ops, AttachJoint, AttachSpec, Axis};
    use crate::document::LayerId;
    use lunco_doc_bevy::JournalResource;
    use lunco_twin_journal::{AuthorTag, UndoManager, UndoScope};

    const RIG: &str =
        "#usda 1.0\ndef Xform \"Rig\"\n{\n    def Xform \"Chassis\"\n    {\n    }\n}\n";

    fn wheel_spec() -> AttachSpec {
        AttachSpec::new(
            LayerId::root(),
            "/Rig/Chassis",
            "Wheel",
            "components/mobility/wheel.usda",
            [0.5, -0.3, 1.2],
            AttachJoint::Revolute { axis: Axis::X },
        )
    }

    /// A journal-wired world holding one open USD document.
    fn world_with_doc() -> (World, DocumentId, JournalResource) {
        let mut world = World::new();
        let journal = JournalResource::default();
        world.insert_resource(journal.clone());

        let mut registry = DocumentRegistry::<UsdDocument>::default();
        // The A3 auto-bridge, done by hand (the system that does this in-app is
        // `wire_usd_journal_handle`): the recorder is what journals each op.
        registry.set_journal(journal.clone());
        let (doc, _) = registry.open_file("/tmp/lunco_h10_attach.usda", RIG.to_string());
        world.insert_resource(registry);
        (world, doc, journal)
    }

    #[test]
    fn attach_component_journals_one_change_set_and_undoes_as_one_unit() {
        let (mut world, doc, journal) = world_with_doc();
        let spec = wheel_spec();
        let ops = attach_component_ops(&spec);
        let n = ops.len();
        assert!(
            n > 1,
            "the attach lowering is multi-op — that is the whole finding"
        );

        let (applied, total) = apply_ops_as_change_set(&mut world, doc, "Attach Wheel", ops);
        assert_eq!(
            (applied, total),
            (n, n),
            "every op applies onto a valid host"
        );

        journal.with_read(|j| {
            let entries: Vec<_> = j.entries_for_doc(doc).collect();
            assert_eq!(entries.len(), n, "one journal entry per op (unchanged)");

            // THE FIX: they all belong to ONE change set.
            let cs = entries[0]
                .change_set
                .expect("the handler must open a change set — this is H10");
            assert!(
                entries.iter().all(|e| e.change_set == Some(cs)),
                "every op of the command must join the SAME change set"
            );
            assert_eq!(
                j.change_set_entries(cs).len(),
                n,
                "the change set groups all {n} ops"
            );

            // And the undo view takes the whole group: one undo, whole attach.
            let mut um = UndoManager::new(AuthorTag::local_user());
            for e in &entries {
                um.record_local(e.id.clone());
            }
            let group = um.take_undo_group(&UndoScope::Document(doc), j);
            assert_eq!(
                group.len(),
                n,
                "one undo must peel off the WHOLE attach, not 1-of-{n}"
            );
            assert!(
                !um.can_undo(),
                "nothing left behind — the attach was one unit"
            );
        });
    }

    #[test]
    fn rejected_compound_operation_does_not_partially_apply() {
        let (mut world, doc, journal) = world_with_doc();
        let ops = vec![
            UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: "/Rig/Chassis".to_owned(),
                name: "test:compoundValue".to_owned(),
                type_name: "float".to_owned(),
                value: "1.0".to_owned(),
            },
            UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: "/Rig/Missing".to_owned(),
                name: "test:compoundValue".to_owned(),
                type_name: "float".to_owned(),
                value: "2.0".to_owned(),
            },
        ];

        assert_eq!(
            apply_ops_as_change_set(&mut world, doc, "Invalid compound", ops),
            (0, 2)
        );
        assert_eq!(
            world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .expect("document host")
                .generation(),
            0,
            "validation must happen before the live host is mutated"
        );
        journal.with_read(|j| {
            assert_eq!(j.entries_for_doc(doc).count(), 0);
        });
    }

    /// The un-grouped baseline: applying the same ops one-by-one through the
    /// registry (what every multi-op site did before) journals `n` *independent*
    /// undo units — one undo leaves the object half-attached. This is the bug the
    /// test above asserts is gone, pinned so a regression is unambiguous.
    #[test]
    fn ungrouped_apply_undoes_one_op_at_a_time() {
        let (mut world, doc, journal) = world_with_doc();
        let ops = attach_component_ops(&wheel_spec());
        let n = ops.len();
        {
            let mut registry = world.resource_mut::<DocumentRegistry<UsdDocument>>();
            for op in ops {
                registry.apply(doc, op).expect("applies");
            }
        }
        journal.with_read(|j| {
            let entries: Vec<_> = j.entries_for_doc(doc).collect();
            assert_eq!(entries.len(), n);
            assert!(
                entries.iter().all(|e| e.change_set.is_none()),
                "no ambient change set ⇒ no grouping"
            );
            let mut um = UndoManager::new(AuthorTag::local_user());
            for e in &entries {
                um.record_local(e.id.clone());
            }
            assert_eq!(
                um.take_undo_group(&UndoScope::Document(doc), j).len(),
                1,
                "ungrouped: one undo peels off ONE op — the half-applied state H10 describes"
            );
        });
    }

    /// No journal (headless) — the ops still apply; there is simply nothing to
    /// group. The helper must not require a `JournalResource`.
    #[test]
    fn applies_without_a_journal() {
        let mut world = World::new();
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let (doc, _) = registry.open_file("/tmp/lunco_h10_nojournal.usda", RIG.to_string());
        world.insert_resource(registry);

        let ops = attach_component_ops(&wheel_spec());
        let n = ops.len();
        let (applied, total) = apply_ops_as_change_set(&mut world, doc, "Attach Wheel", ops);
        assert_eq!((applied, total), (n, n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_usd_path_recognises_extensions() {
        assert!(is_usd_path("/tmp/scene.usda"));
        assert!(is_usd_path("scene.USD"));
        assert!(is_usd_path("foo/bar.usdc"));
        assert!(!is_usd_path("/tmp/model.mo"));
        assert!(!is_usd_path("README.md"));
        assert!(!is_usd_path(""));
    }

    /// Smoke-test: building the plugin into a minimal app inserts
    /// the registry, the document kind, and survives one frame.
    #[test]
    fn plugin_boots_and_registers_kind() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        assert!(app
            .world()
            .contains_resource::<DocumentRegistry<UsdDocument>>());
        let kinds = app.world().resource::<DocumentKindRegistry>();
        let meta = kinds
            .meta(&DocumentKindId::new(USD_DOCUMENT_KIND))
            .expect("usd kind registered");
        assert_eq!(meta.display_name, "USD Stage");
        assert_eq!(meta.extensions, vec!["usda", "usdc", "usd"]);
    }

    #[test]
    fn open_file_for_usd_path_creates_document() {
        // Write a tiny .usda to a tempfile we can resolve.
        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join("lunco_usd_open_file_test.usda");
        std::fs::write(&tmp_path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

        // `UsdCommandsPlugin` now owns the whole open pipeline (observer +
        // PendingUsdLoads + drain) — no UI plugin needed. `MinimalPlugins`
        // supplies the `AsyncComputeTaskPool` the read runs on.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(OpenFile {
            path: tmp_path.to_string_lossy().to_string(),
        });
        // Flush the queued world-command (spawns the async read task),
        // then advance a few ticks until the read completes and the document
        // is allocated.
        for _ in 0..5 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        assert_eq!(
            reg.ids().count(),
            1,
            "exactly one USD doc opened (no duplicate)"
        );

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn open_file_file_uri_creates_document() {
        let tmp_path = std::env::temp_dir().join("lunco_usd_open_file_uri_test.usda");
        std::fs::write(&tmp_path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(OpenFile {
            path: format!("file://{}", tmp_path.display()),
        });
        for _ in 0..5 {
            app.update();
        }

        assert_eq!(
            app.world()
                .resource::<DocumentRegistry<UsdDocument>>()
                .ids()
                .count(),
            1,
            "file:// USD paths must use the filesystem document reader"
        );
        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn open_file_for_non_usd_path_is_noop() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(OpenFile {
            path: "/tmp/some_model.mo".to_string(),
        });
        for _ in 0..5 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        assert_eq!(reg.ids().count(), 0, "non-USD path must not allocate");
    }

    #[test]
    fn apply_usd_op_builds_a_rover_through_typed_command_bus() {
        use crate::document::{LayerId, UsdOp};
        use lunco_doc::Document;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        // Allocate a blank document.
        let doc_id = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.allocate(
                "#usda 1.0\n(\n    metersPerUnit = 1\n)\n".to_string(),
                lunco_doc::PathlessOrigin::untitled("UntitledRover.usda"),
            )
        };
        app.update();

        // Drive a sequence of ApplyUsdOp commands — same path UI
        // toolbars and the HTTP API will use.
        let ops = [
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/".into(),
                name: "Rover".into(),
                type_name: Some("Xform".into()),
                reference: None,
            },
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/Rover".into(),
                name: "Body".into(),
                type_name: Some("Cube".into()),
                reference: None,
            },
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/Rover".into(),
                name: "WheelFL".into(),
                type_name: Some("Cube".into()),
                reference: None,
            },
            UsdOp::SetTranslate {
                edit_target: LayerId::root(),
                path: "/Rover/WheelFL".into(),
                value: [1.0, 0.0, 1.0],
            },
        ];
        for op in ops {
            app.world_mut().trigger(ApplyUsdOp { doc: doc_id, op });
            app.update();
        }
        // One more tick to flush any final queued world commands.
        app.update();

        use lunco_usd_bevy::usd_data::UsdDataExt;
        use openusd::sdf::Path as SdfPath;
        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let host = reg.host(doc_id).expect("doc still alive");
        // Assert on the canonical data (the document is data-canonical now;
        // exact serialized-text formatting is openusd's business, not ours).
        let data = host.document().data();
        // `UsdDataExt` on purpose: this asserts what the ops AUTHORED into the
        // document layer, not what a stage composes out of it.
        assert_eq!(
            data.prim_type_name(&SdfPath::new("/Rover").unwrap())
                .as_deref(),
            Some("Xform")
        );
        assert_eq!(
            data.prim_type_name(&SdfPath::new("/Rover/Body").unwrap())
                .as_deref(),
            Some("Cube")
        );
        assert_eq!(
            data.prim_type_name(&SdfPath::new("/Rover/WheelFL").unwrap())
                .as_deref(),
            Some("Cube")
        );
        assert_eq!(
            data.prim_attribute_value::<[f64; 3]>(
                &SdfPath::new("/Rover/WheelFL").unwrap(),
                "xformOp:translate"
            ),
            Some([1.0, 0.0, 1.0])
        );
        // Generation advanced once per op.
        assert_eq!(host.document().generation(), 4);
    }

    /// Phase A1: every `ApplyUsdOp` that lands records one **lossless**
    /// `EntryKind::Op` into the canonical Twin journal — the recorded op
    /// deserializes back to the exact `UsdOp` (not a hand summary), and a
    /// real `UsdOp` inverse rides alongside it.
    #[test]
    fn apply_usd_op_records_lossless_journal_entries() {
        use crate::document::{LayerId, UsdOp};
        use lunco_twin_journal::{DomainKind, EntryKind};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        // The Twin-journal plugin isn't part of `UsdCommandsPlugin`; install
        // the resource directly so the apply funnel has somewhere to record.
        app.insert_resource(lunco_doc_bevy::JournalResource::default());
        app.update();

        let doc_id = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.allocate(
                "#usda 1.0\n".to_string(),
                lunco_doc::PathlessOrigin::untitled("UntitledJournal.usda"),
            )
        };
        app.update();

        let forward_ops = [
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/".into(),
                name: "Rover".into(),
                type_name: Some("Xform".into()),
                reference: None,
            },
            UsdOp::SetTranslate {
                edit_target: LayerId::root(),
                path: "/Rover".into(),
                value: [2.0, 0.0, 5.0],
            },
        ];
        for op in forward_ops.clone() {
            app.world_mut().trigger(ApplyUsdOp { doc: doc_id, op });
            app.update();
        }
        app.update();

        let journal = app.world().resource::<lunco_doc_bevy::JournalResource>();
        journal.with_read(|j| {
            let ops: Vec<_> = j
                .entries_for_doc(doc_id)
                .filter_map(|e| match &e.kind {
                    EntryKind::Op {
                        domain,
                        op,
                        inverse,
                    } => Some((domain.clone(), op.clone(), inverse.clone())),
                    _ => None,
                })
                .collect();
            assert_eq!(ops.len(), 2, "one Op entry recorded per applied UsdOp");
            for (i, (domain, op_val, inv_val)) in ops.iter().enumerate() {
                assert_eq!(*domain, DomainKind::Usd);
                // Lossless: the recorded op deserializes back to the exact UsdOp.
                let decoded: UsdOp = serde_json::from_value(op_val.clone())
                    .expect("recorded op round-trips to UsdOp");
                assert_eq!(format!("{decoded:?}"), format!("{:?}", forward_ops[i]));
                // The inverse is a real UsdOp too. Phase C3 records TYPED
                // inverses where exact: AddPrim of a brand-new prim inverts to
                // a RemovePrim; SetTranslate that synthesizes `xformOpOrder`
                // falls back to a coarse full-source ReplaceSource snapshot.
                let inv: UsdOp = serde_json::from_value(inv_val.clone())
                    .expect("recorded inverse round-trips to UsdOp");
                match i {
                    0 => assert!(
                        matches!(inv, UsdOp::RemovePrim { .. }),
                        "AddPrim of a new prim inverts to a typed RemovePrim, got {inv:?}"
                    ),
                    1 => assert!(
                        matches!(inv, UsdOp::ReplaceSource { .. }),
                        "SetTranslate inverts to a coarse ReplaceSource, got {inv:?}"
                    ),
                    _ => unreachable!(),
                }
            }
        });
    }

    /// What the twin-open observer decided to do with the viewport.
    #[derive(Resource, Default)]
    struct SceneCmds {
        /// `LoadScene.path` values emitted (one per scene loaded).
        loads: Vec<String>,
        /// Count of `ClearScene` emitted.
        clears: usize,
    }

    /// Build a temp Twin folder (two `.usda`, one `.mo`, given
    /// `twin.toml`), drive a `TwinAdded`, and report which scene
    /// command the observer emitted. `LoadScene`/`ClearScene` handlers
    /// live in `UsdSimPlugin` (not added here); counting observers
    /// capture the observer's decision directly.
    #[cfg(test)]
    fn scene_cmds_for_twin(toml_body: &str, dir_name: &str) -> SceneCmds {
        use lunco_twin::TwinMode;
        use lunco_workspace::WorkspaceResource;

        let tmp = std::env::temp_dir().join(dir_name);
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("twin.toml"), toml_body).unwrap();
        std::fs::write(tmp.join("scene_a.usda"), "#usda 1.0\ndef Xform \"A\" {}\n").unwrap();
        std::fs::write(tmp.join("scene_b.usda"), "#usda 1.0\ndef Xform \"B\" {}\n").unwrap();
        std::fs::write(
            tmp.join("controller.mo"),
            "model Controller end Controller;\n",
        )
        .unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<WorkspaceResource>();
        app.init_resource::<lunco_assets::twin_source::TwinRoots>();
        app.add_plugins(UsdCommandsPlugin);
        app.init_resource::<SceneCmds>();
        app.add_observer(|t: On<LoadScene>, mut c: ResMut<SceneCmds>| {
            c.loads.push(t.event().path.clone());
        });
        app.add_observer(|_t: On<ClearScene>, mut c: ResMut<SceneCmds>| {
            c.clears += 1;
        });
        app.update();

        let twin = match TwinMode::open(&tmp).expect("twin opens") {
            TwinMode::Twin(t) | TwinMode::Folder(t) => t,
            other => panic!("expected Twin/Folder variant, got {:?}", other),
        };
        let twin_id = app
            .world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(twin);
        app.world_mut()
            .trigger(lunco_workspace::TwinAdded { twin: twin_id });
        for _ in 0..4 {
            app.update();
        }
        let out = std::mem::take(app.world_mut().resource_mut::<SceneCmds>().as_mut());
        let _ = std::fs::remove_dir_all(&tmp);
        out
    }

    /// Drive `TwinAdded` for a folder containing **no `.usda` files**
    /// (and no `twin.toml`), returning the observer's decision.
    #[cfg(test)]
    fn scene_cmds_for_empty_folder(dir_name: &str) -> SceneCmds {
        use lunco_twin::TwinMode;
        use lunco_workspace::WorkspaceResource;

        let tmp = std::env::temp_dir().join(dir_name);
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("notes.txt"), "no scenes here\n").unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<WorkspaceResource>();
        app.init_resource::<lunco_assets::twin_source::TwinRoots>();
        app.add_plugins(UsdCommandsPlugin);
        app.init_resource::<SceneCmds>();
        app.add_observer(|t: On<LoadScene>, mut c: ResMut<SceneCmds>| {
            c.loads.push(t.event().path.clone());
        });
        app.add_observer(|_t: On<ClearScene>, mut c: ResMut<SceneCmds>| {
            c.clears += 1;
        });
        app.update();

        let twin = match TwinMode::open(&tmp).expect("folder opens") {
            TwinMode::Twin(t) | TwinMode::Folder(t) => t,
            other => panic!("expected Folder variant, got {:?}", other),
        };
        let twin_id = app
            .world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(twin);
        app.world_mut()
            .trigger(lunco_workspace::TwinAdded { twin: twin_id });
        for _ in 0..4 {
            app.update();
        }
        let out = std::mem::take(app.world_mut().resource_mut::<SceneCmds>().as_mut());
        let _ = std::fs::remove_dir_all(&tmp);
        out
    }

    #[test]
    fn twin_added_loads_only_declared_starting_scene() {
        // `[usd] default_scene` names the one scene to load (clear +
        // replace). scene_b is an asset library — must NOT load.
        let cmds = scene_cmds_for_twin(
            "name = \"t\"\nversion = \"0.1.0\"\n\n[usd]\ndefault_scene = \"scene_a.usda\"\n",
            "lunco_usd_twin_starting_scene_test",
        );
        assert_eq!(cmds.loads.len(), 1, "exactly one scene loaded");
        assert!(
            cmds.loads[0].ends_with("scene_a.usda"),
            "the declared starting scene, got {:?}",
            cmds.loads
        );
        assert_eq!(
            cmds.clears, 0,
            "LoadScene clears internally — no extra ClearScene"
        );
    }

    #[test]
    fn twin_added_without_default_scene_clears_viewport() {
        // No `default_scene` (also covers a folder with no `.usda`):
        // clear to an empty viewport, load nothing.
        let cmds = scene_cmds_for_twin(
            "name = \"t\"\nversion = \"0.1.0\"\n",
            "lunco_usd_twin_no_scene_test",
        );
        assert!(
            cmds.loads.is_empty(),
            "no scene loaded, got {:?}",
            cmds.loads
        );
        assert_eq!(cmds.clears, 1, "viewport cleared to empty");
    }

    #[test]
    fn open_folder_with_no_usda_shows_nothing() {
        // Folder with no `.usda` and no `twin.toml`: clear to empty,
        // load nothing — the viewport must show nothing.
        let cmds = scene_cmds_for_empty_folder("lunco_usd_empty_folder_test");
        assert!(
            cmds.loads.is_empty(),
            "nothing to load, got {:?}",
            cmds.loads
        );
        assert_eq!(cmds.clears, 1, "empty folder clears the viewport");
    }

    /// Opening a folder with NO `twin.toml` (the "wrong folder" mistake)
    /// must record a diagnostic reason naming that cause, so the viewport
    /// placeholder can tell the user WHY it is empty instead of a generic
    /// hint. Drives the REAL `open_usd_docs_on_twin_asset_mounted` observer.
    #[test]
    fn folder_with_no_manifest_records_wrong_folder_reason() {
        use lunco_twin::TwinMode;
        use lunco_workspace::WorkspaceResource;

        let tmp = std::env::temp_dir().join("lunco_usd_no_manifest_reason");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("readme.txt"), "not a twin\n").unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<WorkspaceResource>();
        app.init_resource::<lunco_assets::twin_source::TwinRoots>();
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        let twin = match TwinMode::open(&tmp).expect("folder opens") {
            TwinMode::Folder(t) => t,
            other => panic!("a folder with no twin.toml is Folder, got {:?}", other),
        };
        let twin_id = app
            .world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(twin);
        app.world_mut()
            .trigger(lunco_workspace::TwinAdded { twin: twin_id });
        for _ in 0..4 {
            app.update();
        }

        let reason = app
            .world()
            .get_resource::<EmptyViewportReason>()
            .expect("EmptyViewportReason is always present")
            .0
            .as_ref()
            .expect("a no-twin.toml folder must record a reason");
        assert!(
            reason.contains("no twin.toml"),
            "reason should name the missing manifest, got: {reason}"
        );
        assert!(
            reason.contains("wrong folder"),
            "reason should hint the likely cause, got: {reason}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn new_document_with_usd_kind_creates_untitled() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(NewDocument {
            kind: USD_DOCUMENT_KIND.to_string(),
        });
        app.update();
        app.update();

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        assert_eq!(reg.ids().count(), 1);
        let id = reg.ids().next().unwrap();
        assert!(reg.host(id).unwrap().document().origin().is_untitled());
    }

    #[test]
    fn save_as_untitled_usd_writes_source_and_rebinds_origin() {
        let tmp = tempfile::tempdir().expect("save destination");
        let target = tmp.path().join("scene.usda");
        let source = "#usda 1.0\ndef Xform \"World\" {}\n";

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        let doc = {
            let mut registry = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            registry.allocate(
                source.to_string(),
                lunco_doc::PathlessOrigin::untitled("UntitledStage.usda"),
            )
        };
        app.world_mut().trigger(lunco_doc_bevy::SaveAsDocument {
            doc,
            path: target.display().to_string(),
        });
        app.update();

        let registry = app.world().resource::<DocumentRegistry<UsdDocument>>();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            registry.host(doc).unwrap().document().source()
        );
        assert_eq!(
            registry
                .host(doc)
                .unwrap()
                .document()
                .origin()
                .canonical_path(),
            Some(target.as_path())
        );
        assert!(!registry.host(doc).unwrap().document().is_dirty());
    }

    #[test]
    fn new_usd_document_is_registered_as_the_active_workspace_document() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_workspace::WorkspaceResource>();
        app.add_plugins(UsdCommandsPlugin);
        app.add_plugins(crate::ui::UsdUiPlugin);
        app.update();

        app.world_mut().trigger(NewDocument {
            kind: USD_DOCUMENT_KIND.to_string(),
        });
        app.update();
        app.update();

        let workspace = app.world().resource::<lunco_workspace::WorkspaceResource>();
        let doc = workspace.active_document.expect("new USD doc is active");
        let entry = workspace.document(doc).expect("new USD doc is registered");
        assert_eq!(entry.kind.as_str(), USD_DOCUMENT_KIND);
        assert!(entry.origin.is_untitled());
    }

    /// A scene present on screen beats every empty-state message — even a
    /// recorded reason — so a stale reason can't haunt a viewport that now has
    /// a real world in it. (The UI system clears the reason too; this asserts
    /// the pure decision agrees.)
    #[test]
    fn empty_viewport_message_prefers_a_mounted_scene_over_a_reason() {
        assert_eq!(
            empty_viewport_message(false, Some("opened the wrong folder")),
            None
        );
    }

    /// An empty viewport WITH a recorded reason shows that reason — the whole
    /// point of the channel: tell the user *why* the scene they expected never
    /// appeared, instead of the generic "open a scene" hint.
    #[test]
    fn empty_viewport_message_reason_beats_generic_hint() {
        let reason = "`/x` has no twin.toml — you may have opened the wrong folder.";
        assert_eq!(
            empty_viewport_message(true, Some(reason)).as_deref(),
            Some(reason)
        );
    }

    /// An empty viewport WITHOUT a recorded reason uses the generic hint for
    /// cold start / cleared scenes.
    #[test]
    fn empty_viewport_message_falls_back_to_generic_hint() {
        assert_eq!(
            empty_viewport_message(true, None).as_deref(),
            Some(GENERIC_EMPTY_HINT)
        );
    }
}
