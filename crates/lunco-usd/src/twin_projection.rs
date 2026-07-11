//! Doc-backed twin default scene — web-ready via the twin asset source.
//!
//! This is the **doc-backed live-projection path**: the default twin scene loads
//! through the `twin://` asset source and the async [`UsdLoader`], which
//! re-attaches the scheme so co-located refs (terrain `.glb`) resolve on every
//! platform the source supports. It is made doc-backed by serving the scene
//! document's **composed** (`base ⊕ runtime`) source as a *byte-overlay* on the
//! twin source, so the live world composes from the editable document — and
//! reloaded runtime spawns/moves appear live. (The former native-only,
//! filesystem-composing `live_projection` path for `OpenFile` scenes has been
//! removed; opened files mount through the same storage-based async loader.)
//!
//! Flow (doc-first: the document exists and its composed source is the overlay
//! BEFORE the scene mounts, so the world is projected exactly once):
//! 1. On `TwinAdded` with a `[usd] default_scene`, kick an async
//!    [`UsdSourceText`] load of `twin://<name>/<scene>` (raw base layer, read
//!    through the twin source — web-ready) and record it in [`PendingTwinDocs`].
//!    No mount yet — mounting first built the stage from the raw base, and the
//!    open-time `restore_runtime` then forced a whole-scene rebuild (every prim
//!    spawned twice ~70 ms apart).
//! 2. [`drain_pending_twin_docs`] — once the source text is in hand, allocate a
//!    [`UsdDocument`](crate::document) for it (origin = the on-disk path, so Save
//!    and dedup work), restore its persisted `.lunco/runtime` overlay, publish
//!    the composed source as the twin overlay, record it in
//!    [`DocBackedTwinScenes`] (synced at the current generation), and only then
//!    fire `LoadScene` — the single mount composes `base ⊕ runtime`.
//! 3. [`sync_twin_overlays`] — whenever the document generation moves (initial
//!    mount, open-time `restore_runtime`, or a later spawn/move), refresh the
//!    twin **overlay** (for persistence / re-open) and **author the delta onto
//!    the live composed stage**: translates and structural spawns/removes are
//!    authored onto the scene's [`CanonicalStage`](lunco_usd_bevy::CanonicalStage)
//!    directly, firing its openusd change sink so `project_stage_changes`
//!    projects the edit in place — no whole-scene asset reload. A referenced
//!    spawn whose asset isn't loaded yet is fetched once through
//!    [`drain_ref_spawns`], then authored the same way.
//!
//! Scope: the **default twin scene** only. Arbitrary `OpenFile` scenes stay on
//! E1's path; `mem://` / `bundled://` keep the file-backed import.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;
use lunco_assets::twin_source::TwinRoots;
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::{UsdPrimPath, UsdSourceText, UsdStageAsset, UsdVisualSynced};
use lunco_usd_sim::cosim::LoadScene;

use crate::document::UsdOp;
use crate::registry::UsdDocumentRegistry;

/// Marks a live prim entity that refreshes its own content **in place**, so the
/// twin projection must NOT structurally despawn/reload it on an attribute-only
/// document change. The DEM terrain sets this: its heavy base grid is retained
/// and re-stamped from the registry document on edits (the sandbox's
/// `refresh_docbacked_terrain_from_doc`), so a whole-scene reload would force a
/// full GeoTIFF re-read per edit. Consumed by [`sync_twin_overlays`], which
/// suppresses the reload when a generation's only structural trigger is
/// attribute edits confined to such a subtree.
#[derive(Component)]
pub struct LiveRebuildExempt;

/// A default-twin-scene document waiting for its base source text to finish
/// loading through the twin source.
struct PendingTwinDoc {
    /// In-flight raw-source load of `twin://<name>/<rel>`.
    handle: Handle<UsdSourceText>,
    /// Twin name (the `twin://` first segment).
    name: String,
    /// Scene path relative to the twin root (the `twin://` remainder).
    rel: String,
    /// On-disk absolute path — the document origin (Save target + dedup key).
    abs_path: PathBuf,
    attempts: u32,
}

/// Default twin scenes whose base source is still loading. Drained by
/// [`drain_pending_twin_docs`].
#[derive(Resource, Default)]
pub struct PendingTwinDocs {
    items: Vec<PendingTwinDoc>,
}

impl PendingTwinDocs {
    /// Queue a default twin scene for doc-backed projection.
    pub fn push(&mut self, handle: Handle<UsdSourceText>, name: String, rel: String, abs_path: PathBuf) {
        self.items.push(PendingTwinDoc { handle, name, rel, abs_path, attempts: 0 });
    }
}

/// The twin-source coordinates + last-synced generation for a doc-backed twin
/// scene, so [`sync_twin_overlays`] re-serializes only when the document moved.
struct TwinSceneRef {
    name: String,
    rel: String,
    synced_generation: Option<u64>,
    /// Generation the **persistence overlay** was last serialized at. Tracked apart
    /// from `synced_generation` so the (expensive, whole-stage) overlay serialize can
    /// be DEBOUNCED — done once the document settles after an edit burst, not on every
    /// brush stroke — while the live projection still applies each op immediately.
    overlay_synced_generation: Option<u64>,
}

/// Map of document → the twin scene it backs. Populated by
/// [`drain_pending_twin_docs`], consumed by [`sync_twin_overlays`].
#[derive(Resource, Default)]
pub struct DocBackedTwinScenes {
    map: HashMap<DocumentId, TwinSceneRef>,
}

impl DocBackedTwinScenes {
    /// The registry document backing the twin scene at `twin://<name>/<rel>`, if
    /// any. Lets a twin-projected consumer (e.g. a DEM terrain, which carries no
    /// `LiveDocScene`) recover its authoring document from its `twin://` stage
    /// asset path.
    pub fn doc_for(&self, name: &str, rel: &str) -> Option<DocumentId> {
        self.map
            .iter()
            .find(|(_, s)| s.name == name && s.rel == rel)
            .map(|(doc, _)| *doc)
    }

