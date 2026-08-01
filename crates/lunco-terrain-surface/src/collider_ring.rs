//! Per-physical-body **physics collider ring** (milestone M7, physics half).
//!
//! Opt-in via `collider_ring` (USD `lunco:terrain:colliderRing`). When on, the
//! single static full-DEM heightfield collider is **suppressed** (replacing it,
//! not augmenting — overlapping heightfields would double-up contacts) and instead
//! a small ring of per-tile `Collider::heightfield`s is streamed around the moving
//! dynamic physical assemblies, each sampled from the retained DEM
//! (`DemHeightField`).
//!
//! **Deterministic, decoupled from visual LOD.** Tiles are selected at a single
//! *canonical depth* from each body's **world position** (not the camera, not a
//! screen metric) — so every peer and the headless server pick the identical tile
//! set and agree on contact (the networking invariant in [`crate::quadtree`]). The
//! collider resolution is fixed (≈ native DEM spacing), independent of how coarse
//! or fine the visual tiles happen to be.
//!
//! v1 maintains the canonical-depth tiles covering each free dynamic body or
//! joint-connected dynamic assembly, plus one tile of build-ahead in every
//! direction. The footprint comes from Avian's runtime collider AABBs plus any
//! runtime contact probes (such as raycast wheels), and the assembly comes from
//! Avian's joint graph; no USD prim, schema, or authoring marker participates in
//! the selection. The global wanted set is deduplicated before it is diffed
//! against the resident set.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use avian3d::prelude::{Collider, ColliderAabb, ColliderOf, RigidBody};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use big_space::prelude::Grid;
use lunco_core::coords::GridPos;
use lunco_core::{on_command, register_commands, Command, WorldGrid};
use lunco_terrain_core::{quantize, HeightSource};

use crate::band::SurfaceBand;
use crate::oracle::SurfaceOracle;
use crate::quadtree::{QuadCoord, Quadtree, Square};
use crate::stream_viz::DemHeightField;

/// Seed values for the collider ring. `for_viz` replaces these with the active
/// visual tile lattice. Keeping the fallback here is useful for reflected/test
/// construction, but production terrain must not have a second resolution
/// contract: a collider sampled on another lattice can put a body above a
/// visible slope or below a visible crater rim.
const COLLIDER_DEPTH: u8 = 8;
const COLLIDER_RES: usize = 49;
/// Determinism lattice (metres) collider heights snap to — peers build
/// byte-identical heightfields from the same oracle. Anchored in the WORLD
/// frame (heights are quantized before the tile-local `origin_y` rebase), so
/// abutting tiles with different datums still agree exactly on shared edges.
const COLLIDER_QUANT_STEP: f64 = 1e-3;

/// Marker + params: this terrain streams a per-body collider ring instead of one
/// static heightfield. Inserted by the DEM build when the request set
/// `collider_ring`. Needs the retained [`DemHeightField`] to sample tiles from.
///
/// **Live-tunable.** The consts above only SEED this; the ring systems read these
/// fields, so editing them (Inspector, reflection API, or a scene authoring them)
/// re-shapes contact at runtime — `invalidate_ring_on_retune` re-bakes the resident
/// tiles so the change applies to the ground already under the wheels, not only to
/// tiles baked later. Contact fidelity is a property of the TERRAIN, which is why
/// it lives here per-entity rather than in a global.
#[derive(Component, Reflect, Debug, Clone, PartialEq)]
#[reflect(Component)]
pub struct TerrainColliderRing {
    /// Canonical depth the ring tiles are realized at.
    pub depth: u8,
    /// Heightfield samples per tile side.
    pub res: usize,
    /// The shared contact-band filter policy — the surface a wheel touches,
    /// floored so it agrees with what the drawn visual leaf carries. Built once
    /// at spawn from the terrain's viz config (see [`Self::for_viz`]), so the
    /// collider and the visual leaf provably sample one band. See
    /// `WHEEL_SINKING_ANALYSIS_v3.md` §4.1/§5(2).
    pub contact_band: SurfaceBand,
}

impl TerrainColliderRing {
    /// Construct from the terrain's visual LOD config + half-extent. The contact
    /// band's floor is the visual leaf's gate: the leaf is at `viz.max_depth`
    /// with `viz.tile_res` samples, so its step is
    /// `(2·half_extent) / 2^max_depth / (tile_res − 1)` and its gate is `2·step`.
    /// The collider's own native gate (`2·collider_step`) is finer, so the floor
    /// picks the coarser visual gate — what the body touches is what the eye
    /// sees, not a finer band the mesh flattens out.
    pub fn for_viz(viz: &crate::stream_viz::TerrainLodViz, half_extent: f64) -> Self {
        let depth = viz.max_depth;
        let res = viz.tile_res.max(2);
        let tile_side = (2.0 * half_extent) / (1u32 << depth) as f64;
        let step = tile_side / (res - 1) as f64;
        TerrainColliderRing {
            depth,
            res,
            // The collider and visual tile now sample exactly the same lattice
            // and therefore use the same band-limit. This is the important
            // invariant; it also avoids a body riding a finer physics surface
            // than the mesh currently shown to the user.
            contact_band: SurfaceBand::contact(step, step),
        }
    }
}

impl Default for TerrainColliderRing {
    fn default() -> Self {
        // Without a viz config we cannot know the visual leaf step, so default
        // to the collider's own native gate (no contact floor). Real terrains
        // construct via [`Self::for_viz`] at spawn; this default exists only for
        // reflection / tests that don't drive the contact invariant.
        let collider_side_factor = 1.0 / (1u32 << COLLIDER_DEPTH) as f64;
        let collider_step = collider_side_factor / (COLLIDER_RES.max(2) - 1) as f64;
        TerrainColliderRing {
            depth: COLLIDER_DEPTH,
            res: COLLIDER_RES,
            contact_band: SurfaceBand::visual(collider_step),
        }
    }
}

/// The collider tiles currently resident for a terrain, keyed by quadtree node.
#[derive(Component, Default)]
pub struct ColliderTiles {
    pub map: HashMap<QuadCoord, Entity>,
    /// Resident tiles invalidated by an oracle swap but **kept covering the
    /// ground** until their replacement bake lands — the fresh `Collider` is
    /// then swapped onto the SAME entity. Despawn-then-async-respawn left
    /// nothing under a driving rover for the bake's frames: it sank into the
    /// surface and the late tile depenetrated it out with an abrupt kick that
    /// read as "hit an invisible wall" on every recompose.
    stale: HashSet<QuadCoord>,
    /// `surface_key()` of the oracle the resident tiles were baked from. The
    /// terrain's [`DemHeightField`] is **swapped** on layer recompose (craters
    /// added, live edits) — the boot sequence alone swaps it at least once — and
    /// a resident tile is never re-baked by the wanted-set diff, so without this
    /// tether the rover keeps driving the PRE-swap surface (visibly floating
    /// above every crater the recompose added).
    oracle_key: u64,
    /// The canonical-depth assembly-ring nodes last frame (sorted). The cheap
    /// gate: when no physics footprint crossed a node boundary the wanted set
    /// is unchanged by construction, so with nothing stale and nothing baking
    /// the whole wanted/diff/queue rebuild is skipped for the frame.
    last_ring_nodes: Vec<QuadCoord>,
}

/// In-flight off-thread collider-tile bakes for a terrain. Sampling the oracle
/// (65² points × craters/over-zoom) AND constructing the parry heightfield are
/// both real work — doing them synchronously stalled the frame every time a
/// rover crossed a tile boundary. The main thread now only spawns the finished
/// component; the 3×3 build-ahead ring means the tile under a body always
/// exists before it is needed.
/// The baked collider travels with its `origin_y` — the tile-centre surface
/// height the heightfield was rebased by — so the spawn site can anchor the
/// tile's `CellCoord` at that same height (mirroring the visual CDLOD tiles).
#[derive(Component, Default)]
pub struct PendingColliderBakes(HashMap<QuadCoord, Task<(Collider, f64)>>);

/// Back-pointer from a spawned collider tile to its owning terrain. Tiles are
/// children of the big_space **grid** (each carries its own `CellCoord`), so they
/// don't die with the terrain entity; [`despawn_orphaned_collider_tiles`] reaps
/// them when the owner is gone (twin reload).
#[derive(Component)]
pub struct ColliderTileOf(pub Entity);

