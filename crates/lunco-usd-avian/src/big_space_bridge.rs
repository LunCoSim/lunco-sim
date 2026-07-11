//! Phase 5 — avian ↔ big_space physics bridge (Option B core).
//!
//! big_space's stated principle (its CHANGELOG): physics runs on the integer-cell
//! representation (`BigSpaceCorePlugin`), *separate* from the origin-relative
//! rendering propagation. Our avian is built with `f64`, so its `Position` /
//! `Rotation` are precise at any magnitude — the only precision leak is avian's
//! *own* f32 `transform_to_position` / `position_to_transform`, which round-trip
//! through the origin-relative f32 `GlobalTransform` (32 m ULP at 1 AU once the
//! floating origin travels — Phase 6).
//!
//! This bridge disables those two f32 sync systems and owns the sync instead,
//! routing through the f64 cell+Transform chain (`world_pose`), so the floating
//! origin can travel without corrupting physics.
//!
//! Body-type split avoids the read↔writeback change-detection fight:
//! - **Static / Kinematic**: read every step (`cell+Transform → Position`), so
//!   they follow parent-grid motion. No writeback (their `Transform` is owned
//!   externally — scene/ephemeris/gameplay).
//! - **Dynamic**: the solver owns `Position`; writeback every step
//!   (`Position → cell+Transform`) for rendering. Initialised at spawn by
//!   avian's required-component hook. No per-step read (would fight the solver).

use bevy::prelude::*;
use bevy::math::{DQuat, DVec3};
use avian3d::prelude::*;
use avian3d::schedule::{PhysicsSchedule, PhysicsStepSystems, PhysicsSystems};
use avian3d::physics_transform::{PhysicsTransformConfig, PhysicsTransformSystems, Position, Rotation};
use big_space::prelude::{CellCoord, Grid};
use lunco_core::coords::{ancestor_grid, world_pose, world_pose_seeded};

/// Decouple avian from the f32 render `Transform`; own the f64
/// `Position` ↔ (cell, `Transform`) bridge.
pub struct BigSpacePhysicsBridgePlugin;

impl Plugin for BigSpacePhysicsBridgePlugin {
    fn build(&self, app: &mut App) {
        // Replace avian's f32 GT↔Position sync with the f64 cell-chain bridge.
        // Keep `propagate_before_physics` + `transform_to_collider_scale` at
        // their defaults (not the precision leak; minimises the change).
        app.insert_resource(PhysicsTransformConfig {
            transform_to_position: false,
            position_to_transform: false,
            ..default()
        });
        app.add_systems(
            PhysicsSchedule,
            cell_to_position
                .in_set(PhysicsSystems::Prepare)
                // Run before the entire physics STEP (PhysicsStepSystems:
                // BroadPhase → NarrowPhase → Solver/joints/substep → SpatialQuery
                // → Finalize), which is where Position/Rotation are consumed, and
                // before avian's own (disabled) TransformToPosition slot. Pinning
                // against PhysicsStepSystems::First (not PhysicsSystems::
                // StepSimulation) is what resolves the schedule-ambiguity panic:
                // the solver systems live in PhysicsStepSystems, a parallel chain.
                .after(PhysicsSystems::First)
                .before(PhysicsStepSystems::First)
                .before(PhysicsTransformSystems::TransformToPosition),
        );
        app.add_systems(
            PhysicsSchedule,
            position_to_cell
                .in_set(PhysicsSystems::Writeback)
                // After the entire physics step + avian's (disabled)
                // PositionToTransform, before the post-step Last slot.
                .after(PhysicsStepSystems::Last)
                .after(PhysicsTransformSystems::PositionToTransform)
                .before(PhysicsSystems::Last),
        );
    }
}

/// READ: big_space cell+Transform → avian f64 `Position`/`Rotation` for
/// non-dynamic bodies. Runs every step so they follow parent-grid motion; the
/// floating origin never enters this loop.
#[allow(clippy::type_complexity)]
fn cell_to_position(
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut q_bodies: Query<(
        Entity,
        &CellCoord,
        &Transform,
        &mut Position,
        &mut Rotation,
        &RigidBody,
    )>,
) {
    for (e, cell, tf, mut pos, mut rot, rb) in &mut q_bodies {
        if matches!(rb, RigidBody::Dynamic) {
            continue;
        }
        let (p, r) = world_pose_seeded(e, cell, tf, &q_parents, &q_grids, &q_spatial);
        pos.0 = p;
        rot.0 = r;
    }
}

