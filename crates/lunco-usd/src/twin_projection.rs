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
//! Flow (`open_usd_docs_on_twin_added` keeps firing `LoadScene` for the immediate
//! live mount; E1b runs alongside):
//! 1. On `TwinAdded` with a `[usd] default_scene`, kick an async
//!    [`UsdSourceText`] load of `twin://<name>/<scene>` (raw base layer, read
//!    through the twin source — web-ready) and record it in [`PendingTwinDocs`].
//! 2. [`drain_pending_twin_docs`] — once the source text is in hand, allocate a
//!    [`UsdDocument`](crate::document) for it (origin = the on-disk path, so Save
//!    and dedup work) and record it in [`DocBackedTwinScenes`].
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

use crate::registry::UsdDocumentRegistry;

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

/// Make a twin scene **doc-backed** (E1b) outside the `TwinAdded` observer — for
/// binaries that mount a twin scene via a direct [`LoadScene`](crate::LoadScene)
/// (e.g. the sandbox `--scene`) rather than opening a workspace Twin. Reads the
/// scene's base layer through the twin source (`twin://<name>/<rel>`, web-ready)
/// and queues a document for it; once allocated, [`sync_twin_overlays`] serves the
/// composed (`base ⊕ runtime`) source so runtime USD edits project to the live
/// world. The immediate file-backed mount still comes from the caller's
/// `LoadScene` — exactly the dual path `open_usd_docs_on_twin_added` runs in
/// production. `abs_path` is the scene's on-disk path (the document origin → Save
/// target + dedup key).
pub fn doc_back_twin_scene(
    asset_server: &AssetServer,
    pending: &mut PendingTwinDocs,
    twin_name: &str,
    rel: &str,
    abs_path: PathBuf,
) {
    let handle = asset_server.load::<UsdSourceText>(format!("twin://{twin_name}/{rel}"));
    pending.push(handle, twin_name.to_string(), rel.to_string(), abs_path);
}

/// The twin-source coordinates + last-synced generation for a doc-backed twin
/// scene, so [`sync_twin_overlays`] re-serializes only when the document moved.
struct TwinSceneRef {
    name: String,
    rel: String,
    synced_generation: Option<u64>,
}

/// Map of document → the twin scene it backs. Populated by
/// [`drain_pending_twin_docs`], consumed by [`sync_twin_overlays`].
#[derive(Resource, Default)]
pub struct DocBackedTwinScenes {
    map: HashMap<DocumentId, TwinSceneRef>,
}

impl DocBackedTwinScenes {
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
        self.map
            .entry(doc)
            .or_insert(TwinSceneRef { name, rel, synced_generation: None });
    }
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
/// has loaded through the twin source. Idempotent: reuses an existing document
/// for the same on-disk path (twin re-add) rather than double-allocating.
pub(crate) fn drain_pending_twin_docs(
    mut pending: ResMut<PendingTwinDocs>,
    mut registry: ResMut<UsdDocumentRegistry>,
    mut backed: ResMut<DocBackedTwinScenes>,
    sources: Res<Assets<UsdSourceText>>,
) {
    if pending.items.is_empty() {
        return;
    }
    let taken = std::mem::take(&mut pending.items);
    let mut still = Vec::new();
    for mut item in taken {
        let Some(UsdSourceText(source)) = sources.get(&item.handle) else {
            item.attempts += 1;
            if item.attempts < MAX_TWIN_DOC_ATTEMPTS {
                still.push(item);
            } else {
                warn!(
                    "[usd-e1b] base source for `twin://{}/{}` never loaded — no doc-backed projection",
                    item.name, item.rel
                );
            }
            continue;
        };
        let doc = find_doc_for_abs(&registry, &item.abs_path).unwrap_or_else(|| {
            registry.allocate(source.clone(), DocumentOrigin::writable_file(item.abs_path.clone()))
        });
        backed.map.entry(doc).or_insert(TwinSceneRef {
            name: item.name.clone(),
            rel: item.rel.clone(),
            synced_generation: None,
        });
        info!("[usd-e1b] default scene `twin://{}/{}` is now doc-backed ({doc})", item.name, item.rel);
    }
    pending.items.extend(still);
}

