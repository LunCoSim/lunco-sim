//! DVec3 helpers that abstract over the big_space hierarchy.
//!
//! Consumers should never assemble cross-Grid math themselves. They go
//! through these helpers. The previous practice — querying
//! `(&CellCoord, &Transform)` on arbitrary targets and calling
//! `grid.grid_position_double(...)` — works only inside one Grid and
//! breaks across Grid boundaries; these helpers cover both cases.

use bevy::ecs::{query::QueryFilter, system::SystemParam};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::*;

use crate::markers::GridAnchor;
use crate::world::WorldGrid;

// ─────────────────────────────────────────────────────────────────────
// Frame newtypes — grid-absolute vs floating-origin render
// ─────────────────────────────────────────────────────────────────────
//
// The two frames that actually get MIXED (three fixed-by-symptom incidents:
// the CQ-201 wheel lever arm, the elevated-site ray origin, the autopilot
// render-frame target). Typing follows `lunco-celestial/src/frames.rs`'s own
// criterion — "type frames where they get mixed, not everywhere" — so these
// wrap the values that cross crate boundaries; body-local offsets and
// frame-free deltas stay bare.
//
// Idiom: public tuple field, math through `.0`. Deliberately NO `Deref` —
// deref would let a `GridPos` flow back into a bare-`DVec3` slot silently,
// which is the exact mixing this type exists to stop.

/// A position in the GRID-ABSOLUTE world frame — the f64 frame avian
/// `Position` carries, authored placement uses, and the cell chain composes
/// to. NOT camera-relative; never write it into a render `Transform` except
/// through the big_space bridge writeback.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GridPos(pub DVec3);

/// A rotation composed in the grid-absolute chain. big_space never rotates
/// the render frame, so render-side rotations are numerically identical —
/// see [`GridRot::from_render_rotation`], the documented identity bridge.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GridRot(pub DQuat);

/// A point in the FLOATING-ORIGIN render frame (camera-relative). f64 so the
/// blessed conversions don't round-trip through f32; construct from render
/// `Transform`/`GlobalTransform` data via [`RenderPos::from_render_f32`].
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RenderPos(pub DVec3);

impl GridPos {
    pub fn new(v: DVec3) -> Self {
        Self(v)
    }
}

impl GridRot {
    /// big_space translates the render frame but never rotates it, so a
    /// rotation read from `GlobalTransform` is the same quaternion in both
    /// frames. This constructor exists so that fact is stated at the call
    /// site instead of implied by a bare cast.
    pub fn from_render_rotation(q: Quat) -> Self {
        Self(q.as_dquat())
    }
}

impl RenderPos {
    pub fn from_render_f32(v: Vec3) -> Self {
        Self(v.as_dvec3())
    }
}

/// grid − grid = a frame-free lever arm / offset vector. This is the CQ-201
/// invariant as a type: subtracting two grid-absolute points is always legal;
/// subtracting a grid point from a render point no longer compiles.
impl std::ops::Sub for GridPos {
    type Output = DVec3;
    fn sub(self, rhs: Self) -> DVec3 {
        self.0 - rhs.0
    }
}

/// grid + frame-free offset = grid.
impl std::ops::Add<DVec3> for GridPos {
    type Output = GridPos;
    fn add(self, rhs: DVec3) -> GridPos {
        GridPos(self.0 + rhs)
    }
}

/// The canonical conversion boundary between rendered points (which are
/// floating-origin relative) and the simulation's world frame.
///
/// Screen-space tools must depend on this parameter rather than selecting a
/// `Grid` or reconstructing a render-to-physics shift themselves. The world
/// shell owns exactly one [`WorldGrid`]; celestial/body grids are local frames
/// and are never valid substitutes for document/runtime placement.
#[derive(SystemParam)]
pub struct WorldFrame<'w, 's> {
    world_grid: Query<'w, 's, (Entity, &'static Grid), With<WorldGrid>>,
}

impl<'w, 's> WorldFrame<'w, 's> {
    /// The one canonical grid that scenes mount under.
    pub fn grid(&self) -> Option<(Entity, &Grid)> {
        self.world_grid.single().ok()
    }