    /// The `twin://` coordinates (`name`, `rel`) a document is already backed
    /// under, if any — so a second consumer (e.g. the editor viewport) reuses
    /// the same overlay + asset instead of registering a duplicate.
    pub fn coords_of(&self, doc: DocumentId) -> Option<(String, String)> {
        self.map.get(&doc).map(|s| (s.name.clone(), s.rel.clone()))
    }

    /// Track an already-allocated document as doc-backed under `(name, rel)`, so
    /// [`sync_twin_overlays`] keeps its overlay + live entities in step with the
    /// document generation. Idempotent — a document already tracked (e.g. a
    /// default twin scene) keeps its existing coordinates.
    pub fn track(&mut self, doc: DocumentId, name: String, rel: String) {
        self.map.entry(doc).or_insert(TwinSceneRef {
            name,
            rel,
            synced_generation: None,
            overlay_synced_generation: None,
        });
    }
}

/// The editable document backing a running scene's stage asset, if the scene is
/// **doc-backed** (loaded as `twin://<name>/<rel>`). Returns `None` for a raw-file
/// scene — which has no savable source document, so a caller (e.g. saving a
/// live-edited scenario back onto its prim) must refuse rather than silently drop
/// the edit. This is the asset↔document bridge that unblocks scenario save-back:
/// a runtime entity carries a `UsdPrimPath { stage_handle, path }`, and this maps
/// that stage handle to the `UsdDocumentRegistry` document you can `ApplyUsdOp` on.
pub fn scene_document_for(
    backed: &DocBackedTwinScenes,
    asset_server: &AssetServer,
    scene: AssetId<UsdStageAsset>,
) -> Option<DocumentId> {
    // `AssetPath::path()` is the path WITHOUT the `twin://` source scheme, i.e.
    // `<name>/<rel>`. `rel` may contain slashes (`scenes/sandbox/scene.usda`), so
    // split only on the FIRST one. (Same idiom as `cache_terrain_document`.)
    let asset_path = asset_server.get_path(scene)?;
    let rel_path = asset_path.path().to_string_lossy();
    let (name, rel) = rel_path.split_once('/')?;
    backed.doc_for(name, rel)
}

/// A **referenced spawn** whose asset closure is being fetched before it can be
/// authored onto the live scene stage. When a structural edit adds a prim that
/// references an asset whose layer bytes aren't loaded into the scene's live
/// resolver yet (a first-of-its-kind rover spawn), [`sync_twin_overlays`] loads
/// that asset as a `UsdStageAsset` (whose loader fetches the full closure,
/// web-ready) and queues this. [`drain_ref_spawns`] injects the fetched bytes
/// into the scene stage's resolver and authors the prim + `references` arc, so
/// the openusd change sink fires and `project_stage_changes` instantiates the
/// composed subtree — no whole-scene reload.
struct RefSpawn {
    /// The scene whose live [`CanonicalStage`](lunco_usd_bevy::CanonicalStage)
    /// the spawn is authored onto.
    scene_id: AssetId<UsdStageAsset>,
    /// The prim path to spawn (e.g. `/World/rover_1`).
    prim_path: String,
    /// The prim's composed `typeName`, authored before the reference.
    type_name: Option<String>,
    /// The reference asset path exactly as authored in the document — PCP
    /// re-derives its canonical id against the scene layer, matching the id the
    /// closure bytes are injected under.
    asset_path: String,
    /// In-flight load of the referenced asset (its loader fetches the closure).
    ref_handle: Handle<UsdStageAsset>,
}

/// Referenced spawns waiting on their asset closure to finish loading.
/// Populated by [`sync_twin_overlays`], drained by [`drain_ref_spawns`].
#[derive(Resource, Default)]
pub struct PendingRefSpawns {
    items: Vec<RefSpawn>,
}

/// Give up on a pending twin doc after this many frames without its source
/// loading (a missing / unreadable scene), so it doesn't retry forever.
const MAX_TWIN_DOC_ATTEMPTS: u32 = 600;

/// The registered USD document whose origin is the on-disk file `abs`, if any.
fn find_doc_for_abs(registry: &UsdDocumentRegistry, abs: &std::path::Path) -> Option<DocumentId> {
    registry.ids().find(|id| {
        registry
            .host(*id)
            .map(|h| match h.document().origin() {
                DocumentOrigin::File { path, .. } => path == abs,
                _ => false,
            })
            .unwrap_or(false)
    })
}

/// Allocate the document for each pending twin scene once its base source text
/// has loaded through the twin source, restore its persisted runtime overlay,
/// publish the composed (`base ⊕ runtime`) source as the twin overlay — and only
/// THEN mount the scene ([`LoadScene`]). Ordering is the whole point: the async
/// stage load reads the overlay bytes, so the one and only projection already
/// composes the runtime state. Mounting eagerly on `TwinAdded` and doc-backing
/// afterwards built the stage from the raw base, then the open-time
/// `restore_runtime` (a coarse `ReplaceSource` marker) forced a whole-scene
/// rebuild ~70 ms in — every prim spawned, despawned, and respawned once.
/// Idempotent: reuses an existing document for the same on-disk path (twin
/// re-add) rather than double-allocating; if that scene is already mounted,
/// `LoadScene` no-ops (same path + root) or force-reloads a resident stale
/// stage, so the composed overlay still wins.
pub(crate) fn drain_pending_twin_docs(
    mut pending: ResMut<PendingTwinDocs>,
    mut registry: ResMut<UsdDocumentRegistry>,
    mut backed: ResMut<DocBackedTwinScenes>,
    sources: Res<Assets<UsdSourceText>>,
    twin_roots: Res<TwinRoots>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    if pending.items.is_empty() {
        return;
    }
    let taken = std::mem::take(&mut pending.items);
    let mut still = Vec::new();
    for mut item in taken {
        let twin_path = format!("twin://{}/{}", item.name, item.rel);
        let Some(UsdSourceText(source)) = sources.get(&item.handle) else {
            item.attempts += 1;
            if item.attempts < MAX_TWIN_DOC_ATTEMPTS {
                still.push(item);
            } else {
                // Doc-backing failed (missing / unreadable scene) — mount
                // file-backed anyway so the viewport isn't left empty.
                warn!(
                    "[usd-e1b] base source for `{twin_path}` never loaded — mounting without doc projection"
                );
                commands.trigger(LoadScene { path: twin_path, root_prim: String::new() });
            }
            continue;
        };
        let doc = find_doc_for_abs(&registry, &item.abs_path).unwrap_or_else(|| {
            registry.allocate(source.clone(), DocumentOrigin::writable_file(item.abs_path.clone()))
        });
        // Restore the persisted `.lunco/runtime` overlay NOW, before the mount
        // below. The `DocumentOpened` observer restore fires a flush later —
        // after the stage load has already read its bytes. Guarded: whichever
        // runs second is a no-op.
        if let Some(ws) = workspace.as_deref() {
            crate::runtime_persistence::restore_doc_runtime(ws, &mut registry, doc);
        }
        // Publish the composed source as the twin overlay so the stage build
        // reads `base ⊕ runtime`, and mark the scene synced at this generation —
        // every op through it is reflected by the mount itself, so
        // `sync_twin_overlays` only has to project edits made AFTER open.
        let (cur_gen, composed) = match registry.host(doc) {
            Some(h) => (h.document().generation(), h.document().composed_source()),
            None => continue,
        };
        twin_roots.set_overlay(&item.name, &item.rel, Arc::new(composed.into_bytes()));
        backed.map.entry(doc).or_insert(TwinSceneRef {
            name: item.name.clone(),
            rel: item.rel.clone(),
            synced_generation: Some(cur_gen),
            overlay_synced_generation: Some(cur_gen),
        });
        info!("[usd-e1b] default scene `{twin_path}` is doc-backed ({doc}) — mounting composed");
        commands.trigger(LoadScene { path: twin_path, root_prim: String::new() });
    }
    pending.items.extend(still);
}