/// Sample the composed surface oracle over a tile `region` into Avian's
/// heightfield layout (`Vec<Vec<f64>>` indexed `[x][z]`, paired with a
/// `(side, 1, side)` scale — Parry centres it at the entity origin). It samples
/// the SAME band-limited source as the visual leaf and only quantizes to the
/// deterministic 1 mm lattice; collision geometry must not silently reshape the
/// visible terrain.
fn sample_heights_xz(
    oracle: &SurfaceOracle,
    region: Square,
    res: usize,
    origin_y: f64,
    band: SurfaceBand,
) -> Vec<Vec<f64>> {
    let res = res.max(2);
    let step = region.side() / (res as f64 - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    // The contact band — the shared filter policy floored at the visual leaf's
    // gate, so what the rover TOUCHES is the band the drawn leaf CARRIES (not a
    // finer band the mesh flattens out → wheel-sinking). Sub-sample features
    // below the gate would rasterise as contact-flipping noise anyway, and the
    // gate rounds the sharp crater rim LIP into a rollable bump — a chassis
    // nosing over an un-rounded lip stopped dead on a ~60° face ("stuck on a
    // wall inside the crater"). See `WHEEL_SINKING_ANALYSIS_v3.md` §4.1/§5(2).
    // Scoped to this collider tile's own square (+ a metre of slack): the crater
    // field gathers the placements over the tile once instead of per lattice
    // point. Values inside the region are identical — the contract of
    // `detail_limited_region` — so the collider still samples exactly the band
    // the visual leaf carries.
    let limited = band.limited_region(oracle, region, 1.0);
    let mut cols = Vec::with_capacity(res);
    for ix in 0..res {
        let wx = x0 + ix as f64 * step;
        let mut col = Vec::with_capacity(res);
        for iz in 0..res {
            let wz = z0 + iz as f64 * step;
            // Quantize the ABSOLUTE height, then rebase. The lattice must be
            // anchored in the shared world frame: quantizing the rebased value
            // snaps each tile to a lattice offset by its OWN `origin_y`, so two
            // tiles sampling the same world point on a shared edge disagreed by
            // up to a lattice step (~1e-4 m measured at 1937 m) — the seam
            // "invisible wall" the adjacency test guards against.
            col.push(quantize(limited.height_at(wx, wz), COLLIDER_QUANT_STEP) - origin_y);
        }
        cols.push(col);
    }
    cols
}

/// Build one tile's heightfield collider — `Collider::heightfield` layout
/// (`[x][z]` columns, parry centres the field at the entity origin) but through
/// parry directly so `FIX_INTERNAL_EDGES` is set: without it every interior
/// triangle edge is a raw convex edge, and a loaded wheel pressing into a steep
/// bowl wall (solver slop penetrates a few mm) catches horizontal contact
/// normals off those edges — the classic tiled-heightfield "invisible bump/wall",
/// load-dependent and worst on crater walls.
fn heightfield_collider(heights: Vec<Vec<f64>>, side: f64) -> Collider {
    use avian3d::parry::shape::{HeightFieldFlags, SharedShape};
    use avian3d::parry::utils::Array2;
    let rows = heights.len();
    let cols = heights.first().map_or(0, Vec::len);
    let data: Vec<f64> = heights.into_iter().flatten().collect();
    debug_assert_eq!(data.len(), rows * cols);
    let grid = Array2::new(rows, cols, data);
    SharedShape::heightfield_with_flags(
        grid,
        DVec3::new(side, 1.0, side),
        HeightFieldFlags::FIX_INTERNAL_EDGES,
    )
    .into()
}

/// Re-bake the resident ring when its shape is retuned.
///
/// `depth`/`res` are read when a tile is BAKED, so a runtime edit would otherwise
/// apply only to tiles baked afterwards — the ground already under the wheels would
/// keep the old lattice, and the two would disagree about where the surface is.
///
/// Marks every resident tile stale rather than despawning: the existing swap path
/// then replaces each collider in place once its replacement bake lands, so the
/// rover is never left standing over a hole (that despawn-then-async-respawn gap is
/// what made recomposes feel like "hitting an invisible wall").
pub fn invalidate_ring_on_retune(
    mut q: Query<(&TerrainColliderRing, &mut ColliderTiles), Changed<TerrainColliderRing>>,
) {
    for (_ring, mut tiles) in &mut q {
        let resident: Vec<QuadCoord> = tiles.map.keys().copied().collect();
        for coord in resident {
            tiles.stale.insert(coord);
        }
    }
}

/// Per-frame: maintain the collider ring around dynamic bodies for each terrain.
/// The edited region + the oracle version it belongs to, handed from
/// `finish_dem_restamp` so [`update_collider_ring`] re-bakes ONLY the ring tiles the
/// edit touched. `bounds` = `[min_x, min_z, max_x, max_z]` terrain-local metres;
/// `None` = whole terrain. `oracle_key` matches the swap it describes (so a stale
/// region can't scope the wrong oracle). Consumed once applied.
#[derive(Component)]
pub struct ColliderDirtyRegion {
    pub bounds: Option<[f64; 4]>,
    pub oracle_key: u64,
}

#[derive(Debug, Clone, Copy)]
struct CachedCollider {
    owner: Entity,
    bounds: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Copy)]
struct CachedBody {
    support_bounds: [f64; 4],
    bounds: [f64; 4],
}

#[derive(Debug, Clone)]
struct CachedAssembly {
    members: Vec<Entity>,
    bounds: [f64; 4],
}

/// Change-driven runtime support index shared by ring selection and readiness.
///
/// Avian updates positions and broad-phase AABBs only when physics changes. The
/// index consumes those change events and keeps the assembled support geometry;
/// terrain consumers therefore never scan every body, collider, or joint on a
/// quiet frame. This is the architectural boundary: the producers expose
/// physics geometry, while terrain only reads the resulting assemblies.
#[derive(Resource, Default)]
pub struct PhysicsSupportCache {
    bodies: HashMap<Entity, CachedBody>,
    colliders: HashMap<Entity, CachedCollider>,
    colliders_by_body: HashMap<Entity, HashSet<Entity>>,
    assemblies: Vec<CachedAssembly>,
    assembly_of: HashMap<Entity, usize>,
    topology_dirty: bool,
}

impl PhysicsSupportCache {
    fn remove_collider(&mut self, entity: Entity, changed_bodies: &mut HashSet<Entity>) {
        let Some(collider) = self.colliders.remove(&entity) else {
            return;
        };
        if let Some(colliders) = self.colliders_by_body.get_mut(&collider.owner) {
            colliders.remove(&entity);
            if colliders.is_empty() {
                self.colliders_by_body.remove(&collider.owner);
            }
        }
        changed_bodies.insert(collider.owner);
    }

    fn update_collider(
        &mut self,
        entity: Entity,
        owner: Entity,
        bounds: Option<[f64; 4]>,
        changed_bodies: &mut HashSet<Entity>,
    ) {
        if let Some(previous) = self
            .colliders
            .insert(entity, CachedCollider { owner, bounds })
        {
            changed_bodies.insert(previous.owner);
            if previous.owner != owner {
                if let Some(colliders) = self.colliders_by_body.get_mut(&previous.owner) {
                    colliders.remove(&entity);
                    if colliders.is_empty() {
                        self.colliders_by_body.remove(&previous.owner);
                    }
                }
            }
        }
        self.colliders_by_body
            .entry(owner)
            .or_default()
            .insert(entity);
        changed_bodies.insert(owner);
    }

    fn recompute_body(&mut self, entity: Entity) {
        let Some(body) = self.bodies.get(&entity).copied() else {
            return;
        };
        let mut bounds = Some(body.support_bounds);
        if let Some(colliders) = self.colliders_by_body.get(&entity) {
            for collider in colliders {
                if let Some(Some(collider_bounds)) = self.colliders.get(collider).map(|c| c.bounds)
                {
                    bounds = Some(union_xz_bounds(bounds, collider_bounds));
                }
            }
        }
        if let Some(body) = self.bodies.get_mut(&entity) {
            body.bounds = bounds.unwrap_or(body.support_bounds);
        }
    }

    fn rebuild_assemblies(&mut self, joints: &JointGraph) {
        let adjacency = joints.adjacency(|entity| self.bodies.contains_key(&entity));
        self.assemblies.clear();
        self.assembly_of.clear();
        let mut seen = HashSet::new();
        let entities: Vec<Entity> = self.bodies.keys().copied().collect();
        for seed in entities {
            if !seen.insert(seed) {
                continue;
            }
            let members = joint_component(seed, &adjacency);
            seen.extend(members.iter().copied());
            let index = self.assemblies.len();
            let mut bounds = None;
            for &member in &members {
                self.assembly_of.insert(member, index);
                if let Some(body) = self.bodies.get(&member) {
                    bounds = Some(union_xz_bounds(bounds, body.bounds));
                }
            }
            if let Some(bounds) = bounds {
                self.assemblies.push(CachedAssembly { members, bounds });
            }
        }
        self.topology_dirty = false;
    }

    fn recompute_assembly(&mut self, index: usize) {
        let Some(assembly) = self.assemblies.get(index) else {
            return;
        };
        let members = assembly.members.clone();
        let mut bounds = None;
        for member in members {
            if let Some(body) = self.bodies.get(&member) {
                bounds = Some(union_xz_bounds(bounds, body.bounds));
            }
        }
        if let Some(bounds) = bounds {
            if let Some(assembly) = self.assemblies.get_mut(index) {
                assembly.bounds = bounds;
            }
        }
    }
}