    /// Convert a floating-origin-relative render point to world-grid absolute
    /// coordinates.
    pub fn render_to_world(&self, render_point: RenderPos) -> Option<GridPos> {
        self.grid()
            .map(|(_, grid)| render_to_grid_absolute(grid, render_point))
    }

    /// Split a render point into the canonical grid parent plus its cell/local
    /// transform pair, ready for an entity that will be a direct world-grid
    /// child.
    pub fn render_to_world_grid_local(
        &self,
        render_point: RenderPos,
    ) -> Option<(Entity, CellCoord, Vec3)> {
        let (entity, grid) = self.grid()?;
        let world = render_to_grid_absolute(grid, render_point);
        let (cell, local) = grid.translation_to_grid(world.0);
        Some((entity, cell, local))
    }
}

/// Walks ancestors of `entity` and returns the nearest one tagged
/// `GridAnchor`. Returns `entity` itself if it is already a `GridAnchor`.
///
/// This is the canonical "what unit am I touching?" lookup for UI:
/// selection, gizmo target, possession all use this to resolve a clicked
/// mesh entity to the rover/ball/vessel it belongs to.
pub fn ancestor_grid_anchor(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_anchors: &Query<(), With<GridAnchor>>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..32 {
        if q_anchors.contains(current) {
            return Some(current);
        }
        let Ok(child_of) = q_parents.get(current) else {
            return None;
        };
        current = child_of.parent();
    }
    None
}

/// Walks `entity` and its ancestors to find the nearest live [`Grid`].
///
/// A streamed surface tile must be a direct child of the same grid frame as
/// the terrain it represents. Looking only for `WorldGrid` is wrong for a
/// site-anchored body: the terrain branch belongs to the body's surface grid.
/// This is the shared frame lookup so visual and collider streaming cannot
/// silently choose different parents.
pub fn ancestor_grid<'a>(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &'a Query<&Grid>,
) -> Option<(Entity, &'a Grid)> {
    let mut current = entity;
    for _ in 0..32 {
        if let Ok(grid) = q_grids.get(current) {
            return Some((current, grid));
        }
        let Ok(child_of) = q_parents.get(current) else {
            return None;
        };
        current = child_of.parent();
    }
    None
}