/// Keep each doc-backed twin scene's twin-source overlay in step with its
/// document: when the generation moves, serialize the composed (`base ⊕ runtime`)
/// source into the overlay and `reload` the scene asset so the live world
/// re-composes from the document (web-ready — the async loader anchors at the
/// `twin://` identity). Drops entries whose document has closed.
/// Serialize a doc-backed scene's composed source into its twin overlay (the
/// persistence / next-load source) and mark it overlay-synced at `gen`. O(stage) — a
/// whole-stage recompose + serialize — so call it only once the document has SETTLED
/// (see the debounce in [`sync_twin_overlays`]), never on every edit.
fn write_twin_overlay(world: &mut World, doc: DocumentId, name: &str, rel: &str, gen: u64) {
    let composed_source =
        world.resource::<UsdDocumentRegistry>().host(doc).map(|h| h.document().composed_source());
    if let Some(src) = composed_source {
        world.resource::<TwinRoots>().set_overlay(name, rel, Arc::new(src.into_bytes()));
        if let Some(s) = world.resource_mut::<DocBackedTwinScenes>().map.get_mut(&doc) {
            s.overlay_synced_generation = Some(gen);
        }
    }
}

pub(crate) fn sync_twin_overlays(world: &mut World) {
    // Snapshot tracked scenes (owned) so no resource borrow is held across the
    // world mutations below.
    let entries: Vec<(DocumentId, String, String, Option<u64>, Option<u64>)> = world
        .resource::<DocBackedTwinScenes>()
        .map
        .iter()
        .map(|(doc, s)| {
            (*doc, s.name.clone(), s.rel.clone(), s.synced_generation, s.overlay_synced_generation)
        })
        .collect();

    for (doc, name, rel, synced, overlay_synced) in entries {
        // Cheap generation probe FIRST — then early-out. The expensive payloads
        // below (`composed_source()` re-serializes the whole composed stage to a
        // String; `composed()` recomposes it) must NOT be computed every frame:
        // the document is unchanged on the overwhelming majority of frames and we
        // `continue` right after this check, so computing them up-front was
        // ~212µs/frame of pure waste (profiled on the moonbase twin). Read only
        // the generation here; pay for the payloads only once it has moved.
        let cur_gen = match world.resource::<UsdDocumentRegistry>().host(doc) {
            Some(h) => h.document().generation(),
            None => {
                world.resource::<TwinRoots>().clear_overlay(&name, &rel);
                world.resource_mut::<DocBackedTwinScenes>().map.remove(&doc);
                continue;
            }
        };
        if Some(cur_gen) == synced {
            // Projection is already up to date. DEBOUNCED persistence overlay: once the
            // document has settled at this generation, serialize the overlay ONCE (not
            // per edit — a rapid brush burst advances the generation every frame, and
            // re-serializing a thousand-prim stage each time was the last per-edit
            // main-thread hitch). The live edits were already projected incrementally.
            if Some(cur_gen) != overlay_synced {
                write_twin_overlay(world, doc, &name, &rel, cur_gen);
            }
            continue;
        }

        // Author-once: the scene's live stage is keyed by the cached
        // `twin://name/rel` UsdStageAsset id (AssetServer dedups by path). We
        // replay the **typed ops** the document recorded since the last sync
        // directly onto that stage — the op is the single delta description, so we
        // never re-derive an edit's value by reading it back out of `composed`.
        let twin_path = format!("twin://{}/{}", name, rel);
        let scene_id = world
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(twin_path.clone())
            .id();

        // `None` = the op ring overflowed (more edits than capacity since the last
        // sync) → we can't trust an incremental replay, so rebuild.
        let ops = world
            .resource::<UsdDocumentRegistry>()
            .host(doc)
            .and_then(|h| h.document().ops_since(synced.unwrap_or(0)));
        let has_work = synced.is_none() || ops.as_ref().map(|o| !o.is_empty()).unwrap_or(true);

        // Every projection authors onto the live stage, so it must exist. On the
        // first generations the async `LoadScene` is still building it — DEFER
        // (leave `synced` unchanged) so we retry once it lands.
        let stage_ready = world
            .get_non_send_resource::<lunco_usd_bevy::CanonicalStages>()
            .map(|s| s.get(scene_id).is_some())
            .unwrap_or(false);
        if has_work && !stage_ready {
            continue;
        }

        if synced.is_none() {
            // First mount MUST publish the overlay so the async (re)load composes
            // base ⊕ runtime from it (this is NOT deferrable, unlike a live edit).
            // Already done at this generation for a twin default scene
            // (`drain_pending_twin_docs` publishes before mounting); still needed
            // here for editor-viewport docs tracked via `track()`.
            if overlay_synced != Some(cur_gen) {
                write_twin_overlay(world, doc, &name, &rel, cur_gen);
            }
            // The async load already built the stage from the overlay, so every
            // recorded op is already reflected — just reconcile restored runtime
            // spawns idempotently (never replay ops → double-author). This is the
            // only consumer of the whole-stage recompose.
            let composed = {
                let reg = world.resource::<UsdDocumentRegistry>();
                reg.host(doc)
                    .expect("doc host present: generation read above, exclusive system")
                    .document()
                    .composed_arc()
            };
            reconcile_full_to_composed(world, scene_id, &composed);
        } else {
            match ops {
                // Overflow, or a coarse op (ReplaceSource / MovePrim / keyframe /
                // relationship — no incremental stage-author yet, and whole-source
                // undo may change surviving prims' attribute values): rebuild the
                // stage from composed_source + the already-loaded closure. (The
                // overlay is refreshed on the next settled frame.)
                None => {
                    let cs = world
                        .resource::<UsdDocumentRegistry>()
                        .host(doc)
                        .map(|h| h.document().composed_source())
                        .unwrap_or_default();
                    rebuild_scene_from_composed(world, scene_id, &cs);
                }
                Some(ops) if ops.iter().any(op_needs_rebuild) => {
                    let cs = world
                        .resource::<UsdDocumentRegistry>()
                        .host(doc)
                        .map(|h| h.document().composed_source())
                        .unwrap_or_default();
                    rebuild_scene_from_composed(world, scene_id, &cs);
                }
                // Incremental: replay each op's typed delta onto the live stage. NO
                // whole-stage serialize here — the overlay catches up when settled.
                Some(ops) => {
                    for op in &ops {
                        apply_incremental_op_to_stage(world, scene_id, op);
                    }
                }
            }
        }

        if let Some(s) = world.resource_mut::<DocBackedTwinScenes>().map.get_mut(&doc) {
            s.synced_generation = Some(cur_gen);
        }
    }
}