/// Maintain the physics support index from ECS change detection.
pub fn update_physics_support_cache(
    mut cache: ResMut<PhysicsSupportCache>,
    bodies: Query<
        (
            Entity,
            &RigidBody,
            &avian3d::prelude::Position,
            Option<&avian3d::prelude::Rotation>,
            Option<&lunco_physics::PhysicsSupportFootprint>,
        ),
        Or<(
            Added<RigidBody>,
            Changed<RigidBody>,
            Changed<avian3d::prelude::Position>,
            Changed<avian3d::prelude::Rotation>,
            Added<lunco_physics::PhysicsSupportFootprint>,
            Changed<lunco_physics::PhysicsSupportFootprint>,
        )>,
    >,
    colliders: Query<
        (Entity, &ColliderAabb, Option<&ColliderOf>),
        Or<(
            Added<ColliderAabb>,
            Changed<ColliderAabb>,
            Added<ColliderOf>,
        )>,
    >,
    mut removed_bodies: RemovedComponents<RigidBody>,
    mut removed_colliders: RemovedComponents<ColliderAabb>,
    added_revolute: Query<(), Added<avian3d::prelude::RevoluteJoint>>,
    added_fixed: Query<(), Added<avian3d::prelude::FixedJoint>>,
    added_prismatic: Query<(), Added<avian3d::prelude::PrismaticJoint>>,
    added_spherical: Query<(), Added<avian3d::prelude::SphericalJoint>>,
    added_distance: Query<(), Added<avian3d::prelude::DistanceJoint>>,
    mut removed_revolute: RemovedComponents<avian3d::prelude::RevoluteJoint>,
    mut removed_fixed: RemovedComponents<avian3d::prelude::FixedJoint>,
    mut removed_prismatic: RemovedComponents<avian3d::prelude::PrismaticJoint>,
    mut removed_spherical: RemovedComponents<avian3d::prelude::SphericalJoint>,
    mut removed_distance: RemovedComponents<avian3d::prelude::DistanceJoint>,
    joints: JointGraph,
) {
    let mut changed_bodies = HashSet::new();
    let mut topology_dirty = cache.topology_dirty;

    if added_revolute.iter().next().is_some()
        || added_fixed.iter().next().is_some()
        || added_prismatic.iter().next().is_some()
        || added_spherical.iter().next().is_some()
        || added_distance.iter().next().is_some()
        || removed_revolute.read().count() > 0
        || removed_fixed.read().count() > 0
        || removed_prismatic.read().count() > 0
        || removed_spherical.read().count() > 0
        || removed_distance.read().count() > 0
    {
        topology_dirty = true;
    }

    for entity in removed_colliders.read() {
        cache.remove_collider(entity, &mut changed_bodies);
    }
    for entity in removed_bodies.read() {
        if cache.bodies.remove(&entity).is_some() {
            topology_dirty = true;
            if let Some(colliders) = cache.colliders_by_body.remove(&entity) {
                for collider in colliders {
                    cache.colliders.remove(&collider);
                }
            }
        }
    }

    for (entity, rigid_body, position, rotation, footprint) in &bodies {
        if !matches!(rigid_body, RigidBody::Dynamic) {
            if cache.bodies.remove(&entity).is_some() {
                topology_dirty = true;
            }
            continue;
        }
        let support_bounds =
            runtime_support_xz_bounds(GridPos(position.0), rotation, None, footprint);
        cache
            .bodies
            .entry(entity)
            .and_modify(|body| body.support_bounds = support_bounds)
            .or_insert(CachedBody {
                support_bounds,
                bounds: support_bounds,
            });
        cache.recompute_body(entity);
        changed_bodies.insert(entity);
        if !cache.assembly_of.contains_key(&entity) {
            topology_dirty = true;
        }
    }

    for (collider_entity, aabb, collider_of) in &colliders {
        let owner = collider_of.map_or(collider_entity, |collider_of| collider_of.body);
        let bounds = (aabb.min.is_finite()
            && aabb.max.is_finite()
            && aabb.min.x <= aabb.max.x
            && aabb.min.z <= aabb.max.z)
            .then_some([aabb.min.x, aabb.min.z, aabb.max.x, aabb.max.z]);
        cache.update_collider(collider_entity, owner, bounds, &mut changed_bodies);
    }

    // A body and its first AABB are commonly added in the same frame. Rebuild
    // body bounds after both event streams have been applied so the first
    // assembly projection already includes its colliders.
    for &entity in &changed_bodies {
        cache.recompute_body(entity);
    }

    if changed_bodies.is_empty() && !topology_dirty {
        return;
    }

    cache.topology_dirty = topology_dirty;
    if cache.topology_dirty {
        cache.rebuild_assemblies(&joints);
        return;
    }

    let dirty_assemblies: HashSet<usize> = changed_bodies
        .iter()
        .filter_map(|entity| cache.assembly_of.get(entity).copied())
        .collect();
    for index in dirty_assemblies {
        cache.recompute_assembly(index);
    }
}

/// Whether a node's world [`Square`] overlaps an `[min_x, min_z, max_x, max_z]` box.
fn square_overlaps_aabb(s: Square, a: [f64; 4]) -> bool {
    s.center[0] - s.half <= a[2]
        && s.center[0] + s.half >= a[0]
        && s.center[1] - s.half <= a[3]
        && s.center[1] + s.half >= a[1]
}

/// Return the X/Z footprint of one dynamic body in Avian's world frame.
///
/// `ColliderAabb` is the authoritative broad-phase geometry, so this remains
/// correct as a body rotates or as a runtime-generated collider changes shape.
/// A body can briefly lack a valid AABB while its collider is being registered;
/// during that bounded initialization window its authoritative position is the
/// safe conservative fallback.
fn collider_xz_bounds(
    position: GridPos,
    collider_aabb: Option<&avian3d::prelude::ColliderAabb>,
) -> [f64; 4] {
    match collider_aabb {
        Some(aabb)
            if aabb.min.is_finite()
                && aabb.max.is_finite()
                && aabb.min.x <= aabb.max.x
                && aabb.min.z <= aabb.max.z =>
        {
            [aabb.min.x, aabb.min.z, aabb.max.x, aabb.max.z]
        }
        _ => [position.0.x, position.0.z, position.0.x, position.0.z],
    }
}

/// Extend a body's collision footprint with runtime contact probes.
///
/// Raycast wheels are intentionally not Avian colliders: their contact point is
/// a query origin, not a body shape. They still need terrain coverage. The
/// shared [`lunco_physics::PhysicsSupportFootprint`] is published by the
/// physics model and stores offsets in the body's local frame, so this remains
/// independent of USD identity and works at any world/grid position.
fn contact_footprint_xz_bounds(
    position: GridPos,
    rotation: DQuat,
    footprint: Option<&lunco_physics::PhysicsSupportFootprint>,
) -> Option<[f64; 4]> {
    let mut bounds = None;
    for contact in footprint?.0.iter() {
        let center = position.0 + rotation * contact.local_offset;
        let radius = contact.radius.max(0.0);
        bounds = Some(union_xz_bounds(
            bounds,
            [
                center.x - radius,
                center.z - radius,
                center.x + radius,
                center.z + radius,
            ],
        ));
    }
    bounds
}

/// The complete terrain-support footprint of one dynamic body.
///
/// This is the single geometry contract consumed by both ring selection and the
/// physics-readiness hold. Keeping those consumers on the same footprint makes
/// it impossible to release physics while a wheel's terrain tile is still
/// absent, which is the boundary failure mode for a raycast rover.
fn runtime_support_xz_bounds(
    position: GridPos,
    rotation: Option<&avian3d::prelude::Rotation>,
    collider_aabb: Option<&ColliderAabb>,
    footprint: Option<&lunco_physics::PhysicsSupportFootprint>,
) -> [f64; 4] {
    let bounds = collider_xz_bounds(position, collider_aabb);
    let rotation = rotation.map_or(DQuat::IDENTITY, |rotation| rotation.0);
    contact_footprint_xz_bounds(position, rotation, footprint).map_or(bounds, |contact_bounds| {
        union_xz_bounds(Some(bounds), contact_bounds)
    })
}

/// Union two X/Z footprints.
fn union_xz_bounds(current: Option<[f64; 4]>, next: [f64; 4]) -> [f64; 4] {
    current.map_or(next, |a| {
        [
            a[0].min(next[0]),
            a[1].min(next[1]),
            a[2].max(next[2]),
            a[3].max(next[3]),
        ]
    })
}

/// Add the canonical-depth footprint and one tile of build-ahead to `out`.
///
/// A free body therefore gets the familiar 3×3 ring. A physically jointed
/// assembly gets one bounded ring around its complete runtime footprint rather
/// than one ring for every wheel/link. The caller deduplicates the resulting
/// coordinates across assemblies.
fn push_assembly_ring_nodes(
    qt: &Quadtree,
    depth: u8,
    half_extent: f64,
    node_count: u32,
    bounds: [f64; 4],
    out: &mut Vec<QuadCoord>,
) {
    if bounds[2] < -half_extent
        || bounds[0] > half_extent
        || bounds[3] < -half_extent
        || bounds[1] > half_extent
    {
        return;
    }
    let max_coord = half_extent - f64::EPSILON * half_extent.max(1.0);
    let min_x = bounds[0].max(-half_extent).min(max_coord);
    let min_z = bounds[1].max(-half_extent).min(max_coord);
    let max_x = bounds[2].max(-half_extent).min(max_coord);
    let max_z = bounds[3].max(-half_extent).min(max_coord);
    if min_x > max_x || min_z > max_z {
        return;
    }

    let min_node = qt.node_containing(depth, [min_x, min_z]);
    let max_node = qt.node_containing(depth, [max_x, max_z]);
    let min_x = min_node.x as i64 - 1;
    let min_z = min_node.z as i64 - 1;
    let max_x = max_node.x as i64 + 1;
    let max_z = max_node.z as i64 + 1;
    for z in min_z.max(0)..=max_z.min(node_count as i64 - 1) {
        for x in min_x.max(0)..=max_x.min(node_count as i64 - 1) {
            out.push(QuadCoord {
                depth,
                x: x as u32,
                z: z as u32,
            });
        }
    }
}

