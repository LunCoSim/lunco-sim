//! Cube-sphere (**globe-scale**) tiling: the pure geometry spine.
//!
//! **This crate is a LIBRARY, not a subsystem — by design.** `TerrainPlugin::build`
//! registers zero systems (only `init_resource` + `register_type`) because this
//! crate owns no behaviour: it owns the cube→sphere projection
//! ([`quad_sphere::cube_to_sphere`]), the camera-driven quadtree LOD selection
//! ([`quad_sphere::subdivide_face`]), the tile mesh builder
//! ([`create_quadsphere_tile_mesh`]), and the tile identity components
//! ([`TerrainTile`], [`TileCoord`]).
//!
//! **The systems that drive it live in [`lunco_celestial::globe_lod`]** —
//! `update_globe_lod`, which is registered and runs every frame — because scene
//! integration (spawn/despawn, grids, textures, appearance intent) needs the
//! bodies, and `lunco-celestial` owns those. `lunco-usd-avian` also queries
//! `TerrainTile`. So: **the tiles you see on a globe from orbit come from here.**
//!
//! Do not confuse this with the **surface**-scale terrain
//! (`lunco-terrain-core` / `-surface` / `-bake`): that is the CDLOD heightfield you
//! drive a rover across. Two different scales, two different systems, both live.
//! The globe↔surface handover is not implemented in either.
//!
//! (An older version of this header claimed the crate was "VESTIGIAL — not wired".
//! That was wrong and would have cost someone the orbital view: the plugin having
//! no systems is not the same as the code having no callers.)
//!
//! ## Known-dead within this crate
//!
//! [`PendingTile`], [`TerrainTileConfig`] and [`registry`]'s `TerrainMapRegistry` /
//! `CustomMap` are leftovers of the abandoned in-crate tiling attempt: they are
//! constructed (`init_resource`) but **never read** by anything, and no system
//! populates them. `update_globe_lod` carries its own params on [`GlobeLod`].
//! They are still `pub` and named across crate boundaries, so removing them is an
//! API change, not a cleanup — see the staleness sweep report.
//!
//! [`lunco_celestial::globe_lod`]: https://docs.rs/lunco-celestial
//! [`GlobeLod`]: https://docs.rs/lunco-celestial

use bevy::prelude::*;

pub mod quad_sphere;
pub mod registry;
pub mod tile;

pub use quad_sphere::*;
pub use registry::*;
pub use tile::*;

/// Terrain tile configuration resource.
#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct TerrainTileConfig {
    pub tile_size_m: f64,
    pub tile_resolution: u32,
    pub grid_radius: i32,
    pub spawn_threshold: f64,
    pub max_lod: u32,
    pub lod_distance_factor: f64,
    pub physics_lod_threshold: u32,
    pub max_tile_entities: usize,
    pub spawn_cooldown_frames: u32,
}

impl Default for TerrainTileConfig {
    fn default() -> Self {
        Self {
            tile_size_m: 500.0,
            tile_resolution: 32,
            grid_radius: 4,
            spawn_threshold: 100_000.0,
            max_lod: 12,
            lod_distance_factor: 2.0,
            physics_lod_threshold: 8,
            max_tile_entities: 2000,
            spawn_cooldown_frames: 10,
        }
    }
}

/// Marker component for a spawned terrain tile entity.
#[derive(Component)]
pub struct TerrainTile;

/// Tile coordinate identifier.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
#[reflect(Component)]
pub struct TileCoord {
    pub body: Entity,
    pub face: u8,
    pub level: u32,
    pub i: i32,
    pub j: i32,
}

/// Pending tile that is being generated asynchronously.
#[derive(Component)]
pub struct PendingTile;

/// Plugin that registers terrain systems.
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainTileConfig>()
            .register_type::<TerrainTileConfig>()
            .register_type::<TileCoord>();
    }
}
