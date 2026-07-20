//! USD → terrain projection.
//!
//! Reads authored terrain prims off the live composed stage and projects them into
//! `lunco-terrain-surface`'s domain types — a [`DemTerrainRequest`] (the ground DEM:
//! source, window, resolution, streaming knobs) plus the composable
//! [`TerrainLayerStack`] built from the prim's child LAYER prims (craters / rocks /
//! shader / edits / …). It also carries the authoring tier back: a hand edit (brush,
//! flatten, crater, rock) on a doc-backed terrain becomes USD ops on the document's
//! **runtime** layer — journaled, undoable, non-destructive — and the re-projection
//! is what makes it visible. An edit that does not go through here escapes save,
//! journal, undo, and the network.
//!
//! One crate per USD→domain projection: `lunco-usd-avian` (physics), `lunco-usd-sim`
//! (behaviour), `lunco-usd-bevy` (render), and this one (terrain).
//!
//! Render-free by construction, so a headless server can project a USD terrain — and
//! get its collider for deterministic physics — without linking a UI. The terrain's
//! material is a `UsdShade` binding like any other (`lunco-usd-sim`'s shader pass), so
//! nothing here names a material. `lunco-terrain-surface` stays USD-free in turn and
//! is read through its [`LayerAttrSource`](lunco_terrain_surface::LayerAttrSource)
//! port, implemented here by [`UsdLayerAttrs`].
//!
//! [`DemTerrainRequest`]: lunco_terrain_surface::DemTerrainRequest
//! [`TerrainLayerStack`]: lunco_terrain_surface::TerrainLayerStack

use bevy::prelude::*;
// Two read planes, two traits: `UsdRead` = the live COMPOSED stage (what the terrain
// projects from); `UsdDataExt` = a raw authored `sdf::Data` layer, which is what the
// document registry hands back for the authoring tier's child walks.
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::{StageView, UsdRead};

/// Projects authored USD terrain prims into `lunco-terrain-surface`, and authors hand
/// edits back onto the backing document's runtime layer.
///
/// Core (never render-gated): the collider a headless server needs for deterministic
/// physics comes out of this projection.
pub struct UsdTerrainPlugin;

impl Plugin for UsdTerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                bridge_usd_dem_terrain,
                refresh_layered_terrain_layers,
                cache_terrain_document,
                refresh_docbacked_terrain_from_doc,
            ),
        );
        // Authoring tier: doc-backed terrains route live edits to their USD document's
        // runtime layer (journaled, non-destructive) instead of mutating the runtime
        // layer stack directly. Document-free terrains are handled in lunco-terrain-surface.
        app.init_resource::<TerrainEditPrimSeq>()
            .add_observer(on_brush_terrain_authored)
            .add_observer(on_flatten_terrain_authored)
            .add_observer(on_place_crater_authored)
            .add_observer(on_place_rock_authored)
            .add_observer(on_remove_terrain_edit_authored)
            // Doc-backed crater/rock tuning authors to USD (→ project → regen), instead
            // of the direct stack-mutation path (which handles document-free terrains).
            .add_observer(on_obstacle_spec_authored);
    }
}

/// Marks a USD prim already examined by the DEM bridge (one-shot per prim).
///
/// Public because the app's ground-collider gate reads it: while any prim is still
/// unexamined, a terrain build may yet be requested, and dynamic bodies must not
/// activate over not-yet-collidable ground.
#[derive(Component)]
pub struct DemBridged;

/// USD-backed [`LayerAttrSource`](lunco_terrain_surface::LayerAttrSource): reads a
/// child layer prim's attributes through the stage reader, so terrain-surface's layer
/// parsers stay USD-free.
///
/// One lifetime, not two: `StageView` holds only shared references, so it is
/// covariant in its own lifetime and a longer-lived `&'a StageView<'b>` coerces
/// here freely.
struct UsdLayerAttrs<'a> {
    reader: &'a StageView<'a>,
    sdf: openusd::sdf::Path,
    /// The USD namespace the logical names bind into: `lunco:layer:` for a layer
    /// prim's parameters (`LunCoTerrainLayerAPI`), `lunco:edit:` for an edit prim's
    /// (`LunCoTerrainEditAPI`). One adapter, because the two differ ONLY in prefix —
    /// and the prefix is exactly what a USD-free parser must not know.
    ns: &'static str,
}

/// `LunCoTerrainLayerAPI` — a layer prim's parameters.
const NS_LAYER: &str = "lunco:layer:";
/// `LunCoTerrainEditAPI` — one hand edit's parameters.
const NS_EDIT: &str = "lunco:edit:";

/// The USD property name for a layer parameter: `"size"` → `"lunco:layer:size"`
/// (`LunCoTerrainLayerAPI`).
///
/// The one place the mapping lives. Layer parsers speak *logical* names (`x`,
/// `size`, `seed`) — they are USD-free by design — and this adapter is what binds
/// them to USD, so the namespace belongs here rather than smeared across a dozen
/// parsers that would each have to remember it.
///
/// They used to be authored BARE, in the root property namespace, which is how a
/// rock layer's `size` came to collide with `UsdGeomCube`'s real `double size`:
/// two different meanings for one property name on prims that can be both.
fn ns_attr(ns: &str, name: &str) -> String {
    let full = format!("{ns}{name}");
    // The mapping is stringly, so VERIFY it instead of trusting it: a parser reading
    // a parameter `LunCoTerrainLayerAPI` does not declare is either a typo or a new
    // parameter someone forgot to add to the schema, and both should be loud. This
    // fires the first time any test or debug run touches a layer, so the schema and
    // the parsers cannot drift apart silently — which is the whole failure mode the
    // bare names had.
    debug_assert!(
        lunco_usd::schema::SchemaRegistry::global()
            .read()
            .map(|r| r.property(&full).is_some())
            .unwrap_or(false),
        "`{full}` is not declared by luncoSchema \
         (crates/lunco-usd/schema/schema.usda) — add it there, or fix the typo"
    );
    full
}

impl UsdLayerAttrs<'_> {
    fn attr(&self, name: &str) -> String {
        ns_attr(self.ns, name)
    }
}

impl lunco_terrain_surface::LayerAttrSource for UsdLayerAttrs<'_> {
    fn get_f32(&self, name: &str) -> Option<f32> {
        self.reader.real_f32(&self.sdf, &self.attr(name))
    }
    fn get_f64(&self, name: &str) -> Option<f64> {
        self.reader.real(&self.sdf, &self.attr(name))
    }
    fn get_vec2(&self, name: &str) -> Option<[f64; 2]> {
        self.reader
            .attr_value(&self.sdf, &self.attr(name))
            .and_then(|v| v.try_as_vec_2d())
            .map(|v| [v.x, v.y])
    }
    fn get_i64(&self, name: &str) -> Option<i64> {
        // `TryFrom<Value>` is strict per variant, so probe both authored widths:
        // `int64` (the Inspector authors seeds full-range) and hand-authored `int`.
        let name = self.attr(name);
        self.reader
            .scalar::<i64>(&self.sdf, &name)
            .or_else(|| self.reader.scalar::<i32>(&self.sdf, &name).map(|v| v as i64))
    }
    fn get_string(&self, name: &str) -> Option<String> {
        // Textual USD types only — `lunco:layer:mode` is a `token`. A file reference
        // (`demSource`) is `asset`-typed and read via `get_asset`, not here.
        // `scalar::<String>` would read only `string`; `text` also reads `token`.
        self.reader.text(&self.sdf, &self.attr(name))
    }
    fn get_asset(&self, name: &str) -> Option<String> {
        // `asset`-typed reference (`lunco:layer:demSource`) — its own `Value::AssetPath`
        // variant, which `text`/`scalar::<String>` do NOT read. Returns the authored path.
        self.reader.asset(&self.sdf, &self.attr(name))
    }
    fn get_bool(&self, name: &str) -> Option<bool> {
        self.reader.scalar::<bool>(&self.sdf, &self.attr(name))
    }
}

