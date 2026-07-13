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
//!   [`UsdDocumentRegistry::contains`].
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

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_storage::Storage; // brings `write_sync` / `read_sync` into scope
use lunco_doc_bevy::{
    DocumentChanged, DocumentClosed, DocumentOpened, NewDocument, OpenFile, SaveDocument,
};
use lunco_twin::{DocumentKindId, DocumentKindMeta, DocumentKindRegistry};
// The empty-viewport placeholder is a workbench (egui shell) concept; the
// document/file command surface below is headless-safe. Gate only this.
#[cfg(feature = "ui")]
use lunco_workbench::ViewportPlaceholder;
use lunco_workspace::{TwinAdded, WorkspaceResource};
use lunco_usd_bevy::UsdPrimPath;

use crate::document::UsdOp;
use lunco_usd_sim::cosim::{ClearScene, LoadScene};
use crate::registry::UsdDocumentRegistry;

/// Stable id for the USD document kind in
/// [`DocumentKindRegistry`].
pub const USD_DOCUMENT_KIND: &str = "usd";

/// Plugin that registers the USD document kind, the typed-command
/// observers, and the pending-event drain system.
///
/// **Layer 2 (domain).** No UI, no Bevy renderer touches — added by
/// [`UsdPlugins`](crate::UsdPlugins) so any binary that pulls in USD
/// gets the document surface, even headless / sandbox bins.
pub struct UsdCommandsPlugin;