/// WRITEBACK: solver f64 `Position`/`Rotation` → big_space cell+Transform for
/// dynamic bodies (rendering follows physics). World pose → parent-grid-local
/// (via the parent grid's world pose) → `translation_to_grid`.
#[allow(clippy::type_complexity)]
fn position_to_cell(
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_grids_check: Query<(), With<Grid>>,
    // `Without<RigidBody>` keeps this disjoint from `q_bodies` (which mutates
    // CellCoord/Transform). The parent grid has no RigidBody, so it is visible.
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<RigidBody>>,
    mut q_bodies: Query<(
        Entity,
        &Position,
        &Rotation,
        &mut CellCoord,
        &mut Transform,
        &RigidBody,
    )>,
) {
    for (e, pos, rot, mut cell, mut tf, rb) in &mut q_bodies {
        if !matches!(rb, RigidBody::Dynamic) {
            continue;
        }
        let Some(grid_e) = ancestor_grid(e, &q_parents, &q_grids_check) else {
            continue;
        };
        let Ok(grid) = q_grids.get(grid_e) else {
            continue;
        };
        let (g_cell, g_tf) = match q_spatial.get(grid_e) {
            Ok((c, t)) => (c.copied().unwrap_or_default(), *t),
            Err(_) => continue,
        };
        let (gp, grot) = world_pose_seeded(grid_e, &g_cell, &g_tf, &q_parents, &q_grids, &q_spatial);
        let inv = grot.inverse();
        let local_pos = inv * (pos.0 - gp);
        let local_rot: DQuat = inv * rot.0;
        let (new_cell, new_tf) = grid.translation_to_grid(local_pos);
        *cell = new_cell;
        tf.translation = new_tf;
        tf.rotation = local_rot.as_quat();
    }
}

#[cfg(test)]
mod tests {
    //! Round-trip proof of the bridge math at astronomical magnitude + with a
    //! rotating ancestor grid: `world_pose(body)` → world (pos, rot); the
    //! writeback conversion (world → parent-grid-local → `translation_to_grid`)
    //! must reproduce the original (cell, translation). This is the contract
    //! the live drive-replay relies on.
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn world_pose_round_trips_through_translation_to_grid() {
        let mut world = World::new();
        // Parent grid at a large heliocentric offset (cell (150_000_000, 0, 0)
        // on a 1 km edge ≈ 1.5e11 m — the 16 km ULP regime), rotated 37° about Y
        // (a non-trivial ancestor rotation, the "spinning grid" case).
        let edge = 1_000.0_f32;
        let grid = Grid::new(edge, 0.0);
        let grid_cell = CellCoord::new(150_000_000, 0, 0);
        let grid_rot = Quat::from_rotation_y(0.6435); // ~37°
        // A grid's cell is relative to its PARENT grid — nest under a root grid
        // (cell 0) so grid_e's cell×edge contributes 1.5e11 m.
        let root_grid = world
            .spawn((Grid::new(edge, 0.0), CellCoord::ZERO, Transform::default(), GlobalTransform::default()))
            .id();
        let grid_e = world
            .spawn((grid, grid_cell, Transform::from_rotation(grid_rot), GlobalTransform::default(), ChildOf(root_grid)))
            .id();
        // Body in cell (3, -1, 2), translation (120.0, -40.0, 80.0), yaw 10°.
        let b_cell = CellCoord::new(3, -1, 2);
        let b_tf = Transform::from_xyz(120.0, -40.0, 80.0)
            .with_rotation(Quat::from_rotation_y(0.1745));
        let body = world
            .spawn((b_cell, b_tf, GlobalTransform::default(), ChildOf(grid_e)))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(), With<Grid>>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_grids, q_grids_check, q_spatial) = state.get(&world).expect("read-only queries always validate");

        // READ: body world pose.
        let (p, r) = world_pose(body, &q_parents, &q_grids, &q_spatial).unwrap();
        // sanity: it is far from the origin (≈1.5e11 m).
        assert!(p.length() > 1.0e11, "world pose {p:?} not at astronomical scale");

        // WRITEBACK conversion: world → parent-grid-local → translation_to_grid.
        let (gp, grot) = world_pose(grid_e, &q_parents, &q_grids, &q_spatial).unwrap();
        let inv = grot.inverse();
        let local_pos = inv * (p - gp);
        let local_rot = inv * r;
        let g = Grid::new(edge, 0.0);
        let (cell_back, tf_back) = g.translation_to_grid(local_pos);

        assert_eq!((cell_back.x, cell_back.y, cell_back.z), (3, -1, 2), "cell {cell_back:?}");
        assert!(
            (tf_back - Vec3::new(120.0, -40.0, 80.0)).length() < 1e-2,
            "translation {tf_back:?}"
        );
        // The writeback's local rotation (= inv(grid_world_rot) × body_world_rot)
        // must reproduce the body's authored local rotation — it is a direct
        // child of the grid, so the grid rotation cancels.
        let rot_err = local_rot.angle_between(b_tf.rotation.as_dquat()).abs();
        assert!(rot_err < 1e-4, "rotation error {rot_err}");
    }
}