/// The first reference asset path authored on a prim spec (the runtime-spawn
/// arc), if any — reads the `references` list op the document authored via
/// [`author::author_reference`](lunco_usd_bevy::author::author_reference).
fn first_reference(spec: &openusd::sdf::SpecData) -> Option<String> {
    match spec.get("references") {
        Some(openusd::sdf::Value::ReferenceListOp(op)) => {
            op.iter().find(|r| !r.asset_path.is_empty()).map(|r| r.asset_path.clone())
        }
        _ => None,
    }
}

/// Whether an op has no incremental live-stage author yet, so the projector must
/// rebuild the scene from the composed source rather than replay it: a
/// whole-source replace, a namespace move (re-keys entities by path; whole-source
/// undo may also change surviving prims' attribute values), a keyframe *removal*
/// (openusd exposes no live-stage sample removal — unlike `SetTimeSample`, which
/// authors incrementally), or a composition-arc edit whose effect is non-local
/// (a variant selection or payload re-composes a whole subtree). The common
/// interactive ops — translate, attribute, spawn, remove, keyframe *authoring*,
/// and now relationship / connection edits — return `false` and replay
/// incrementally via [`apply_incremental_op_to_stage`].
///
/// `SetRelationship` and `SetConnection` used to rebuild here. That put a physics
/// joint — two `physics:body` relationships — on the whole-scene-respawn path, so
/// assembling a vehicle from parts rebuilt the world once per joint. They now have
/// live-stage authors (`CanonicalStage::author_relationship` / `author_connection`)
/// whose consumers (the Avian joint builder, the cosim wire reconcile) re-read on a
/// subtree refresh — so the incremental path fully reconciles them.
///
/// `SetApiSchemas` and `SetActive` do NOT: their effect is which ECS *components* a
/// prim carries (rigid body, collider) and whether its entity exists at all — and
/// the incremental subtree refresh only re-derives an entity's *visual*, not its
/// physics extraction or its presence. So they rebuild, which re-derives both
/// correctly. This is not the hot path: `AttachComponent` emits neither, so
/// building a vehicle from parts stays rebuild-free.
fn op_needs_rebuild(op: &UsdOp) -> bool {
    matches!(
        op,
        UsdOp::ReplaceSource { .. }
            | UsdOp::MovePrim { .. }
            | UsdOp::RemoveTimeSample { .. }
            // Composition-arc changes: value resolution re-composes the prim's
            // subtree wholesale, which the incremental sink can't express.
            | UsdOp::SetVariantSelection { .. }
            | UsdOp::SetPayload { .. }
            // ECS-structural changes the visual-only subtree refresh can't reconcile
            // (physics component set / entity presence). A rebuild re-derives both.
            | UsdOp::SetApiSchemas { .. }
            | UsdOp::SetActive { .. }
    )
}

