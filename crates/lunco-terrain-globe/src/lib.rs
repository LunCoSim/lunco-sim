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
//! [`lunco_celestial::globe_lod`]: https://docs.rs/lunco-celestial

use bevy::prelude::*;

pub mod quad_sphere;
pub mod tile;

pub use quad_sphere::*;
pub use tile::*;

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

/// Plugin that registers terrain systems.
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TileCoord>();
    }
}