pub fn update_collider_ring(
    mut commands: Commands,
    grids: Query<(Entity, &Grid), With<WorldGrid>>,
    cache: Res<PhysicsSupportCache>,
    mut terrains: Query<(
        Entity,
        &DemHeightField,
        &TerrainColliderRing,
        &mut ColliderTiles,
        &mut PendingColliderBakes,
        Option<&ColliderDirtyRegion>,
    )>,
    mut ring_nodes: Local<Vec<QuadCoord>>,
    mut wanted: Local<HashSet<QuadCoord>>,
    mut done: Local<Vec<(QuadCoord, Collider, f64)>>,
) {
    let Ok((grid_entity, grid)) = grids.single() else {
        return;
    };
    let pool = AsyncComputeTaskPool::get();

    // Per-frame heightfield-bake budget, shared across all terrains. On WEB the
    // `AsyncComputeTaskPool` has no threads: a spawned bake is a synchronous
    // future that runs to completion on the MAIN thread the instant it is polled.
    // The initial 3×3 ring per rover is ~a dozen tiles × 161² oracle samples; baking
    // them all in one frame froze the page (the "stuck at 33%" stall). Cap new bakes
    // per frame so the ring fills over a handful of frames instead — the physics-hold
    // keeps rovers pinned until their tile lands, so a few frames' delay is invisible.
    // Native keeps real threads → no cap.
    #[cfg(target_arch = "wasm32")]
    let mut bake_budget: usize = 2;
    #[cfg(not(target_arch = "wasm32"))]
    let mut bake_budget: usize = usize::MAX;

    for (terrain, hf, ring, mut tiles, mut pending, dirty_region) in &mut terrains {
        let oracle = &hf.0;
        let h = oracle.half_extent() as f64;
        let nodes = 1u32 << ring.depth;
        let side = (2.0 * h) / nodes as f64;
        // Quadtree only for the coord↔region maps (range_factor irrelevant here).
        let qt = Quadtree::new(h, ring.depth, 1.0, h);
        // Oracle swapped (layer recompose / live edit) → resident tiles baked from the
        // OLD surface are stale. A BOUNDED edit (matching this oracle version) changed
        // heights only inside its AABB, so re-bake ONLY the tiles overlapping it — tiles
        // outside still sample identical heights and KEEP their collider, so we don't
        // despawn+respawn the whole ring (the broadphase-churn physics spike on a burst).
        // A whole-terrain change (`None`) invalidates the whole ring, as before.
        let oracle_key = oracle.surface_key();
        let oracle_swapped = tiles.oracle_key != oracle_key;
        if oracle_swapped {
            let dirty = dirty_region
                .filter(|d| d.oracle_key == oracle_key)
                .and_then(|d| d.bounds);
            let t = &mut *tiles;
            // Mark — don't despawn. A stale tile keeps supporting the rover until
            // its replacement collider is baked and swapped in place (below).
            for coord in t.map.keys() {
                let hit = match dirty {
                    Some(aabb) => square_overlaps_aabb(qt.region(*coord), aabb),
                    None => true,
                };
                if hit {
                    t.stale.insert(*coord);
                }
            }
            // In-flight bakes for touched tiles sampled the OLD oracle — drop them.
            pending.0.retain(|coord, _| match dirty {
                Some(aabb) => !square_overlaps_aabb(qt.region(*coord), aabb),
                None => false,
            });
            t.oracle_key = oracle_key;
            commands.entity(terrain).try_remove::<ColliderDirtyRegion>();
        }

        // Each assembly's canonical-depth footprint plus one tile of build-ahead.
        // The DEM frame IS the grid frame: a grid-absolute body position is
        // already DEM-local, and no render transform belongs in between.
        ring_nodes.clear();
        for assembly in &cache.assemblies {
            let bounds = assembly.bounds;
            push_assembly_ring_nodes(&qt, ring.depth, h, nodes, bounds, &mut ring_nodes);
        }
        // Sorted so the comparison is order-independent (entity iteration order
        // is not a signal; two assemblies swapping nodes is still "unchanged").
        ring_nodes.sort_unstable_by_key(|c| (c.depth, c.x, c.z));
        ring_nodes.dedup();
        // The cheap per-frame gate: no physics footprint crossed a node boundary → the
        // wanted set is unchanged by construction, and with nothing stale and
        // nothing baking there is no diff to run, no bake to poll, no tile to
        // queue. This was the only remaining ungated per-frame rebuild in the
        // crate. (A retune reshapes the node lattice, so its nodes differ.) A
        // swap frame must fall through even with nothing resident or baking:
        // the swap above can DROP every in-flight bake (map still empty, foci
        // still), and skipping then would never re-queue them — the physics
        // hold waits on those exact tiles.
        if !oracle_swapped
            && *ring_nodes == tiles.last_ring_nodes
            && tiles.stale.is_empty()
            && pending.0.is_empty()
        {
            continue;
        }
        tiles.last_ring_nodes.clone_from(&ring_nodes);

        // The canonical-depth footprint + build-ahead set, already deduplicated
        // across every body and assembly.
        wanted.clear();
        wanted.extend(ring_nodes.iter().copied());

        // Despawn tiles no longer wanted; drop in-flight bakes for them too.
        let t = &mut *tiles;
        t.map.retain(|coord, ent| {
            let keep = wanted.contains(coord);
            if !keep {
                commands.entity(*ent).try_despawn();
                t.stale.remove(coord);
            }
            keep
        });
        pending.0.retain(|coord, _| wanted.contains(coord));

        // Finalize completed off-thread bakes: spawn the tile entity. Each
        // anchors to its own big_space `CellCoord` (from its world centre);
        // Parry centres the heightfield at that origin.
        done.clear();
        let done = &mut *done;
        pending.0.retain(
            |coord, task| match block_on(future::poll_once(&mut *task)) {
                Some((collider, origin_y)) => {
                    done.push((*coord, collider, origin_y));
                    false
                }
                None => true,
            },
        );
        for (coord, collider, origin_y) in done.drain(..) {
            let region = qt.region(coord);
            let center = region.center;
            // Anchor the tile's CellCoord at the same `origin_y` its heightfield
            // was rebased by — the SAME (cell, local) convention the visual tiles
            // use — so the collider surface lands in the render tile's big_space
            // cell rather than ~1945 m below it.
            let (cell, local) =
                grid.translation_to_grid(DVec3::new(center[0], origin_y, center[1]));
            if let Some(&ent) = tiles.map.get(&coord) {
                // Replacement for a stale tile: swap the collider onto the
                // existing entity — the ground never vanishes under the rover.
                // Re-anchor too: an edit can shift the tile-centre datum, so the
                // rebased geometry needs its CellCoord/Transform kept in step.
                if tiles.stale.remove(&coord) {
                    commands.entity(ent).try_insert((
                        collider,
                        cell,
                        Transform::from_translation(local),
                    ));
                }
                continue;
            }
            let ent = commands
                .spawn((
                    RigidBody::Static,
                    collider,
                    cell,
                    Transform::from_translation(local),
                    ColliderTileOf(terrain),
                    Name::new(format!("ColliderTile {},{}", coord.x, coord.z)),
                    // A collider tile is streamed runtime implementation detail,
                    // just like its visual LOD counterpart.  Marking it closes
                    // the ownership boundary for entity-tree invalidation and
                    // automatic port telemetry discovery.
                    lunco_core::SystemManaged,
                    ChildOf(grid_entity),
                ))
                .id();
            tiles.map.insert(coord, ent);
        }

        // Queue bakes for newly-wanted (or stale-resident) tiles OFF-THREAD
        // (oracle sampling + parry heightfield build used to stall the frame at
        // every tile-boundary cross).
        for coord in wanted.iter() {
            if pending.0.contains_key(coord)
                || (tiles.map.contains_key(coord) && !tiles.stale.contains(coord))
            {
                continue;
            }
            // Web: bounded new bakes per frame (see `bake_budget`). Break, don't
            // continue — the remaining wanted tiles are picked up next frame (they
            // stay wanted while the rover sits on them).
            if bake_budget == 0 {
                break;
            }
            bake_budget -= 1;
            let region = qt.region(*coord);
            let res = ring.res;
            let band = ring.contact_band;
            let oracle_arc: Arc<SurfaceOracle> = hf.0.clone();
            let task = pool.spawn(async move {
                // Off-thread body → own Tracy zone.
                let _span = bevy::log::info_span!("collider_ring_tile_bake").entered();
                // Anchor the tile's geometry to its own centre surface height,
                // exactly like the visual CDLOD tiles (`tile_mesh.rs` rebases
                // vertices by `origin_y`, `stream_viz.rs` anchors the CellCoord
                // there). Baking ABSOLUTE heights (~1945 m on the Moon) onto a
                // tile anchored at cell Y=0 put the parry heightfield ~1945 m
                // from its own entity origin; physics bodies fell straight
                // through the visual surface. The `origin_y` travels back out so
                // the spawn site anchors the CellCoord at the same height.
                let origin_y = oracle_arc.height_at(region.center[0], region.center[1]);
                let heights = sample_heights_xz(&oracle_arc, region, res, origin_y, band);
                (heightfield_collider(heights, side), origin_y)
            });
            pending.0.insert(*coord, task);
        }
    }
}

/// Reap collider tiles whose owning terrain no longer exists (or no longer rings)
/// — e.g. after a twin reload. Tiles are children of the grid, so they don't die
/// with the terrain entity; this is their lifecycle tether (mirrors the LOD-tile
/// reaper in [`crate::stream_viz`]).
///
/// Change-driven: a tile only orphans when its owner loses [`TerrainColliderRing`]
/// (component removal or terrain despawn — both emit the removal event), so the
/// per-frame every-tile ownership poll is skipped until one fires. The liveness
/// re-check keeps the exact old semantics for a remove-and-re-add in one frame.
pub fn despawn_orphaned_collider_tiles(
    mut commands: Commands,
    mut removed: RemovedComponents<TerrainColliderRing>,
    tiles: Query<(Entity, &ColliderTileOf)>,
    ringing: Query<(), With<TerrainColliderRing>>,
) {
    let orphaned: HashSet<Entity> = removed.read().collect();
    if orphaned.is_empty() {
        return;
    }
    for (ent, owner) in &tiles {
        if orphaned.contains(&owner.0) && ringing.get(owner.0).is_err() {
            commands.entity(ent).try_despawn();
        }
    }
}

/// The ring node (canonical-depth quadtree coord) covering terrain-local
/// `(x, z)`, or `None` outside the terrain footprint. Agrees with the
/// wanted-set derivation in [`update_collider_ring`] by construction — both
/// are [`Quadtree::node_containing`].
#[cfg(test)]
fn ring_node(half: f64, depth: u8, x: f64, z: f64) -> Option<QuadCoord> {
    if x.abs() > half || z.abs() > half {
        return None;
    }
    Some(Quadtree::new(half, depth, 1.0, half).node_containing(depth, [x, z]))
}