/// Replay one **incremental** op's typed delta onto the scene's live
/// `CanonicalStage` — author-once: the value comes straight from the op, never
/// re-read from `composed`. Firing the openusd sink lets
/// [`project_stage_changes`](crate::live_consume::project_stage_changes) reconcile
/// ECS. Only incremental ops reach here; coarse ops ([`op_needs_rebuild`]) rebuild
/// instead. Reads/authors the `!Send` stage under short borrows.
fn apply_incremental_op_to_stage(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    op: &UsdOp,
) {
    use lunco_usd_bevy::CanonicalStages;
    match op {
        UsdOp::SetTranslate { path, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                if let Err(e) = cs.author_translate(&sp, *value) {
                    warn!("[twin] author translate {path}: {e}");
                }
            }
        }
        UsdOp::SetRotate { path, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                if let Err(e) = cs.author_rotate(&sp, *value) {
                    warn!("[twin] author rotate {path}: {e}");
                }
            }
        }
        UsdOp::SetAttribute { path, name, type_name, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            // Mirror the document op: a `string` value is RAW (`Value::String`, no
            // literal parse); every other type is a parsed literal.
            let is_string = type_name == "string";
            let v = if is_string {
                openusd::sdf::Value::String(value.clone())
            } else {
                match lunco_usd_bevy::author::parse_attribute_value(type_name, value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[twin] parse attribute {path}.{name} ({type_name}): {e}");
                        return;
                    }
                }
            };
            let authored = match world
                .get_non_send_resource::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs.author_attribute(&sp, name, type_name, v) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author attribute {path}.{name}: {e}");
                        false
                    }
                },
                None => false,
            };
            // A `string` attribute is non-visual metadata/behavior (`lunco:script`,
            // descriptions, `lunco:policy:source`) — no geometry/material
            // consequence, and a refresh would hot-reload a running scenario
            // (resetting its `this`) on a mere save. So author, don't refresh.
            if is_string {
                return;
            }
            // Refresh only what the edit can actually change: a material/shader
            // edit fans out through `material:binding` to meshes anywhere (whole
            // scene), but a geometry/xform attribute edit is local to its own prim
            // — so re-instantiate just that subtree and leave unrelated roots
            // (including live physics bodies) alone.
            if authored {
                let prim_ty = world
                    .get_non_send_resource::<CanonicalStages>()
                    .and_then(|s| s.get(scene_id))
                    .and_then(|cs| cs.view().prim_type_name(&sp));
                if attribute_edit_needs_full_refresh(prim_ty.as_deref()) {
                    // …unless the edit is confined to a `LiveRebuildExempt` subtree (a
                    // DEM terrain's crater/rock/edit layer prims are untyped `def`s →
                    // `prim_ty == None` → would otherwise take this full-refresh path).
                    // Reinstantiating the whole scene there re-bridges the terrain and
                    // re-spawns the avatar camera on EVERY edit — a self-sustaining
                    // recomposition loop (persistent 1 FPS + camera-order ambiguity, as
                    // the old cameras are never despawned). The terrain re-bakes itself
                    // in place off the registry document (`refresh_docbacked_terrain_from_doc`),
                    // so suppress the structural reload. This is the long-dead consumer
                    // the `LiveRebuildExempt` doc-comment describes.
                    if !edit_confined_to_exempt_subtree(world, scene_id, path) {
                        refresh_scene_visuals(world, scene_id);
                    }
                } else {
                    refresh_prim_subtree(world, scene_id, path);
                }
            }
        }
        UsdOp::AddPrim { parent_path, name, type_name, reference, .. } => {
            let prim_path = if parent_path == "/" || parent_path.is_empty() {
                format!("/{name}")
            } else {
                format!("{}/{name}", parent_path.trim_end_matches('/'))
            };
            spawn_prim_op(world, scene_id, &prim_path, type_name.clone(), reference.clone());
        }
        UsdOp::RemovePrim { path, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                if let Err(e) = cs.remove_prim_at(&sp) {
                    warn!("[twin] remove {path}: {e}");
                }
            }
        }
        UsdOp::SetTimeSample { path, name, type_name, time, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            let v = match lunco_usd_bevy::author::parse_attribute_value(type_name, value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[twin] parse keyframe {path}.{name} ({type_name}) @ {time}: {e}");
                    return;
                }
            };
            let authored = match world
                .get_non_send_resource::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs.author_time_sample(&sp, name, type_name, *time, v) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author keyframe {path}.{name} @ {time}: {e}");
                        false
                    }
                },
                None => false,
            };
            // The per-frame sampler (`sample_usd_animation`) reads the live stage,
            // so a key on an ALREADY-animated prim shows up next tick with no
            // refresh. But the FIRST key turns a static prim animated — its entity
            // isn't `UsdAnimated` yet, so re-instantiate the subtree to let the
            // extractor tag + plan it. Steady-state keyframing stays refresh-free.
            if authored && !prim_entity_is_animated(world, scene_id, path) {
                refresh_prim_subtree(world, scene_id, path);
            }
        }
        UsdOp::SetRelationship { path, name, targets, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            let authored = match world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                Some(cs) => match cs.author_relationship(&sp, name, targets) {
                    Ok(()) => true,
                    Err(e) => { warn!("[twin] author relationship {path}.{name}: {e}"); false }
                },
                None => false,
            };
            // A relationship is InfoOnly — it never spawns/despawns, so the sink
            // won't reconcile it. Whoever consumes the target (the Avian joint
            // builder reads `physics:body0/1`; a material binding fans out to
            // meshes) is re-run by re-instantiating the owning prim's subtree.
            if authored {
                refresh_relationship_dependents(world, scene_id, path, name);
            }
        }
        UsdOp::SetConnection { path, name, type_name, sources, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else { return };
            let authored = match world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                Some(cs) => match cs.author_connection(&sp, name, type_name, sources) {
                    Ok(()) => true,
                    Err(e) => { warn!("[twin] author connection {path}.{name}: {e}"); false }
                },
                None => false,
            };
            // Cosim wires (`SimConnection`) are derived from `connectionPaths` by
            // `reconcile_usd_connections`, which re-scans the composed stage — the
            // subtree refresh re-triggers it for the owning prim.
            if authored {
                refresh_prim_subtree(world, scene_id, path);
            }
        }
        // Coarse ops never reach here (the caller rebuilds for them) — that now
        // includes SetApiSchemas / SetActive, whose ECS effect (physics component
        // set / entity presence) the visual-only subtree refresh can't reconcile.
        _ => {}
    }
}

/// Re-instantiate the subtree(s) that depend on relationship `name` on `path`.
/// A `material:binding` fans out to every mesh it reaches, so a whole-scene visual
/// refresh is honest; any other relationship (physics bodies, collections) is
/// local to its owning prim's consumer, so refresh just that subtree.
fn refresh_relationship_dependents(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
    name: &str,
) {
    if name == "material:binding" {
        refresh_scene_visuals(world, scene_id);
    } else {
        refresh_prim_subtree(world, scene_id, path);
    }
}