/// Keep each doc-backed twin scene's twin-source overlay in step with its
/// document: when the generation moves, serialize the composed (`base ⊕ runtime`)
/// source into the overlay and `reload` the scene asset so the live world
/// re-composes from the document (web-ready — the async loader anchors at the
/// `twin://` identity). Drops entries whose document has closed.
pub(crate) fn sync_twin_overlays(world: &mut World) {
    // Snapshot tracked scenes (owned) so no resource borrow is held across the
    // world mutations below.
    let entries: Vec<(DocumentId, String, String, Option<u64>)> = world
        .resource::<DocBackedTwinScenes>()
        .map
        .iter()
        .map(|(doc, s)| (*doc, s.name.clone(), s.rel.clone(), s.synced_generation))
        .collect();

    for (doc, name, rel, synced) in entries {
        // Current generation + composed payloads, or drop if the doc closed.
        let resolved = world.resource::<UsdDocumentRegistry>().host(doc).map(|h| {
            (
                h.document().generation(),
                h.document().composed_source(),
                h.document().composed(),
            )
        });
        let Some((cur_gen, composed_source, composed)) = resolved else {
            world.resource::<TwinRoots>().clear_overlay(&name, &rel);
            world.resource_mut::<DocBackedTwinScenes>().map.remove(&doc);
            continue;
        };
        if Some(cur_gen) == synced {
            continue;
        }

        // Always refresh the overlay so persistence / next load reflect the doc,
        // regardless of whether we reload the live asset.
        world
            .resource::<TwinRoots>()
            .set_overlay(&name, &rel, Arc::new(composed_source.into_bytes()));

        // Classify the changes since the last sync. The scene's live stage is
        // keyed by the cached `twin://name/rel` UsdStageAsset id (AssetServer
        // dedups by path), shared by every child prim entity.
        let twin_path = format!("twin://{}/{}", name, rel);
        let batch = crate::live_consume::classify_changes_since(
            world.resource::<UsdDocumentRegistry>(),
            doc,
            synced.unwrap_or(0),
            cur_gen,
        );
        let scene_id = world
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(twin_path.clone())
            .id();

        let needs_translate = batch.as_ref().map(|b| !b.translate_paths.is_empty()).unwrap_or(false);
        let needs_attr = batch.as_ref().map(|b| !b.attr_paths.is_empty()).unwrap_or(false);
        let needs_structural = batch.as_ref().map(|b| b.needs_structural).unwrap_or(true);

        // Every projection below authors onto the live stage, so it must exist.
        // On the very first generations the scene's async `LoadScene` is still in
        // flight and `sync_canonical_stages` hasn't built the stage yet — DEFER
        // (leave `synced` unchanged) so we retry once it lands. A clean scene with
        // nothing to project doesn't wait.
        let stage_ready = world
            .get_non_send_resource::<lunco_usd_bevy::CanonicalStages>()
            .map(|s| s.get(scene_id).is_some())
            .unwrap_or(false);
        if (needs_translate || needs_attr || needs_structural) && !stage_ready {
            continue;
        }

        // Moves: author each translate onto the LIVE canonical stage. Its change
        // sink fires and `project_stage_changes` moves the entity incrementally.
        if let Some(batch) = &batch {
            for path in &batch.translate_paths {
                let Ok(sp) = openusd::sdf::Path::new(path) else { continue };
                let Some(v) = lunco_usd_bevy::read_vec3_f64(&composed, &sp, "xformOp:translate")
                else {
                    continue;
                };
                if let Some(cs) = world
                    .get_non_send_resource::<lunco_usd_bevy::CanonicalStages>()
                    .and_then(|s| s.get(scene_id))
                {
                    if let Err(e) = cs.author_translate(&sp, v) {
                        warn!("[twin] author translate {path}: {e}");
                    }
                }
            }
        }

        // Non-translate attribute edits (material colour, roughness, size, …):
        // author each onto the live stage and refresh the prim's visual so it
        // re-reads — the sink-driven successor to the reload that used to re-read
        // these attributes.
        if let Some(batch) = &batch {
            let mut authored_attr = false;
            for (prim, attr) in &batch.attr_paths {
                authored_attr |= author_attribute_edit(world, scene_id, prim, attr, &composed);
            }
            if authored_attr {
                refresh_scene_visuals(world, scene_id);
            }
        }

        // Structural changes: author each delta onto the live stage (spawn plain,
        // spawn referenced, remove), firing the sink so `project_stage_changes`
        // reconciles ECS in place. A `full_reload` (Save-As / MovePrim / a
        // restored runtime overlay at first mount / a change-ring overflow) has no
        // trustworthy per-prim delta, so we reconcile the whole authored spine
        // against the composed document.
        if needs_structural {
            let full = batch.as_ref().map(|b| b.full_reload).unwrap_or(true);
            let resync = batch.as_ref().map(|b| b.resync_paths.clone()).unwrap_or_default();
            if full || synced.is_none() {
                reconcile_full_to_composed(world, scene_id, &composed);
            } else {
                for path in &resync {
                    author_structural_edit(world, scene_id, path, &composed);
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

/// Author one structural delta at `path` onto the scene's live `CanonicalStage`,
/// firing the openusd sink so [`project_stage_changes`](crate::live_consume::project_stage_changes)
/// reconciles ECS. Classifies against the composed document:
/// - **absent in `composed`** → the prim was removed → `remove_prim_at`;
/// - **present, no reference** → a plain prim → `author_prim`;
/// - **present, references an already-loaded asset** → `author_prim` +
///   `author_reference` (PCP composes the subtree from the live resolver);
/// - **present, references a not-yet-loaded asset** → queue a
///   [`RefSpawn`]: load the asset closure, then [`drain_ref_spawns`] injects it
///   and authors the arc.
///
/// Reads what it needs from the `!Send` stage under a short borrow, then acts —
/// the stage can't be held across a resource fetch (`AssetServer`) or the
/// authoring calls that re-borrow it.
fn author_structural_edit(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
    composed: &openusd::sdf::Data,
) {
    use lunco_usd_bevy::CanonicalStages;
    let Ok(sp) = openusd::sdf::Path::new(path) else { return };
    let spec = composed.spec(&sp);

    // Removal: the prim is gone from the composed document.
    let Some(spec) = spec.filter(|s| s.ty == openusd::sdf::SpecType::Prim) else {
        if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
            if let Err(e) = cs.remove_prim_at(&sp) {
                warn!("[twin] remove {path}: {e}");
            }
        }
        return;
    };

    let type_name = composed.prim_type_name(&sp);
    let reference = first_reference(spec);

    // Decide under a short immutable borrow of the `!Send` stage, then release it.
    enum Plan {
        Plain,
        RefNow,
        RefFetch(String), // canonical load id for the asset closure
    }
    let plan = {
        let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id))
        else {
            return;
        };
        match &reference {
            None => Plan::Plain,
            Some(asset_path) => {
                let ref_id = cs.canonical_reference_id(asset_path);
                if cs.has_layer_bytes(&ref_id) {
                    Plan::RefNow
                } else {
                    Plan::RefFetch(ref_id)
                }
            }
        }
    };

    match plan {
        Plan::Plain => {
            if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                if let Err(e) = cs.author_prim(&sp, type_name.as_deref()) {
                    warn!("[twin] spawn {path}: {e}");
                }
            }
        }
        Plan::RefNow => {
            let asset_path = reference.expect("RefNow implies a reference");
            if let Some(cs) = world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
                if let Err(e) = cs
                    .author_prim(&sp, type_name.as_deref())
                    .and_then(|_| cs.author_reference(&sp, &asset_path))
                {
                    warn!("[twin] referenced spawn {path}: {e}");
                }
            }
        }
        Plan::RefFetch(ref_id) => {
            let asset_path = reference.expect("RefFetch implies a reference");
            let ref_handle = world.resource::<AssetServer>().load::<UsdStageAsset>(ref_id);
            world.resource_mut::<PendingRefSpawns>().items.push(RefSpawn {
                scene_id,
                prim_path: path.to_string(),
                type_name,
                asset_path,
                ref_handle,
            });
        }
    }
}

