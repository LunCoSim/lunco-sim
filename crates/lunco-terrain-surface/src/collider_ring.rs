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
use std::sync::Arc;

use avian3d::prelude::{Collider, RigidBody};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::Grid;
use lunco_core::WorldGrid;
use lunco_terrain_core::{prepare_collider_heights, HeightSource};

use crate::oracle::SurfaceOracle;
use crate::quadtree::{QuadCoord, Quadtree, Square};
use crate::stream_viz::DemHeightField;

/// Canonical quadtree depth the collider tiles are realized at. Fixed → the ring
/// is identical across peers. At a ±4 km DEM, depth 7 → 62.5 m tiles.
const COLLIDER_DEPTH: u8 = 7;
/// Heightfield samples per tile side (independent of visual LOD). 129 over a
/// 62.5 m tile ≈ 0.49 m spacing — fine enough that the crater bowls and synthetic
/// craterlets the rover SEES also exist in what it TOUCHES: the Nyquist gate
/// passes features ≥ ~1.5 m at this spacing (anything smaller is ankle-deep).
/// At the original 3.9 m spacing the gate faded out everything below ~12 m, so
/// rovers drove flat across visually deep bowls.
const COLLIDER_RES: usize = 129;
/// Max height-delta/spacing ratio a collider tile may present (≈ 68° slope).
/// Analytic crater rims sampled onto a coarse heightfield can rasterise to
/// near-vertical steps that flip rover contacts; the monotone min-sweep shaves
/// them to a drivable ramp (collider ONLY — visuals keep the crisp rim).
const COLLIDER_MAX_SLOPE: f64 = 2.5;
/// Determinism lattice (metres) collider heights snap to — peers build
/// byte-identical heightfields from the same oracle.
const COLLIDER_QUANT_STEP: f64 = 1e-3;

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
pub struct ColliderTiles {
    pub map: HashMap<QuadCoord, Entity>,
    /// `surface_key()` of the oracle the resident tiles were baked from. The
    /// terrain's [`DemHeightField`] is **swapped** on layer recompose (craters
    /// added, live edits) — the boot sequence alone swaps it at least once — and
    /// a resident tile is never re-baked by the wanted-set diff, so without this
    /// tether the rover keeps driving the PRE-swap surface (visibly floating
    /// above every crater the recompose added).
    oracle_key: u64,
}

/// In-flight off-thread collider-tile bakes for a terrain. Sampling the oracle
/// (65² points × craters/over-zoom) AND constructing the parry heightfield are
/// both real work — doing them synchronously stalled the frame every time a
/// rover crossed a tile boundary. The main thread now only spawns the finished
/// component; the 3×3 build-ahead ring means the tile under a body always
/// exists before it is needed.
#[derive(Component, Default)]
pub struct PendingColliderBakes(HashMap<QuadCoord, Task<Collider>>);

/// Back-pointer from a spawned collider tile to its owning terrain. Tiles are
/// children of the big_space **grid** (each carries its own `CellCoord`), so they
/// don't die with the terrain entity; [`despawn_orphaned_collider_tiles`] reaps
/// them when the owner is gone (twin reload).
#[derive(Component)]
pub struct ColliderTileOf(pub Entity);