/// Freeze the sim while a DEM terrain is still building, so physics-driven bodies
/// (rovers) don't fall through the not-yet-ready terrain collider.
///
/// The DEM byte-fetch + decode + crater-stamp is ~instant natively but takes
/// **seconds on web** (HTTP-fetching a ~40 MB heightmap and decoding it on the
/// wasm main thread). During that window a `Dynamic` rover has gravity but no
/// ground collider — the ring in [`update_collider_ring`] only streams tiles once
/// [`DemHeightField`] exists — so it free-falls and is gone by the time the
/// collider appears (rovers "missing" on the web moonbase). We hold the transport
/// `Paused` while any [`DemTerrainRequest`](crate::terrain::DemTerrainRequest) is
/// outstanding (it's present from the USD bridge until `finish_dem_builds` removes
/// it — even on a build error, so this can never dead-lock), then release to
/// `Playing` so the rovers settle onto the freshly-built collider.
/// Additionally to the DEM build itself, the hold now covers the **collider-ring
/// warm-up gap**: on a ring terrain the static collider is suppressed, and the
/// per-body ring tiles bake OFF-THREAD after the build lands — releasing physics
/// in that window let a joint-suspension (fully `Dynamic`) rover free-fall,
/// tunnel under the heightfield, and be gone (the raycast rovers merely hovered).
/// The transport stays `Paused` until every `Dynamic` body inside a ring
/// terrain's footprint has a RESIDENT ring tile under it. Never dead-locks:
/// `update_collider_ring` derives its wanted set from the same bodies every
/// frame, so the awaited tiles are exactly the ones already baking.
#[allow(clippy::type_complexity)]
pub fn hold_physics_until_dem_ready(
    building: Query<(), With<crate::terrain::DemTerrainRequest>>,
    rings: Query<(
        &crate::stream_viz::DemHeightField,
        &TerrainColliderRing,
        &ColliderTiles,
    )>,
    cache: Res<PhysicsSupportCache>,
    // Same frame rule as `update_collider_ring`: avian `Position` is the
    // grid-absolute pose the DEM is sampled in. This guard exists to keep a
    // rover pinned until there is ground under it, so asking the question in the
    // WRONG frame is worse than not asking: it answered about a point ~a
    // FloatingOrigin cell away, found a resident tile there, released physics —
    // and the rover fell through the hole it was supposed to be protected from.
    // Broad-phase liveness probe. `ColliderAabb` is a REQUIRED component of
    // `Collider`, so it exists from spawn — but initialised to `INVALID`
    // (min=+∞, max=−∞); avian fills a real AABB only AFTER its prepare/broad-phase
    // pass. Testing mere presence is a no-op; we must test VALIDITY.
    q_live: Query<&avian3d::prelude::ColliderAabb>,
    holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut required_nodes: Local<Vec<QuadCoord>>,
) {
    let Some(mut holds) = holds else { return };
    let mut wait = !building.is_empty();
    if !wait {
        'terrains: for (hf, ring, tiles) in &rings {
            let half = hf.0.half_extent() as f64;
            let qt = Quadtree::new(half, ring.depth, 1.0, half);
            for assembly in &cache.assemblies {
                required_nodes.clear();
                push_assembly_ring_nodes(
                    &qt,
                    ring.depth,
                    half,
                    1u32 << ring.depth,
                    assembly.bounds,
                    &mut required_nodes,
                );
                required_nodes.sort_unstable_by_key(|coord| (coord.depth, coord.x, coord.z));
                required_nodes.dedup();
                if required_nodes.is_empty() {
                    continue; // off this terrain — its ring doesn't apply
                }
                // Gate on avian broad-phase LIVENESS, not `tiles.map` membership.
                // The map gains a coordinate when its tile is QUEUED, one or more
                // frames before avian builds that tile's `ColliderAabb`.
                // Releasing on membership unpaused physics into that gap and a
                // fully-Dynamic (physical) wheel free-fell through the not-yet-live
                // collider — the tunnel. Require every footprint tile to be truly
                // collidable.
                for coord in required_nodes.iter().copied() {
                    let live = tiles.map.get(&coord).is_some_and(|&e| {
                        q_live
                            .get(e)
                            .is_ok_and(|aabb| aabb.min.x.is_finite() && aabb.max.x.is_finite())
                    });
                    if !live {
                        wait = true;
                        break 'terrains;
                    }
                }
            }
        }
    }
    // Gate PHYSICS, not the clock. This suspends rigid-body integration only — the
    // transport, the tick, the epoch and the celestial chain all keep running — so
    // the scene is not born "paused" (the user never has to press play to undo an
    // engine wait) and the planets don't stop while a heightfield bakes. Edge-guarded
    // so the `ResMut` is only dereferenced when the state actually flips.
    if holds.holds(lunco_physics::PhysicsHolds::TERRAIN_READY) != wait {
        holds.set(lunco_physics::PhysicsHolds::TERRAIN_READY, wait);
    }
}

/// Clearance above the surface a rescued assembly's deepest member is placed at.
/// Kept small: over cratered ground the OTHER members already hang higher by the
/// local relief, and every extra metre is drop energy that can tip the vehicle.
const RESCUE_CLEARANCE: f64 = 0.25;

/// All avian joint types, as one connectivity view. Rescue/righting must move a
/// jointed assembly (chassis + wheels) as a unit — teleporting one member tears
/// the assembly and the joint solver wrenches it into a tumble.
#[derive(bevy::ecs::system::SystemParam)]
pub struct JointGraph<'w, 's> {
    revolute: Query<'w, 's, &'static avian3d::prelude::RevoluteJoint>,
    fixed: Query<'w, 's, &'static avian3d::prelude::FixedJoint>,
    prismatic: Query<'w, 's, &'static avian3d::prelude::PrismaticJoint>,
    spherical: Query<'w, 's, &'static avian3d::prelude::SphericalJoint>,
    distance: Query<'w, 's, &'static avian3d::prelude::DistanceJoint>,
}

impl JointGraph<'_, '_> {
    /// Adjacency over the joint edges, restricted to entities `keep` admits
    /// (pass a Dynamic-bodies filter so a joint to a static anchor can't glue
    /// two assemblies together through the ground).
    fn adjacency(&self, keep: impl Fn(Entity) -> bool) -> HashMap<Entity, Vec<Entity>> {
        let mut adj: HashMap<Entity, Vec<Entity>> = HashMap::new();
        let mut link = |a: Entity, b: Entity| {
            if keep(a) && keep(b) {
                adj.entry(a).or_default().push(b);
                adj.entry(b).or_default().push(a);
            }
        };
        self.revolute.iter().for_each(|j| link(j.body1, j.body2));
        self.fixed.iter().for_each(|j| link(j.body1, j.body2));
        self.prismatic.iter().for_each(|j| link(j.body1, j.body2));
        self.spherical.iter().for_each(|j| link(j.body1, j.body2));
        self.distance.iter().for_each(|j| link(j.body1, j.body2));
        adj
    }
}

/// BFS the joint-connected component containing `seed` over `adj`.
fn joint_component(seed: Entity, adj: &HashMap<Entity, Vec<Entity>>) -> Vec<Entity> {
    let mut members = vec![seed];
    let mut seen: HashSet<Entity> = HashSet::from([seed]);
    let mut queue = std::collections::VecDeque::from([seed]);
    while let Some(e) = queue.pop_front() {
        for &n in adj.get(&e).into_iter().flatten() {
            if seen.insert(n) {
                members.push(n);
                queue.push_back(n);
            }
        }
    }
    members
}

/// Clearance the settle lifts a member's CENTRE to above the local surface —
/// generous enough to clear a wheel radius so the drop-in makes contact from
/// ABOVE (a body already below a one-sided heightfield never recovers).
const SETTLE_CLEARANCE: f64 = 0.6;

// A raycast contact is the *wheel axle*, not a rigid tyre volume. Its cast
// starts at the suspension strut top and only supports the chassis once it is
// slightly compressed, so placing the tyre a large distance above the terrain
// leaves the ground outside the spring's rest-length window. Keep the contact
// just below the datum to establish a real normal force on the first step.
const RAYCAST_SETTLE_CLEARANCE: f64 = -0.02;