/// Author a non-translate attribute edit (`prim.attr`) onto the scene's live
/// `CanonicalStage` from the composed document's value + type. Returns `true` if
/// it authored (so the caller refreshes the scene visuals once). Skips
/// samples-only attributes (no `default` opinion) and attributes with no
/// composable type. Does NOT refresh — a material/shader edit fans out through
/// `material:binding` to bound meshes, so the caller re-reads the whole scene.
fn author_attribute_edit(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    prim_str: &str,
    attr: &str,
    composed: &openusd::sdf::Data,
) -> bool {
    use lunco_usd_bevy::CanonicalStages;
    let Ok(prim) = openusd::sdf::Path::new(prim_str) else { return false };
    let Ok(attr_path) = prim.append_property(attr) else { return false };
    let Some(spec) = composed.spec(&attr_path) else { return false };
    let type_name = match spec.get("typeName") {
        Some(openusd::sdf::Value::Token(t)) => t.to_string(),
        Some(openusd::sdf::Value::String(s)) => s.clone(),
        _ => return false, // no authored type to create the attribute with
    };
    let Some(value) = spec.get("default").cloned() else {
        return false; // samples-only edit (keyframes) — not handled on this path
    };
    match world.get_non_send_resource::<CanonicalStages>().and_then(|s| s.get(scene_id)) {
        Some(cs) => match cs.author_attribute(&prim, attr, &type_name, value) {
            Ok(()) => true,
            Err(e) => {
                warn!("[twin] author attribute {prim_str}.{attr}: {e}");
                false
            }
        },
        None => false,
    }
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
        if let Ok(mut em) = world.get_entity_mut(root) {
            em.remove::<UsdVisualSynced>();
            em.despawn_related::<Children>();
            if let Some(pp) = em.take::<UsdPrimPath>() {
                em.insert(pp);
            }
        }
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
