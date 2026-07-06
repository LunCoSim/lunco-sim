//! Composable **terrain layer stack**.
//!
//! A DEM terrain is built from a *stack of layers*, authored as composable USD child
//! prims (`lunco:layer = "dem" | "craters" | "rocks" | "shader" | …`). Each non-ground
//! layer contributes in one of these ways:
//! - **height_modifier** — an ANALYTIC [`HeightModifier`](lunco_terrain_core::HeightModifier)
//!   folded into the terrain's [`SurfaceOracle`](crate::oracle::SurfaceOracle)
//!   (craters, runtime edits). Sampled by the tile baker AND the collider at their
//!   own resolution, so feature crispness is unbounded by any grid;
//! - **stamp** height deltas into the working raster `HeightGrid` — only for layers
//!   that genuinely rasterise; prefer `height_modifier`;
//! - **scatter** entities onto the built surface (rocks, props, …) — main thread;
//! - **configure** the terrain's render material (the surface shader IS a layer).
//!
//! The build / scatter / regenerate systems iterate the per-terrain [`TerrainLayerStack`]
//! uniformly, so **adding a new layer type needs no changes to them**: drop a new file
//! next to [`craters`] / [`rocks`] / [`shader`], implement [`TerrainLayer`], write a
//! `fn(&dyn LayerAttrSource)` parser, and register it under a `lunco:layer` type with
//! [`TerrainLayerAppExt::add_terrain_layer`]. One layer = one file.
//!
//! Parsing is USD-free here (the USD bridge in `lunco-sandbox` wraps a prim reader as
//! a [`LayerAttrSource`]), so this crate stays composition-engine-only. Layers are
//! deterministic from a seed and held as `Arc<dyn TerrainLayer>` (`Send + Sync`) so a
//! stamp layer can be moved into the off-thread bake task.

mod craters;
mod edits;
mod overzoom;
mod rocks;
mod shader;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use lunco_obstacle_field::field::HeightGrid;

use crate::stream_viz::DemHeightField;

pub use craters::{crater_layer, make_crater_layer};
pub use edits::{edit_attr_write, parse_edit, EditKind, EditsLayer, EDIT_ATTR};
pub use rocks::{rock_layer, TerrainRock};

/// Rebuild the `craters`/`rocks` layers of `stack` from a typed [`ObstacleFieldSpec`]
/// (the Inspector's editable model), preserving every other layer (the surface
/// shader, the ground `dem`, …). Mutating the stack through this trips
/// `Changed<TerrainLayerStack>`, so the terrain's off-thread `start_dem_restamp`
/// re-bakes — NO full scene reload, so the live world is never duplicated. This is
/// how the obstacle-field Inspector panel drives the composable USD-layer terrain.
pub fn apply_obstacle_spec_to_stack(
    stack: &mut TerrainLayerStack,
    spec: &lunco_obstacle_field::spec::ObstacleFieldSpec,
) {
    // The Inspector owns a single crater/rock config, so this path legitimately
    // replaces them by kind (multiple same-kind layers come from USD prims, which
    // build a fresh stack and are addressed by their `LayerId` prim path instead).
    stack.0.retain(|e| !matches!(e.layer.id(), "craters" | "rocks"));
    if spec.craters.enabled && spec.craters.density > 0.0 {
        stack.push_layer("craters", crater_layer(spec.craters, spec.seed));
    }
    if spec.rocks.enabled && spec.rocks.density > 0.0 {
        // Rocks scatter across the WHOLE DEM (the layer clamps `f32::MAX` to the grid
        // half-extent), not just the ±region centre — capped to a sane total in the
        // layer so a 16 km map doesn't try to spawn hundreds of thousands of entities.
        //
        // FORCE Uniform sampling here (ignore `spec.pattern`): Poisson-disk fills the
        // ENTIRE region at `min_spacing` (a ~14 M-cell background grid + millions of
        // candidate points over an 8 km map) before subsetting — a ~12 s MAIN-THREAD
        // freeze. Blue-noise spacing is meaningless at full-map scale anyway; Uniform
        // is O(count). Poisson stays available for the small standalone arena path.
        stack.push_layer(
            "rocks",
            rock_layer(spec.rocks, f32::MAX, lunco_obstacle_field::spec::Pattern::Uniform, spec.seed),
        );
    }
}