impl Plugin for UsdCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdDocumentRegistry>();

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
        // A3 auto-bridge: when the journal appears, hand it to the registry
        // once (reactive — `resource_added`, not per-frame). Headless builds
        // without a journal never run it.
        app.add_systems(
            Update,
            wire_usd_journal_handle
                .run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );
        // Workbench-only: the empty-viewport placeholder lives in the egui
        // shell; headless / sandbox / server bins don't add it.
        #[cfg(feature = "ui")]
        app.add_systems(Update, update_viewport_placeholder);
        app.add_observer(open_usd_docs_on_twin_added);
        // C5-A: persist/reload the runtime overlay (C4b spawns + moves) to
        // `<twin>/.lunco/runtime/<scene>.usda`, parallel to the journal.
        app.add_observer(crate::runtime_persistence::on_doc_opened_load_runtime);
        app.add_observer(crate::runtime_persistence::on_doc_changed_save_runtime);
        // E1b: make the default twin scene doc-backed by serving its composed
        // source as a `twin://` byte-overlay (web-ready via the async loader).
        app.init_resource::<crate::twin_projection::PendingTwinDocs>();
        app.init_resource::<crate::twin_projection::DocBackedTwinScenes>();
        // Referenced spawns whose asset closure is still loading (fetched once,
        // then authored onto the live stage — no whole-scene reload).
        app.init_resource::<crate::twin_projection::PendingRefSpawns>();
        // Gated on the asset pipeline: these need `AssetServer` (to fetch a
        // referenced asset's closure) and the `Assets<UsdSourceText>` store
        // (UsdBevyPlugin's `init_asset`). Both are absent in headless
        // `MinimalPlugins` test apps — and a partial setup can have one without
        // the other — so require both. Chained before `project_stage_changes`
        // (below) so a spawn authored this frame projects the same frame.
        app.add_systems(
            Update,
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

/// On `TwinAdded`, make the viewport **reflect the opened Twin/folder**
/// — clear-and-replace, so a previously loaded scene never lingers:
///
/// - **Has `[usd] default_scene`** → [`LoadScene`] it (path relative to
///   the Twin root). `LoadScene` clears the old scene, then mounts this
///   one as the single active stage; cosim wires `lunco:modelicaModel`
///   / `lunco:simWires` participants from its prim attributes through
///   [`UsdSimPlugin`](lunco_usd_sim::UsdSimPlugin).
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
fn open_usd_docs_on_twin_added(
    trigger: On<TwinAdded>,
    workspace: Res<WorkspaceResource>,
    twin_roots: Res<lunco_assets::twin_source::TwinRoots>,
    // Optional: headless/test apps (MinimalPlugins) have no `AssetServer` /
    // `Assets<UsdSourceText>`. The doc-backed mount path (`drain_pending_twin_docs`)
    // is gated on BOTH, so defer to it only when both exist — otherwise E1b is
    // skipped and `LoadScene` mounts the scene directly.
    asset_server: Option<Res<AssetServer>>,
    usd_sources: Option<Res<Assets<lunco_usd_bevy::UsdSourceText>>>,
    mut pending_twin: ResMut<crate::twin_projection::PendingTwinDocs>,
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
    // Key the `twin://` source by the Twin's name (its `twin.toml` `name`),
    // falling back to the root folder name. This yields a stable, per-Twin,
    // machine-independent asset identity: `twin://<name>/<scene>`.
    let twin_name = twin
        .manifest
        .as_ref()
        .map(|m| m.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| twin.root.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "twin".to_string());
    // Register the OPENED FOLDER as this Twin's resolve root — unconditionally,
    // before any scene decision. This is what routes `twin://<name>/…` AND what
    // the spawn-catalog scan (`maintain_catalogs`) walks to find the Twin's
    // `structures/…` parts. Doing it only in the `Some(default_scene)` branch
    // meant a folder opened without a declared starting scene never registered,
    // so its parts never reached the Spawn palette even though the folder was
    // open. Keyed by the folder we actually opened (`twin.root`).
    twin_roots.register(&twin_name, &twin.root);
    match default_scene {
        Some(scene) => {
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
            // Headless/test apps without the asset pipeline mount directly —
            // they have no doc projection to wait for.
            if let (Some(asset_server), Some(_)) = (&asset_server, &usd_sources) {
                info!(
                    "[twin] doc-backing starting scene `twin://{}/{}` (twin `{}`) — mount follows",
                    twin_name,
                    scene,
                    twin.root.display()
                );
                let handle = asset_server
                    .load::<lunco_usd_bevy::UsdSourceText>(format!("twin://{}/{}", twin_name, scene));
                pending_twin.push(handle, twin_name.clone(), scene.to_string(), twin.root.join(scene));
            } else {
                info!(
                    "[twin] loading starting scene `twin://{}/{}` (twin `{}`)",
                    twin_name,
                    scene,
                    twin.root.display()
                );
                commands.trigger(LoadScene {
                    path: format!("twin://{}/{}", twin_name, scene),
                    root_prim: String::new(),
                });
            }
        }
        None => {
            info!(
                "[twin] `{}` declares no starting scene — clearing viewport",
                twin.root.display()
            );
            commands.trigger(ClearScene {});
        }
    }
}

/// Keep the workbench's [`ViewportPlaceholder`] in sync with whether a
/// USD scene is loaded. With **no** `UsdPrimPath` entities — an empty
/// viewport, e.g. right after [`ClearScene`] from opening a scene-less
/// folder — show an empty-state hint; otherwise clear it so the message
/// vanishes the instant a scene mounts. No-op in headless binaries that
/// don't add the workbench (the resource is absent).
#[cfg(feature = "ui")]
fn update_viewport_placeholder(
    scene: Query<(), With<UsdPrimPath>>,
    placeholder: Option<ResMut<ViewportPlaceholder>>,
) {
    let Some(mut placeholder) = placeholder else {
        return;
    };
    let want = scene
        .is_empty()
        .then(|| "Nothing to show — open a scene or a Twin.".to_string());
    if placeholder.message != want {
        placeholder.message = want;
    }
}

register_commands!(
    on_apply_usd_op,
    on_undo_edit,
    on_redo_edit,
    on_attach_component,
    on_new_document,
    on_open_file,
    on_open_file_for_usd,
    on_save_document,
);

// ─────────────────────────────────────────────────────────────────────
// OpenFile — gated on USD extensions
// ─────────────────────────────────────────────────────────────────────

// `OpenFile` for a USD path drives two independent halves, each its own
// observer so headless bins get both without the UI:
//
//   1. `on_open_file_for_usd` — document **registration**: async read via
//      `lunco-storage`, idempotent allocate into `UsdDocumentRegistry`.
//   2. `on_open_file` (this one) — additive **scene import** (Blender's
//      File → Append): brings the stage into the running 3D scene so
//      `UsdSimPlugin` can wire `lunco:modelicaModel` / `lunco:simWires`
//      participants (the path `open_usd_docs_on_twin_added` relies on).
//
// `spawn_scene_root_world` loads the stage through the `AssetServer` (by
// path, no fs), so this half carries no I/O of its own.
#[on_command(OpenFile)]
fn on_open_file(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    if !is_usd_path(&path) {
        return;
    }
    // `mem://` / `bundled://` scenes never become a registered file document
    // (`on_open_file_for_usd` skips them), so keep the legacy file-backed
    // additive import for those — the helper no-ops on a repeated
    // `(asset, root_prim)` and warns + skips files outside the asset root.
    if path.is_empty() || path.starts_with("mem://") || path.starts_with("bundled://") {
        commands.queue(move |world: &mut World| {
            lunco_usd_sim::cosim::spawn_scene_root_world(world, &path, "");
        });
        return;
    }
    // Real file paths DO get a document (allocated asynchronously by
    // `on_open_file_for_usd` → `drain_pending_usd_file_loads`) for editing.
    // Mount the scene into the live world through the storage-based async loader
    // (`spawn_scene_root_world` → `UsdLoader` → `StageRecipe` → `CanonicalStage`)
    // — the same web-ready path `mem://` / `bundled://` take, so every scene
    // reads the live composed stage. Doc-overlay projection of runtime edits to
    // an opened file (the deleted `live_projection`'s job) is folded into the
    // `twin://` overlay path.
    //
    // Only mount when the asset pipeline is present: a headless doc-only context
    // (API / MCP open, or the open-file unit test) has no `AssetServer`, and the
    // document still opens through the async read path above.
    commands.queue(move |world: &mut World| {
        if world.contains_resource::<bevy::asset::AssetServer>() {
            lunco_usd_sim::cosim::spawn_scene_root_world(world, &path, "");
        }
    });
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
        if path.is_empty() || path.starts_with("mem://") || path.starts_with("bundled://") {
            return;
        }
        let stripped = path.strip_prefix("file://").unwrap_or(&path);
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
        .push(PendingUsdLoad { path: abs_path, task });
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
                // Idempotent re-open: if this exact path already lives in
                // the registry, don't re-allocate.
                let existing = {
                    let reg = world.resource::<UsdDocumentRegistry>();
                    reg.ids().find(|id| {
                        reg.host(*id)
                            .map(|h| match h.document().origin() {
                                DocumentOrigin::File { path, .. } => path == &load.path,
                                _ => false,
                            })
                            .unwrap_or(false)
                    })
                };
                if existing.is_none() {
                    world
                        .resource_mut::<UsdDocumentRegistry>()
                        .allocate(source, DocumentOrigin::writable_file(load.path.clone()));
                }
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
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        let next = registry.ids().count() + 1;
        let doc_id = registry.allocate(
            DEFAULT_USDA_SCAFFOLD.to_string(),
            DocumentOrigin::untitled(format!("UntitledStage-{}.usda", next)),
        );
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
        let registry = world.resource::<UsdDocumentRegistry>();
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
                bevy::log::warn!(
                    "[SaveUsd] {} is Untitled — Save-As required",
                    doc_id
                );
                return;
            }
            DocumentOrigin::Bundled { .. } => {
                bevy::log::warn!(
                    "[SaveUsd] {} is a bundled example — read-only",
                    doc_id
                );
                return;
            }
        };
        let source = doc.source().to_string();
        // Route through the storage abstraction instead of a direct
        // `std::fs::write` (clippy-banned in domain crates, wasm-broken).
        // `write_sync` blocks on `FileStorage`'s write future, which wraps
        // synchronous fs and is already `Ready` — no reactor, no hang.
        let storage = lunco_storage::FileStorage::new();
        let handle = lunco_storage::StorageHandle::File(path.clone());
        if let Err(e) = storage.write_sync(&handle, source.as_bytes()) {
            bevy::log::error!("[SaveUsd] {} write to {} failed: {:?}", doc_id, path.display(), e);
            return;
        }
        // Borrow mut to mark saved. `host_mut` doesn't bump the
        // change ring because saving doesn't change the document — it
        // only resets the dirty marker.
        if let Some(host) = world
            .resource_mut::<UsdDocumentRegistry>()
            .host_mut(doc_id)
        {
            host.document_mut().mark_saved();
        }
        bevy::log::info!("[SaveUsd] {} saved to {}", doc_id, path.display());
    });
}

