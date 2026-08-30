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

/// Failure while resolving a BigSpace coordinate chain.
///
/// A missing intermediate spatial component is not a valid "best effort"
/// result: returning the prefix of a chain produces a plausible but wrong
/// astronomical pose.  Callers at interactive boundaries may turn this into
/// an explicit unavailable result, while simulation/physics boundaries should
/// surface it as an integration error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinateError {
    MissingSpatial { entity: Entity },
    MissingAncestorSpatial { entity: Entity, parent: Entity },
    CellCoordWithoutGrid { entity: Entity, parent: Entity },
    Cycle { entity: Entity },
    DepthExceeded { entity: Entity, limit: usize },
}

impl core::fmt::Display for CoordinateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingSpatial { entity } => {
                write!(f, "entity {entity:?} has no spatial component")
            }
            Self::MissingAncestorSpatial { entity, parent } => write!(
                f,
                "entity {entity:?} has an ancestor {parent:?} without a spatial component"
            ),
            Self::CellCoordWithoutGrid { entity, parent } => write!(
                f,
                "entity {entity:?} carries a CellCoord but its parent {parent:?} is not a Grid"
            ),
            Self::Cycle { entity } => {
                write!(f, "coordinate hierarchy contains a cycle at {entity:?}")
            }
            Self::DepthExceeded { entity, limit } => write!(
                f,
                "coordinate hierarchy for {entity:?} exceeds the {limit}-node limit"
            ),
        }
    }
}

impl std::error::Error for CoordinateError {}

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

/// The canonical authored frame for surface vehicles.
///
/// Vehicle assets are Y-up, right-handed, with local forward along `-Z` and
/// local right along `+X`. Scene placement supplies the vehicle's initial
/// heading; runtime control and traction both transform these same local axes
/// from the authoritative physics rotation. Keeping this contract beside the
/// typed grid rotation prevents render-frame transforms and duplicated axis
/// conventions from crossing the navigation/physics boundary.
pub struct VehicleFrame;

impl VehicleFrame {
    /// Canonical vehicle-local forward direction.
    pub const FORWARD_LOCAL: DVec3 = DVec3::NEG_Z;
    /// Canonical vehicle-local right direction.
    pub const RIGHT_LOCAL: DVec3 = DVec3::X;

    /// Transform the canonical forward direction by an authoritative body
    /// rotation into the grid-absolute physics frame.
    #[inline]
    pub fn forward(rotation: GridRot) -> DVec3 {
        rotation.0 * Self::FORWARD_LOCAL
    }

    /// Transform the canonical right direction by an authoritative body
    /// rotation into the grid-absolute physics frame.
    #[inline]
    pub fn right(rotation: GridRot) -> DVec3 {
        rotation.0 * Self::RIGHT_LOCAL
    }

    /// Return the vehicle forward direction projected onto the world yaw plane.
    /// Ground navigation and clearance sensing use this projection so body
    /// pitch/roll cannot turn a horizontal waypoint into a vertical command.
    #[inline]
    pub fn yaw_forward(rotation: GridRot) -> DVec3 {
        let forward = Self::forward(rotation);
        DVec3::new(forward.x, 0.0, forward.z).normalize_or_zero()
    }
}

#[cfg(test)]
mod vehicle_frame_tests {
    use super::*;

    #[test]
    fn canonical_axes_follow_physics_rotation_and_yaw_projection() {
        let rotation = GridRot(DQuat::from_rotation_y(core::f64::consts::FRAC_PI_2));
        assert!((VehicleFrame::forward(rotation) - DVec3::NEG_X).length() < 1e-9);
        assert!((VehicleFrame::right(rotation) - DVec3::NEG_Z).length() < 1e-9);

        let tilted = GridRot(
            DQuat::from_rotation_z(0.25)
                * DQuat::from_rotation_y(core::f64::consts::FRAC_PI_2)
                * DQuat::from_rotation_x(0.2),
        );
        let yaw = VehicleFrame::yaw_forward(tilted);
        assert!(
            yaw.y.abs() < 1e-12,
            "yaw projection retained vertical motion"
        );
        assert!((yaw - DVec3::NEG_X).length() < 1e-9, "yaw forward={yaw:?}");
    }
}

/// Rigid f64 transform from one concrete BigSpace grid into another.
///
/// Grid topology is a precision implementation detail. Systems that need to
/// cross it resolve their semantic source/target frames first, obtain this
/// transform once, then apply it consistently to positions, orientations and
/// free vectors such as linear/angular velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFrameTransform {
    pub translation: DVec3,
    pub rotation: DQuat,
}

impl GridFrameTransform {
    pub const IDENTITY: Self = Self {
        translation: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
    };

    pub fn transform_position(self, position: DVec3) -> DVec3 {
        self.translation + self.rotation * position
    }