/// The `dem` (ground) child layer prim of a layered terrain, if authored.
fn find_dem_layer(
    reader: &StageView<'_>,
    terrain: &openusd::sdf::Path,
) -> Option<openusd::sdf::Path> {
    reader
        .children(terrain)
        .into_iter()
        .find(|c| reader.text(c, "lunco:layer").as_deref() == Some("dem"))
}

/// Parse the non-ground child layer prims (`craters`/`rocks`/`shader`/…) into the
/// composable [`TerrainLayerStack`](lunco_terrain_surface::TerrainLayerStack) via the
/// registry. Shared by the bridge (initial build) and the live-edit refresh.
fn parse_terrain_layer_stack(
    reader: &StageView<'_>,
    terrain: &openusd::sdf::Path,
    registry: &lunco_terrain_surface::TerrainLayerParserRegistry,
) -> lunco_terrain_surface::TerrainLayerStack {
    let mut stack = lunco_terrain_surface::TerrainLayerStack::default();
    // Runtime edit prims (`lunco:layer = "edit"`) — one prim per edit — aggregate into
    // the single `EditsLayer` (the runtime projection tier), folded on top at the end.
    let mut edits: Vec<(lunco_terrain_surface::LayerId, lunco_terrain_surface::EditKind)> = Vec::new();
    // Whether the scene authors its OWN overzoom prim (even a zeroed/disabled one
    // counts — that's an explicit opt-out of the default sub-DEM detail).
    let mut authored_overzoom = false;
    // CANONICAL child order. `children()` iterates a hash map, so its order varies
    // per process AND per parse (bridge vs composed re-parse). The stack's fold
    // order feeds `SurfaceOracle::content_key` — unsorted, every launch minted a
    // fresh surface key for identical content, invalidating the entire tile/derived
    // map cache (cold-bake storm on every boot) and reordering non-commutative
    // edits. Sorting by path makes stack order — and thus the key and the composed
    // surface — a pure function of the document.
    let mut children: Vec<_> = reader.children(terrain).into_iter().collect();
    children.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for child in children {
        // An edit prim (`LunCoTerrainEditAPI`)? Aggregate into the single edits layer,
        // keyed by its prim path (its stable identity).
        let edit_attrs = UsdLayerAttrs { reader, sdf: child.clone(), ns: NS_EDIT };
        if let Some(edit) =
            lunco_terrain_surface::parse_edit(lunco_terrain_surface::LayerId::new(child.as_str()), &edit_attrs)
        {
            edits.push(edit);
            continue;
        }
        let attrs = UsdLayerAttrs { reader, sdf: child.clone(), ns: NS_LAYER };
        // Otherwise a normal composable layer prim (`lunco:layer = …`).
        let Some(layer_type) = reader.text(&child, "lunco:layer") else {
            continue;
        };
        if layer_type == "dem" {
            continue;
        }
        if layer_type == "overzoom" {
            authored_overzoom = true;
        }
        if !registry.knows(&layer_type) {
            warn!("[usd-dem] child layer '{layer_type}' has no registered terrain layer parser");
            continue;
        }
        if let Some(layer) = registry.parse(&layer_type, &attrs) {
            // Identity = the layer prim's path: unique, stable, already in hand. Lets
            // several same-kind layers coexist and be addressed individually.
            stack.push_layer(child.as_str(), layer);
        }
    }
    // Sub-DEM detail defaults ON: without it the ground between the finest shader
    // grain (~12 cm) and the DEM data resolution (~5 m) is empty in every channel
    // and reads as flat plastic one step from the camera. Authoring an `overzoom`
    // prim — including a zeroed one — takes over from the default.
    if !authored_overzoom {
        stack.push_layer("overzoom/default", lunco_terrain_surface::default_overzoom_layer());
    }
    if !edits.is_empty() {
        stack.push_layer(
            lunco_terrain_surface::EDITS_LAYER_ID,
            std::sync::Arc::new(lunco_terrain_surface::EditsLayer::from_edits(edits)),
        );
    }
    stack
}

/// Seed the shared [`ObstacleFieldSpec`] from the USD-authored `craters`/`rocks` child
/// layer prims so the Inspector's "Craters & Rocks" panel opens showing the scene's
/// ACTUAL values (density, size, ratios) instead of the resource defaults. Mirrors the
/// `SizeDist` the layer parsers build — `sizeMin`/`sizeMax` attrs with the parsers'
/// defaults (`craters` → 2/60, `rocks` → 0.2/(mode*4).max(2.5)) and the same
/// min ≤ mode ≤ max clamp — so a subsequent panel edit starts from the authored
/// look rather than jumping. Writes the resource only (no `UpdateObstacleFieldSpec`,
/// no re-stamp — the terrain already built from the same USD stack).
///
/// [`ObstacleFieldSpec`]: lunco_obstacle_field::spec::ObstacleFieldSpec
fn sync_obstacle_spec_from_usd(
    reader: &StageView<'_>,
    terrain: &openusd::sdf::Path,
    spec: &mut lunco_obstacle_field::spec::ObstacleFieldSpec,
) {
    use lunco_obstacle_field::spec::SizeDist;
    use lunco_terrain_surface::LayerAttrSource;
    for child in reader.children(terrain) {
        // Read through the SAME adapter the layer parsers use, so the `lunco:layer:`
        // namespace is applied in one place ([`ns_attr`]) and this panel cannot
        // drift from the parsers by reading a name they no longer author.
        let a = UsdLayerAttrs {
            reader,
            sdf: child.clone(),
            ns: NS_LAYER,
        };
        match reader.text(&child, "lunco:layer").as_deref() {
            Some("craters") => {
                let density = a.get_f32("density").unwrap_or(0.0);
                let mode = a.get_f32("sizeMode").unwrap_or(22.0);
                spec.craters.enabled = density > 0.0;
                spec.craters.density = density;
                spec.craters.depth_ratio = a.get_f32("depthRatio").unwrap_or(0.4);
                spec.craters.rim_height_ratio = a.get_f32("rimRatio").unwrap_or(0.18);
                let size_min = a.get_f32("sizeMin").unwrap_or(2.0);
                let size_max = a.get_f32("sizeMax").unwrap_or(60.0);
                spec.craters.size =
                    SizeDist::new(size_min.min(mode), mode, size_max.max(mode), 0.7);
                if let Some(seed) = a.get_i64("seed") {
                    spec.seed = seed as u64;
                }
            }
            Some("rocks") => {
                let density = a.get_f32("density").unwrap_or(0.0);
                let mode = a.get_f32("sizeMode").unwrap_or(0.6);
                spec.rocks.enabled = density > 0.0;
                spec.rocks.density = density;
                let size_min = a.get_f32("sizeMin").unwrap_or(0.2);
                let size_max = a.get_f32("sizeMax").unwrap_or((mode * 4.0).max(2.5));
                spec.rocks.size =
                    SizeDist::new(size_min.min(mode), mode, size_max.max(mode), 0.6);
                spec.rocks.dynamic_fraction = a.get_f32("dynamicFrac").unwrap_or(0.0);
            }
            _ => {}
        }
    }
}