// ─────────────────────────────────────────────────────────────────────
// ApplyUsdOp — typed entry for programmatic / UI-driven edits
// ─────────────────────────────────────────────────────────────────────

/// Apply a [`UsdOp`] to the named document via the typed-command bus.
///
/// Same shape as `lunco-modelica`'s op-dispatch commands: UI clicks,
/// HTTP API calls, and scripts all dispatch this; the observer
/// routes it through [`UsdDocumentRegistry::apply`] so undo/redo,
/// change notification, and read-only enforcement stay in one place.
#[Command(default)]
pub struct ApplyUsdOp {
    /// Target document.
    pub doc: DocumentId,
    /// Operation to apply.
    pub op: UsdOp,
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
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        match registry.apply(doc, op) {
            Ok(ack) => {
                bevy::log::debug!("[ApplyUsdOp] {} → gen {}", doc, ack.new_gen.unwrap_or(0));
            }
            Err(reject) => {
                bevy::log::warn!("[ApplyUsdOp] {} rejected: {:?}", doc, reject);
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// UndoEdit / RedoEdit — the ONE undo
// ─────────────────────────────────────────────────────────────────────

/// Undo the document's last op.
///
/// This is the **only** undo. Every authored edit — spawn, move, delete, terrain
/// stroke, waypoint, property — reaches the world as a [`UsdOp`] through
/// [`ApplyUsdOp`], and `UsdDocument::apply` hands back a typed inverse for each. So
/// undo is a document concern, not an editor one: pop the inverse, apply it, and the
/// projection re-derives the ECS ([`crate::live_consume`]). It journals (undo/redo
/// record through the same `OpRecorder` seam) and replicates like any other op.
///
/// An editor-side "remember the old Transform and write it back" stack cannot do
/// this: it does not know about the document, so an undone spawn stays in the layer
/// and the journal, and the two disagree. There used to be one; it is gone.
#[Command(default)]
pub struct UndoEdit {
    /// Document to undo in.
    pub doc: DocumentId,
}

#[on_command(UndoEdit)]
fn on_undo_edit(trigger: On<UndoEdit>, mut commands: Commands) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        let Some(host) = registry.host_mut(doc) else { return };
        match host.undo() {
            Ok(true) => {
                // The op changed the doc; the twin projection re-derives the scene.
                registry.mark_changed(doc);
                bevy::log::info!("[undo] {doc}");
            }
            Ok(false) => bevy::log::info!("[undo] nothing to undo"),
            Err(err) => bevy::log::warn!("[undo] failed: {err:?}"),
        }
    });
}