/// Whether the live entity projecting `path` in `scene_id` is already tagged
/// [`UsdAnimated`](lunco_usd_bevy::UsdAnimated) — so the per-frame animation
/// sampler already drives it and a fresh keyframe needs no re-instantiation. False
/// when the prim is static (or has no live entity yet), which is when a first
/// keyframe must trigger a subtree refresh to (re-)tag it.
fn prim_entity_is_animated(world: &mut World, scene_id: AssetId<UsdStageAsset>, path: &str) -> bool {
    let mut q = world.query::<(&UsdPrimPath, Option<&lunco_usd_bevy::UsdAnimated>)>();
    q.iter(world)
        .any(|(upp, anim)| upp.stage_handle.id() == scene_id && upp.path == *path && anim.is_some())
}

/// Author a spawn onto the live stage: a plain prim authors immediately; a
/// referenced prim authors the arc when its asset closure is already loaded, else
/// queues a [`RefSpawn`] fetch that [`drain_ref_spawns`] completes. The
/// short-borrow / pre-decide pattern — the `!Send` stage can't be held across the
/// `AssetServer` fetch or the authoring re-borrow. Shared by op replay
/// ([`apply_incremental_op_to_stage`]) and the first-mount composed reconcile
/// ([`reconcile_full_to_composed`]).
fn spawn_prim_op(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    prim_path: &str,
    type_name: Option<String>,
    reference: Option<String>,
) {
    use lunco_usd_bevy::CanonicalStages;
    let Ok(sp) = openusd::sdf::Path::new(prim_path) else { return };

    let Some(asset_path) = reference else {
        // Plain prim — author now.
        if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
            if let Err(e) = cs.author_prim(&sp, type_name.as_deref()) {
                warn!("[twin] spawn {prim_path}: {e}");
            }
        }
        return;
    };

    // Referenced spawn: decide RefNow vs RefFetch under a short borrow, then act.
    enum Plan {
        Now,
        Fetch(String),
    }
    let plan = {
        let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id))
        else {
            return;
        };
        let ref_id = cs.canonical_reference_id(&asset_path);
        if cs.has_layer_bytes(&ref_id) {
            Plan::Now
        } else {
            Plan::Fetch(ref_id)
        }
    };
    match plan {
        Plan::Now => {
            if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                if let Err(e) = cs
                    .author_prim(&sp, type_name.as_deref())
                    .and_then(|_| cs.author_reference(&sp, &asset_path))
                {
                    warn!("[twin] referenced spawn {prim_path}: {e}");
                }
            }
        }
        Plan::Fetch(ref_id) => {
            let ref_handle = world.resource::<AssetServer>().load::<UsdStageAsset>(ref_id);
            world.resource_mut::<PendingRefSpawns>().items.push(RefSpawn {
                scene_id,
                prim_path: prim_path.to_string(),
                type_name,
                asset_path,
                ref_handle,
            });
        }
    }
}

/// Reconcile one structural delta at `path` against the composed document — used
/// only by the first-mount [`reconcile_full_to_composed`]. Restored runtime
/// spawns aren't recorded as ops (they arrive via a bulk
/// [`restore_runtime`](crate::document::UsdDocument::restore_runtime)), so their
/// type + reference are read from the composed spec here rather than replayed.
/// Absent in `composed` → remove; present → spawn (plain / referenced) via the
/// shared [`spawn_prim_op`].
fn author_structural_edit(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
    composed: &openusd::sdf::Data,
) {
    use lunco_usd_bevy::CanonicalStages;
    let Ok(sp) = openusd::sdf::Path::new(path) else { return };
    let Some(spec) = composed.spec(&sp).filter(|s| s.ty == openusd::sdf::SpecType::Prim) else {
        // Removed from the document → remove from the live stage.
        if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
            if let Err(e) = cs.remove_prim_at(&sp) {
                warn!("[twin] remove {path}: {e}");
            }
        }
        return;
    };
    let type_name = composed.prim_type_name(&sp);
    let reference = first_reference(spec);
    spawn_prim_op(world, scene_id, path, type_name, reference);
}

/// Re-read the whole scene from the (now-authored) live stage: for each
/// scene-root entity of `scene_id`, drop its `UsdVisualSynced` marker + children
/// and re-insert `UsdPrimPath`, re-firing `on_usd_prim_added` to re-instantiate
/// the subtree — so an attribute edit that fans out through a material binding
/// reaches every bound mesh. A scene root is an entity of this scene whose parent
/// is *not* itself a prim of the same scene. The per-edit, synchronous successor
/// to the old reload-driven whole-scene rebuild (matches the viewport's former
/// `DocumentChanged` → clear-`UsdVisualSynced`-on-scene-root refresh).
fn refresh_scene_visuals(world: &mut World, scene_id: AssetId<UsdStageAsset>) {
    let scene: Vec<(Entity, Option<Entity>)> = {
        let mut q = world.query::<(Entity, &UsdPrimPath, Option<&ChildOf>)>();
        q.iter(world)
            .filter(|(_, upp, _)| upp.stage_handle.id() == scene_id)
            .map(|(e, _, parent)| (e, parent.map(|p| p.0)))
            .collect()
    };
    let members: std::collections::HashSet<Entity> = scene.iter().map(|(e, _)| *e).collect();
    let roots: Vec<Entity> = scene
        .iter()
        .filter(|(_, parent)| parent.map(|p| !members.contains(&p)).unwrap_or(true))
        .map(|(e, _)| *e)
        .collect();
    for root in roots {
        reinstantiate_entity(world, root);
    }
}

/// Drop `entity`'s [`UsdVisualSynced`] marker + children and re-insert its
/// [`UsdPrimPath`], re-firing `on_usd_prim_added` so its subtree rebuilds from
/// the (now-authored) live stage. The shared primitive under both the whole-scene
/// [`refresh_scene_visuals`] and the single-prim [`refresh_prim_subtree`].
fn reinstantiate_entity(world: &mut World, entity: Entity) {
    if let Ok(mut em) = world.get_entity_mut(entity) {
        em.remove::<UsdVisualSynced>();
        em.despawn_related::<Children>();
        if let Some(pp) = em.take::<UsdPrimPath>() {
            em.insert(pp);
        }
    }
}

