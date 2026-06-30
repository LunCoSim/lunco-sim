//! Composable **terrain layer stack**.
//!
//! A DEM terrain is built from a *stack of layers*, authored as composable USD child
//! prims (`lunco:layer = "dem" | "craters" | "rocks" | "shader" | …`). Each non-ground
//! layer contributes in one of three ways:
//! - **stamp** height deltas into the working `HeightGrid` (craters, ridges, …) —
//!   runs OFF-THREAD in the DEM build, before the collider/tiles derive;
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
mod rocks;
mod shader;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use lunco_obstacle_field::field::HeightGrid;

use crate::stream_viz::DemHeightField;

pub use craters::{crater_layer, make_crater_layer};
pub use rocks::{rock_layer, TerrainRock};

/// Rebuild the `craters`/`rocks` layers of `stack` from a typed [`ObstacleFieldSpec`]
/// (the Inspector's editable model), preserving every other layer (the surface
/// shader, the ground `dem`, …). Mutating the stack through this trips
/// `Changed<TerrainLayerStack>`, so the terrain's incremental `regenerate_dem_layers`
/// re-bakes — NO full scene reload, so the live world is never duplicated. This is
/// how the obstacle-field Inspector panel drives the composable USD-layer terrain.
pub fn apply_obstacle_spec_to_stack(
    stack: &mut TerrainLayerStack,
    spec: &lunco_obstacle_field::spec::ObstacleFieldSpec,
) {
    // Drop the existing crater/rock layers; keep the rest in order.
    stack.0.retain(|l| !matches!(l.id(), "craters" | "rocks"));
    // The near-field high-fidelity crater overlay extent isn't in the spec — keep
    // the established default (the value the USD `detailRegionM` defaults to).
    const DETAIL_REGION_M: f32 = 400.0;
    if spec.craters.enabled && spec.craters.density > 0.0 {
        stack.0.push(crater_layer(spec.craters, spec.seed, DETAIL_REGION_M));
    }
    if spec.rocks.enabled && spec.rocks.density > 0.0 {
        stack.0.push(rock_layer(spec.rocks, spec.region_half_extent, spec.pattern, spec.seed));
    }
}

/// The composed, ordered layers of a DEM terrain (the non-ground stamp / scatter /
/// shader layers; the `dem` ground layer drives the build itself). Authored as USD
/// child prims; consumed by the build/scatter/regenerate systems.
#[derive(Component, Clone, Default)]
pub struct TerrainLayerStack(pub Vec<Arc<dyn TerrainLayer>>);

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

/// Context handed to [`TerrainLayer::scatter`]: the terrain entity, its height grid
/// (already stamped, so scatter sits correctly in/around craters), and the command +
/// asset handles to spawn children. Assets are `None` headless (server builds
/// colliders only; visuals are client-side).
pub struct LayerScatterCx<'a, 'w, 's> {
    pub terrain: Entity,
    /// The crater-stamped working grid (rocks resolve ground height off this).
    pub grid: &'a HeightGrid,
    /// The pristine pre-crater grid, when retained — a layer that adds its OWN
    /// high-fidelity geometry (the craters overlay mesh) builds on this smooth base
    /// so it doesn't double the already-stamped crater in `grid`.
    pub base_grid: Option<&'a HeightGrid>,
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
    /// Stable id (logging / debugging).
    fn id(&self) -> &'static str;
    /// Stamp height deltas into the working grid (off-thread in the DEM build, and on
    /// the main thread on regenerate). Default: contributes no height.
    fn stamp(&self, _grid: &mut HeightGrid) {}
    /// Scatter entities onto the built surface. Default: scatters nothing.
    fn scatter(&self, _cx: &mut LayerScatterCx) {}
    /// Configure the terrain entity itself — its render material / shader. Runs on the
    /// main thread once the height field + streaming components exist. Default: no-op.
    fn configure(&self, _terrain: Entity, _commands: &mut Commands) {}
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
        (Entity, &DemHeightField, Option<&crate::terrain::DemBaseGrid>, &TerrainLayerStack),
        Without<TerrainLayersApplied>,
    >,
) {
    if q.is_empty() {
        return;
    }
    let mut meshes = meshes;
    let mut materials = materials;
    let mut shader_materials = shader_materials;
    for (entity, dem, base, stack) in &q {
        // `try_insert`: a doc-backed scene reload (E1b) can despawn + re-instantiate
        // this terrain in the same frame, so the entity may be gone by the time
        // these deferred commands apply — skip silently rather than panic.
        commands.entity(entity).try_insert(TerrainLayersApplied);
        // Material/shader layers configure the terrain entity first…
        for layer in &stack.0 {
            layer.configure(entity, &mut commands);
        }
        // …then scatter layers spawn their entities.
        let mut cx = LayerScatterCx {
            terrain: entity,
            grid: &dem.0,
            base_grid: base.map(|b| &*b.0),
            commands: &mut commands,
            meshes: meshes.as_deref_mut(),
            materials: materials.as_deref_mut(),
            shader_materials: shader_materials.as_deref_mut(),
            asset_server: &asset_server,
        };
        for layer in &stack.0 {
            layer.scatter(&mut cx);
        }
    }
}