/// Live-edit: when a stage is modified (a terrain layer prim was edited in the
/// Inspector / via `SetObjectProperty`), re-parse the composable stack of every
/// layered terrain on that stage and re-insert it. The change is picked up by
/// `regenerate_dem_layers` (it re-stamps off the retained base grid + re-scatters —
/// no GeoTIFF re-read), so crater/rock/shader tuning applies live.
///
/// **Document-free terrains only** (`Without<DocBackedTerrain>`). A doc-backed
/// terrain re-bakes from its registry document instead
/// ([`refresh_docbacked_terrain_from_doc`]) — the source of truth — so it doesn't
/// depend on the twin stage asset being reloaded (its `LiveRebuildExempt` marker
/// deliberately suppresses that reload). Routing exactly one path per terrain
/// avoids a double re-parse.
fn refresh_layered_terrain_layers(
    mut ev: MessageReader<AssetEvent<lunco_usd::UsdStageAsset>>,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    registry: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    q: Query<
        (Entity, &lunco_usd::UsdPrimPath),
        (
            With<lunco_terrain_surface::DemTerrainSurface>,
            Without<lunco_terrain_surface::DocBackedTerrain>,
        ),
    >,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    let mut modified = std::collections::HashSet::new();
    for e in ev.read() {
        if let AssetEvent::Modified { id } = e {
            modified.insert(*id);
        }
    }
    if modified.is_empty() {
        return;
    }
    for (entity, prim_path) in &q {
        if !modified.contains(&prim_path.stage_handle.id()) {
            continue;
        }
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        // Read the LIVE canonical stage (reflects the in-place edit that raised
        // this Modified event).
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            // No live stage (asset carries no recipe / build failed) — skip.
            continue;
        };
        let stack = parse_terrain_layer_stack(&cs.view(), &sdf, &registry);
        // Despawn-safe: a scene reload can despawn this terrain between queue
        // time and apply_deferred — no-op instead of panicking.
        commands.entity(entity).try_insert(stack);
    }
}

/// Caches the backing USD **document** on a doc-projected DEM terrain: the raw
/// `DocumentId` handle of the live scene the terrain belongs to, plus the
/// [`DocBackedTerrain`](lunco_terrain_surface::DocBackedTerrain) marker. Its presence
/// is the switch that routes live edits to the **authoring tier** (author a USD op →
/// journal → project). Its *absence* means a document-free terrain (quick
/// `SpawnDemTerrain`, headless, tests — those carry no `UsdPrimPath`, so they never
/// match here), whose edits apply **directly** to the runtime layer.
///
/// Resolution is uniform: every doc-backed scene — twin default (`--scene` / workspace
/// Twin) and live-imported (`OpenFile`) alike — is a doc-backed twin scene, so the doc
/// is recovered from
/// [`DocBackedTwinScenes`](lunco_usd::twin_projection::DocBackedTwinScenes) via the
/// stage's `twin://<name>/<rel>` asset path. Retries each frame (guarded by
/// `Without<TerrainDocument>`) until the doc mounts; once resolved, it stops.
#[derive(Component)]
struct TerrainDocument {
    /// Raw `DocumentId` of the backing doc (rebuilt as `DocumentId` at the authoring
    /// boundary). The document is the edit authority; edits author there and project in.
    doc: u64,
}

/// Monotonic suffix for authored edit prim names (`edit_<n>` / `rock_<n>`), unique per
/// session so a removed edit's name is never reused. Starts at 0 but is re-seeded past
/// any existing children at every authoring site ([`seed_edit_seq_past_children`]) — a
/// runtime overlay restored from `.lunco/runtime/…` carries last session's prims, and
/// reusing a taken name would make the `AddPrim` fail (the edit silently dropped).
#[derive(Resource, Default)]
struct TerrainEditPrimSeq(u64);

/// Advance `seq` past every `edit_<n>` / `rock_<n>` child already present under
/// `terrain_path` in the composed (`base ⊕ runtime`) document, so the next authored
/// name can never collide with a restored or historical prim. Runs at authoring time
/// (not doc-mount time) so it cannot race the `DocumentOpened` runtime-overlay
/// restore; `composed_arc` is memoized by generation, so this is a cheap child walk.
fn seed_edit_seq_past_children(
    registry: &lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>,
    doc: lunco_doc::DocumentId,
    terrain_path: &str,
    seq: &mut TerrainEditPrimSeq,
) {
    let Some(host) = registry.host(doc) else { return };
    let Ok(sdf) = openusd::sdf::Path::new(terrain_path) else { return };
    let composed = host.document().composed_arc();
    for child in composed.prim_children(&sdf) {
        let Some(name) = child.as_str().rsplit('/').next() else { continue };
        for prefix in ["edit_", "rock_"] {
            if let Some(n) = name.strip_prefix(prefix).and_then(|s| s.parse::<u64>().ok()) {
                seq.0 = seq.0.max(n + 1);
            }
        }
    }
}

/// Author one edit onto every **doc-backed** terrain as USD ops on its document's
/// **runtime** layer — non-destructive, ephemeral over the base DEM (Omniverse
/// session-layer pattern): an `AddPrim` for the edit prim + a `SetAttribute` per
/// `LunCoTerrainEditAPI` parameter. `registry.apply` records them to the journal (undo
/// / sync), then the twin projection re-projects the composed `base ⊕ runtime` →
/// `parse_edit` → the one `EditsLayer`. The direct-path observer in
/// lunco-terrain-surface handles document-FREE terrains (`Without<DocBackedTerrain>`),
/// so exactly one path fires per terrain.
fn author_terrain_edit(
    kind: lunco_terrain_surface::EditKind,
    terrains: &Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: &mut lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>,
    seq: &mut TerrainEditPrimSeq,
    journal: Option<&lunco_doc_bevy::JournalResource>,
) {
    for (prim_path, td) in terrains {
        let doc = lunco_doc::DocumentId::new(td.doc);
        seed_edit_seq_past_children(registry, doc, &prim_path.path, seq);
        let name = format!("edit_{}", seq.0);
        seq.0 += 1;
        let edit_prim = format!("{}/{name}", prim_path.path.trim_end_matches('/'));
        // The edit prim + its `LunCoTerrainEditAPI` attributes, on the ephemeral
        // runtime layer (non-destructive), committed as ONE journal change set — so an
        // edit stays a single undo step even though it is now five ops rather than two.
        //
        // The parameters used to be PACKED into one string attribute precisely so that
        // undo could be a single op. That traded a real USD type for an undo trick:
        // nothing validated the string, `allowedTokens` could not constrain the kind,
        // and no other DCC could read it. The change set gives us the atomicity without
        // the encoding.
        let mut ops = vec![lunco_usd::UsdOp::AddPrim {
            edit_target: lunco_usd::LayerId::runtime(),
            parent_path: prim_path.path.clone(),
            name,
            type_name: None,
            reference: None,
        }];
        // Logical names from the USD-free layer crate; `ns_attr` binds them into
        // `lunco:edit:` — the one place that namespace is applied.
        for (attr, ty, value) in lunco_terrain_surface::edit_attr_writes(&kind) {
            ops.push(lunco_usd::UsdOp::SetAttribute {
                edit_target: lunco_usd::LayerId::runtime(),
                path: edit_prim.clone(),
                name: ns_attr(NS_EDIT, attr),
                type_name: ty.to_string(),
                value,
            });
        }

        let apply_all = |registry: &mut lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>| {
            for op in ops {
                if let Err(e) = registry.apply(doc, op) {
                    warn!("[terrain-edit] {edit_prim} op rejected — edit may be partial: {e:?}");
                }
            }
        };
        match journal {
            Some(j) => j.change_set("Terrain edit", || apply_all(registry)),
            None => apply_all(registry),
        }
    }
}

fn on_brush_terrain_authored(
    trigger: On<lunco_terrain_surface::BrushTerrain>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let Some(mut registry) = registry else { return };
    author_terrain_edit(
        lunco_terrain_surface::EditKind::Brush {
            center: [ev.x as f64, ev.z as f64],
            radius: ev.radius as f64,
            amplitude: ev.amplitude as f64,
        },
        &terrains,
        &mut registry,
        &mut seq,
        journal.as_deref(),
    );
}