/// Absolute world position of `entity` expressed in the BigSpace root
/// frame, as a `DVec3`.
///
/// Walks ancestors. Each `(CellCoord, Transform)` step under a `Grid`
/// contributes `grid.grid_position_double(cell, tf)` in DVec3 to the
/// accumulator. Plain-`Transform` ancestors compose their `Transform`
/// onto the running pose.
///
/// Returns `None` if `entity` has no spatial component at all.
pub fn world_position(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<GridPos> {
    world_pose(entity, q_parents, q_grids, q_spatial).map(|(p, _)| p)
}

/// Absolute world pose (position + rotation) of `entity` in the BigSpace root
/// frame, as `(DVec3, DQuat)`. See [`world_position`] for details; this variant
/// also returns the composed rotation — needed by the avian physics bridge
/// (Phase 5), which must sync both `Position` and `Rotation` from the cell
/// chain (rotation-aware, unlike the origin-relative f32 `GlobalTransform`).
pub fn world_pose<F: QueryFilter>(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(GridPos, GridRot)> {
    // Collect the chain entity → root. Each step records the entity's local
    // offset in its PARENT's frame (cell×edge + translation; edge comes from
    // the parent grid if any) and its local rotation.
    let (first_cell, first_tf) = match q_spatial.get(entity) {
        Ok((c, t)) => (c.copied().unwrap_or_default(), *t),
        Err(_) => return None,
    };
    let mut chain: Vec<(DVec3, Quat)> = Vec::with_capacity(8);
    let mut current = entity;
    let mut cur_cell = first_cell;
    let mut cur_tf = first_tf;
    for _ in 0..32 {
        let edge = match q_parents.get(current) {
            Ok(co) => q_grids
                .get(co.parent())
                .ok()
                .map(|g| g.cell_edge_length() as f64),
            Err(_) => None,
        };
        let cell_off = match edge {
            Some(e) => DVec3::new(
                cur_cell.x as f64 * e,
                cur_cell.y as f64 * e,
                cur_cell.z as f64 * e,
            ),
            None => DVec3::ZERO,
        };
        chain.push((cell_off + cur_tf.translation.as_dvec3(), cur_tf.rotation));
        let parent = match q_parents.get(current) {
            Ok(co) => co.parent(),
            Err(_) => break,
        };
        match q_spatial.get(parent) {
            Ok((c, t)) => {
                cur_cell = c.copied().unwrap_or_default();
                cur_tf = *t;
            }
            Err(_) => break,
        }
        current = parent;
    }
    // Compose top-down (root first): world = parent_world × local at each level,
    // so each ancestor's rotation IS applied to its descendants' offsets. The
    // previous implementation added offsets without rotating — wrong for any
    // ancestor grid that rotates (e.g. the spinning Moon grid; see
    // `world_position_applies_parent_grid_rotation`).
    let mut pos = DVec3::ZERO;
    let mut rot = DQuat::IDENTITY;
    for (off, local_rot) in chain.iter().rev() {
        pos += rot * off;
        rot *= local_rot.as_dquat();
    }
    Some((GridPos(pos), GridRot(rot)))
}

/// Compose `entity`'s pose in the coordinate frame of its ancestor `target_grid`.
///
/// This is deliberately different from [`world_pose`]. It stops at the target
/// grid and never visits the solar/world ancestors above it. A surface camera,
/// terrain owner, and streamed tile that share a body grid must be compared and
/// placed in this common local frame; composing both through a distant root and
/// subtracting the resulting large coordinates loses precision when that root is
/// re-pinned or rotated.
///
/// The returned translation is the target grid's grid-absolute local translation
/// (cell edge plus local offset), and the rotation maps entity-local axes into
/// target-grid axes. The target grid's own `CellCoord` and `Transform` are not part
/// of the result. Returns `None` when the target is not an ancestor or a spatial
/// component in the path is unavailable.
pub fn grid_relative_pose<F: QueryFilter>(
    entity: Entity,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    if entity == target_grid {
        return Some((DVec3::ZERO, DQuat::IDENTITY));
    }

    let (cell, transform) = q_spatial.get(entity).ok()?;
    let mut chain: Vec<(DVec3, Quat)> = Vec::with_capacity(8);
    let mut current = entity;
    let mut current_cell = cell.copied().unwrap_or_default();
    let mut current_transform = *transform;

    for _ in 0..32 {
        let parent = q_parents.get(current).ok()?.parent();
        let edge = q_grids
            .get(parent)
            .ok()
            .map(|grid| grid.cell_edge_length() as f64);
        let cell_offset = edge.map_or(DVec3::ZERO, |edge| {
            DVec3::new(
                current_cell.x as f64 * edge,
                current_cell.y as f64 * edge,
                current_cell.z as f64 * edge,
            )
        });
        chain.push((
            cell_offset + current_transform.translation.as_dvec3(),
            current_transform.rotation,
        ));

        if parent == target_grid {
            let mut position = DVec3::ZERO;
            let mut rotation = DQuat::IDENTITY;
            for (offset, local_rotation) in chain.iter().rev() {
                position += rotation * offset;
                rotation *= local_rotation.as_dquat();
            }
            return Some((position, rotation));
        }

        current = parent;
        let (next_cell, next_transform) = q_spatial.get(current).ok()?;
        current_cell = next_cell.copied().unwrap_or_default();
        current_transform = *next_transform;
    }

    None
}

/// Position of `entity` in its parent Grid's frame: `cell × edge + local`.
///
/// This is the frame **USD authors in**. A grid-direct prim's
/// `xformOp:translate` is grid-absolute: the prim spawns at `CellCoord::ZERO`
/// with the whole authored value sitting in `Transform`, and big_space's
/// recentring then re-splits it into `(cell, small local)`. So a prim's
/// `Transform.translation` is grid-absolute ONLY on the first frame, and only
/// while it stays in cell 0 — read it back later and it is short by
/// `cell × edge` (2 km per cell at the moonbase). Anything that authors a
/// translate, seats a physics pose, or shows a number to the user must go
/// through this, not `Transform.translation`.
///
/// A prim that is NOT grid-direct (a nested child under a referenced scene) has
/// no cell, and its authored translate IS its parent-local `Transform` — that
/// case returns the local translation unchanged.
pub fn grid_absolute<F: QueryFilter>(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<GridPos> {
    let (cell, tf) = q_spatial.get(entity).ok()?;
    Some(grid_absolute_seeded(
        entity,
        &cell.copied().unwrap_or_default(),
        tf,
        q_parents,
        q_grids,
    ))
}

/// [`grid_absolute`] seeded with an explicit `(cell, tf)` — for callers whose
/// `Transform` access is `&mut` (a second `&Transform` query would collide) or
/// whose entity is filtered out of their spatial query.
pub fn grid_absolute_seeded(
    entity: Entity,
    cell: &CellCoord,
    tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> GridPos {
    let Some(edge) = parent_grid(entity, q_parents, q_grids).map(|g| g.cell_edge_length() as f64)
    else {
        return GridPos(tf.translation.as_dvec3());
    };
    GridPos(
        DVec3::new(
            cell.x as f64 * edge,
            cell.y as f64 * edge,
            cell.z as f64 * edge,
        ) + tf.translation.as_dvec3(),
    )
}

/// Split a grid-absolute position back into the `(CellCoord, Transform)` pair
/// big_space stores — the inverse of [`grid_absolute`], and the only correct way
/// to seat a position onto a grid-direct entity.
///
/// Returns `(None, abs)` when `entity` is not grid-direct: there is no cell to
/// write and the value is already the local translation.
pub fn grid_local_from_absolute(
    entity: Entity,
    abs: GridPos,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> (Option<CellCoord>, Vec3) {
    match parent_grid(entity, q_parents, q_grids) {
        Some(grid) => {
            let (cell, local) = grid.translation_to_grid(abs.0);
            (Some(cell), local)
        }
        None => (None, abs.0.as_vec3()),
    }
}

/// The `Grid` this entity is a direct child of, if any.
fn parent_grid<'a>(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &'a Query<&Grid>,
) -> Option<&'a Grid> {
    q_grids.get(q_parents.get(entity).ok()?.parent()).ok()
}

/// Vector from `from` to `to` in DVec3 absolute world space.
pub fn world_vector(
    from: Entity,
    to: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<DVec3> {
    let a = world_position(from, q_parents, q_grids, q_spatial)?;
    let b = world_position(to, q_parents, q_grids, q_spatial)?;
    Some(b - a)
}

/// Decompose an absolute world position into `(CellCoord, Vec3)` under a
/// target Grid. `target_grid_world` is the target Grid's own absolute
/// world position (obtain via [`world_position`] on the Grid entity).
pub fn world_to_grid_local(
    world_pos: GridPos,
    target_grid_world: GridPos,
    target_grid: &Grid,
) -> (CellCoord, Vec3) {
    target_grid.translation_to_grid(world_pos - target_grid_world)
}

/// Convert a `GlobalTransform`-space point (the floating-origin-relative
/// render frame) into `grid`'s absolute coordinate frame.
///
/// This is the inverse of [`Grid::global_transform`]. It reverses the computed
/// [`big_space::prelude::LocalFloatingOrigin`] affine transform, then restores
/// the grid's origin cell. Use it whenever a camera ray / rendered terrain hit
/// needs to become a `CellCoord` + `Transform` or a physics-frame position.
/// Do not estimate this through an arbitrary body's `Position - GlobalTransform`:
/// that loses the target grid's nesting and rotation.
pub fn render_to_grid_absolute(grid: &Grid, render_point: RenderPos) -> GridPos {
    let local_origin = grid.local_floating_origin();
    let grid_relative = local_origin
        .grid_transform()
        .inverse()
        .transform_point3(render_point.0);
    GridPos(grid_relative + grid.cell_to_float(&local_origin.cell()))
}

/// Absolute world position of `entity`, seeded with an explicit
/// `(initial_cell, initial_tf)`. See [`world_pose_seeded`] (returns the full
/// pose); this returns the position only.
pub fn world_position_seeded<F: QueryFilter>(
    entity: Entity,
    initial_cell: &CellCoord,
    initial_tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> GridPos {
    world_pose_seeded(
        entity,
        initial_cell,
        initial_tf,
        q_parents,
        q_grids,
        q_spatial,
    )
    .0
}

/// Absolute world pose (position + rotation), seeded — the disjoint-query
/// variant of [`world_pose`], for entities not present in `q_spatial`.
pub fn world_pose_seeded<F: QueryFilter>(
    entity: Entity,
    initial_cell: &CellCoord,
    initial_tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> (GridPos, GridRot) {
    // Same rotation-aware chain composition as [`world_position`], but seeded
    // with an explicit (cell, transform) for entities not present in
    // `q_spatial` (disjoint-query / `Without<…>` cases).
    let mut chain: Vec<(DVec3, Quat)> = Vec::with_capacity(8);
    let edge0 = match q_parents.get(entity) {
        Ok(co) => q_grids
            .get(co.parent())
            .ok()
            .map(|g| g.cell_edge_length() as f64),
        Err(_) => None,
    };
    let cell_off0 = match edge0 {
        Some(e) => DVec3::new(
            initial_cell.x as f64 * e,
            initial_cell.y as f64 * e,
            initial_cell.z as f64 * e,
        ),
        None => DVec3::ZERO,
    };
    chain.push((
        cell_off0 + initial_tf.translation.as_dvec3(),
        initial_tf.rotation,
    ));

    let mut current = entity;
    for _ in 0..32 {
        let parent = match q_parents.get(current) {
            Ok(co) => co.parent(),
            Err(_) => break,
        };
        let (cell, tf) = match q_spatial.get(parent) {
            Ok((c, t)) => (c.copied().unwrap_or_default(), *t),
            Err(_) => break,
        };
        let edge = match q_parents.get(parent) {
            Ok(co) => q_grids
                .get(co.parent())
                .ok()
                .map(|g| g.cell_edge_length() as f64),
            Err(_) => None,
        };
        let cell_off = match edge {
            Some(e) => DVec3::new(cell.x as f64 * e, cell.y as f64 * e, cell.z as f64 * e),
            None => DVec3::ZERO,
        };
        chain.push((cell_off + tf.translation.as_dvec3(), tf.rotation));
        current = parent;
    }

    let mut pos = bevy::math::DVec3::ZERO;
    let mut rot = DQuat::IDENTITY;
    for (off, local_rot) in chain.iter().rev() {
        pos += rot * off;
        rot *= local_rot.as_dquat();
    }
    (GridPos(pos), GridRot(rot))
}

#[cfg(test)]
mod tests {
    //! Round-trip proof for the cell↔absolute rebase that the networking apply
    //! path (Phase 3) relies on. Earlier design notes (now in git history) claimed
    //! a `rebase_*` / `world_roundtrip_*` proto-test suite proved this; it never
    //! existed — this
    //! module is that missing safety net. Locks the contract before the snapshot
    //! apply path is made cell-aware.
    use super::*;
    use bevy::ecs::system::SystemState;

    const EDGE: f32 = 2000.0;
    // A recentering-ENABLED grid: `Grid::new` sets `maximum_distance_from_origin
    // = cell_edge/2 + switching_threshold`, and `translation_to_grid` keeps a
    // point in cell 0 until it exceeds that. With threshold 0 ⇒ max_dist =
    // edge/2 = 1000 m, so cells actually bin, and the within-cell offset is
    // bounded by edge/2 here. The live `WorldGrid` bins too, since its
    // `switching_threshold` was corrected from 1e10 (⇒ cell always 0, the whole
    // position in a raw f32 — 32 m of ULP at Earth–Moon distance) to 100 m —
    // see `WorldGridConfig::default`.
    fn grid() -> Grid {
        Grid::new(EDGE, 0.0)
    }

    /// `world_to_grid_local(p, ZERO, grid)` decomposes an absolute position into
    /// `(cell, offset)` whose reassembly returns `p`, and the offset stays inside
    /// one cell (so it is safe to fixed-point quantize in S3).
    #[test]
    fn world_to_grid_local_round_trips() {
        let g = grid();
        let cases = [
            DVec3::ZERO,
            DVec3::new(1500.0, -300.0, 800.0),   // within cell 0
            DVec3::new(2500.0, 0.0, 0.0),        // cell 1, offset 500
            DVec3::new(-7000.3, 4100.0, 0.0),    // negative cells
            DVec3::new(2500.0, -4100.0, 9999.9), // off-axis, multi-cell
            DVec3::new(1.737e6, 0.0, 0.0),       // lunar-radius scale (the precision case)
        ];
        for p in cases {
            let (cell, off) = world_to_grid_local(GridPos(p), GridPos(DVec3::ZERO), &g);
            // translation_to_grid centres the cell, so |offset| <= edge/2.
            assert!(
                (off.abs().max_element() as f64) <= EDGE as f64 / 2.0 + 1e-3,
                "offset {off:?} exceeds half-cell for {p:?}"
            );
            let back = g.grid_position_double(&cell, &Transform::from_translation(off));
            assert!(
                (back - p).length() < 1e-3,
                "round-trip {p:?} -> ({cell:?},{off:?}) -> {back:?}"
            );
        }
    }

    /// `grid_absolute` ↔ `grid_local_from_absolute` round-trip: the pair is the
    /// USD-authoring contract. A prim's authored translate is grid-absolute; its
    /// `Transform` holds only the cell remainder after big_space re-splits it.
    /// Reading one back and authoring it as the other is what teleported a
    /// gizmo-dragged prim exactly `cell × edge` at the moonbase — in cell 0 the
    /// two are equal and the bug is invisible, so this test pins a NON-zero cell.
    #[test]
    fn grid_absolute_round_trips_through_the_cell_split() {
        let mut world = World::new();
        let grid_e = world
            .spawn((
                grid(),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        // A prim two cells up and one over, as a moonbase prim is after spawn.
        let cell = CellCoord::new(1, 2, 0);
        let local = Vec3::new(-53.0, 120.5, 7.25);
        let prim = world
            .spawn((
                cell,
                Transform::from_translation(local),
                GlobalTransform::default(),
                ChildOf(grid_e),
            ))
            .id();
        // Not grid-direct: a nested child under a referenced scene.
        let nested = world
            .spawn((
                Transform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
                GlobalTransform::default(),
                ChildOf(prim),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_grids, q_spatial) = state
            .get(&world)
            .expect("read-only queries always validate");

        let abs = grid_absolute(prim, &q_parents, &q_grids, &q_spatial).expect("prim is spatial");
        let expected = DVec3::new(1.0 * EDGE as f64 - 53.0, 2.0 * EDGE as f64 + 120.5, 7.25);
        assert!(
            (abs.0 - expected).length() < 1e-6,
            "grid_absolute {abs:?} != cell×edge + local {expected:?}"
        );
        assert!(
            (abs.0 - local.as_dvec3()).length() > 1000.0,
            "the local translation must NOT pass for the absolute — that is the bug"
        );

        // Re-splitting the absolute reproduces a pose at the same place (the cell
        // may re-bin; only the reassembly has to match).
        let (back_cell, back_local) = grid_local_from_absolute(prim, abs, &q_parents, &q_grids);
        let back = grid().grid_position_double(
            &back_cell.expect("grid-direct prim gets a cell"),
            &Transform::from_translation(back_local),
        );
        assert!(
            (back - abs.0).length() < 1e-3,
            "round-trip {abs:?} -> {back:?}"
        );

        // A prim with no parent Grid has no cell: its translate IS its local.
        let nested_abs =
            grid_absolute(nested, &q_parents, &q_grids, &q_spatial).expect("nested is spatial");
        assert_eq!(nested_abs.0, DVec3::new(1.0, 2.0, 3.0));
        let (no_cell, same) = grid_local_from_absolute(nested, nested_abs, &q_parents, &q_grids);
        assert!(
            no_cell.is_none(),
            "a non-grid-direct entity must not be given a cell"
        );
        assert_eq!(same, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn render_point_round_trips_through_grid_global_transform() {
        let grid = Grid::new(2_000.0, 100.0);
        let cell = CellCoord::new(4, -3, 2);
        let local = Transform::from_translation(Vec3::new(12.5, -4.0, 99.0));
        let rendered = grid
            .global_transform(&cell, &local)
            .translation()
            .as_dvec3();
        let expected = grid.grid_position_double(&cell, &local);

        assert_eq!(
            render_to_grid_absolute(&grid, RenderPos(rendered)).0,
            expected,
            "the render-to-grid inverse must recover a cursor hit's cell-absolute position"
        );
    }

    /// The `target_grid_world` offset is honoured: decompose against a grid that
    /// is itself displaced from the origin and the reassembly still lands on `p`.
    #[test]
    fn world_to_grid_local_honors_grid_world_offset() {
        let g = grid();
        let grid_world = DVec3::new(10_000.0, 0.0, -5_000.0);
        let p = DVec3::new(12_500.0, 300.0, -5_000.0);
        let (cell, off) = world_to_grid_local(GridPos(p), GridPos(grid_world), &g);
        let back = g.grid_position_double(&cell, &Transform::from_translation(off)) + grid_world;
        assert!((back - p).length() < 1e-3, "p {p:?} -> {back:?}");
    }

    /// `world_position` (the hierarchical accumulator the apply path uses to find
    /// a grid's world pose) agrees with a direct `grid_position_double`, and the
    /// decompose of that absolute returns the original `(cell, offset)`.
    #[test]
    fn world_position_matches_decompose() {
        let mut world = World::new();
        let grid_e = world
            .spawn((
                grid(),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let child_off = Vec3::new(500.0, -123.0, 42.0);
        let child = world
            .spawn((
                CellCoord::new(1, 0, 0),
                Transform::from_translation(child_off),
                GlobalTransform::default(),
                ChildOf(grid_e),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_grids, q_spatial) = state
            .get(&world)
            .expect("read-only queries always validate");

        let abs = world_position(child, &q_parents, &q_grids, &q_spatial).unwrap();
        let g = grid();
        let expected = g.grid_position_double(
            &CellCoord::new(1, 0, 0),
            &Transform::from_translation(child_off),
        );
        assert!(
            (abs.0 - expected).length() < 1e-6,
            "abs {abs:?} expected {expected:?}"
        );

        let (cell, off) = world_to_grid_local(abs, GridPos(DVec3::ZERO), &g);
        assert_eq!((cell.x, cell.y, cell.z), (1, 0, 0), "cell {cell:?}");
        assert!(
            (off - child_off).length() < 1e-3,
            "off {off:?} vs {child_off:?}"
        );
    }

    /// `world_position` must apply a parent GRID's rotation. The Moon grid
    /// spins (`body_rotation_system`), so a child's absolute position rotates
    /// with it. This is load-bearing for gravity/SOI today and for the avian
    /// physics bridge (Phase 5): if the accumulator ignores the grid's
    /// rotation, a surface entity's world pose is wrong whenever its ancestor
    /// grid rotates.
    #[test]
    fn world_position_applies_parent_grid_rotation() {
        let mut world = World::new();
        let g = grid();
        // Parent grid rotated 90° about +Y, cell 0, no translation.
        let rot90y = Quat::from_rotation_y(core::f32::consts::FRAC_PI_2);
        let grid_e = world
            .spawn((
                g,
                CellCoord::ZERO,
                Transform::from_rotation(rot90y),
                GlobalTransform::default(),
            ))
            .id();
        // Child at local +X (100,0,0). A 90° +Y rotation maps +X -> -Z, so the
        // correct world position is (0,0,-100). If `world_position` ignores the
        // grid rotation it returns (100,0,0) — the assertion fails.
        let child = world
            .spawn((
                CellCoord::ZERO,
                Transform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
                GlobalTransform::default(),
                ChildOf(grid_e),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_grids, q_spatial) = state
            .get(&world)
            .expect("read-only queries always validate");

        let pos = world_position(child, &q_parents, &q_grids, &q_spatial).unwrap();
        let expected = DVec3::new(0.0, 0.0, -100.0);
        assert!(
            (pos.0 - expected).length() < 1e-3,
            "world_position ignored parent grid rotation: got {pos:?}, expected {expected:?} \
             (90° +Y should map child +X(100) to -Z(100))"
        );
    }

    #[test]
    fn grid_relative_pose_ignores_distant_root_motion() {
        let mut world = World::new();
        let root = world
            .spawn((
                grid(),
                CellCoord::ZERO,
                Transform::from_translation(Vec3::new(1.0e6, 0.0, -2.0e6)),
                GlobalTransform::default(),
            ))
            .id();
        let target_grid = world
            .spawn((
                grid(),
                CellCoord::ZERO,
                Transform::from_rotation(Quat::from_rotation_y(0.3)),
                GlobalTransform::default(),
                ChildOf(root),
            ))
            .id();
        let scene = world
            .spawn((
                Transform::from_xyz(10.0, 0.0, 20.0)
                    .with_rotation(Quat::from_rotation_y(core::f32::consts::FRAC_PI_2)),
                GlobalTransform::default(),
                ChildOf(target_grid),
            ))
            .id();
        let camera = world
            .spawn((
                Transform::from_xyz(3.0, 2.0, 4.0),
                GlobalTransform::default(),
                ChildOf(scene),
            ))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let first = {
            let (q_parents, q_grids, q_spatial) = state
                .get(&world)
                .expect("read-only queries always validate");
            grid_relative_pose(camera, target_grid, &q_parents, &q_grids, &q_spatial)
                .expect("camera is under target grid")
        };

        world
            .entity_mut(root)
            .insert(Transform::from_translation(Vec3::new(-9.0e11, 0.0, 7.0e11)));
        let second = {
            let (q_parents, q_grids, q_spatial) = state
                .get(&world)
                .expect("read-only queries always validate");
            grid_relative_pose(camera, target_grid, &q_parents, &q_grids, &q_spatial)
                .expect("camera is under target grid")
        };

        assert!((first.0 - second.0).length() < 1e-9);
        assert!(first.1.abs_diff_eq(second.1, 1e-9));
    }
}