/// Re-instantiate only the subtree of the single prim at `path` in `scene_id`
/// (the entity whose [`UsdPrimPath`] matches), leaving every other scene root
/// untouched. Used for a geometry/xform attribute edit, whose visual effect is
/// local to its own prim — unlike a material/shader edit, which fans out through
/// `material:binding` to arbitrary meshes and needs the whole-scene refresh. This
/// avoids re-instantiating unrelated roots (including live physics bodies) on
/// every attribute edit.
fn refresh_prim_subtree(world: &mut World, scene_id: AssetId<UsdStageAsset>, path: &str) {
    let entity = {
        let mut q = world.query::<(Entity, &UsdPrimPath)>();
        q.iter(world)
            .find(|(_, upp)| upp.stage_handle.id() == scene_id && upp.path == *path)
            .map(|(e, _)| e)
    };
    if let Some(e) = entity {
        reinstantiate_entity(world, e);
    }
}

/// Whether a `SetAttribute` on a prim of this type must refresh the **whole**
/// scene rather than just the edited prim's subtree. A material / shader /
/// node-graph opinion propagates through `material:binding` to meshes anywhere in
/// the scene, so the edited prim's own subtree is not where the visual change
/// lands; an unknown type is treated conservatively as needing the full refresh.
/// Every other (geometry/xform) attribute edit is local to its prim, so it takes
/// the cheap [`refresh_prim_subtree`] path.
fn attribute_edit_needs_full_refresh(prim_type: Option<&str>) -> bool {
    match prim_type {
        Some(t) => matches!(t, "Material" | "Shader" | "NodeGraph"),
        None => true,
    }
}

/// Whether an edit at `path` lands inside a [`LiveRebuildExempt`] prim's subtree of
/// `scene_id` — a live prim that refreshes its own content in place (the DEM
/// terrain: it re-bakes off the registry document, so a whole-scene reload would
/// re-bridge the terrain + re-spawn the avatar camera per edit). Matches the
/// exempt prim itself or any descendant. The missing consumer of `LiveRebuildExempt`.
fn edit_confined_to_exempt_subtree(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
) -> bool {
    let mut q = world.query_filtered::<&UsdPrimPath, With<LiveRebuildExempt>>();
    q.iter(world).any(|upp| {
        upp.stage_handle.id() == scene_id
            && (upp.path == path
                || path.starts_with(&format!("{}/", upp.path.trim_end_matches('/'))))
    })
}

/// Rebuild the scene's live `CanonicalStage` from the composed document source
/// (`base ⊕ runtime`) plus the resolver's already-loaded layer closure, then
/// re-instantiate the scene — the coarse `full_reload` path (Save-As / MovePrim /
/// whole-source undo). Unlike [`reconcile_full_to_composed`] (a structural-only
/// authored-spine diff), a rebuild picks up attribute-value changes on surviving
/// prims too, so undoing a `SetAttribute` (which inverts to a `ReplaceSource`)
/// actually reverts the material/param in the live world. References that were
/// already loaded recompose from the byte snapshot with no re-fetch; a brand-new
/// reference introduced by the edit (rare) would fail to resolve — logged by
/// `CanonicalStages::rebuild`.
fn rebuild_scene_from_composed(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    composed_source: &str,
) {
    use lunco_usd_bevy::{CanonicalStages, StageRecipe};
    // Recipe = the edited composed source as the root layer + every referenced
    // `.usda` the current stage already loaded (keyed by the same canonical ids).
    let (scene_layer, mut bytes) = {
        let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id))
        else {
            return;
        };
        (cs.scene_layer.clone(), cs.layer_bytes_snapshot())
    };
    bytes.insert(scene_layer.clone(), composed_source.as_bytes().to_vec());
    let recipe = StageRecipe { root_id: scene_layer, bytes };
    let rebuilt = world
        .get_non_send_resource_mut::<CanonicalStages>()
        .map(|mut stages| stages.rebuild(scene_id, &recipe))
        .unwrap_or(false);
    if rebuilt {
        // Fresh stage (new, empty sink) — re-instantiate every scene root off it.
        refresh_scene_visuals(world, scene_id);
    }
}

/// Reconcile the whole **authored spine** of the scene's live stage against the
/// composed document — the `full_reload` fallback and the first-mount path for a
/// restored runtime overlay. Diffs *authored opinions* (the live stage's root
/// layer, via [`extract_root_layer_data`](lunco_usd_bevy::author::extract_root_layer_data),
/// vs the composed `sdf::Data`) rather than the PCP-expanded prim tree, so
/// reference-expanded children (which exist only on the live stage) are never
/// mistaken for removals. Removes prims dropped from the document, then authors
/// prims added to it (parent-first), each through [`author_structural_edit`].
fn reconcile_full_to_composed(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    composed: &openusd::sdf::Data,
) {
    use lunco_usd_bevy::CanonicalStages;
    use std::collections::BTreeSet;

    // Snapshot the authored-prim sets under a short borrow of the `!Send` stage.
    let (live_authored, composed_prims): (BTreeSet<String>, BTreeSet<String>) = {
        let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id))
        else {
            return;
        };
        let live = match lunco_usd_bevy::author::extract_root_layer_data(cs.stage()) {
            Ok(data) => data
                .iter()
                .filter(|(_, s)| s.ty == openusd::sdf::SpecType::Prim)
                .map(|(p, _)| p.to_string())
                .collect(),
            Err(e) => {
                warn!("[twin] full reconcile: extract root layer failed: {e}");
                return;
            }
        };
        let composed_set = composed
            .iter()
            .filter(|(_, s)| s.ty == openusd::sdf::SpecType::Prim)
            .map(|(p, _)| p.to_string())
            .collect();
        (live, composed_set)
    };

    // Removals first (deepest paths first so children go before parents), then
    // additions (shallowest first so a parent exists before its child spawns).
    let mut removed: Vec<&String> = live_authored.difference(&composed_prims).collect();
    removed.sort_by(|a, b| b.len().cmp(&a.len()));
    for path in removed {
        author_structural_edit(world, scene_id, path, composed);
    }
    let mut added: Vec<&String> = composed_prims.difference(&live_authored).collect();
    added.sort_by_key(|p| p.len());
    for path in added {
        author_structural_edit(world, scene_id, path, composed);
    }
}