    pub fn transform_rotation(self, rotation: DQuat) -> DQuat {
        (self.rotation * rotation).normalize()
    }

    pub fn transform_vector(self, vector: DVec3) -> DVec3 {
        self.rotation * vector
    }
}

/// A point in the OriginAnchor-relative render frame. f64 so the
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

/// Read entity poses in the one f64 frame currently used by physics and
/// site-local interaction.
///
/// This is the user/runtime boundary for generic positions and orientations.
/// Callers do not inspect a `Grid`, choose an ancestor, or read a
/// camera-relative [`GlobalTransform`]; the application/scene-mount owner binds
/// [`crate::ActivePhysicsFrame`] explicitly and this parameter performs the complete
/// BigSpace hierarchy conversion. Explicit astronomical products continue to
/// use their typed semantic reference frames instead.
#[derive(SystemParam)]
pub struct ActiveFramePoseQuery<'w, 's> {
    active_frame: Res<'w, crate::ActivePhysicsFrame>,
    parents: Query<'w, 's, &'static ChildOf>,
    grids: Query<'w, 's, &'static Grid>,
    spatial: Query<'w, 's, (Option<&'static CellCoord>, &'static Transform)>,
}

impl ActiveFramePoseQuery<'_, '_> {
    /// The concrete BigSpace grid that defines this query's coordinates.
    pub fn frame(&self) -> Entity {
        self.active_frame.0
    }

    /// Resolve `entity` into [`Self::frame`]. Missing or disconnected frame
    /// topology returns `None`; choosing another grid would make the answer
    /// load-order dependent.
    pub fn pose(&self, entity: Entity) -> Option<(GridPos, GridRot)> {
        pose_in_grid(
            entity,
            self.active_frame.0,
            &self.parents,
            &self.grids,
            &self.spatial,
        )
        .map(|(position, rotation)| (GridPos(position), GridRot(rotation)))
    }

    /// Resolve only the active-frame position of `entity`.
    pub fn position(&self, entity: Entity) -> Option<GridPos> {
        self.pose(entity).map(|(position, _)| position)
    }

    /// Resolve only the active-frame orientation of `entity`.
    pub fn rotation(&self, entity: Entity) -> Option<GridRot> {
        self.pose(entity).map(|(_, rotation)| rotation)
    }
}

#[cfg(test)]
mod active_frame_pose_tests {
    use super::*;
    use crate::WorldGridConfig;
    use bevy::ecs::system::SystemState;

    fn read_pose(world: &mut World, entity: Entity) -> (GridPos, GridRot) {
        let mut state: SystemState<ActiveFramePoseQuery> = SystemState::new(world);
        state
            .get(world)
            .expect("query validates")
            .pose(entity)
            .expect("entity is connected to the active frame")
    }