/// ONE-TIME drop-onto-terrain placement for freshly-activated physical rovers
/// (marked [`lunco_core::NeedsGroundSettle`] in `activate_dynamic_bodies`).
///
/// Authored physical rovers put the chassis at the surface with the wheels hanging
/// below it, so at the authored pose the wheels start EMBEDDED in the one-sided
/// terrain heightfield and sink forever (no upward contact — proven: a rover that
/// DROPS onto the same heightfield rests perfectly). This lifts the whole
/// joint-connected assembly, in the grid-absolute frame avian `Position` lives in,
/// so its lowest member clears the surface, then consumes the marker. The rover
/// then drops the last few cm and rests via normal contacts. It is NOT a per-frame
/// rescue — it fires exactly once per assembly, at activation, and is pure initial
/// PLACEMENT (the same job the command-spawn rest-depth lift does for GUI spawns).
pub fn settle_grounded_assemblies(
    terrains: Query<&crate::stream_viz::DemHeightField, With<TerrainColliderRing>>,
    q_needs: Query<Entity, With<lunco_core::NeedsGroundSettle>>,
    footprints: Query<Option<&lunco_physics::PhysicsSupportFootprint>>,
    mut bodies: Query<(
        Entity,
        &mut avian3d::prelude::Position,
        Option<&mut avian3d::prelude::LinearVelocity>,
        Option<&mut avian3d::prelude::AngularVelocity>,
    )>,
    rotations: Query<&avian3d::prelude::Rotation>,
    dynamics: Query<&RigidBody>,
    joints: JointGraph,
    holds: Option<Res<lunco_physics::PhysicsHolds>>,
    mut commands: Commands,
) {
    if q_needs.is_empty() {
        return;
    }
    // The marker is an initial-placement request, not an estimate.  The DEM
    // height oracle and its collider ring become usable in different frames;
    // consuming it while the terrain-ready hold is active samples the
    // pre-residency pose and leaves raycast wheels outside their cast range.
    // Keep it armed until the same readiness gate that releases physics has
    // confirmed a live surface under every dynamic body.
    if holds.is_some_and(|holds| holds.holds(lunco_physics::PhysicsHolds::TERRAIN_READY)) {
        return;
    }
    // Query the oracle in the SAME grid-absolute frame as avian `Position` (terrain
    // owner is anchored at the grid origin cell). Wait for it if not built yet —
    // the marker persists.
    let Some(hf) = terrains.iter().next() else {
        return;
    };
    let half = hf.0.half_extent() as f64;

    // Pass 1 (read-only): snapshot every body's grid-absolute Position.
    let mut pos_of: HashMap<Entity, GridPos> = HashMap::default();
    for (e, pos, _, _) in bodies.iter() {
        pos_of.insert(e, GridPos(pos.0));
    }
    let adj = joints.adjacency(|e| {
        dynamics
            .get(e)
            .is_ok_and(|rb| matches!(rb, RigidBody::Dynamic))
    });

    let mut done: HashSet<Entity> = HashSet::new();
    for seed in &q_needs {
        if !done.insert(seed) {
            continue;
        }
        let members = joint_component(seed, &adj);
        done.extend(members.iter().copied());
        let mut lift = 0.0_f64;
        let mut over_terrain = false;
        // Probe-only contact geometry belongs to the same
        // placement pass as rigid members. It is authored in the vehicle frame,
        // transformed once by the solved root pose, and sampled from the same
        // oracle as every other terrain consumer.
        // A dynamic physical wheel can be the first `NeedsGroundSettle` seed
        // encountered for a jointed vehicle. The raycast contact footprint,
        // however, belongs to its DriveMix root. Resolve it from the seed and
        // then across the whole assembly instead of assuming the arbitrary
        // dynamic member owns it. Probe-based vehicles MUST use only this geometry:
        // their high chassis has no terrain contact, and its near-zero generic
        // lift used to consume the request before a wheel probe could settle it.
        let raycast_footprint = footprints
            .get(seed)
            .ok()
            .flatten()
            .map(|footprint| (seed, footprint))
            .or_else(|| {
                members.iter().find_map(|member| {
                    footprints
                        .get(*member)
                        .ok()
                        .flatten()
                        .map(|footprint| (*member, footprint))
                })
            });
        if let Some((footprint_owner, footprint)) = raycast_footprint {
            let Some(root_pos) = pos_of.get(&footprint_owner) else {
                continue;
            };
            let root_rot = rotations
                .get(footprint_owner)
                .map(|rotation| rotation.0)
                .unwrap_or(DQuat::IDENTITY);
            for contact in &footprint.0 {
                let point = root_pos.0 + root_rot * contact.local_offset;
                if point.x.abs() > half || point.z.abs() > half {
                    continue;
                }
                let surface = hf.0.height_at(point.x, point.z);
                lift = lift.max(surface + RAYCAST_SETTLE_CLEARANCE - (point.y - contact.radius));
                over_terrain = true;
            }
        } else {
            // Physical wheels are real bodies, so lift from the deepest dynamic
            // member. The chassis (high) never drives it; the low wheels do.
            for &m in &members {
                let Some(p) = pos_of.get(&m) else { continue };
                if p.0.x.abs() > half || p.0.z.abs() > half {
                    continue; // this member is off the terrain footprint
                }
                over_terrain = true;
                let surface = hf.0.height_at(p.0.x, p.0.z);
                lift = lift.max(surface + SETTLE_CLEARANCE - p.0.y);
            }
        }
        if !over_terrain || lift <= 0.0 {
            continue;
        }
        // Consume only after an actual placement.  During terrain/celestial
        // startup the same assembly can be observed before its final
        // grid-absolute pose exists; treating that zero-lift observation as a
        // completed settle loses the sole chance to lift its wheel probes.
        for &m in &members {
            commands
                .entity(m)
                .try_remove::<lunco_core::NeedsGroundSettle>();
        }
        for &m in &members {
            if let Ok((_, mut pos, lin, ang)) = bodies.get_mut(m) {
                pos.0.y += lift;
                if let Some(mut v) = lin {
                    v.0 = DVec3::ZERO;
                }
                if let Some(mut w) = ang {
                    w.0 = DVec3::ZERO;
                }
            }
        }
        warn!(
            "[ground-settle] dropped assembly (seed {seed:?}, {} bodies) onto the terrain: \
             lifted {lift:.2} m so the wheels clear the one-sided heightfield",
            members.len(),
        );
    }
}

/// Right one overturned vessel, NOW — the primitive behind the Recover tool and
/// the rhai `recover::vessel(id)` verb.
///
/// USER-INVOKED ONLY. This used to run itself: a `KeepUpright` marker plus a
/// `FixedUpdate` system (`rescue_overturned_vessels`) that watched every marked
/// vessel, waited 3 s of near-motionless overturned rest, and rotated it upright
/// — up to three times before giving up. That is gone, deliberately. A rover
/// ending up on its roof is *information* about the terrain, the suspension or
/// the driving, and a runtime that quietly undoes it hides the very thing worth
/// looking at; it also fought any scene whose pose is authored elsewhere. When a
/// vessel is stuck, someone now says so.
///
/// Rotates the whole joint-connected assembly upright about the target's own
/// position (shortest arc, so heading is approximately preserved), reseats it
/// [`RESCUE_CLEARANCE`] above the composed surface, and zeroes velocities.
///
/// Operates on avian's f64 `Position`/`Rotation` — under the big_space physics
/// bridge a Dynamic body's `Transform` is a writeback TARGET (overwritten from
/// `Position` next step), so poses must be written through `Position`.
#[Command(default)]
pub struct RecoverVessel {
    /// API-stable global entity id (the `api_id` from `ListEntities`) of the
    /// vessel to right. `u64` rather than `Entity` for the same reason
    /// `MoveEntity` uses one: `#[Command(default)]` derives `Default`, and
    /// `Entity` has none.
    pub entity_id: u64,
}

#[on_command(RecoverVessel)]
fn on_recover_vessel(
    trigger: On<RecoverVessel>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    terrains: Query<
        (&GlobalTransform, &crate::stream_viz::DemHeightField),
        With<TerrainColliderRing>,
    >,
    mut bodies: Query<(
        &RigidBody,
        &mut avian3d::prelude::Position,
        &mut avian3d::prelude::Rotation,
        Option<&mut avian3d::prelude::LinearVelocity>,
        Option<&mut avian3d::prelude::AngularVelocity>,
    )>,
    dynamics: Query<&RigidBody>,
    joints: JointGraph,
) {
    use bevy::math::DQuat;
    let global_id = lunco_core::GlobalEntityId::from_raw(trigger.event().entity_id);
    let Some(root) = registry.resolve(&global_id) else {
        warn!(
            "[recover] no api_id={} in registry",
            trigger.event().entity_id
        );
        return;
    };
    {
        let Ok((rb, pos, rot, _, _)) = bodies.get(root) else {
            warn!("[recover] {root:?} is not a rigid body — nothing to right");
            return;
        };
        if !matches!(rb, RigidBody::Dynamic) {
            warn!("[recover] {root:?} is not Dynamic — a static body has no pose to fix");
            return;
        }
        let up = rot.0 * DVec3::Y;
        let pivot = GridPos(pos.0);
        // Rigid righting transform about the target's own position: shortest arc
        // from the current up to world up (an exact 180° flip picks an arbitrary
        // axis — any righting is fine there).
        let q_fix = DQuat::from_rotation_arc(up.normalize(), DVec3::Y);
        recover_assembly(
            root,
            pivot,
            q_fix,
            up.y,
            &terrains,
            &mut bodies,
            &dynamics,
            &joints,
        );
    }
}

