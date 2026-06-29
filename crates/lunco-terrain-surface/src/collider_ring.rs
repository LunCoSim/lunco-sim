//! Per-rover **physics collider ring** (milestone M7, physics half).
//!
//! Opt-in via `collider_ring` (USD `lunco:terrain:colliderRing`). When on, the
//! single static full-DEM heightfield collider is **suppressed** (replacing it,
//! not augmenting — overlapping heightfields would double-up contacts) and instead
//! a small ring of per-tile `Collider::heightfield`s is streamed around the moving
//! rovers, each sampled from the retained DEM (`DemHeightField`).
//!
//! **Deterministic, decoupled from visual LOD.** Tiles are selected at a single
//! *canonical depth* from each rover's **world position** (not the camera, not a
//! screen metric) — so every peer and the headless server pick the identical tile
//! set and agree on contact (the networking invariant in [`crate::quadtree`]). The
//! collider resolution is fixed (≈ native DEM spacing), independent of how coarse
//! or fine the visual tiles happen to be.
//!
//! v1 maintains a 3×3 block of canonical-depth tiles around each dynamic body
//! (the body's node + its 8 neighbours = build-ahead in every direction), diffed
//! against the resident set each frame. Memory-LRU and `PhysicsHold` build-ahead
//! pause are deferred — at moonbase scale the ring is a handful of tiles.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{Collider, RigidBody};
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_core::WorldGrid;
use lunco_obstacle_field::field::HeightGrid;

use crate::quadtree::{QuadCoord, Quadtree, Square};
use crate::stream_viz::DemHeightField;

/// Canonical quadtree depth the collider tiles are realized at. Fixed → the ring
/// is identical across peers. At a ±4 km DEM, depth 5 → 250 m tiles.
const COLLIDER_DEPTH: u8 = 5;
/// Heightfield samples per tile side (independent of visual LOD). 65 over a 250 m
/// tile ≈ 3.9 m spacing — near the 5 m native DEM, fine enough to drive on.
const COLLIDER_RES: usize = 65;

/// Marker + params: this terrain streams a per-rover collider ring instead of one
/// static heightfield. Inserted by the DEM build when the request set
/// `collider_ring`. Needs the retained [`DemHeightField`] to sample tiles from.
#[derive(Component)]
pub struct TerrainColliderRing {
    /// Canonical depth the ring tiles are realized at.
    pub depth: u8,
    /// Heightfield samples per tile side.
    pub res: usize,
}

impl Default for TerrainColliderRing {
    fn default() -> Self {
        TerrainColliderRing { depth: COLLIDER_DEPTH, res: COLLIDER_RES }
    }
}

/// The collider tiles currently resident for a terrain, keyed by quadtree node.
#[derive(Component, Default)]
pub struct ColliderTiles(pub HashMap<QuadCoord, Entity>);

/// Back-pointer from a spawned collider tile to its owning terrain. Tiles are
/// children of the big_space **grid** (each carries its own `CellCoord`), so they
/// don't die with the terrain entity; [`despawn_orphaned_collider_tiles`] reaps
/// them when the owner is gone (twin reload).
#[derive(Component)]
pub struct ColliderTileOf(pub Entity);