fn on_flatten_terrain_authored(
    trigger: On<lunco_terrain_surface::FlattenTerrain>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let Some(mut registry) = registry else { return };
    author_terrain_edit(
        lunco_terrain_surface::EditKind::Flatten {
            center: [ev.x as f64, ev.z as f64],
            radius: ev.radius as f64,
            target_y: ev.target_y as f64,
        },
        &terrains,
        &mut registry,
        &mut seq,
        journal.as_deref(),
    );
}

fn on_place_crater_authored(
    trigger: On<lunco_terrain_surface::PlaceCrater>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let Some(mut registry) = registry else { return };
    author_terrain_edit(
        lunco_terrain_surface::EditKind::Crater {
            center: [ev.x as f64, ev.z as f64],
            radius: ev.radius as f64,
            depth: ev.depth_or_default(),
        },
        &terrains,
        &mut registry,
        &mut seq,
        journal.as_deref(),
    );
}

/// Doc-backed manual rock placement: author ONE `lunco:layer = "rock"` child prim
/// (x/z/size/seed attrs) on the runtime layer. The stack re-parse picks it up via
/// the `rock` parser — a single addressable boulder, removable by its prim path.
fn on_place_rock_authored(
    trigger: On<lunco_terrain_surface::PlaceRock>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    mut seq: ResMut<TerrainEditPrimSeq>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let ev = trigger.event();
    let Some(mut registry) = registry else { return };
    for (prim_path, td) in &terrains {
        let doc = lunco_doc::DocumentId::new(td.doc);
        seed_edit_seq_past_children(&registry, doc, &prim_path.path, &mut seq);
        let name = format!("rock_{}", seq.0);
        seq.0 += 1;
        let rock_prim = format!("{}/{name}", prim_path.path.trim_end_matches('/'));

        // `LunCoTerrainLayerAPI`. Namespaced, not bare: a bare `size` here is
        // `UsdGeomCube`'s real `double size` under a different meaning. `ns_attr` is
        // the one place the namespace is applied, and it checks the schema declares it.
        let mut ops = vec![lunco_usd::UsdOp::AddPrim {
            edit_target: lunco_usd::LayerId::runtime(),
            parent_path: prim_path.path.clone(),
            name,
            type_name: None,
            reference: None,
        }];
        let attrs: [(&str, &str, String); 5] = [
            ("lunco:layer", "token", "\"rock\"".to_string()),
            (&ns_attr(NS_LAYER, "x"), "float", format!("{}", ev.x)),
            (&ns_attr(NS_LAYER, "z"), "float", format!("{}", ev.z)),
            (&ns_attr(NS_LAYER, "size"), "float", format!("{}", ev.size_or_default())),
            (
                &ns_attr(NS_LAYER, "seed"),
                "int64",
                format!("{}", ev.seed_or_default() as i64),
            ),
        ];
        for (attr, ty, value) in attrs {
            ops.push(lunco_usd::UsdOp::SetAttribute {
                edit_target: lunco_usd::LayerId::runtime(),
                path: rock_prim.clone(),
                name: attr.to_string(),
                type_name: ty.to_string(),
                value,
            });
        }

        // ONE change set: a rock is one undo step, not six. (It used to apply each op
        // on its own, so undo peeled a rock apart attribute by attribute.)
        let apply_all = |registry: &mut lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>| {
            for op in ops {
                if let Err(e) = registry.apply(doc, op) {
                    warn!("[terrain-edit] {rock_prim} op rejected — rock may be partial: {e:?}");
                }
            }
        };
        match journal.as_deref() {
            Some(j) => j.change_set("Place rock", || apply_all(&mut registry)),
            None => apply_all(&mut registry),
        }
    }
}

/// Remove a doc-backed terrain edit by authoring a `RemovePrim` of its edit prim — the
/// removal `id` IS the prim path. Document-free removal is handled directly in
/// lunco-terrain-surface. Applies to the doc that owns the prim; others reject harmlessly.
fn on_remove_terrain_edit_authored(
    trigger: On<lunco_terrain_surface::RemoveTerrainLayer>,
    terrains: Query<&TerrainDocument, With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
) {
    let Some(mut registry) = registry else { return };
    let path = trigger.event().id.clone();
    for td in &terrains {
        let _ = registry.apply(
            lunco_doc::DocumentId::new(td.doc),
            lunco_usd::UsdOp::RemovePrim { edit_target: lunco_usd::LayerId::runtime(), path: path.clone() },
        );
    }
}

fn cache_terrain_document(
    terrains: Query<
        (Entity, &lunco_usd::UsdPrimPath),
        (With<lunco_terrain_surface::DemTerrainSurface>, Without<TerrainDocument>),
    >,
    twin_scenes: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (entity, terrain_path) in &terrains {
        // Recover the backing document from `DocBackedTwinScenes` via the stage's
        // `twin://<name>/<rel>` asset path. Both twin default scenes (`--scene` /
        // workspace Twin) and live-imported (`OpenFile`) scenes are doc-backed twin
        // scenes now, so this one path covers both.
        let doc = asset_server.get_path(terrain_path.stage_handle.id()).and_then(|asset_path| {
            let rel_path = asset_path.path().to_string_lossy();
            let (name, rel) = lunco_assets::split_twin_rel(&rel_path)?;
            twin_scenes.doc_for(name, rel)
        });
        let Some(doc) = doc else {
            continue; // not mounted yet (retry next frame), or document-free.
        };
        info!("[terrain-doc] terrain {entity} → doc {} (DocBackedTerrain attached)", doc.0);
        // `LiveRebuildExempt`: an authored crater/rock/edit is an attribute-only doc
        // change; without this the twin projection would despawn + re-instantiate the
        // terrain (a full DEM re-read) per edit. The exempt marker suppresses that
        // reload; `refresh_docbacked_terrain_from_doc` re-bakes off the registry doc.
        commands.entity(entity).try_insert((
            TerrainDocument { doc: doc.0 },
            lunco_terrain_surface::DocBackedTerrain,
            lunco_usd::twin_projection::LiveRebuildExempt,
        ));
    }
}

/// Last registry-document generation a doc-backed terrain re-baked at, so
/// [`refresh_docbacked_terrain_from_doc`] re-parses only when the document moved.
#[derive(Component)]
struct TerrainDocGeneration(u64);