/// Stable identity of a **layer instance** (distinct from its `kind`). For a layer
/// authored in USD this is its **prim path** — unique, stable, hierarchical, already
/// in hand at the parse walk. For a runtime tool edit it is an **explicit handle**
/// (`edit/brush#3`), which becomes a real prim path once edits are authored as prims.
/// Never a synthesised heuristic: the path or an explicit id, nothing guessed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayerId(pub String);

impl LayerId {
    pub fn new(s: impl Into<String>) -> Self {
        LayerId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for LayerId {
    fn from(s: &str) -> Self {
        LayerId(s.to_string())
    }
}

impl From<String> for LayerId {
    fn from(s: String) -> Self {
        LayerId(s)
    }
}

/// A layer paired with its stable [`LayerId`], in fold order.
#[derive(Clone)]
pub struct LayerEntry {
    pub id: LayerId,
    pub layer: Arc<dyn TerrainLayer>,
}

/// Kind + stack-id of the single consolidated runtime-edits layer (see [`edits`]).
/// All live tool edits fold into this one layer instead of one-per-stroke.
pub const EDITS_LAYER_ID: &str = "edits";

/// The composed, ordered layers of a DEM terrain (the non-ground stamp / scatter /
/// shader layers; the `dem` ground layer drives the build itself). Authored as USD
/// child prims; consumed by the build/scatter/regenerate systems. Each entry carries
/// a stable [`LayerId`] so a specific layer can be addressed (edited / removed /
/// reordered) — several same-kind layers coexist.
#[derive(Component, Clone, Default)]
pub struct TerrainLayerStack(pub Vec<LayerEntry>);

impl TerrainLayerStack {
    /// Append a layer under an explicit identity.
    pub fn push_layer(&mut self, id: impl Into<LayerId>, layer: Arc<dyn TerrainLayer>) {
        self.0.push(LayerEntry { id: id.into(), layer });
    }

    /// Remove the layer with this identity; returns whether one was removed.
    pub fn remove_layer(&mut self, id: &LayerId) -> bool {
        let before = self.0.len();
        self.0.retain(|e| &e.id != id);
        self.0.len() != before
    }

    /// The current consolidated edits layer (a clone), or an empty one.
    fn edits_layer(&self) -> edits::EditsLayer {
        self.0
            .iter()
            .find(|e| e.layer.id() == EDITS_LAYER_ID)
            .and_then(|e| e.layer.as_any())
            .and_then(|a| a.downcast_ref::<edits::EditsLayer>())
            .cloned()
            .unwrap_or_default()
    }

    /// Replace the edits layer entry (dropped when empty). Always placed last so edits
    /// fold on top of the authored layers.
    fn set_edits_layer(&mut self, edits: edits::EditsLayer) {
        self.0.retain(|e| e.layer.id() != EDITS_LAYER_ID);
        if !edits.is_empty() {
            self.push_layer(EDITS_LAYER_ID, Arc::new(edits));
        }
    }

    /// Fold one edit into the single edits layer under a stable id.
    pub fn add_edit(&mut self, id: impl Into<LayerId>, kind: edits::EditKind) {
        let updated = self.edits_layer().with_edit(id.into(), kind);
        self.set_edits_layer(updated);
    }

    /// World footprint `[min_x, min_z, max_x, max_z]` of the edit identified by `id`,
    /// or `None` if it isn't an edit in this stack (e.g. it's a whole layer). Lets an
    /// undo/remove scope its re-bake to only the tiles the edit touched.
    pub fn edit_bounds(&self, id: &LayerId) -> Option<[f64; 4]> {
        self.edits_layer().edit_bounds(id)
    }

    /// Remove an edit (by its id) from the edits layer; returns whether one was removed.
    pub fn remove_edit(&mut self, id: &LayerId) -> bool {
        match self.edits_layer().without(id) {
            Some(updated) => {
                self.set_edits_layer(updated);
                true
            }
            None => false,
        }
    }
}

/// Marker: this terrain's scatter/material layers have been applied (one-shot until a
/// regenerate removes it). Stamp layers run in the DEM build; this gates the rest.
#[derive(Component)]
pub struct TerrainLayersApplied;

/// Marker on every entity a scatter layer spawns, so a regenerate can despawn the
/// whole set generically (independent of which layer produced it).
#[derive(Component)]
pub struct TerrainScatterEntity;

/// USD-free attribute getter handed to a layer parser. The USD bridge implements this
/// over a prim reader so layer parsers (and 3rd-party ones) need no USD dependency.
pub trait LayerAttrSource {
    fn get_f32(&self, name: &str) -> Option<f32>;
    fn get_i64(&self, name: &str) -> Option<i64>;
    fn get_string(&self, name: &str) -> Option<String>;
    fn get_bool(&self, name: &str) -> Option<bool>;
}

/// Context handed to [`TerrainLayer::scatter`]: the terrain entity, its composed
/// surface oracle (base + analytic craters/edits, so scatter sits correctly
/// in/around craters), and the command + asset handles to spawn children. Assets
/// are `None` headless (server builds colliders only; visuals are client-side).
pub struct LayerScatterCx<'a, 'w, 's> {
    pub terrain: Entity,
    /// The composed surface (rocks resolve ground height off this).
    pub oracle: &'a crate::oracle::SurfaceOracle,
    pub commands: &'a mut Commands<'w, 's>,
    pub meshes: Option<&'a mut Assets<Mesh>>,
    pub materials: Option<&'a mut Assets<StandardMaterial>>,
    /// The terrain `ShaderMaterial` store + asset server — a layer that wants its
    /// overlay geometry to match the streamed regolith tiles (the craters overlay)
    /// builds a `terrain_geomorph` material here instead of a `StandardMaterial`.
    pub shader_materials: Option<&'a mut Assets<lunco_materials::ShaderMaterial>>,
    pub asset_server: &'a AssetServer,
}

/// A geometry/material layer on a DEM terrain. Implement + register a parser (in its
/// own file) to add a composable layer type with no changes to the build/scatter/
/// regenerate systems.
pub trait TerrainLayer: Send + Sync + 'static {
    /// Layer **kind** (`"craters"`, `"rocks"`, …) — selects behaviour via the parser
    /// registry; NOT an instance identity (every crater layer returns `"craters"`).
    ///
    // TODO(identity→USD): a layer's *identity* is its source USD prim path, not this
    // kind string. Carry the `SdfPath` from the parser through the projection so a
    // specific layer can be addressed / edited / removed / reordered (a `UsdOp` on that
    // path). This unblocks several same-kind layers (see `retain` in
    // `apply_obstacle_spec_to_stack`, which nukes ALL "craters" to replace one), dynamic
    // tool edits, and the schema-derived inspector. Do NOT synthesise a Rust-side id —
    // the prim path already is unique/stable/dynamic. Post-networking the canonical
    // StageSink projects per-prim by path, so this folds in for free.
    // See docs/architecture/terrain-substrate.md → "Dynamic modification".
    fn id(&self) -> &'static str;
    /// The layer's **analytic** height contribution: a
    /// [`HeightModifier`](lunco_terrain_core::HeightModifier) folded into the
    /// terrain's [`SurfaceOracle`](crate::oracle::SurfaceOracle), sampled by the
    /// tile baker + collider at their own resolution (crispness unbounded by any
    /// grid). `half_extent` is the terrain's half side (metres) — deterministic
    /// placement generators derive their footprint from it. Default: none.
    fn height_modifier(&self, _half_extent: f32) -> Option<crate::oracle::HeightContribution> {
        None
    }
    /// Stamp height deltas into the working raster grid (off-thread in the DEM
    /// build). Only for layers that genuinely rasterise — prefer
    /// [`height_modifier`](Self::height_modifier). Default: contributes no height.
    fn stamp(&self, _grid: &mut HeightGrid) {}
    /// Scatter entities onto the built surface. Default: scatters nothing.
    fn scatter(&self, _cx: &mut LayerScatterCx) {}
    /// Configure the terrain entity itself — its render material / shader. Runs on the
    /// main thread once the height field + streaming components exist. Default: no-op.
    fn configure(&self, _terrain: Entity, _commands: &mut Commands) {}