/// Sample the DEM over a tile `region` into Avian's heightfield layout
/// (`Vec<Vec<f64>>` indexed `[x][z]`, paired with a `(side, 1, side)` scale —
/// Parry centres it at the entity origin). Mirrors `HeightGrid::to_avian_heights`
/// so the streamed tiles read identically to the static collider.
fn sample_heights_xz(dem: &HeightGrid, region: Square, res: usize) -> Vec<Vec<f64>> {
    let res = res.max(2);
    let step = region.side() / (res as f64 - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    let mut cols = Vec::with_capacity(res);
    for ix in 0..res {
        let wx = x0 + ix as f64 * step;
        let mut col = Vec::with_capacity(res);
        for iz in 0..res {
            let wz = z0 + iz as f64 * step;
            col.push(dem.height_at(wx as f32, wz as f32) as f64);
        }
        cols.push(col);
    }
    cols
}

/// Per-frame: maintain the collider ring around dynamic bodies for each terrain.
pub fn update_collider_ring(
    mut commands: Commands,
    grids: Query<(Entity, &Grid), With<WorldGrid>>,
    // Dynamic bodies (rovers, wheels, dropped payloads) are the ring foci.
    bodies: Query<(&RigidBody, &GlobalTransform)>,
    mut terrains: Query<(
        Entity,
        &GlobalTransform,
        &DemHeightField,
        &TerrainColliderRing,
        &mut ColliderTiles,
    )>,
) {
    let Ok((grid_entity, grid)) = grids.single() else { return };

    // World positions of the dynamic bodies the ring should cover.
    let foci: Vec<Vec3> = bodies
        .iter()
        .filter(|(rb, _)| matches!(rb, RigidBody::Dynamic))
        .map(|(_, gt)| gt.translation())
        .collect();

    for (terrain, t_gt, hf, ring, mut tiles) in &mut terrains {
        let dem = &hf.0;
        let h = dem.half_extent as f64;
        let nodes = 1u32 << ring.depth;
        let side = (2.0 * h) / nodes as f64;
        // Quadtree only for `region(coord)` (depth/range_factor irrelevant here).
        let qt = Quadtree::new(h, ring.depth, 1.0, h);

        // The canonical-depth node set wanted this frame: each focus's node + its
        // 8 neighbours (3×3 build-ahead), deduped across all bodies.
        let mut wanted: HashSet<QuadCoord> = HashSet::new();
        let inv = t_gt.affine().inverse();
        for f in &foci {
            let local = inv.transform_point3(*f);
            let (lx, lz) = (local.x as f64, local.z as f64);
            if lx.abs() > h || lz.abs() > h {
                continue; // body is off the DEM region
            }
            let cx = (((lx + h) / side).floor() as i64).clamp(0, nodes as i64 - 1);
            let cz = (((lz + h) / side).floor() as i64).clamp(0, nodes as i64 - 1);
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let nx = cx + dx;
                    let nz = cz + dz;
                    if nx < 0 || nz < 0 || nx >= nodes as i64 || nz >= nodes as i64 {
                        continue;
                    }
                    wanted.insert(QuadCoord { depth: ring.depth, x: nx as u32, z: nz as u32 });
                }
            }
        }

        // Despawn tiles no longer wanted.
        tiles.0.retain(|coord, ent| {
            let keep = wanted.contains(coord);
            if !keep {
                commands.entity(*ent).despawn();
            }
            keep
        });

        // Spawn newly-wanted tiles. Each anchors to its own big_space `CellCoord`
        // (from its world centre); Parry centres the heightfield at that origin.
        for coord in &wanted {
            if tiles.0.contains_key(coord) {
                continue;
            }
            let region = qt.region(*coord);
            let heights = sample_heights_xz(dem, region, ring.res);
            let collider = Collider::heightfield(heights, DVec3::new(side, 1.0, side));
            let center = region.center;
            let (cell, local) = grid.translation_to_grid(DVec3::new(center[0], 0.0, center[1]));
            let ent = commands
                .spawn((
                    RigidBody::Static,
                    collider,
                    cell,
                    Transform::from_translation(local),
                    ColliderTileOf(terrain),
                    Name::new(format!("ColliderTile {},{}", coord.x, coord.z)),
                    ChildOf(grid_entity),
                ))
                .id();
            tiles.0.insert(*coord, ent);
        }
    }
}

/// Reap collider tiles whose owning terrain no longer exists (or no longer rings)
/// — e.g. after a twin reload. Tiles are children of the grid, so they don't die
/// with the terrain entity; this is their lifecycle tether (mirrors the LOD-tile
/// reaper in [`crate::stream_viz`]).
pub fn despawn_orphaned_collider_tiles(
    mut commands: Commands,
    tiles: Query<(Entity, &ColliderTileOf)>,
    ringing: Query<(), With<TerrainColliderRing>>,
) {
    for (ent, owner) in &tiles {
        if ringing.get(owner.0).is_err() {
            commands.entity(ent).despawn();
        }
    }
}