/// Redo the last undone op — the mirror of [`UndoEdit`].
#[Command(default)]
pub struct RedoEdit {
    /// Document to redo in.
    pub doc: DocumentId,
}

#[on_command(RedoEdit)]
fn on_redo_edit(trigger: On<RedoEdit>, mut commands: Commands) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        let Some(host) = registry.host_mut(doc) else { return };
        match host.redo() {
            Ok(true) => {
                registry.mark_changed(doc);
                bevy::log::info!("[redo] {doc}");
            }
            Ok(false) => bevy::log::info!("[redo] nothing to redo"),
            Err(err) => bevy::log::warn!("[redo] failed: {err:?}"),
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// AttachComponent — build-from-parts (doc 48 §3.1)
// ─────────────────────────────────────────────────────────────────────

/// Attach a component asset to a host body as a jointed child, deriving the
/// joint anchor from the placement so it is authored once, not twice. Lowers to
/// the primitive [`UsdOp`]s in [`crate::attach::attach_component_ops`] — each
/// journals and inverts on its own, so undo peels the attach off op-by-op.
///
/// If any op is rejected (e.g. the host prim doesn't exist), the rest are still
/// attempted and each logs its own rejection — the partial result is visible and
/// undoable rather than silently half-applied behind a rollback the journal can't
/// see. Validate the host exists before dispatching.
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
        let ops = crate::attach::attach_component_ops(&spec);
        let n = ops.len();
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        let mut applied = 0usize;
        for op in ops {
            match registry.apply(doc, op) {
                Ok(_) => applied += 1,
                Err(reject) => {
                    bevy::log::warn!(
                        "[AttachComponent] {doc} op rejected ({}/{n} applied): {reject:?}",
                        applied
                    );
                }
            }
        }
        bevy::log::info!(
            "[AttachComponent] {doc}: attached `{}` to `{}` ({applied}/{n} ops)",
            spec.name,
            spec.host_path
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
    mut registry: ResMut<UsdDocumentRegistry>,
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
    mut registry: ResMut<UsdDocumentRegistry>,
    mut commands: Commands,
) {
    let pending = registry.drain_pending();
    if pending.opened.is_empty()
        && pending.changed.is_empty()
        && pending.closed.is_empty()
    {
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

        assert!(app.world().contains_resource::<UsdDocumentRegistry>());
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
        // then poll the drain system across a few ticks until the read
        // completes and the document is allocated.
        for _ in 0..5 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        assert_eq!(reg.ids().count(), 1, "exactly one USD doc opened (no duplicate)");

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

        let reg = app.world().resource::<UsdDocumentRegistry>();
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
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\n".to_string(),
                DocumentOrigin::untitled("UntitledRover.usda".to_string()),
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
        let reg = app.world().resource::<UsdDocumentRegistry>();
        let host = reg.host(doc_id).expect("doc still alive");
        // Assert on the canonical data (the document is data-canonical now;
        // exact serialized-text formatting is openusd's business, not ours).
        let data = host.document().data();
        // TODO(usd-read-migration): switch these reads to the generic UsdRead surface
        // (`type_name`/`scalar`) instead of the legacy `UsdDataExt`, matching production (doc 21).
        assert_eq!(data.prim_type_name(&SdfPath::new("/Rover").unwrap()).as_deref(), Some("Xform"));
        assert_eq!(data.prim_type_name(&SdfPath::new("/Rover/Body").unwrap()).as_deref(), Some("Cube"));
        assert_eq!(data.prim_type_name(&SdfPath::new("/Rover/WheelFL").unwrap()).as_deref(), Some("Cube"));
        assert_eq!(
            data.prim_attribute_value::<[f64; 3]>(&SdfPath::new("/Rover/WheelFL").unwrap(), "xformOp:translate"),
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
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\n".to_string(),
                DocumentOrigin::untitled("UntitledJournal.usda".to_string()),
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
                    EntryKind::Op { domain, op, inverse } => {
                        Some((domain.clone(), op.clone(), inverse.clone()))
                    }
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
        std::fs::write(tmp.join("controller.mo"), "model Controller end Controller;\n").unwrap();

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
        assert_eq!(cmds.clears, 0, "LoadScene clears internally — no extra ClearScene");
    }

    #[test]
    fn twin_added_without_default_scene_clears_viewport() {
        // No `default_scene` (also covers a folder with no `.usda`):
        // clear to an empty viewport, load nothing.
        let cmds = scene_cmds_for_twin(
            "name = \"t\"\nversion = \"0.1.0\"\n",
            "lunco_usd_twin_no_scene_test",
        );
        assert!(cmds.loads.is_empty(), "no scene loaded, got {:?}", cmds.loads);
        assert_eq!(cmds.clears, 1, "viewport cleared to empty");
    }

    #[test]
    fn open_folder_with_no_usda_shows_nothing() {
        // Folder with no `.usda` and no `twin.toml`: clear to empty,
        // load nothing — the viewport must show nothing.
        let cmds = scene_cmds_for_empty_folder("lunco_usd_empty_folder_test");
        assert!(cmds.loads.is_empty(), "nothing to load, got {:?}", cmds.loads);
        assert_eq!(cmds.clears, 1, "empty folder clears the viewport");
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

        let reg = app.world().resource::<UsdDocumentRegistry>();
        assert_eq!(reg.ids().count(), 1);
        let id = reg.ids().next().unwrap();
        assert!(reg.host(id).unwrap().document().origin().is_untitled());
    }
}
