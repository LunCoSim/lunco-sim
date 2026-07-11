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
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<lunco_settings::TerrainSettings>();
        app.init_resource::<TerrainSurfaceConfig>();
        app.register_type::<crate::georef::TerrainGeoref>();
        app.register_type::<crate::stream_viz::TerrainShaderMode>();
        // Runtime-tunable LOD knobs (Inspector → "Terrain LOD") + the tile-mesh cache.
        app.init_resource::<crate::stream_viz::TerrainLodConfig>();
        app.register_type::<crate::stream_viz::TerrainLodConfig>();
        app.init_resource::<crate::stream_viz::LodMeshCache>();
        app.init_resource::<crate::stream_viz::TerrainStreamStatus>();
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
        // Ensure the `ShaderMaterial` asset store exists even in the lean
        // headless server, where the render-gated `MaterialPlugin::<ShaderMaterial>`
        // is absent: `update_lod_tiles` holds `ResMut<Assets<ShaderMaterial>>` and
        // would panic on a missing store. `init_asset` is idempotent, so the GUI's
        // `MaterialPlugin` reuses this same store.
        bevy::asset::AssetApp::init_asset::<lunco_materials::ShaderMaterial>(app);
        app.init_resource::<crate::stream_viz::LodMaterials>().add_systems(
            Update,
            (
                crate::stream_viz::update_lod_tiles,
                crate::stream_viz::animate_tile_reveal,
                // Late-bind: derived maps / shadow cache finish baking seconds
                // after the first tiles exist — patch the cached tile materials
                // in place.
                crate::stream_viz::bind_derived_maps_to_tiles,
                crate::stream_viz::bind_shadow_cache_to_tiles,
                // Change-driven: early-outs unless a `TerrainLodViz` removal
                // event fired this frame (stays in `Update` so its
                // `RemovedComponents` reader drains every frame).
                crate::stream_viz::despawn_orphaned_lod_tiles,
            ),
        );
        // Composable TERRAIN LAYER stack (authored as USD child layer prims; craters
        // stamp into the grid, rocks scatter on the surface). The parser registry maps
        // each `lunco:layer` type → a parser; register more with `App::add_terrain_layer`
        // — no changes to the build/scatter/regen systems. See `crate::terrain_layers`.
        app.init_resource::<crate::terrain_layers::TerrainLayerParserRegistry>();
        app.add_systems(Update, crate::terrain_layers::scatter_terrain_layers);
        // M7 (physics): opt-in per-rover canonical-res heightfield COLLIDER ring.
        // Inert unless a DEM is built with `collider_ring`; then it replaces the
        // static collider with deterministic per-tile colliders streamed around the
        // dynamic bodies. See `crate::collider_ring`.
        app.add_systems(
            Update,
            (
                // AFTER the restamp swap: `finish_dem_restamp` writes the new
                // `DemHeightField` immediately (Mut) but hands the bounded
                // `ColliderDirtyRegion` over via deferred commands. Unordered,
                // the ring could observe the new oracle key with no region in
                // sight and fall back to invalidating the WHOLE ring on every
                // edit; the `.after` also inserts the sync point that makes the
                // region visible the same frame.
                crate::collider_ring::update_collider_ring
                    .after(crate::terrain::finish_dem_restamp),
                // Change-driven: early-outs unless a `TerrainColliderRing`
                // removal event fired this frame.
                crate::collider_ring::despawn_orphaned_collider_tiles,
            ),
        );
        // Freeze the sim while a DEM terrain is still building so rovers don't fall
        // through the not-yet-ready collider (esp. web, where the DEM load is slow).
        // See `collider_ring::hold_physics_until_dem_ready`.
        app.init_resource::<crate::collider_ring::DemBuildPhysicsHold>();
        app.add_systems(Update, crate::collider_ring::hold_physics_until_dem_ready);
    }
}
