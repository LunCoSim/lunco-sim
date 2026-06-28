//! Bevy plugin for streamed terrain.
//!
//! **M0: inert.** This registers [`TerrainStreamingConfig`] only — no systems,
//! no entities, no behaviour change — mirroring `lunco_terrain::TerrainPlugin`.
//! It establishes the crate's seam in the app so the streaming systems (tile
//! manager + mesh build in M2, collider rings in M3) can be added behind this
//! config without touching the call site again. See `docs/terrain-streaming-PLAN.md`.

use bevy::prelude::*;

/// Tunable streaming parameters. Edited live in the inspector later (M6); for now
/// it is the inert anchor the streaming systems will read.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TerrainStreamingConfig {
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

impl Default for TerrainStreamingConfig {
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
pub struct TerrainStreamingPlugin;

impl Plugin for TerrainStreamingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainStreamingConfig>();
        // M3: spawn a static DEM terrain (mesh + heightfield collider) on the
        // `SpawnDemTerrain` command. See `crate::terrain`.
        crate::terrain::register(app);
        // M7: tile manager (stream by FloatingOrigin) + LOD + per-rover
        // canonical-res collider ring.
    }
}