/// Complete referenced spawns whose asset closure has finished loading: inject
/// the fetched layer bytes into the scene stage's resolver, then author the prim
/// + `references` arc so the openusd sink fires and `project_stage_changes`
/// instantiates the composed subtree. Spawns whose closure hasn't landed yet are
/// retried next frame. Exclusive: authors onto the `!Send` `CanonicalStage`.
pub(crate) fn drain_ref_spawns(world: &mut World) {
    use lunco_usd_bevy::CanonicalStages;
    if world.resource::<PendingRefSpawns>().items.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<PendingRefSpawns>().items);
    let mut still = Vec::new();
    for item in pending {
        // Wait for the asset closure (its loader fetches the full `.usda` tree).
        let recipe = world
            .resource::<Assets<UsdStageAsset>>()
            .get(item.ref_handle.id())
            .and_then(|a| a.recipe.clone());
        let Some(recipe) = recipe else {
            still.push(item);
            continue;
        };
        let Ok(sp) = openusd::sdf::Path::new(&item.prim_path) else { continue };
        let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(item.scene_id))
        else {
            continue; // scene stage gone — drop the spawn
        };
        // Inject the closure bytes so PCP can resolve the reference, then author.
        cs.add_layer_bytes(recipe.bytes.clone());
        if let Err(e) = cs
            .author_prim(&sp, item.type_name.as_deref())
            .and_then(|_| cs.author_reference(&sp, &item.asset_path))
        {
            warn!("[twin] referenced spawn {} (post-fetch): {e}", item.prim_path);
        }
    }
    world.resource_mut::<PendingRefSpawns>().items.extend(still);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LayerId, UsdOp};

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// The rebuild-cliff fix (doc 48 §3.3): `SetRelationship` and `SetConnection`
    /// gained live-stage authors and must NO LONGER force a whole-scene rebuild;
    /// only the two composition-arc ops (which recompose a subtree wholesale) still
    /// do, alongside the pre-existing coarse ops. This is the routing half of the
    /// "attaching a part no longer respawns the world" claim.
    #[test]
    fn op_rebuild_routing_matches_the_incremental_authors() {
        let et = LayerId::root();
        // Incremental now — a joint's two `physics:body` rels and a cosim wire.
        assert!(!op_needs_rebuild(&UsdOp::SetRelationship {
            edit_target: et.clone(), path: "/J".into(), name: "physics:body0".into(), targets: vec![],
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetConnection {
            edit_target: et.clone(), path: "/B".into(), name: "inputs:v".into(),
            type_name: "float".into(), sources: vec![],
        }));
        // apiSchema / active REBUILD: their effect is a prim's ECS component set /
        // entity presence, which the visual-only subtree refresh can't reconcile.
        assert!(op_needs_rebuild(&UsdOp::SetApiSchemas {
            edit_target: et.clone(), path: "/W".into(), schemas: vec![],
        }));
        assert!(op_needs_rebuild(&UsdOp::SetActive {
            edit_target: et.clone(), path: "/W".into(), active: false,
        }));
        // Composition-arc edits also rebuild — value resolution recomposes the
        // subtree, which the incremental sink can't express.
        assert!(op_needs_rebuild(&UsdOp::SetVariantSelection {
            edit_target: et.clone(), path: "/R".into(),
            variant_set: "drivetrain".into(), variant: "physical".into(),
        }));
        assert!(op_needs_rebuild(&UsdOp::SetPayload {
            edit_target: et.clone(), path: "/H".into(), asset_paths: vec![],
        }));
        // Pre-existing coarse ops unchanged.
        assert!(op_needs_rebuild(&UsdOp::MovePrim {
            edit_target: et, from_path: "/a".into(), to_path: "/b".into(),
        }));
    }

    /// A material/shader/node-graph attribute edit fans out through
    /// `material:binding` and needs the whole-scene refresh; an unknown type is
    /// treated conservatively the same way; every other (geometry/xform) edit is
    /// local and takes the cheap single-prim path.
    #[test]
    fn attribute_refresh_scope_is_full_only_for_shading_prims() {
        for shading in ["Material", "Shader", "NodeGraph"] {
            assert!(
                attribute_edit_needs_full_refresh(Some(shading)),
                "{shading} binding fan-out needs a whole-scene refresh"
            );
        }
        assert!(
            attribute_edit_needs_full_refresh(None),
            "unknown prim type is refreshed conservatively (whole scene)"
        );
        for local in ["Mesh", "Xform", "Sphere", "Cube", "Camera"] {
            assert!(
                !attribute_edit_needs_full_refresh(Some(local)),
                "{local} attribute edit is local to its own prim subtree"
            );
        }
    }

    #[test]
    fn find_doc_for_abs_matches_file_origin_only() {
        let mut registry = UsdDocumentRegistry::default();
        let abs = PathBuf::from("/twins/moonbase/scene.usda");
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file(abs.clone()));
        registry.allocate(TINY.to_string(), DocumentOrigin::untitled("Untitled.usda"));

        assert_eq!(find_doc_for_abs(&registry, &abs), Some(doc));
        assert_eq!(find_doc_for_abs(&registry, std::path::Path::new("/twins/x.usda")), None);
    }

    /// The bytes pushed into the overlay are the document's *composed* source —
    /// so a runtime-layer spawn rides into the live world's composition.
    #[test]
    fn composed_source_overlay_carries_runtime_spawn() {
        let mut registry = UsdDocumentRegistry::default();
        let abs = PathBuf::from("/twins/moonbase/scene.usda");
        let doc = registry.allocate(TINY.to_string(), DocumentOrigin::writable_file(abs));
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: Some("Xform".into()),
                reference: Some("lunco://vessels/rovers/skid_rover.usda".into()),
            })
            .unwrap();

        let composed = registry.host(doc).unwrap().document().composed_source();
        assert!(
            composed.contains("rover_1"),
            "overlay bytes carry the runtime spawn:\n{composed}"
        );
        assert!(
            composed.contains("@lunco://vessels/rovers/skid_rover.usda@"),
            "and its asset reference (resolved by the async loader at the twin:// anchor)"
        );
    }
}