    #[test]
    fn active_frame_pose_is_invariant_to_rotating_and_translating_ancestors() {
        let mut world = World::new();
        let root = world
            .spawn((
                WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        let body = world
            .spawn((
                WorldGridConfig::default().grid(),
                CellCoord::new(80_000, -4_000, 7_000),
                Transform::from_rotation(Quat::from_rotation_y(0.8)),
                ChildOf(root),
            ))
            .id();
        let site = world
            .spawn((
                WorldGridConfig::default().grid(),
                CellCoord::new(500, -900, 200),
                Transform::from_rotation(Quat::from_rotation_z(-0.4)),
                ChildOf(body),
            ))
            .id();
        world.insert_resource(crate::ActivePhysicsFrame(site));
        let local_position = DVec3::new(123.25, -1_901.5, -88.5);
        let local_rotation = DQuat::from_rotation_y(0.2);
        let entity = world
            .spawn((
                Transform::from_translation(local_position.as_vec3())
                    .with_rotation(local_rotation.as_quat()),
                ChildOf(site),
            ))
            .id();

        let before = read_pose(&mut world, entity);
        assert!((before.0 .0 - local_position).length() < 1.0e-4);
        assert!(before.1 .0.angle_between(local_rotation).abs() < 1.0e-6);

        world.entity_mut(body).insert((
            CellCoord::new(-120_000, 17_000, 99_000),
            Transform::from_rotation(Quat::from_rotation_x(-1.1)),
        ));
        let after = read_pose(&mut world, entity);
        assert!((after.0 .0 - local_position).length() < 1.0e-4);
        assert!(after.1 .0.angle_between(local_rotation).abs() < 1.0e-6);
    }

    #[test]
    fn active_frame_position_converts_to_the_entity_parent_once() {
        let mut world = World::new();
        let active = world
            .spawn((
                WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        let parent_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let parent = world
            .spawn((
                Transform::from_xyz(100.0, -20.0, 50.0).with_rotation(parent_rotation),
                ChildOf(active),
            ))
            .id();
        let entity = world.spawn((Transform::default(), ChildOf(parent))).id();
        let desired = DVec3::new(125.0, -17.0, 40.0);

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (parents, grids, spatial) = state.get(&world).unwrap();
        let (cell, local) =
            position_in_grid_to_parent_local(entity, desired, active, &parents, &grids, &spatial)
                .expect("entity and active frame are connected");

        assert!(cell.is_none(), "plain parents do not invent BigSpace cells");
        let expected =
            parent_rotation.inverse().as_dquat() * (desired - DVec3::new(100.0, -20.0, 50.0));
        assert!((local.as_dvec3() - expected).length() < 1.0e-5);
    }

    #[test]
    fn active_frame_position_splits_only_for_a_grid_parent() {
        let mut world = World::new();
        let active = world
            .spawn((
                WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let entity = world.spawn((Transform::default(), ChildOf(active))).id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (parents, grids, spatial) = state.get(&world).unwrap();
        let (cell, local) = position_in_grid_to_parent_local(
            entity,
            DVec3::new(2_500.0, 0.0, 0.0),
            active,
            &parents,
            &grids,
            &spatial,
        )
        .expect("entity and active frame are connected");

        assert_eq!(cell, Some(CellCoord::new(1, 0, 0)));
        assert_eq!(local, Vec3::new(500.0, 0.0, 0.0));
    }

    #[test]
    fn active_frame_rotation_converts_to_the_entity_parent_once() {
        let mut world = World::new();
        let active = world
            .spawn((
                WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        let parent_rotation = Quat::from_rotation_y(0.8);
        let parent = world
            .spawn((Transform::from_rotation(parent_rotation), ChildOf(active)))
            .id();
        let entity = world.spawn((Transform::default(), ChildOf(parent))).id();
        let desired = DQuat::from_rotation_x(-0.4);

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (parents, grids, spatial) = state.get(&world).unwrap();
        let local =
            rotation_in_grid_to_parent_local(entity, desired, active, &parents, &grids, &spatial)
                .expect("entity and active frame are connected");

        let expected = (parent_rotation.inverse().as_dquat() * desired).normalize();
        assert!(
            local.dot(expected).abs() > 1.0 - 1.0e-12,
            "local={local:?} expected={expected:?}"
        );
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

/// Find the nearest shared [`Grid`] ancestor of two entities.
///
/// The returned grid is a valid target for [`grid_relative_pose`] for both
/// entities. Keeping this lookup beside the BigSpace pose helpers prevents
/// callers from inventing a second hierarchy walk and then composing one
/// branch through a floating-origin `GlobalTransform`.
pub fn common_grid(
    first: Entity,
    second: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> Option<Entity> {
    let mut first_grids = Vec::with_capacity(16);
    let mut current = first;
    for _ in 0..32 {
        if q_grids.get(current).is_ok() {
            first_grids.push(current);
        }
        let Ok(child_of) = q_parents.get(current) else {
            break;
        };
        current = child_of.parent();
    }

    let mut current = second;
    for _ in 0..32 {
        if q_grids.get(current).is_ok() && first_grids.contains(&current) {
            return Some(current);
        }
        let Ok(child_of) = q_parents.get(current) else {
            break;
        };
        current = child_of.parent();
    }
    None
}

/// Compose two entities into their nearest shared BigSpace Grid.
///
/// This is the frame-safe operation for siblings. Neither entity needs to be
/// a descendant of the other; both only need to share a live Grid branch.
/// Returned rotations map each entity's local axes into the returned Grid.
pub fn common_grid_poses<F: QueryFilter>(
    first: Entity,
    second: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(Entity, DVec3, DQuat, DVec3, DQuat)> {
    let grid = common_grid(first, second, q_parents, q_grids)?;
    let (first_position, first_rotation) =
        grid_relative_pose(first, grid, q_parents, q_grids, q_spatial)?;
    let (second_position, second_rotation) =
        grid_relative_pose(second, grid, q_parents, q_grids, q_spatial)?;
    Some((
        grid,
        first_position,
        first_rotation,
        second_position,
        second_rotation,
    ))
}

#[cfg(test)]
mod common_grid_tests {
    use super::common_grid;
    use crate::WorldGridConfig;
    use bevy::ecs::system::SystemState;
    use bevy::prelude::*;
    use big_space::prelude::{CellCoord, Grid};

    #[test]
    fn chooses_the_nearest_shared_grid() {
        let mut world = World::new();
        let root = world
            .spawn((WorldGridConfig::default().grid(), CellCoord::ZERO))
            .id();
        let first_grid = world
            .spawn((
                WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                ChildOf(root),
            ))
            .id();
        let second_grid = world
            .spawn((
                WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                ChildOf(first_grid),
            ))
            .id();
        let first = world.spawn((ChildOf(second_grid),)).id();
        let second = world.spawn((ChildOf(second_grid),)).id();

        let mut state: SystemState<(Query<&ChildOf>, Query<&Grid>)> = SystemState::new(&mut world);
        let (q_parents, q_grids) = state.get(&world).unwrap();
        assert_eq!(
            common_grid(first, second, &q_parents, &q_grids),
            Some(second_grid)
        );
    }
}

/// Absolute world position of `entity` expressed in the BigSpace root
/// frame, as a `DVec3`.
///
/// Walks ancestors. Each `(CellCoord, Transform)` step under a `Grid`
/// contributes `grid.grid_position_double(cell, tf)` in DVec3 to the
/// accumulator. Plain-`Transform` ancestors compose their `Transform`
/// onto the running pose.
///
/// Returns a topology error if the entity or any required ancestor is not
/// spatial. A missing parent is the explicit root boundary and is valid.
pub fn world_position(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Result<GridPos, CoordinateError> {
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
) -> Result<(GridPos, GridRot), CoordinateError> {
    // Collect the chain entity → root. Each step records the entity's local
    // offset in its PARENT's frame (cell×edge + translation; edge comes from
    // the parent grid if any) and its local rotation.
    let (first_cell, first_tf) = q_spatial
        .get(entity)
        .map(|(c, t)| (c.copied(), *t))
        .map_err(|_| CoordinateError::MissingSpatial { entity })?;
    let mut chain: Vec<(DVec3, Quat)> = Vec::with_capacity(8);
    let mut visited = Vec::with_capacity(32);
    let mut current = entity;
    let mut cur_cell = first_cell;
    let mut cur_tf = first_tf;
    for _ in 0..32 {
        if visited.contains(&current) {
            return Err(CoordinateError::Cycle { entity: current });
        }
        visited.push(current);
        let local_position = match q_parents.get(current) {
            Ok(child_of) => match (cur_cell.as_ref(), q_grids.get(child_of.parent())) {
                (Some(cell), Ok(grid)) => grid.grid_position_double(cell, &cur_tf),
                (Some(_), Err(_)) => {
                    if q_spatial.get(child_of.parent()).is_err() {
                        return Err(CoordinateError::MissingAncestorSpatial {
                            entity,
                            parent: child_of.parent(),
                        });
                    }
                    return Err(CoordinateError::CellCoordWithoutGrid {
                        entity: current,
                        parent: child_of.parent(),
                    });
                }
                (None, _) => cur_tf.translation.as_dvec3(),
            },
            Err(_) => cur_tf.translation.as_dvec3(),
        };
        chain.push((local_position, cur_tf.rotation));
        let parent = match q_parents.get(current) {
            Ok(co) => co.parent(),
            Err(_) => break,
        };
        match q_spatial.get(parent) {
            Ok((c, t)) => {
                cur_cell = c.copied();
                cur_tf = *t;
            }
            Err(_) if q_parents.get(parent).is_err() => break,
            Err(_) => return Err(CoordinateError::MissingAncestorSpatial { entity, parent }),
        }
        current = parent;
    }
    if visited.len() == 32 && q_parents.get(current).is_ok() {
        return Err(CoordinateError::DepthExceeded { entity, limit: 32 });
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
    Ok((GridPos(pos), GridRot(rot)))
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
    let mut current_cell = cell.copied();
    let mut current_transform = *transform;

    for _ in 0..32 {
        let parent = q_parents.get(current).ok()?.parent();
        let local_position = match (current_cell.as_ref(), q_grids.get(parent)) {
            (Some(cell), Ok(grid)) => grid.grid_position_double(cell, &current_transform),
            (Some(_), Err(_)) => return None,
            (None, _) => current_transform.translation.as_dvec3(),
        };
        chain.push((local_position, current_transform.rotation));

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
        current_cell = next_cell.copied();
        current_transform = *next_transform;
    }

    None
}

/// Compose an entity's pose in an arbitrary target Grid's coordinates.
///
/// The target does not need to be an ancestor. BigSpace branches such as a
/// Moon body and the active Moon surface grid are siblings below the same
/// inertial grid; their pose must be compared in that shared grid, then
/// expressed in the target grid. This is the canonical cross-branch
/// conversion used by the single Avian physics frame.
pub fn pose_in_grid<F: QueryFilter>(
    entity: Entity,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    let common = common_grid(entity, target_grid, q_parents, q_grids)?;
    let (entity_position, entity_rotation) =
        grid_relative_pose(entity, common, q_parents, q_grids, q_spatial)?;
    let (target_position, target_rotation) =
        grid_relative_pose(target_grid, common, q_parents, q_grids, q_spatial)?;
    let inverse = target_rotation.normalize().inverse();
    Some((
        inverse * (entity_position - target_position),
        (inverse * entity_rotation.normalize()).normalize(),
    ))
}

/// Convert a complete semantic pose into the raw local coordinates of
/// `parent`, before applying the parent's storage representation.
///
/// User-facing placement commands speak in `target_grid`; this helper owns the
/// one inverse parent-pose operation those commands need. Callers that are
/// writing a direct child of a `Grid` must use
/// [`pose_in_grid_to_parent_storage`] to perform the required cell split.
pub fn pose_in_parent_local<F: QueryFilter>(
    position: DVec3,
    rotation: DQuat,
    parent: Entity,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    let (parent_position, parent_rotation) = if parent == target_grid {
        (DVec3::ZERO, DQuat::IDENTITY)
    } else {
        pose_in_grid(parent, target_grid, q_parents, q_grids, q_spatial)?
    };
    let inverse_parent = parent_rotation.normalize().inverse();
    Some((
        inverse_parent * (position - parent_position),
        (inverse_parent * rotation).normalize(),
    ))
}

/// Convert a complete semantic pose into the representation stored on a new
/// child of `parent`.
///
/// A direct child of a `Grid` stores its position as `(CellCoord, Transform)`;
/// a descendant below an ordinary entity stores only its parent-local
/// `Transform`. This is the single placement boundary used by scene spawns,
/// replicated spawns, and runtime waypoint markers.
pub fn pose_in_grid_to_parent_storage<F: QueryFilter>(
    position: DVec3,
    rotation: DQuat,
    parent: Entity,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(Option<CellCoord>, Vec3, DQuat)> {
    let (parent_local, local_rotation) = pose_in_parent_local(
        position,
        rotation,
        parent,
        target_grid,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    if let Ok(grid) = q_grids.get(parent) {
        let (cell, local_translation) = grid.translation_to_grid(parent_local);
        Some((Some(cell), local_translation, local_rotation))
    } else {
        Some((None, parent_local.as_vec3(), local_rotation))
    }
}

/// Convert a point expressed in `target_grid` into the storage coordinates of
/// `entity`'s actual parent.
///
/// This is the inverse boundary used by user-facing placement commands. Callers
/// speak one stable semantic frame (normally [`crate::ActivePhysicsFrame`]); this
/// helper walks the existing hierarchy once, removes the parent's complete f64
/// pose, and performs BigSpace's cell split only when the parent is a [`Grid`].
/// A disconnected entity returns `None` rather than silently treating either
/// frame as identity.
pub fn position_in_grid_to_parent_local<F: QueryFilter>(
    entity: Entity,
    position: DVec3,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(Option<CellCoord>, Vec3)> {
    let parent = q_parents.get(entity).ok()?.parent();
    let (cell, local, _) = pose_in_grid_to_parent_storage(
        position,
        DQuat::IDENTITY,
        parent,
        target_grid,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    Some((cell, local))
}

/// Convert an orientation expressed in `target_grid` into the local rotation
/// stored on `entity` below its actual parent. The returned rotation is the
/// same regardless of whether that parent is a Grid; position storage and
/// rotation storage are split independently at the same boundary.
///
/// Rotations are not frame-invariant: a body-fixed parent Grid rotates relative
/// to an inertial grid, and a nested assembly parent may itself be rotated. This
/// is the rotational half of [`position_in_grid_to_parent_local`]; user-facing
/// pose commands must use both halves rather than writing an active-frame
/// quaternion directly into `Transform`.
pub fn rotation_in_grid_to_parent_local<F: QueryFilter>(
    entity: Entity,
    rotation: DQuat,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<DQuat> {
    let parent = q_parents.get(entity).ok()?.parent();
    pose_in_grid_to_parent_storage(
        DVec3::ZERO,
        rotation,
        parent,
        target_grid,
        q_parents,
        q_grids,
        q_spatial,
    )
    .map(|(_, _, local_rotation)| local_rotation)
}

/// Convert a complete f64 pose from one arbitrary BigSpace Grid to another.
///
/// This is the representation boundary used by networking, camera migration,
/// and frame handover. Callers supply semantic source/target grid identities;
/// this function owns the hierarchy composition, including every translated
/// cell and rotated parent. No caller should reproduce the origin/axes formula.
pub fn transform_pose_between_grids<F: QueryFilter>(
    position: DVec3,
    rotation: DQuat,
    source_grid: Entity,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    let transform =
        grid_transform_between_grids(source_grid, target_grid, q_parents, q_grids, q_spatial)?;
    Some((
        transform.transform_position(position),
        transform.transform_rotation(rotation),
    ))
}

/// Resolve the complete rigid transform between two concrete BigSpace grids.
///
/// This is the lower-level counterpart of [`transform_pose_between_grids`]
/// used when one conversion must be applied to a complete dynamics state.
pub fn grid_transform_between_grids<F: QueryFilter>(
    source_grid: Entity,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<GridFrameTransform> {
    if source_grid == target_grid {
        return Some(GridFrameTransform::IDENTITY);
    }
    let (translation, rotation) =
        pose_in_grid(source_grid, target_grid, q_parents, q_grids, q_spatial)?;
    Some(GridFrameTransform {
        translation,
        rotation: rotation.normalize(),
    })
}

/// Seeded counterpart of [`pose_in_grid`] for a mutably borrowed entity.
pub fn pose_in_grid_seeded<F: QueryFilter>(
    entity: Entity,
    target_grid: Entity,
    initial_cell: Option<&CellCoord>,
    initial_transform: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    let common = common_grid(entity, target_grid, q_parents, q_grids)?;
    let (entity_position, entity_rotation) = grid_relative_pose_seeded(
        entity,
        common,
        initial_cell,
        initial_transform,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let (target_position, target_rotation) =
        grid_relative_pose(target_grid, common, q_parents, q_grids, q_spatial)?;
    let inverse = target_rotation.normalize().inverse();
    Some((
        inverse * (entity_position - target_position),
        (inverse * entity_rotation.normalize()).normalize(),
    ))
}

/// Seeded counterpart of [`grid_relative_pose`] for a target entity that is
/// borrowed mutably by the caller (for example Avian's `Position` bridge).
///
/// The conversion is still performed by each owning BigSpace [`Grid`].  This
/// is the only variant that accepts the caller's current `(CellCoord,
/// Transform)` instead of reading a second, disjoint spatial query, so the
/// frame rule remains identical for bodies and ordinary scene entities.
pub fn grid_relative_pose_seeded<F: QueryFilter>(
    entity: Entity,
    target_grid: Entity,
    initial_cell: Option<&CellCoord>,
    initial_transform: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(DVec3, DQuat)> {
    if entity == target_grid {
        return Some((DVec3::ZERO, DQuat::IDENTITY));
    }

    let mut chain: Vec<(DVec3, Quat)> = Vec::with_capacity(8);
    let mut current = entity;
    let mut current_cell = initial_cell.copied();
    let mut current_transform = *initial_transform;

    for _ in 0..32 {
        let parent = q_parents.get(current).ok()?.parent();
        let local_position = match (current_cell.as_ref(), q_grids.get(parent)) {
            (Some(cell), Ok(grid)) => grid.grid_position_double(cell, &current_transform),
            (Some(_), Err(_)) => return None,
            (None, _) => current_transform.translation.as_dvec3(),
        };
        chain.push((local_position, current_transform.rotation));

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
        current_cell = next_cell.copied();
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
    grid_absolute_seeded(entity, cell, tf, q_parents, q_grids)
}

/// [`grid_absolute`] seeded with an explicit optional `(cell, tf)` — for callers whose
/// `Transform` access is `&mut` (a second `&Transform` query would collide) or
/// whose entity is filtered out of their spatial query.
pub fn grid_absolute_seeded(
    entity: Entity,
    cell: Option<&CellCoord>,
    tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> Option<GridPos> {
    match (cell, parent_grid(entity, q_parents, q_grids)) {
        (Some(cell), Some(grid)) => Some(GridPos(grid.grid_position_double(cell, tf))),
        (Some(_), None) => None,
        (None, _) => Some(GridPos(tf.translation.as_dvec3())),
    }
}

/// Reassemble BigSpace's stored cell/local split in the parent grid frame.
///
/// `edge == None` means the entity is not grid-direct, so its Transform is an
/// ordinary parent-local point and `cell` must also be absent.
pub fn compose_cell_local(
    cell: Option<&CellCoord>,
    edge: Option<f64>,
    local_translation: Vec3,
) -> GridPos {
    let local = local_translation.as_dvec3();
    match (cell, edge) {
        (Some(cell), Some(edge)) => GridPos(
            DVec3::new(
                cell.x as f64 * edge,
                cell.y as f64 * edge,
                cell.z as f64 * edge,
            ) + local,
        ),
        (None, _) => GridPos(local),
        (Some(_), None) => {
            panic!("CellCoord cannot be composed without a direct parent Grid")
        }
    }
}

/// Inverse of [`compose_cell_local`] for an already-selected BigSpace cell.
///
/// This does not choose or mutate a cell; BigSpace owns re-splitting. It only
/// returns the f64 remainder that the terminal render `Transform` stores.
pub fn cell_local_remainder(point: GridPos, cell: Option<&CellCoord>, edge: Option<f64>) -> DVec3 {
    point - compose_cell_local(cell, edge, Vec3::ZERO)
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

/// Return the cell-local remainder of `abs` while retaining an entity's
/// existing cell.  BigSpace owns when the cell changes; bridge writeback uses
/// this inverse without inventing a second rebranch policy.
pub fn grid_local_remainder(
    entity: Entity,
    abs: GridPos,
    cell: &CellCoord,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> DVec3 {
    parent_grid(entity, q_parents, q_grids)
        .map(|grid| abs.0 - grid.grid_position_double(cell, &Transform::default()))
        .unwrap_or_else(|| panic!("grid-direct entity {entity:?} has no parent Grid"))
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
    let a = world_position(from, q_parents, q_grids, q_spatial).ok()?;
    let b = world_position(to, q_parents, q_grids, q_spatial).ok()?;
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

/// Convert a complete pose in the BigSpace root frame into the cell/local pose
/// stored by an entity whose parent is `target_grid`.
///
/// The target grid's world pose is resolved through the same typed hierarchy as
/// every other coordinate conversion. This is the canonical boundary for
/// camera/path or other authored pose writers; callers must not subtract a
/// grid translation in the root axes and then bin the result, because a
/// body-fixed grid can also be rotated.
pub fn world_pose_to_grid_local<F: QueryFilter>(
    position: GridPos,
    rotation: GridRot,
    target_grid: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(CellCoord, Transform)> {
    let (target_position, target_rotation) =
        world_pose(target_grid, q_parents, q_grids, q_spatial).ok()?;
    let grid = q_grids.get(target_grid).ok()?;
    let inverse = target_rotation.0.inverse();
    let target_local_position = inverse * (position - target_position);
    let (cell, translation) = grid.translation_to_grid(target_local_position);
    let local_rotation = (inverse * rotation.0).normalize();
    Some((
        cell,
        Transform::from_translation(translation).with_rotation(local_rotation.as_quat()),
    ))
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

/// Convert a complete floating-origin render pose into the absolute pose of a
/// BigSpace grid.
///
/// [`render_to_grid_absolute`] is intentionally a point-only helper. Gizmos,
/// cameras, and other rigid tools must use this pose variant so a rotated
/// local grid transforms orientation as well as position. The affine supplied
/// by BigSpace is rigid for grid frames; its inverse rotation is therefore the
/// render-to-grid orientation conversion.
pub fn render_pose_to_grid_absolute(
    grid: &Grid,
    render_position: RenderPos,
    render_rotation: GridRot,
) -> (GridPos, GridRot) {
    let local_origin = grid.local_floating_origin();
    let inverse = local_origin.grid_transform().inverse();
    let grid_relative = inverse.transform_point3(render_position.0);
    let rotation = DQuat::from_mat3(&inverse.matrix3) * render_rotation.0;
    (
        GridPos(grid_relative + grid.cell_to_float(&local_origin.cell())),
        GridRot(rotation.normalize()),
    )
}

/// Convert a complete absolute BigSpace-grid pose into the floating-origin
/// render frame used by Bevy's `GlobalTransform` and the transform gizmo.
///
/// This is the exact inverse of [`render_pose_to_grid_absolute`]. Keeping both
/// directions here prevents editor code from reconstructing a camera-relative
/// translation while accidentally leaving the orientation in another frame.
pub fn grid_absolute_pose_to_render(
    grid: &Grid,
    grid_position: GridPos,
    grid_rotation: GridRot,
) -> (RenderPos, GridRot) {
    let local_origin = grid.local_floating_origin();
    let grid_relative = grid_position.0 - grid.cell_to_float(&local_origin.cell());
    let transform = local_origin.grid_transform();
    let rotation = DQuat::from_mat3(&transform.matrix3) * grid_rotation.0;
    (
        RenderPos(transform.transform_point3(grid_relative)),
        GridRot(rotation.normalize()),
    )
}

/// Absolute world position of `entity`, seeded with an explicit
/// `(initial_cell, initial_tf)`. See [`world_pose_seeded`] (returns the full
/// pose); this returns the position only.
pub fn world_position_seeded<F: QueryFilter>(
    entity: Entity,
    initial_cell: Option<&CellCoord>,
    initial_tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Result<GridPos, CoordinateError> {
    world_pose_seeded(
        entity,
        initial_cell,
        initial_tf,
        q_parents,
        q_grids,
        q_spatial,
    )
    .map(|(position, _)| position)
}

/// Absolute world pose (position + rotation), seeded — the disjoint-query
/// variant of [`world_pose`], for entities not present in `q_spatial`.
pub fn world_pose_seeded<F: QueryFilter>(
    entity: Entity,
    initial_cell: Option<&CellCoord>,
    initial_tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Result<(GridPos, GridRot), CoordinateError> {
    // Same rotation-aware chain composition as [`world_position`], but seeded
    // with an explicit (cell, transform) for entities not present in
    // `q_spatial` (disjoint-query / `Without<…>` cases).
    let mut chain: Vec<(DVec3, Quat)> = Vec::with_capacity(8);
    let cell_off0 = match (initial_cell, q_parents.get(entity)) {
        (Some(cell), Ok(co)) => q_grids
            .get(co.parent())
            .map(|grid| grid.cell_to_float(cell))
            .map_err(|_| CoordinateError::CellCoordWithoutGrid {
                entity,
                parent: co.parent(),
            })?,
        (Some(_), Err(_)) => {
            return Err(CoordinateError::CellCoordWithoutGrid {
                entity,
                parent: entity,
            });
        }
        (None, _) => DVec3::ZERO,
    };
    chain.push((
        cell_off0 + initial_tf.translation.as_dvec3(),
        initial_tf.rotation,
    ));

    let mut current = entity;
    let mut visited = vec![entity];
    for _ in 0..32 {
        let parent = match q_parents.get(current) {
            Ok(co) => co.parent(),
            Err(_) => break,
        };
        let (cell, tf) = match q_spatial.get(parent) {
            Ok((c, t)) => (c.copied(), *t),
            Err(_) if q_parents.get(parent).is_err() => break,
            Err(_) => return Err(CoordinateError::MissingAncestorSpatial { entity, parent }),
        };
        if visited.contains(&parent) {
            return Err(CoordinateError::Cycle { entity: parent });
        };
        visited.push(parent);
        let local_position = match (cell.as_ref(), q_parents.get(parent)) {
            (Some(cell), Ok(co)) => q_grids
                .get(co.parent())
                .map(|grid| grid.grid_position_double(cell, &tf))
                .map_err(|_| CoordinateError::CellCoordWithoutGrid {
                    entity: parent,
                    parent: co.parent(),
                })?,
            (Some(_), Err(_)) => {
                return Err(CoordinateError::CellCoordWithoutGrid {
                    entity: parent,
                    parent,
                });
            }
            (None, _) => tf.translation.as_dvec3(),
        };
        chain.push((local_position, tf.rotation));
        current = parent;
    }
    if visited.len() == 32 && q_parents.get(current).is_ok() {
        return Err(CoordinateError::DepthExceeded { entity, limit: 32 });
    }

    let mut pos = bevy::math::DVec3::ZERO;
    let mut rot = DQuat::IDENTITY;
    for (off, local_rot) in chain.iter().rev() {
        pos += rot * off;
        rot *= local_rot.as_dquat();
    }
    Ok((GridPos(pos), GridRot(rot)))
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
    use crate::WorldGridConfig;
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
        let grid = WorldGridConfig::default().grid();
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

    #[test]
    fn render_pose_round_trips_position_and_rotation() {
        let grid = WorldGridConfig::default().grid();
        let cell = CellCoord::new(4, -3, 2);
        let local_rotation = DQuat::from_rotation_y(0.37) * DQuat::from_rotation_x(-0.19);
        let local = Transform::from_translation(Vec3::new(12.5, -4.0, 99.0))
            .with_rotation(local_rotation.as_quat());
        let rendered = grid.global_transform(&cell, &local);
        let (_, render_rotation, render_translation) = rendered.to_scale_rotation_translation();

        let (position, rotation) = render_pose_to_grid_absolute(
            &grid,
            RenderPos::from_render_f32(render_translation),
            GridRot::from_render_rotation(render_rotation),
        );

        assert!((position.0 - grid.grid_position_double(&cell, &local)).length() < 1e-3);
        assert!(rotation.0.angle_between(local_rotation) < 1e-6);

        let (back_position, back_rotation) =
            grid_absolute_pose_to_render(&grid, position, rotation);
        assert!((back_position.0 - render_translation.as_dvec3()).length() < 1e-3);
        assert!(back_rotation.0.angle_between(render_rotation.as_dquat()) < 1e-6);
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
    fn world_position_rejects_a_partial_spatial_chain() {
        let mut world = World::new();
        let root = world
            .spawn((
                grid(),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let missing_spatial = world.spawn(ChildOf(root)).id();
        let child = world
            .spawn((
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(missing_spatial),
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
        assert_eq!(
            world_position(child, &q_parents, &q_grids, &q_spatial),
            Err(CoordinateError::MissingAncestorSpatial {
                entity: child,
                parent: missing_spatial,
            })
        );
    }

    #[test]
    fn world_position_rejects_cell_coord_without_parent_grid() {
        let mut world = World::new();
        let parent = world
            .spawn((
                Transform::from_translation(Vec3::new(10.0, 20.0, 30.0)),
                GlobalTransform::default(),
            ))
            .id();
        let child = world
            .spawn((
                CellCoord::new(4, -2, 1),
                Transform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
                GlobalTransform::default(),
                ChildOf(parent),
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

        assert_eq!(
            world_position(child, &q_parents, &q_grids, &q_spatial),
            Err(CoordinateError::CellCoordWithoutGrid {
                entity: child,
                parent
            })
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