    /// Downcast hook — `Some(self)` for layers whose concrete type a caller needs to
    /// read back (e.g. the consolidated [`EditsLayer`]). Default: opaque.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}

/// Parses a `lunco:layer` child prim into a layer instance. Returns `None` if the prim
/// disables the layer (e.g. density 0). USD-free via [`LayerAttrSource`].
pub type TerrainLayerParser = fn(&dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>>;

/// Maps a `lunco:layer` type string → its parser. The USD bridge looks up each child
/// layer prim's type here. Defaults to the built-ins (`craters`, `rocks`, `shader`);
/// register more with [`TerrainLayerAppExt::add_terrain_layer`].
#[derive(Resource, Clone)]
pub struct TerrainLayerParserRegistry {
    parsers: HashMap<String, TerrainLayerParser>,
}

impl Default for TerrainLayerParserRegistry {
    fn default() -> Self {
        let mut parsers = HashMap::new();
        parsers.insert("craters".to_string(), craters::parse_crater_layer as TerrainLayerParser);
        parsers.insert("overzoom".to_string(), overzoom::parse_overzoom_layer as TerrainLayerParser);
        parsers.insert("rocks".to_string(), rocks::parse_rock_layer as TerrainLayerParser);
        parsers.insert("shader".to_string(), shader::parse_shader_layer as TerrainLayerParser);
        Self { parsers }
    }
}

impl TerrainLayerParserRegistry {
    /// Parse one child layer prim of the given `lunco:layer` type. `None` if the type
    /// is unknown or the layer is disabled.
    pub fn parse(&self, layer_type: &str, attrs: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
        self.parsers.get(layer_type).and_then(|p| p(attrs))
    }

    /// True if a parser is registered for this `lunco:layer` type.
    pub fn knows(&self, layer_type: &str) -> bool {
        self.parsers.contains_key(layer_type)
    }
}

/// App extension: register a parser for a new composable terrain layer type.
pub trait TerrainLayerAppExt {
    fn add_terrain_layer(&mut self, layer_type: &str, parser: TerrainLayerParser) -> &mut Self;
}

impl TerrainLayerAppExt for App {
    fn add_terrain_layer(&mut self, layer_type: &str, parser: TerrainLayerParser) -> &mut Self {
        self.init_resource::<TerrainLayerParserRegistry>();
        self.world_mut()
            .resource_mut::<TerrainLayerParserRegistry>()
            .parsers
            .insert(layer_type.to_string(), parser);
        self
    }
}

/// Apply the stack's scatter + material layers onto each DEM terrain whose height
/// field is built but not yet applied. Colliders spawn always (headless physics
/// parity); visual meshes only when render assets exist.
#[allow(clippy::too_many_arguments)]
pub fn scatter_terrain_layers(
    mut commands: Commands,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    shader_materials: Option<ResMut<Assets<lunco_materials::ShaderMaterial>>>,
    asset_server: Res<AssetServer>,
    q: Query<
        (Entity, &DemHeightField, &TerrainLayerStack),
        Without<TerrainLayersApplied>,
    >,
) {
    if q.is_empty() {
        return;
    }
    let mut meshes = meshes;
    let mut materials = materials;
    let mut shader_materials = shader_materials;
    for (entity, dem, stack) in &q {
        // `try_insert`: a doc-backed scene reload (E1b) can despawn + re-instantiate
        // this terrain in the same frame, so the entity may be gone by the time
        // these deferred commands apply — skip silently rather than panic.
        commands.entity(entity).try_insert(TerrainLayersApplied);
        // Material/shader layers configure the terrain entity first…
        for entry in &stack.0 {
            entry.layer.configure(entity, &mut commands);
        }
        // …then scatter layers spawn their entities.
        let mut cx = LayerScatterCx {
            terrain: entity,
            oracle: &dem.0,
            commands: &mut commands,
            meshes: meshes.as_deref_mut(),
            materials: materials.as_deref_mut(),
            shader_materials: shader_materials.as_deref_mut(),
            asset_server: &asset_server,
        };
        for entry in &stack.0 {
            entry.layer.scatter(&mut cx);
        }
    }
}
