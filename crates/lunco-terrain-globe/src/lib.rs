//! Terrain generation, QuadSphere tiling, and collision for LunCoSim.
//!
//! This crate provides:
//! - **QuadSphere**: Cube-to-sphere projection and LOD subdivision
//! - **Terrain Tiles**: Procedural mesh generation with height sampling
//! - **Collision**: Avian3D integration for physics interaction
//!
//! Terrain is split into two layers:
//! - **Layer 2 (Domain)**: Tile definitions, collision shapes (always loaded, server + client)
//! - **Layer 3 (Visual)**: Mesh generation, rendering (feature-gated, client only)

use bevy::prelude::*;

pub mod tile;
pub mod quad_sphere;
pub mod registry;

pub use tile::*;
pub use quad_sphere::*;
pub use registry::*;

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