/// The righting itself, split out so the command reads as intent and the pose
/// arithmetic stays testable on its own terms.
#[allow(clippy::too_many_arguments)]
fn recover_assembly(
    root: Entity,
    pivot: GridPos,
    q_fix: bevy::math::DQuat,
    was_up_y: f64,
    terrains: &Query<
        (&GlobalTransform, &crate::stream_viz::DemHeightField),
        With<TerrainColliderRing>,
    >,
    bodies: &mut Query<(
        &RigidBody,
        &mut avian3d::prelude::Position,
        &mut avian3d::prelude::Rotation,
        Option<&mut avian3d::prelude::LinearVelocity>,
        Option<&mut avian3d::prelude::AngularVelocity>,
    )>,
    dynamics: &Query<&RigidBody>,
    joints: &JointGraph,
) {
    {
        let adj = joints.adjacency(|e| {
            dynamics
                .get(e)
                .is_ok_and(|rb| matches!(rb, RigidBody::Dynamic))
        });
        let members = joint_component(root, &adj);
        // Rotate every member's authoritative pose about the pivot, collecting
        // the post-rotation positions for the reseat pass. `grid − grid` is a
        // frame-free lever arm; rotating it and re-adding the pivot stays grid.
        let mut post: Vec<GridPos> = Vec::with_capacity(members.len());
        for &m in &members {
            let Ok((_, mut pos, mut rot, _, _)) = bodies.get_mut(m) else {
                continue;
            };
            let rotated = pivot + q_fix * (GridPos(pos.0) - pivot);
            pos.0 = rotated.0;
            rot.0 = (q_fix * rot.0).normalize();
            post.push(rotated);
        }
        // Reseat: the deepest post-rotation member ends RESCUE_CLEARANCE above
        // its local surface (only ever lifting — gravity handles settling down).
        let mut lift = 0.0_f64;
        for (_t_gt, hf) in terrains {
            let half = hf.0.half_extent() as f64;
            // Grid-absolute frame: `post` positions ARE avian `Position`
            // (grid-absolute) and the oracle is sampled in that frame. The terrain
            // render `GlobalTransform` (origin-relative) must NOT be interposed
            // here — it desynced the surface by the FloatingOrigin cell offset
            // (~2 km) at the elevated moonbase.
            for world in &post {
                let (x, z) = (world.0.x, world.0.z);
                if x.abs() > half || z.abs() > half {
                    continue;
                }
                lift = lift.max(hf.0.height_at(x, z) - world.0.y + RESCUE_CLEARANCE);
            }
        }
        for &m in &members {
            let Ok((_, mut pos, _, lin, ang)) = bodies.get_mut(m) else {
                continue;
            };
            if lift > 0.0 {
                pos.0.y += lift;
            }
            if let Some(mut v) = lin {
                v.0 = DVec3::ZERO;
            }
            if let Some(mut w) = ang {
                w.0 = DVec3::ZERO;
            }
        }
        // ONE line per invocation, and it says what the user asked for and what
        // happened — no attempt counter, because there is no retry loop to count.
        // A recover that does not stick is now visible as the user clicking again.
        info!(
            "[recover] righted vessel {root:?} ({} bodies): up·Y was {was_up_y:.2}, \
             lifted {lift:.1} m, velocities zeroed",
            members.len(),
        );
    }
}