/// Re-bake a doc-backed DEM terrain from its backing registry document whenever
/// that document's generation advances (an authored crater/rock/edit op). Reads
/// the composed (`base ⊕ runtime`) layer straight from the registry — the source
/// of truth — and re-parses the composable `TerrainLayerStack` in place;
/// `regenerate_dem_layers` then re-stamps off the retained base grid (no GeoTIFF
/// re-read, no entity despawn).
///
/// This is the twin-scene counterpart to the asset-event
/// [`refresh_layered_terrain_layers`] (now document-free only): a doc-backed terrain's
/// `LiveRebuildExempt` marker suppresses the twin stage reload, so the registry
/// generation is the re-bake trigger. One re-bake path keyed on the document, not the
/// projected asset — covering twin default and live-imported (`OpenFile`) scenes alike.
fn refresh_docbacked_terrain_from_doc(
    registry: Option<Res<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    // The live, PCP-composed stage. The terrain layer stack used to be re-parsed
    // from the DOCUMENT's merged `sdf::Data` layers, which meant the terrain
    // parser had to be generic over both a composed stage and a raw authored
    // layer — the very thing that kept the flattened reader alive. The
    // `CanonicalStage` IS the composed document (twin_projection replays every op
    // onto it), so there is one source, and it is the same one everything else
    // projects from.
    stages: NonSend<lunco_usd_bevy::CanonicalStages>,
    parser: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    mut terrains: Query<
        (
            Entity,
            &lunco_usd::UsdPrimPath,
            &TerrainDocument,
            Option<&mut TerrainDocGeneration>,
            Has<lunco_terrain_surface::DemBaseGrid>,
        ),
        With<lunco_terrain_surface::DemTerrainSurface>,
    >,
    mut commands: Commands,
) {
    // Brings the `Document::generation` trait method into scope (method
    // resolution only — the name isn't bound, so it can't clash).
    use lunco_doc::Document as _;
    let Some(registry) = registry else { return };
    for (entity, prim_path, td, tracker, has_base_grid) in &mut terrains {
        let doc = lunco_doc::DocumentId::new(td.doc);
        let Some(host) = registry.host(doc) else { continue };
        let cur_gen = host.document().generation();
        match tracker {
            Some(mut g) => {
                if g.0 == cur_gen {
                    continue; // document unchanged since our last re-bake
                }
                g.0 = cur_gen; // live edit — re-bake from composed below
            }
            None => {
                // First sight. The initial bridge parse (`bridge_usd_dem_terrain`) read
                // the BASE stage only, so a runtime overlay restored from
                // `.lunco/runtime/…` on `DocumentOpened` (e.g. a crater/rock layer the
                // user disabled last session) is NOT reflected in the just-built terrain.
                // If such an overlay exists we MUST re-bake from the composed (base ⊕
                // runtime) doc — otherwise the persisted disable is silently ignored and
                // the terrain shows the base values on every launch. `start_dem_restamp`
                // needs the retained `DemBaseGrid`, so wait for the async DEM build to
                // deposit it before triggering. With no runtime overlay the bridge parse
                // is authoritative → seed + skip (no wasted startup re-stamp).
                let has_runtime_override = host
                    .document()
                    .runtime_data()
                    .iter()
                    .any(|(_, spec)| spec.ty == openusd::sdf::SpecType::Prim);
                if has_runtime_override && !has_base_grid {
                    continue; // retry next frame, once the base grid is built
                }
                commands.entity(entity).try_insert(TerrainDocGeneration(cur_gen));
                if !has_runtime_override {
                    continue; // nothing persisted to re-apply
                }
                // fall through: re-parse composed + insert stack → one startup re-bake
            }
        }
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        let Some(cs) = stages.get(prim_path.stage_handle.id()) else { continue };
        let stack = parse_terrain_layer_stack(&cs.view(), &sdf, &parser);
        // Despawn-safe: a scene reload can despawn this terrain between queue
        // time and apply_deferred — no-op instead of panicking.
        commands.entity(entity).try_insert(stack);
    }
}

/// Author one attribute onto a prim's **runtime** layer (non-destructive override).
fn author_layer_attr(
    registry: &mut lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>,
    doc: lunco_doc::DocumentId,
    path: &str,
    name: &str,
    type_name: &str,
    value: String,
) {
    let _ = registry.apply(
        doc,
        lunco_usd::UsdOp::SetAttribute {
            edit_target: lunco_usd::LayerId::runtime(),
            path: path.to_string(),
            name: name.to_string(),
            type_name: type_name.to_string(),
            value,
        },
    );
}

/// Inspector crater/rock tuning on a **doc-backed** terrain: author the changed params
/// onto its USD `craters`/`rocks` layer prims (runtime layer) rather than mutating the
/// `TerrainLayerStack` directly. The USD mutation then drives everything automatically
/// — the registry document's generation advances → `refresh_docbacked_terrain_from_doc`
/// re-parses the stack from the composed (`base ⊕ runtime`) doc → `start_dem_restamp`
/// re-bakes off the retained base grid (off-thread, debounced; no GeoTIFF re-read). The
/// terrain's `LiveRebuildExempt` marker suppresses the twin whole-scene reload this edit
/// would otherwise trigger. This is the USD-source-of-truth path; the direct
/// `on_obstacle_spec_rebuild_layers` handles only document-free terrains
/// (`Without<DocBackedTerrain>`), so exactly one path fires.
fn on_obstacle_spec_authored(
    trigger: On<lunco_obstacle_field::plugin::UpdateObstacleFieldSpec>,
    terrains: Query<(&lunco_usd::UsdPrimPath, &TerrainDocument), With<lunco_terrain_surface::DemTerrainSurface>>,
    registry: Option<ResMut<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
) {
    let Some(mut registry) = registry else { return };
    let spec = &trigger.event().spec;
    // The USD crater/rock layer parsers use `density > 0` as the on/off signal
    // (`parse_crater_layer`/`parse_rock_layer` drop the layer at density ≤ 0), so the
    // Inspector's `enabled` checkbox must fold into the authored density here — else an
    // unchecked-but-nonzero layer re-parses as still-on and stays visible. Author the
    // EFFECTIVE density (0 when disabled); the live in-memory spec keeps the real value,
    // so re-checking restores it within the session.
    let crater_density = if spec.craters.enabled { spec.craters.density } else { 0.0 };
    let rock_density = if spec.rocks.enabled { spec.rocks.density } else { 0.0 };
    for (prim_path, td) in &terrains {
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else { continue };
        let doc = lunco_doc::DocumentId::new(td.doc);
        // Enumerate the terrain's child layer prims from the composed (base ⊕ runtime)
        // document — the stage asset no longer carries a flattened reader. `composed()`
        // is owned, so the registry borrow ends here and `author_layer_attr` below can
        // take it mutably.
        let Some(composed) = registry.host(doc).map(|h| h.document().composed()) else { continue };
        let layers: Vec<(String, String)> = composed
            .prim_children(&sdf)
            .into_iter()
            .filter_map(|child| {
                composed
                    .prim_attribute_value::<String>(&child, "lunco:layer")
                    .map(|ty| (child.as_str().to_string(), ty))
            })
            .collect();
        for (path, layer_type) in layers {
            match layer_type.as_str() {
                "craters" => {
                    info!("[obstacle-usd] authoring craters density={crater_density} (enabled={}) sizeMode={} seed={:#x} → {path} (doc {})", spec.craters.enabled, spec.craters.size.mode, spec.seed, td.doc);
                    author_layer_attr(&mut registry, doc, &path, "density", "float", crater_density.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMode", "float", spec.craters.size.mode.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMin", "float", spec.craters.size.min.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMax", "float", spec.craters.size.max.to_string());
                    author_layer_attr(&mut registry, doc, &path, "depthRatio", "float", spec.craters.depth_ratio.to_string());
                    author_layer_attr(&mut registry, doc, &path, "rimRatio", "float", spec.craters.rim_height_ratio.to_string());
                    // The u64 seed bit-casts through int64; `parse_crater_layer` casts back
                    // (`s as u64`), so the full Reseed range round-trips. Without this attr
                    // every doc-driven re-parse falls back to the parser default and the
                    // crater layout silently flips between the resource seed and 0xC0FFEE.
                    author_layer_attr(&mut registry, doc, &path, "seed", "int64", (spec.seed as i64).to_string());
                }
                "rocks" => {
                    author_layer_attr(&mut registry, doc, &path, "density", "float", rock_density.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMode", "float", spec.rocks.size.mode.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMin", "float", spec.rocks.size.min.to_string());
                    author_layer_attr(&mut registry, doc, &path, "sizeMax", "float", spec.rocks.size.max.to_string());
                    author_layer_attr(&mut registry, doc, &path, "dynamicFrac", "float", spec.rocks.dynamic_fraction.to_string());
                    author_layer_attr(&mut registry, doc, &path, "seed", "int64", (spec.seed as i64).to_string());
                }
                _ => {}
            }
        }
    }
}

fn bridge_usd_dem_terrain(
    q: Query<(Entity, &lunco_usd::UsdPrimPath), Without<DemBridged>>,
    // Live terrains already realized from a PRIOR instantiation pass. A stage
    // recompose (runtime-overlay restore, doc-backing) hands every prim a fresh
    // ECS entity; the previous pass's terrain survives long enough to double
    // the DEM build. Two live terrains for one authored prim stream two
    // collider rings from two oracles — the rover rides whichever surface is
    // higher (a stale smooth ring over the cratered fresh one reads as
    // "floating over every crater").
    q_prior_terrains: Query<
        (Entity, &lunco_usd::UsdPrimPath),
        Or<(
            With<lunco_terrain_surface::DemTerrainRequest>,
            With<lunco_terrain_surface::DemHeightField>,
        )>,
    >,
    stages: Res<Assets<lunco_usd::UsdStageAsset>>,
    twins: Res<lunco_assets::twin_source::TwinRoots>,
    asset_server: Res<AssetServer>,
    registry: Res<lunco_terrain_surface::TerrainLayerParserRegistry>,
    mut obstacle_spec: ResMut<lunco_obstacle_field::ObstacleFieldSpec>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    for (entity, prim_path) in &q {
        // Read the LIVE canonical stage (built on demand from the asset's recipe)
        // — the source of truth. Wait until it is available before reading attrs.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
                canonical.get_or_build(id, &recipe);
            }
        }
        if canonical.get(id).is_none() {
            // No live stage (asset carries no recipe / build failed) — retry next frame.
            continue;
        }
        let Ok(sdf) = openusd::sdf::Path::new(&prim_path.path) else {
            commands.entity(entity).try_insert(DemBridged);
            continue;
        };
        commands.entity(entity).try_insert(DemBridged); // examined — don't re-scan
        // Newest pass wins: retire any prior terrain realized for this same
        // authored prim (same path + same stage asset). Its LOD tiles, ring
        // tiles, and scatter are reaped by their respective orphan reapers.
        for (prior, prior_path) in &q_prior_terrains {
            if prior != entity
                && prior_path.path == prim_path.path
                && prior_path.stage_handle.id() == prim_path.stage_handle.id()
            {
                warn!(
                    "[usd-dem] retiring duplicate terrain entity {prior} for {} \
                     (superseded by a re-composed instantiation pass)",
                    prim_path.path
                );
                commands.entity(prior).try_despawn();
            }
        }
        // Directory of the scene asset this prim came from (e.g.
        // `twins/moonbase`), used to resolve a relative `demSource` when NO
        // Twin is open — the web autoload path (LoadScene from the staged asset
        // tree) has no `twin://` root, so the DEM is resolved against the
        // scene's own folder instead. `None` for in-memory stages.
        let asset_path = asset_server.get_path(id);
        // The root a relative `demSource` resolves against is the root the SCENE
        // itself came from. Every twin — local or downloaded — is addressed
        // `twin://<name>/<rel>`, and `TwinRoots` maps that name to wherever THIS
        // peer keeps the bytes (a checkout, or a downloaded scenario's cache dir).
        // So one lookup covers both, with no per-origin flag and no `#[cfg]`.
        let scene_dir = asset_path
            .as_ref()
            .and_then(|p| p.path().parent().map(|d| d.to_path_buf()));
        let scene_root = asset_path
            .as_ref()
            .filter(|p| matches!(p.source(), bevy::asset::io::AssetSourceId::Name(_)))
            .and_then(|p| p.path().components().next())
            .and_then(|c| c.as_os_str().to_str())
            .and_then(|name| twins.root_of(name))
            // A scene with no source root (the web autoload path loads from the
            // staged `assets/` tree) resolves against its own folder. That is the
            // scene's real location, not a guess about which twin is open.
            .or_else(|| scene_dir.clone());
        let cs = canonical.get(id).expect("checked above");
        bridge_dem_prim_read(
            &cs.view(), entity, prim_path, &sdf, scene_root.as_deref(),
            &registry, obstacle_spec.bypass_change_detection(), &mut commands,
        );
    }
}

