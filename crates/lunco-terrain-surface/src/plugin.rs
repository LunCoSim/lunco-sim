//! Bevy plugin for streamed terrain.
//!
//! **M0: inert.** This registers [`TerrainSurfaceConfig`] only — no systems,
//! no entities, no behaviour change — mirroring `lunco_terrain_globe::TerrainPlugin`.
//! It establishes the crate's seam in the app so the streaming systems (tile
//! manager + mesh build in M2, collider rings in M3) can be added behind this
//! config without touching the call site again. See `docs/terrain-streaming-PLAN.md`.

use bevy::prelude::*;

/// Tunable streaming parameters. Edited live in the inspector later (M6); for now
/// it is the inert anchor the streaming systems will read.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TerrainSurfaceConfig {
    /// Tile edge in metres. MUST be ≤ the big_space cell edge so a tile never
    /// straddles a cell boundary (see crate docs / Part F.2).
    pub tile_size_m: f64,
    /// Chebyshev radius (in tiles) of the resident visual ring around the viewer.
    pub load_radius_tiles: i32,
    /// Chebyshev radius (in tiles) of the high-res **physics collider** ring
    /// around dynamic bodies. Independent of visual LOD — kept small and at a
    /// canonical resolution so contact is deterministic across peers (Part F.4).
    pub collider_radius_tiles: i32,
    /// Number of LOD steps from the finest (ring centre) outward.
    pub lod_levels: u32,
}

impl Default for TerrainSurfaceConfig {
    fn default() -> Self {
        Self {
            // 128 m ≪ the 2000 m world cell → tiles are plain children of one
            // cell in the common case; well clear of any boundary.
            tile_size_m: 128.0,
            load_radius_tiles: 6,
            collider_radius_tiles: 2,
            lod_levels: 4,
        }
    }
}

/// Streamed-terrain plugin. Inert at M0 (config registration only).
pub struct TerrainSurfacePlugin;

impl Plugin for TerrainSurfacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainSurfaceConfig>();
        app.register_type::<crate::georef::TerrainGeoref>();
        app.register_type::<crate::stream_viz::TerrainShaderMode>();
        // Runtime-tunable LOD knobs (Inspector → "Terrain LOD") + the tile-mesh cache.
        app.init_resource::<crate::stream_viz::TerrainLodConfig>();
        app.register_type::<crate::stream_viz::TerrainLodConfig>();
        app.init_resource::<crate::stream_viz::LodMeshCache>();
        // M3: spawn a static DEM terrain (mesh + heightfield collider) on the
        // `SpawnDemTerrain` command. See `crate::terrain`.
        crate::terrain::register(app);
        // Expose the DEM height field to the API / scripting surface as
        // `query("TerrainHeight", #{x, z})` — analytic height/normal/slope, no
        // raycast. See `crate::query`.
        crate::query::register_terrain_queries(app);
        // P3b: bake DEM-derived surface (rough/AO/hazard) + normal layers off the
        // main thread and bind them onto the terrain `ShaderMaterial`. Inert
        // headless (gated on render assets existing). See `crate::derived_layers`.
        crate::derived_layers::register(app);
        // S3 (visual-only): opt-in camera-driven CDLOD tile streaming for SEEING
        // LODs. Inert unless a DEM is built with `lod_viz`. Physics still rides the
        // static heightfield collider. See `crate::stream_viz`.
        app.init_resource::<crate::stream_viz::LodMaterials>().add_systems(
            Update,
            (
                crate::stream_viz::update_lod_tiles,
                crate::stream_viz::despawn_orphaned_lod_tiles,
            ),
        );
        // M7 (physics): opt-in per-rover canonical-res heightfield COLLIDER ring.
        // Inert unless a DEM is built with `collider_ring`; then it replaces the
        // static collider with deterministic per-tile colliders streamed around the
        // dynamic bodies. See `crate::collider_ring`.
        app.add_systems(
            Update,
            (
                crate::collider_ring::update_collider_ring,
                crate::collider_ring::despawn_orphaned_collider_tiles,
            ),
        );
    }
}