register_commands!(on_recover_vessel);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadtree::QuadCoord;
    use avian3d::parry::query::Ray;
    use lunco_obstacle_field::field::HeightGrid;
    use lunco_terrain_core::{Crater, Craters};

    /// Absolute DEM-like altitude of the flat base — deliberately far from 0 so
    /// any hidden Y-recentering in the heightfield build would show up.
    const BASE_H: f64 = 1945.0;

    /// The collider's native band (`2·step`) for tests that exercise the
    /// heightfield build in isolation, with no viz config to floor against.
    /// Matches the pre-`SurfaceBand` behaviour (`detail_limited(2.0 * step)`).
    fn native_band(region: Square) -> SurfaceBand {
        let step = region.side() / (COLLIDER_RES as f64 - 1.0);
        SurfaceBand::visual(step)
    }

    #[test]
    fn collider_ring_uses_one_runtime_assembly_footprint() {
        let qt = Quadtree::new(16.0, 2, 1.0, 16.0);
        let mut nodes = Vec::new();
        push_assembly_ring_nodes(&qt, 2, 16.0, 1 << 2, [1.0, 1.0, 2.0, 2.0], &mut nodes);
        nodes.sort_unstable_by_key(|node| (node.x, node.z));
        nodes.dedup();
        assert_eq!(nodes.len(), 9);

        // Extending the physical assembly across a tile boundary expands the
        // one bounded ring; it does not create an independent 3x3 ring per
        // member.
        nodes.clear();
        push_assembly_ring_nodes(&qt, 2, 16.0, 1 << 2, [-7.0, 1.0, 7.0, 2.0], &mut nodes);
        nodes.sort_unstable_by_key(|node| (node.x, node.z));
        nodes.dedup();
        assert_eq!(nodes.len(), 12);
    }

    #[test]
    fn collider_ring_includes_raycast_contact_footprint() {
        let footprint = lunco_physics::PhysicsSupportFootprint(vec![
            lunco_physics::PhysicsSupportContact {
                local_offset: DVec3::new(-7.0, -0.5, 0.0),
                radius: 0.5,
            },
            lunco_physics::PhysicsSupportContact {
                local_offset: DVec3::new(7.0, -0.5, 0.0),
                radius: 0.5,
            },
        ]);
        let bounds = runtime_support_xz_bounds(GridPos(DVec3::ZERO), None, None, Some(&footprint));
        assert_eq!(bounds, [-7.5, -0.5, 7.5, 0.5]);

        // A chassis at x=0 whose raycast wheels reach both sides of a tile
        // boundary must keep the complete support footprint resident.
        let qt = Quadtree::new(16.0, 2, 1.0, 16.0);
        let mut nodes = Vec::new();
        push_assembly_ring_nodes(&qt, 2, 16.0, 1 << 2, bounds, &mut nodes);
        nodes.sort_unstable_by_key(|node| (node.x, node.z));
        nodes.dedup();
        assert_eq!(nodes.len(), 16);
    }

    /// Downward parry ray in TILE-LOCAL coordinates → ABSOLUTE surface altitude at
    /// (lx, lz). The heightfield is rebased by `origin_y` (see `sample_heights_xz`),
    /// so it sits near its own entity origin: the ray is cast in that local frame
    /// and `origin_y` is added back, letting the assertions below stay in the
    /// absolute datum they are written against.
    fn surface_y(collider: &Collider, lx: f64, lz: f64, origin_y: f64) -> f64 {
        // Local, not `BASE_H + 500`: a rebased tile's surface lives near 0.
        let top = 500.0;
        let ray = Ray::new(DVec3::new(lx, top, lz), DVec3::new(0.0, -1.0, 0.0));
        let toi = collider
            .shape()
            .cast_local_ray(&ray, 10_000.0, true)
            .unwrap_or_else(|| panic!("ray at local ({lx},{lz}) missed the tile"));
        origin_y + (top - toi)
    }

    /// DIAGNOSTIC: how faithfully does the baked collider reproduce the BOWL DEPTH
    /// of craters of various radii, at the ±4 km / depth-7 production config? A large
    /// gap for small radii = the collider band-limit / slope firewall flattens small
    /// craters, so a rover rides the shallower collider and "floats" over the deeper
    /// visual crater. Run with: `cargo test -p lunco-terrain-surface small_crater -- --nocapture`.
    #[test]
    fn collider_small_crater_depth_fidelity() {
        use lunco_terrain_core::HeightSource;
        let h = 4000.0_f64;
        let depth = COLLIDER_DEPTH;
        let qt = Quadtree::new(h, depth, 1.0, h);
        let coord = QuadCoord {
            depth,
            x: 70,
            z: 45,
        };
        let region = qt.region(coord);
        let side = region.side();
        let step = side / (COLLIDER_RES as f64 - 1.0);
        println!(
            "\n[collider fidelity] tile side={side:.2} m, step={step:.3} m, detail_limit={:.3} m",
            2.0 * step
        );
        for radius in [2.0_f64, 3.0, 5.0, 8.0, 15.0] {
            let crater_depth = 0.4 * radius; // depth_ratio 0.4 (fresh)
            let mut grid = HeightGrid::new_flat(129, h as f32);
            for v in grid.heights.iter_mut() {
                *v = BASE_H;
            }
            let crater = Crater {
                center: [region.center[0], region.center[1]],
                radius,
                depth: crater_depth,
                rim_height: 0.18 * crater_depth,
                softness: 0.0,
                bowl_power: 4.0,
            };
            let oracle = SurfaceOracle::new(
                std::sync::Arc::new(grid),
                vec![crate::oracle::HeightContribution {
                    modifier: std::sync::Arc::new(Craters::new(vec![crater])),
                    content_key: 1,
                }],
            );
            let oracle_center =
                HeightSource::height_at(&oracle, region.center[0], region.center[1]);
            let oracle_bowl = BASE_H - oracle_center;
            // The datum the runtime bake uses (`update_collider_ring`): the tile-centre
            // surface height — which at this probe point IS `oracle_center`.
            let origin_y = oracle_center;
            let heights =
                sample_heights_xz(&oracle, region, COLLIDER_RES, origin_y, native_band(region));
            let collider = heightfield_collider(heights, side);
            let collider_center = surface_y(&collider, 0.0, 0.0, origin_y);
            let collider_bowl = BASE_H - collider_center;
            let gap = collider_center - oracle_center; // >0 => collider ABOVE oracle (rover floats)
            println!(
                "  r={radius:>4.1} m depth={crater_depth:>4.2} m | oracle bowl={oracle_bowl:>5.2} m  collider bowl={collider_bowl:>5.2} m  GAP(collider-oracle)={gap:>5.2} m"
            );
        }
        println!();
    }

    /// End-to-end geometry proof for one collider-ring tile: sample an oracle with
    /// a single off-centre analytic crater over a canonical-depth region exactly
    /// the way `update_collider_ring` does, build the same `Collider::heightfield`,
    /// and ray-cast it in tile-local space. Proves (a) [x][z] layout is not
    /// transposed, (b) scale = full side length, (c) heights are REBASED by the
    /// tile-centre `origin_y` — local offsets, not absolute altitudes — and recover
    /// the absolute datum exactly when `origin_y` is added back, (d) the bowl depth
    /// survives the collider conditioning.
    #[test]
    fn collider_tile_reproduces_offcenter_crater_in_local_frame() {
        // Root region matching a ±4 km DEM; the canonical-depth tile side follows
        // COLLIDER_DEPTH (15.6 m at depth 9), so all probe geometry below is
        // derived from `region.half` — hardcoded depth-7 metres put the probes
        // outside the tile when the ring was retuned deeper.
        let h = 4000.0_f64;
        let depth = COLLIDER_DEPTH;
        let mut grid = HeightGrid::new_flat(129, h as f32);
        for v in grid.heights.iter_mut() {
            *v = BASE_H;
        }
        let qt = Quadtree::new(h, depth, 1.0, h);
        // An arbitrary interior tile.
        let coord = QuadCoord {
            depth,
            x: 70,
            z: 45,
        };
        let region = qt.region(coord);
        let side = region.side();

        // One crater, off-centre in the tile at an AXIS-ASYMMETRIC local offset
        // (+0.32·half in x, −0.58·half in z) so a transposed [z][x] layout puts
        // the bowl at a measurably different spot. Sized so the crater's full
        // 1.6·radius reach stays inside the tile and clear of the corner/far
        // probes, at any COLLIDER_DEPTH.
        let (dx, dz) = (0.32 * region.half, -0.58 * region.half);
        let crater = Crater {
            center: [region.center[0] + dx, region.center[1] + dz],
            radius: 0.38 * region.half,
            depth: 2.0,
            rim_height: 0.4,
            softness: 0.0,
            bowl_power: 4.0,
        };
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(grid),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(Craters::new(vec![crater])),
                content_key: 1,
            }],
        );

        // EXACTLY the runtime bake: the tile-centre datum, then sample + condition,
        // then the same collider constructor call as `update_collider_ring`.
        let origin_y = HeightSource::height_at(&oracle, region.center[0], region.center[1]);
        let heights =
            sample_heights_xz(&oracle, region, COLLIDER_RES, origin_y, native_band(region));

        // (c) The rebase itself: sampled heights are LOCAL offsets from `origin_y`.
        // The tile corner is flat base, so it must read ~0 — NOT ~1945. Asserted on
        // the raw samples, before parry sees them: a tile baked at absolute altitude
        // puts its heightfield ~1945 m above its own entity origin, which is the
        // "avatar moves up" / floating-collider bug this datum exists to prevent.
        assert!(
            heights[0][0].abs() < 0.05,
            "flat corner should be ~0 in the rebased local datum, got {} \
             (heights still ABSOLUTE — origin_y rebase lost?)",
            heights[0][0]
        );

        let collider = heightfield_collider(heights, side);

        // (c) …and adding `origin_y` back recovers the absolute datum exactly.
        // A flat interior probe far from the crater's reach (crater sits in the
        // −z half; 0.8·half in +x/+z is well outside 1.6·radius).
        let far_l = 0.8 * region.half;
        let far = surface_y(&collider, far_l, far_l, origin_y);
        assert!(
            (far - BASE_H).abs() < 0.05,
            "flat field should recover absolute {BASE_H}, got {far} (Y scaled, or origin_y lost?)"
        );

        // (a)+(d) Bowl at the crater's true local position.
        let bowl = surface_y(&collider, dx, dz, origin_y);
        assert!(
            bowl < BASE_H - 1.0,
            "crater bowl missing at local ({dx},{dz}): surface {bowl} vs base {BASE_H}"
        );

        // (a) NOT at the transposed position: a [z][x] mixup would dig here instead.
        let transposed = surface_y(&collider, dz, dx, origin_y);
        assert!(
            (transposed - BASE_H).abs() < 0.5,
            "surface dips at TRANSPOSED local ({dz},{dx}): {transposed} — heightfield layout is flipped"
        );

        // (b) Collider surface tracks the same band-limited oracle everywhere.
        let step = side / (COLLIDER_RES as f64 - 1.0);
        // The collider samples the oracle through the same band
        // (`native_band(region)` above, = `2·step`) — compare against that same
        // band-limited surface.
        let gated = native_band(region).limited(&oracle);
        for iz in (0..COLLIDER_RES).step_by(8) {
            for ix in (0..COLLIDER_RES).step_by(8) {
                let lx = -region.half + ix as f64 * step;
                let lz = -region.half + iz as f64 * step;
                let expect =
                    HeightSource::height_at(&gated, region.center[0] + lx, region.center[1] + lz);
                let got = surface_y(&collider, lx, lz, origin_y);
                assert!(
                    (got - expect).abs() <= COLLIDER_QUANT_STEP + 1e-6,
                    "collider/oracle mismatch at local ({lx:.2},{lz:.2}): collider {got}, oracle {expect}"
                );
            }
        }
    }

    /// Two abutting collider tiles must agree EXACTLY on their shared world
    /// column. Both tiles sample and quantize the same world-space oracle points,
    /// so the shared edge must remain byte-identical.
    #[test]
    fn adjacent_collider_tiles_agree_on_shared_edge() {
        let h = 4000.0_f64;
        let depth = COLLIDER_DEPTH;
        let mut grid = HeightGrid::new_flat(129, h as f32);
        for v in grid.heights.iter_mut() {
            *v = BASE_H;
        }
        let qt = Quadtree::new(h, depth, 1.0, h);
        let a = QuadCoord {
            depth,
            x: 70,
            z: 45,
        };
        let b = QuadCoord {
            depth,
            x: 71,
            z: 45,
        };
        let (ra, rb) = (qt.region(a), qt.region(b));
        // A fresh steep crater straddles the seam.
        let seam_x = ra.center[0] + ra.half;
        let crater = Crater {
            center: [seam_x + 4.0, ra.center[1] - 7.0],
            radius: 10.0,
            depth: 8.0,
            rim_height: 4.0,
            softness: 0.0,
            bowl_power: 4.0,
        };
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(grid),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(Craters::new(vec![crater])),
                content_key: 1,
            }],
        );
        // Each tile is rebased by ITS OWN tile-centre datum, exactly as the runtime
        // bakes them — so the seam must be compared in ABSOLUTE space (local +
        // origin). Comparing the raw local columns would be meaningless: two tiles
        // with different centres carry different origins, and identical world
        // geometry would read as an `origin_a - origin_b` step.
        let oya = HeightSource::height_at(&oracle, ra.center[0], ra.center[1]);
        let oyb = HeightSource::height_at(&oracle, rb.center[0], rb.center[1]);
        let ha = sample_heights_xz(&oracle, ra, COLLIDER_RES, oya, native_band(ra));
        let hb = sample_heights_xz(&oracle, rb, COLLIDER_RES, oyb, native_band(rb));
        // Tile A's last x-column and tile B's first x-column sample the same
        // world positions — they must agree once each is lifted back to absolute.
        for iz in 0..COLLIDER_RES {
            let (ya, yb) = (ha[COLLIDER_RES - 1][iz] + oya, hb[0][iz] + oyb);
            assert!(
                (ya - yb).abs() < 1e-9,
                "seam step {:.3} m at iz={iz}: {ya} vs {yb} — invisible wall",
                (ya - yb).abs()
            );
        }
    }

    /// **The ring must follow the ROVER, not the render origin.**
    ///
    /// Regression for: declaring celestial in a site-anchored scene dropped the
    /// rover through the ground (summer_space_school, 2026-07-21). The ring took
    /// its foci from `GlobalTransform` — origin-relative — and converted them
    /// through the terrain's `GlobalTransform` inverse, while the DEM frame is
    /// grid-absolute. With no celestial the FloatingOrigin happened to sit in the
    /// site's own cell, the two frames coincided, and everything worked BY
    /// ACCIDENT. The solar pin (`anchor_solar_frame_to_site`) moves the origin,
    /// the offset appears, and the rover's node was computed ~966 m away — so it
    /// got no tiles at all and fell.
    ///
    /// The assertion that matters is the SECOND one: the same body, at the same
    /// grid-absolute place, must select the same node no matter where the render
    /// origin is. A test that only checks "a tile appears under the rover" passes
    /// on the broken code, because at origin 0 the two frames agree.
    #[test]
    fn ring_node_is_chosen_in_the_grid_frame_not_the_render_frame() {
        let half = 997.0; // change4's ±997 m crop
        let depth = COLLIDER_DEPTH;

        // The rover's grid-absolute position — what avian `Position` holds.
        let rover = DVec3::new(140.0, -5923.1, -660.0);
        let node = ring_node(half, depth, rover.x, rover.z).expect("rover is on the DEM");

        // The render origin is somewhere else entirely (celestial pins the solar
        // tree; the FloatingOrigin lands in another cell). A grid-absolute read is
        // untouched by that — which is the whole property being asserted.
        for origin in [
            DVec3::ZERO,
            DVec3::new(506.0, 76.0, 823.0),      // the measured offset
            DVec3::new(-2000.0, 1945.0, 2000.0), // a whole cell away (moonbase-scale)
        ] {
            let same = ring_node(half, depth, rover.x, rover.z);
            assert_eq!(
                same,
                Some(node),
                "the node under the rover must not depend on the render origin ({origin:?})"
            );
            // And the frame error the bug actually made: reading the body through
            // an origin-relative transform shifts it by the origin offset.
            let as_if_origin_relative = rover - origin;
            let wrong = ring_node(
                half,
                depth,
                as_if_origin_relative.x,
                as_if_origin_relative.z,
            );
            if origin != DVec3::ZERO {
                assert_ne!(
                    wrong,
                    Some(node),
                    "an origin-relative read must be DETECTABLY wrong, or this test \
                     cannot fail on the bug it guards ({origin:?})"
                );
            }
        }
    }

    /// A body outside the crop is legitimately skipped — but a body *inside* it
    /// must always resolve, or the physics hold releases over a hole.
    #[test]
    fn ring_node_covers_the_whole_crop_and_only_the_crop() {
        let half = 997.0;
        for (x, z, want) in [
            (0.0, 0.0, true),
            (-996.9, 996.9, true), // just inside a corner
            (140.0, -660.0, true), // the change4 rover start
            (1000.0, 0.0, false),  // off the east edge
            (0.0, -1200.0, false),
        ] {
            assert_eq!(
                ring_node(half, COLLIDER_DEPTH, x, z).is_some(),
                want,
                "({x}, {z}) coverage"
            );
        }
    }
}