/// The DEM-bridge read body, over the composed read surface ([`UsdRead`]) — reads
/// the authored `lunco:assetMode` / child-layer / anchor attributes off the live
/// [`StageView`](lunco_usd_bevy::StageView) and attaches the terrain request +
/// composed stack + georef. Split out of `bridge_usd_dem_terrain` so the read
/// body can be driven directly by tests.
#[allow(clippy::too_many_arguments)]
fn bridge_dem_prim_read(
    reader: &StageView<'_>,
    entity: Entity,
    prim_path: &lunco_usd::UsdPrimPath,
    sdf: &openusd::sdf::Path,
    scene_root: Option<&std::path::Path>,
    registry: &lunco_terrain_surface::TerrainLayerParserRegistry,
    obstacle_spec: &mut lunco_obstacle_field::spec::ObstacleFieldSpec,
    commands: &mut Commands,
) {
    // A DEM-backed terrain: `lunco:assetMode = "dem"` (or "layered"). Its surface
    // is COMPOSED from child LAYER prims (`lunco:layer = "dem" | "craters" |
    // "rocks" | "shader" | …`) — add a layer by adding a prim. The `dem` (ground)
    // layer supplies the heightmap source + window; the rest stamp/scatter/shade.
    let asset_mode = reader.text(sdf, "lunco:assetMode");
    if !matches!(asset_mode.as_deref(), Some("dem") | Some("layered")) {
        return;
    }

    // The ground (`dem`) layer + the composable stack (craters/rocks/shader/…),
    // parsed from the child layer prims (helpers shared with the live-edit refresh).
    let dem_layer_sdf = find_dem_layer(reader, sdf);
    let stack = parse_terrain_layer_stack(reader, sdf, registry);
    // Seed the Inspector's shared spec from the authored values so the panel opens
    // showing THIS scene's craters/rocks, not the resource defaults (caller passes
    // `bypass_change_detection` so it doesn't look like a runtime edit).
    sync_obstacle_spec_from_usd(reader, sdf, obstacle_spec);

    // DEM/ground parameters live on the `dem` child LAYER prim, as
    // `LunCoTerrainLayerAPI` (`lunco:layer:*`) — one prim, one name.
    //
    // There used to be a fallback chain: bare names (`windowM`, `demSource`) on the
    // dem prim, else `lunco:terrain:*` on the Terrain prim. Two names for one thing,
    // on two different prims, is not back-compat — it is two ways to be right and
    // several ways to be silently wrong, and the bare half collided with core USD
    // (`size`). The namespace split is now by prim: a LAYER prim carries
    // `lunco:layer:*`, the terrain SURFACE carries `lunco:terrain:*`.
    use lunco_terrain_surface::LayerAttrSource;
    let dem = dem_layer_sdf.clone();
    let dem_attrs = dem.as_ref().map(|d| UsdLayerAttrs {
        reader,
        sdf: d.clone(),
        ns: NS_LAYER,
    });
    let attr_f32 = |name: &str| -> Option<f32> { dem_attrs.as_ref()?.get_f32(name) };
    let attr_i32 = |name: &str| -> Option<i32> {
        dem_attrs.as_ref()?.get_i64(name).map(|v| v as i32)
    };
    let attr_bool = |name: &str| -> Option<bool> { dem_attrs.as_ref()?.get_bool(name) };

    let rel = dem_attrs.as_ref().and_then(|a| a.get_asset("demSource"));
    let Some(rel) = rel else {
        warn!("[usd-dem] prim {} is a DEM terrain but has no dem-layer demSource", prim_path.path);
        return;
    };
    // Resolve the DEM source to a byte-readable URI.
    //
    // `demSource` is relative to the root the SCENE came from, and `scene_root` is
    // that root — resolved once by the caller from the scene's own asset path.
    // There is no per-origin branch here: a local twin and a downloaded scenario
    // are both `twin://<name>/<rel>`, and `TwinRoots` maps that name to wherever
    // THIS peer keeps the bytes (a checkout, or the scenario cache dir).
    //
    // Deliberately NO fallback to "whichever twin is open": a client usually has
    // an unrelated local twin open, which would capture the lookup and resolve a
    // downloaded twin's DEM under the wrong root.
    //
    // Native yields an absolute path; the web autoload path stays
    // cache/asset-relative, which is what the wasm DEM reader probes against OPFS.
    let Some(root) = scene_root else {
        warn!("[usd-dem] cannot resolve DEM source '{rel}': the scene has no root directory");
        return;
    };
    // Native gives an absolute path; the web autoload path keeps it
    // cache/asset-relative, which is what the wasm DEM reader probes against OPFS.
    let uri = lunco_assets::asset_path::slashed(root.join(&rel));
    // `windowM` = side length (m) realized at native res. 0 = whole map; >0 = side;
    // absent/negative = a safe 4 km window (avoid an accidental full-map build).
    let half_window = match attr_f32("windowM") {
        Some(w) if w == 0.0 => f64::INFINITY,
        Some(w) if w > 0.0 => (w * 0.5) as f64,
        _ => 2048.0,
    };
    // `targetRes` = visual-quality downsample target (samples/side). ≤ 0 = native.
    let target_res = attr_i32("targetRes")
        .filter(|&r| r > 0)
        .map(|r| r as usize)
        .unwrap_or(0);
    // `lodViz` = stream CDLOD tiles (default ON) vs one static mesh.
    let lod_viz = attr_bool("lodViz").unwrap_or(true);
    // `colliderRing` = stream a per-body collider ring vs one static collider.
    // The static full-DEM collider is Nyquist-gated at the DEM base spacing
    // (~3.9 m), so it fades every crater below ~12 m radius FLAT in physics while
    // the 0.65 m near tiles render deep bowls (rovers visibly float above / sink
    // into what they see). Analytic height-modifier layers (craters / edits /
    // overzoom) live ENTIRELY below that limit — a static collider therefore CANNOT
    // represent them, so whenever the terrain both streams fine visuals (`lodViz`)
    // AND carries such layers, force the ring (it samples the oracle at each tile's
    // own resolution, matching the surface exactly). Only when there are no height
    // layers does the authored attr decide (default = `lodViz`); an explicit
    // `colliderRing = false` still keeps the static collider for a plain DEM.
    let has_height_layers = stack
        .0
        .iter()
        .any(|e| matches!(e.layer.id(), "craters" | "edits" | "overzoom"));
    let collider_ring = if lod_viz && has_height_layers {
        true
    } else {
        attr_bool("colliderRing").unwrap_or(lod_viz)
    };
    // (`detailUpsample` is retired: craters/edits are ANALYTIC modifiers on the
    // surface oracle now, sampled at each consumer's own resolution — grid
    // upscaling has nothing left to buy.)

    let layer_count = stack.0.len();
    commands.entity(entity).try_insert((
        lunco_terrain_surface::DemTerrainRequest {
            uri,
            half_window,
            target_res,
            lod_viz,
            collider_ring,
            with_default_material: false,
        },
        stack,
        lunco_terrain_surface::DemTerrainSurface,
    ));
    // `lunco:terrain:lodFrozen` — a scripted shot pins its LOD once loaded rather
    // than re-selecting under a moving camera. On the SURFACE prim, per the
    // namespace split above (`lunco:terrain:*` here, `lunco:layer:*` on layers).
    if reader.scalar::<bool>(sdf, "lunco:terrain:lodFrozen").unwrap_or(false) {
        commands.entity(entity).try_insert(lunco_terrain_surface::LodFrozen);
        info!("[usd-dem] {} — LOD selection frozen after first load", prim_path.path);
    }
    // Georeference (#5): the `lunco:anchor:*` lat/lon/height anchor + the stage
    // `metersPerUnit`. The terrain math is metres, so a non-1 `metersPerUnit`
    // is recorded but flagged loudly (we don't rescale the DEM). Attach a
    // `TerrainGeoref` whenever any of these are authored.
    let anchor_lat = reader.real(sdf, "lunco:anchor:lat");
    let anchor_lon = reader.real(sdf, "lunco:anchor:lon");
    let anchor_height = reader.real(sdf, "lunco:anchor:height");
    // The BODY is part of the terrain's own georeference: its radius folds into
    // the surface oracle as curvature, so it must come from the document, not
    // from whichever `SiteAnchor` an ECS query happened to yield first (see
    // `TerrainGeoref::body`).
    let anchor_body = reader.scalar::<i32>(sdf, "lunco:anchor:body");
    let meters_per_unit = reader.real(sdf, "metersPerUnit");
    if let Some(mpu) = meters_per_unit {
        if (mpu - 1.0).abs() >= 1e-6 {
            warn!(
                "[usd-dem] prim {} authors metersPerUnit={mpu}; terrain assumes 1 m/unit — \
                 heights/colliders are NOT rescaled",
                prim_path.path
            );
        }
    }
    if anchor_lat.is_some()
        || anchor_lon.is_some()
        || anchor_height.is_some()
        || anchor_body.is_some()
    {
        let georef = lunco_terrain_surface::TerrainGeoref {
            body: anchor_body.unwrap_or(lunco_terrain_surface::DEFAULT_ANCHOR_BODY),
            center_lat_deg: anchor_lat.unwrap_or(0.0),
            center_lon_deg: anchor_lon.unwrap_or(0.0),
            anchor_height_m: anchor_height.unwrap_or(0.0),
            meters_per_unit: meters_per_unit.unwrap_or(1.0),
        };
        commands.entity(entity).try_insert(georef);
        info!(
            "[usd-dem] georef: lat {:.4} lon {:.4} height {:.1} m (mpu {})",
            georef.center_lat_deg, georef.center_lon_deg, georef.anchor_height_m, georef.meters_per_unit
        );
    }
    info!(
        "[usd-dem] bridged layered terrain prim {} → DEM '{rel}' (target_res {target_res}, \
         lod_viz {lod_viz}, collider_ring {collider_ring}, {layer_count} composed layer(s))",
        prim_path.path
    );
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod dem_bridge_tests {
    //! The DEM bridge's authored-attribute contract, exercised through the REAL
    //! read body ([`bridge_dem_prim_read`]) off a live composed stage — the same
    //! path `bridge_usd_dem_terrain` runs, minus the asset-server plumbing a
    //! render-free test cannot (and need not) stand up. Commands are applied to a
    //! real `World`, and the assertions read back the components the projection
    //! actually attached — not intermediate parse values.

    use super::bridge_dem_prim_read;
    use bevy::ecs::world::CommandQueue;
    use bevy::prelude::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};
    use openusd::sdf::Path as SdfPath;

    /// A minimal layered DEM terrain: `lunco:assetMode = "dem"` + a `dem` ground
    /// child layer carrying the `demSource`. `extra` is spliced into the Terrain
    /// prim's body; `layer_extra` into the ground layer prim's body.
    fn dem_scene(extra: &str, layer_extra: &str) -> String {
        format!(
            "#usda 1.0\n(\n    defaultPrim = \"Terrain\"\n)\n\
             def Xform \"Terrain\"\n{{\n\
             \x20   token lunco:assetMode = \"dem\"\n\
             {extra}\
             \x20   def Xform \"ground\"\n    {{\n\
             \x20       token lunco:layer = \"dem\"\n\
             \x20       asset lunco:layer:demSource = @site/heightmap.tif@\n\
             {layer_extra}\
             \x20   }}\n}}\n"
        )
    }

    /// Run the real bridge body for `/Terrain` on a fresh world; returns the
    /// world + entity so each test reads back exactly the components it pins.
    fn bridge(scene: &str) -> (World, Entity) {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", scene))
            .expect("stage builds");
        let view = cs.view();
        let registry = lunco_terrain_surface::TerrainLayerParserRegistry::default();
        let mut spec = lunco_obstacle_field::spec::ObstacleFieldSpec::default();
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let prim_path = lunco_usd::UsdPrimPath {
            path: "/Terrain".to_string(),
            ..Default::default()
        };
        let sdf = SdfPath::new("/Terrain").unwrap();
        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            bridge_dem_prim_read(
                &view,
                entity,
                &prim_path,
                &sdf,
                Some(std::path::Path::new("/twin/moonbase")),
                &registry,
                &mut spec,
                &mut commands,
            );
        }
        queue.apply(&mut world);
        (world, entity)
    }

    #[test]
    fn lod_frozen_attr_attaches_lodfrozen_component() {
        // `lunco:terrain:lodFrozen = true` on the SURFACE prim (the
        // `lunco:terrain:*` half of the namespace split) must come out as the
        // `LodFrozen` component alongside the terrain request — that component
        // is what the streaming selector gates on for a cinematic shot.
        let scene = dem_scene("    bool lunco:terrain:lodFrozen = true\n", "");
        let (world, e) = bridge(&scene);
        assert!(
            world.get::<lunco_terrain_surface::DemTerrainRequest>(e).is_some(),
            "dem terrain still projects a DemTerrainRequest"
        );
        assert!(
            world.get::<lunco_terrain_surface::LodFrozen>(e).is_some(),
            "authored lodFrozen=true must attach LodFrozen"
        );
    }

    #[test]
    fn absent_lod_frozen_attr_leaves_lod_live() {
        let scene = dem_scene("", "");
        let (world, e) = bridge(&scene);
        assert!(
            world.get::<lunco_terrain_surface::DemTerrainRequest>(e).is_some(),
            "the bridge ran (request attached)"
        );
        assert!(
            world.get::<lunco_terrain_surface::LodFrozen>(e).is_none(),
            "no authored lodFrozen ⇒ LOD selection stays live"
        );
    }

    #[test]
    fn lod_frozen_false_is_not_frozen() {
        // An explicit `= false` is the same as absent — only an authored `true`
        // freezes.
        let scene = dem_scene("    bool lunco:terrain:lodFrozen = false\n", "");
        let (world, e) = bridge(&scene);
        assert!(world.get::<lunco_terrain_surface::LodFrozen>(e).is_none());
    }

    #[test]
    fn dem_layer_attrs_project_into_request() {
        // `lunco:layer:*` on the ground layer prim: windowM halves into
        // half_window, targetRes passes through, demSource resolves against the
        // scene root.
        let scene = dem_scene(
            "",
            "        float lunco:layer:windowM = 512\n\
             \x20       int lunco:layer:targetRes = 128\n",
        );
        let (world, e) = bridge(&scene);
        let req = world
            .get::<lunco_terrain_surface::DemTerrainRequest>(e)
            .expect("request attached");
        assert_eq!(req.half_window, 256.0, "windowM = side length ⇒ half_window = windowM/2");
        assert_eq!(req.target_res, 128);
        assert!(
            req.uri.ends_with("site/heightmap.tif") && req.uri.starts_with("/twin/moonbase"),
            "demSource resolves against the scene root, got `{}`",
            req.uri
        );
        // Defaults: lodViz unauthored ⇒ streaming ON.
        assert!(req.lod_viz, "lodViz defaults to true");
    }

    #[test]
    fn lod_viz_false_defaults_collider_ring_off() {
        // `colliderRing` unauthored follows `lodViz` when no forcing applies:
        // a static-mesh terrain (lodViz=false) keeps the static collider.
        let scene = dem_scene("", "        bool lunco:layer:lodViz = false\n");
        let (world, e) = bridge(&scene);
        let req = world
            .get::<lunco_terrain_surface::DemTerrainRequest>(e)
            .expect("request attached");
        assert!(!req.lod_viz);
        assert!(!req.collider_ring, "unauthored colliderRing follows lodViz=false");
    }

    #[test]
    fn explicit_collider_ring_wins_over_lod_viz_default() {
        let scene = dem_scene(
            "",
            "        bool lunco:layer:lodViz = false\n\
             \x20       bool lunco:layer:colliderRing = true\n",
        );
        let (world, e) = bridge(&scene);
        let req = world
            .get::<lunco_terrain_surface::DemTerrainRequest>(e)
            .expect("request attached");
        assert!(req.collider_ring, "authored colliderRing=true overrides the lodViz-follow default");
    }

    #[test]
    fn streaming_terrain_with_height_layers_forces_collider_ring() {
        // The Nyquist rule: lodViz + any analytic height layer (the default
        // overzoom counts) ⇒ the ring is FORCED even if the author says no —
        // a static full-DEM collider cannot represent sub-DEM height layers.
        let scene = dem_scene("", "        bool lunco:layer:colliderRing = false\n");
        let (world, e) = bridge(&scene);
        let req = world
            .get::<lunco_terrain_surface::DemTerrainRequest>(e)
            .expect("request attached");
        assert!(req.lod_viz, "streaming visuals on (default)");
        assert!(
            req.collider_ring,
            "lodViz + height layers must force the collider ring despite colliderRing=false"
        );
    }

    #[test]
    fn anchor_attrs_attach_terrain_georef() {
        let scene = dem_scene(
            "    double lunco:anchor:lat = -26.1332\n\
             \x20   double lunco:anchor:lon = 3.6335\n\
             \x20   double lunco:anchor:height = 1946\n",
            "",
        );
        let (world, e) = bridge(&scene);
        let georef = world
            .get::<lunco_terrain_surface::TerrainGeoref>(e)
            .expect("authored anchor attrs attach TerrainGeoref");
        assert_eq!(georef.center_lat_deg, -26.1332);
        assert_eq!(georef.center_lon_deg, 3.6335);
        assert_eq!(georef.anchor_height_m, 1946.0);
    }

    #[test]
    fn no_anchor_attrs_no_georef() {
        let (world, e) = bridge(&dem_scene("", ""));
        assert!(
            world.get::<lunco_terrain_surface::TerrainGeoref>(e).is_none(),
            "no authored anchor ⇒ no TerrainGeoref (the default is absence, not zeros)"
        );
    }

    #[test]
    fn non_dem_prim_is_ignored() {
        // No `lunco:assetMode` ⇒ the bridge must not attach anything.
        let scene = "#usda 1.0\n(\n    defaultPrim = \"Terrain\"\n)\n\
                     def Xform \"Terrain\"\n{\n    bool lunco:terrain:lodFrozen = true\n}\n";
        let (world, e) = bridge(scene);
        assert!(world.get::<lunco_terrain_surface::DemTerrainRequest>(e).is_none());
        assert!(
            world.get::<lunco_terrain_surface::LodFrozen>(e).is_none(),
            "lodFrozen on a non-terrain prim must not freeze anything"
        );
    }

    #[test]
    fn dem_terrain_without_dem_source_attaches_nothing() {
        // A dem-mode prim whose ground layer lacks `demSource` warns and bails —
        // no half-built request.
        let scene = "#usda 1.0\n(\n    defaultPrim = \"Terrain\"\n)\n\
                     def Xform \"Terrain\"\n{\n\
                     \x20   token lunco:assetMode = \"dem\"\n\
                     \x20   def Xform \"ground\"\n    {\n\
                     \x20       token lunco:layer = \"dem\"\n    }\n}\n";
        let (world, e) = bridge(scene);
        assert!(world.get::<lunco_terrain_surface::DemTerrainRequest>(e).is_none());
    }
}