/// Sample the composed surface oracle over a tile `region` into Avian's
/// heightfield layout (`Vec<Vec<f64>>` indexed `[x][z]`, paired with a
/// `(side, 1, side)` scale — Parry centres it at the entity origin). The SAME
/// source the visual tile baker samples, so the bowl the rover hits is the bowl
/// it sees. Heights are conditioned through the core collider firewall
/// (slope-limit + quantize) for contact stability + peer determinism.
fn sample_heights_xz(oracle: &SurfaceOracle, region: Square, res: usize) -> Vec<Vec<f64>> {
    let res = res.max(2);
    let step = region.side() / (res as f64 - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    // Row-major [z*res + x] flat grid for the conditioning pass…
    // Gate synthetic over-zoom detail at the collider's own sample spacing —
    // sub-sample features would rasterise as contact-flipping noise.
    let limited = oracle.detail_limited(step);
    let mut flat = vec![0.0f64; res * res];
    for iz in 0..res {
        let wz = z0 + iz as f64 * step;
        for ix in 0..res {
            let wx = x0 + ix as f64 * step;
            flat[iz * res + ix] = limited.height_at(wx, wz);
        }
    }
    prepare_collider_heights(&mut flat, res, step, COLLIDER_MAX_SLOPE, COLLIDER_QUANT_STEP);
    // …then Avian's [x][z] column layout.
    let mut cols = Vec::with_capacity(res);
    for ix in 0..res {
        let mut col = Vec::with_capacity(res);
        for iz in 0..res {
            col.push(flat[iz * res + ix]);
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
        &mut PendingColliderBakes,
    )>,
) {
    let Ok((grid_entity, grid)) = grids.single() else { return };
    let pool = AsyncComputeTaskPool::get();

    // World positions of the dynamic bodies the ring should cover.
    let foci: Vec<Vec3> = bodies
        .iter()
        .filter(|(rb, _)| matches!(rb, RigidBody::Dynamic))
        .map(|(_, gt)| gt.translation())
        .collect();

    for (terrain, t_gt, hf, ring, mut tiles, mut pending) in &mut terrains {
        let oracle = &hf.0;
        // Oracle swapped (layer recompose / live edit) → every resident tile is
        // stale geometry. Despawn them all and drop in-flight bakes; the wanted
        // set below re-bakes the ring against the CURRENT surface.
        let oracle_key = oracle.surface_key();
        if tiles.oracle_key != oracle_key {
            for (_, ent) in tiles.map.drain() {
                commands.entity(ent).try_despawn();
            }
            pending.0.clear();
            tiles.oracle_key = oracle_key;
        }
        let h = oracle.half_extent() as f64;
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

        // Despawn tiles no longer wanted; drop in-flight bakes for them too.
        tiles.map.retain(|coord, ent| {
            let keep = wanted.contains(coord);
            if !keep {
                commands.entity(*ent).try_despawn();
            }
            keep
        });
        pending.0.retain(|coord, _| wanted.contains(coord));

        // Finalize completed off-thread bakes: spawn the tile entity. Each
        // anchors to its own big_space `CellCoord` (from its world centre);
        // Parry centres the heightfield at that origin.
        let mut done: Vec<(QuadCoord, Collider)> = Vec::new();
        pending.0.retain(|coord, task| match block_on(future::poll_once(&mut *task)) {
            Some(collider) => {
                done.push((*coord, collider));
                false
            }
            None => true,
        });
        for (coord, collider) in done {
            if tiles.map.contains_key(&coord) {
                continue;
            }
            let region = qt.region(coord);
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
            tiles.map.insert(coord, ent);
        }

        // Queue bakes for newly-wanted tiles OFF-THREAD (oracle sampling + parry
        // heightfield build used to stall the frame at every tile-boundary cross).
        for coord in &wanted {
            if tiles.map.contains_key(coord) || pending.0.contains_key(coord) {
                continue;
            }
            let region = qt.region(*coord);
            let res = ring.res;
            let oracle_arc: Arc<SurfaceOracle> = hf.0.clone();
            let task = pool.spawn(async move {
                let heights = sample_heights_xz(&oracle_arc, region, res);
                Collider::heightfield(heights, DVec3::new(side, 1.0, side))
            });
            pending.0.insert(*coord, task);
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

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::parry::query::{Ray, RayCast};
    use lunco_obstacle_field::field::HeightGrid;
    use lunco_terrain_core::{Crater, Craters};
    use crate::quadtree::QuadCoord;

    /// Absolute DEM-like altitude of the flat base — deliberately far from 0 so
    /// any hidden Y-recentering in the heightfield build would show up.
    const BASE_H: f64 = 1945.0;

    /// Downward parry ray in TILE-LOCAL coordinates → surface height at (lx, lz).
    fn surface_y(collider: &Collider, lx: f64, lz: f64) -> f64 {
        let top = BASE_H + 500.0;
        let ray = Ray::new(DVec3::new(lx, top, lz), DVec3::new(0.0, -1.0, 0.0));
        let toi = collider
            .shape()
            .cast_local_ray(&ray, 10_000.0, true)
            .unwrap_or_else(|| panic!("ray at local ({lx},{lz}) missed the tile"));
        top - toi
    }

    /// End-to-end geometry proof for one collider-ring tile: sample an oracle with
    /// a single off-centre analytic crater over a canonical-depth region exactly
    /// the way `update_collider_ring` does, build the same `Collider::heightfield`,
    /// and ray-cast it in tile-local space. Proves (a) [x][z] layout is not
    /// transposed, (b) scale = full side length, (c) heights stay ABSOLUTE
    /// (no Y recentering), (d) the bowl depth survives the collider conditioning.
    #[test]
    fn collider_tile_reproduces_offcenter_crater_in_local_frame() {
        // Root region matching a ±4 km DEM; depth-7 tiles are 62.5 m.
        let h = 4000.0_f64;
        let depth = COLLIDER_DEPTH;
        let mut grid = HeightGrid::new_flat(129, h as f32);
        for v in grid.heights.iter_mut() {
            *v = BASE_H;
        }
        let qt = Quadtree::new(h, depth, 1.0, h);
        // An arbitrary interior tile.
        let coord = QuadCoord { depth, x: 70, z: 45 };
        let region = qt.region(coord);
        let side = region.side();

        // One crater, off-centre in the tile at an AXIS-ASYMMETRIC local offset
        // (+10 in x, −18 in z) so a transposed [z][x] layout puts the bowl at a
        // measurably different spot.
        let (dx, dz) = (10.0, -18.0);
        let crater = Crater {
            center: [region.center[0] + dx, region.center[1] + dz],
            radius: 8.0,
            depth: 2.0,
            rim_height: 0.4,
        };
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(grid),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(Craters::new(vec![crater])),
                content_key: 1,
            }],
        );

        // EXACTLY the runtime bake: sample + condition, then the same collider
        // constructor call as `update_collider_ring`.
        let heights = sample_heights_xz(&oracle, region, COLLIDER_RES);
        let collider = Collider::heightfield(heights, DVec3::new(side, 1.0, side));

        // (c) Far corner: flat base at ABSOLUTE altitude — no recentering.
        let far = surface_y(&collider, 25.0, 25.0);
        assert!(
            (far - BASE_H).abs() < 0.05,
            "flat field should sit at absolute {BASE_H}, got {far} (Y recentered or scaled?)"
        );

        // (a)+(d) Bowl at the crater's true local position.
        let bowl = surface_y(&collider, dx, dz);
        assert!(
            bowl < BASE_H - 1.0,
            "crater bowl missing at local ({dx},{dz}): surface {bowl} vs base {BASE_H}"
        );

        // (a) NOT at the transposed position: a [z][x] mixup would dig here instead.
        let transposed = surface_y(&collider, dz, dx);
        assert!(
            (transposed - BASE_H).abs() < 0.5,
            "surface dips at TRANSPOSED local ({dz},{dx}): {transposed} — heightfield layout is flipped"
        );

        // (b) Sweep: collider surface tracks the oracle within conditioning slack
        // everywhere on a coarse probe lattice (rim shaving allowed near the lip,
        // so tolerate slope-limit slack of one cell's max step there).
        let step = side / (COLLIDER_RES as f64 - 1.0);
        let slack = COLLIDER_MAX_SLOPE * step + 2.0 * COLLIDER_QUANT_STEP;
        for iz in (0..COLLIDER_RES).step_by(8) {
            for ix in (0..COLLIDER_RES).step_by(8) {
                let lx = -region.half + ix as f64 * step;
                let lz = -region.half + iz as f64 * step;
                let expect = HeightSource::height_at(&oracle, region.center[0] + lx, region.center[1] + lz);
                let got = surface_y(&collider, lx, lz);
                assert!(
                    got <= expect + 1e-6 + 2.0 * COLLIDER_QUANT_STEP && got >= expect - slack - 1e-6,
                    "collider/oracle mismatch at local ({lx:.2},{lz:.2}): collider {got}, oracle {expect}"
                );
            }
        }
    }
}
