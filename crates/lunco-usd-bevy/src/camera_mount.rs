//! Rover/vehicle-mounted cameras as **grid-direct followers**.
//!
//! big_space requires the `FloatingOrigin` to sit on a GRID-DIRECT entity
//! ("FloatingOrigin must be on a Grid"), so a camera literally parented under a
//! moving prim could never host the active-view origin at full precision. A
//! `def Camera` authored nested under a rover is therefore **realised as a
//! grid-direct camera that FOLLOWS the mount** each frame — exactly the pattern
//! `SpringArmCamera` uses. The nested USD authoring only supplies the mount
//! offset (its local `xformOp:translate` + `lunco:cameraLookAt` rotation).
//!
//! Two systems:
//! - [`resolve_camera_mounts`] — once per camera explicitly authored with
//!   `LunCoCameraAPI:cameraPose = "mounted"`: realise it as a grid-direct
//!   [`MountedCamera`] follower. Other camera poses remain in USD composition.
//! - [`follow_mounted_cameras`] — each frame: write the mount's double-precision
//!   world pose × offset back into the camera's grid-local `CellCoord`+`Transform`.
//!
//! Result: a rover cam at planet-scale distance from the origin renders with
//! the same precision as the free camera — no nested-camera caveat.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_render::SceneCamera;

use crate::camera::UsdCameraPose;

/// Walk this far up a `ChildOf` chain looking for the enclosing `Grid`.
const MAX_MOUNT_GRID_WALK: usize = 16;

/// A camera that rigidly rides `mount` at a fixed local `offset` (grid-direct;
/// see module docs). Realised from a `def Camera` authored nested under `mount`.
#[derive(Component)]
pub struct MountedCamera {
    /// The prim this camera rides (its original USD parent entity).
    pub mount: Entity,
    /// Fixed pose relative to the mount (authored translate + lookAt rotation).
    pub offset: Transform,
}

/// One-shot marker: this camera's mount has been resolved, so the resolver
/// skips it thereafter (grid-direct cameras get it too — nothing more to do).
#[derive(Component)]
pub struct CameraMountResolved;

/// Realise only cameras whose explicit [`UsdCameraPose`] is `Mounted` as
/// grid-direct mount followers.
/// Runs once per camera. Retries next frame if the mount's grid isn't spawned
/// yet (async scene load).
///
/// `mounted` declares a static camera-local offset and gives the follower sole
/// pose authority. Projection rejects a mounted camera with local transform time
/// samples; a path driver changes the pose to `Path` before this system runs.
pub fn resolve_camera_mounts(
    q_new: Query<
        (Entity, &ChildOf, &Transform, &UsdCameraPose),
        (With<SceneCamera>, Without<CameraMountResolved>),
    >,
    q_is_grid: Query<(), With<Grid>>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for (cam, child_of, tf, pose) in q_new.iter() {
        if *pose != UsdCameraPose::Mounted {
            continue;
        }
        let parent = child_of.parent();
        if q_is_grid.contains(parent) {
            // Already grid-direct — nothing to rig, just mark it done.
            commands.entity(cam).try_insert(CameraMountResolved);
            continue;
        }

        // Nested under a moving prim → find the mount's enclosing grid.
        let mut node = parent;
        let mut grid = None;
        for _ in 0..MAX_MOUNT_GRID_WALK {
            if q_is_grid.contains(node) {
                grid = Some(node);
                break;
            }
            match q_parents.get(node) {
                Ok(c) => node = c.parent(),
                Err(_) => break,
            }
        }
        let Some(grid) = grid else { continue }; // grid not ready — retry next frame

        // Reparent to the grid and capture the authored local pose as the mount
        // offset. `follow_mounted_cameras` corrects the grid-local position the
        // same frame (Update commands flush before PostUpdate), and the camera
        // is inactive during load, so there is no visible pop.
        commands.entity(cam).try_insert((
            MountedCamera {
                mount: parent,
                offset: *tf,
            },
            CellCoord::default(),
            lunco_core::GridAnchor,
            ChildOf(grid),
            CameraMountResolved,
        ));
        info!("[camera] {cam:?} mounted on {parent:?} → grid-direct follower");
    }
}

/// Keep each mounted camera rigidly at `mount · offset`, computed in double
/// precision so a far-from-origin rover cam stays jitter-free (the whole point
/// of making it grid-direct). Mirrors `chase_camera_system`'s grid write-back.
///
/// Assumes the camera shares its mount's grid (established by
/// [`resolve_camera_mounts`]); a rover that migrates grids would need the same
/// cross-grid handling `spring_arm_system` has — deferred (rovers stay put).
///
/// The explicit pose authority is established before this system runs, so this
/// follower is the only writer of transforms for a mounted camera.
pub fn follow_mounted_cameras(
    mut q_cam: Query<
        (&MountedCamera, &mut CellCoord, &mut Transform, &ChildOf),
        (With<SceneCamera>,),
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<MountedCamera>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
) {
    for (mounted, mut cell, mut tf, child_of) in q_cam.iter_mut() {
        let Some((mount_world, mount_rotation)) =
            lunco_core::coords::world_pose(mounted.mount, &q_parents, &q_grids, &q_spatial)
        else {
            continue;
        };
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            continue;
        };

        // Compose the camera's root-world pose before its canonical grid write-back.
        let cam_world = mount_world + mount_rotation.0 * mounted.offset.translation.as_dvec3();
        let cam_rotation =
            lunco_core::coords::GridRot(mount_rotation.0 * mounted.offset.rotation.as_dquat());

        // Camera world pose = mount · offset.
        let Some((new_cell, new_tf)) = lunco_core::coords::world_pose_to_live_grid_local(
            cam_world,
            cam_rotation,
            child_of.parent(),
            grid,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            continue;
        };
        *cell = new_cell;
        tf.translation = new_tf.translation;
        tf.rotation = new_tf.rotation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_camera_never_enters_mounted_follower_path() {
        let mut app = App::new();
        app.add_systems(Update, resolve_camera_mounts);
        let grid = app.world_mut().spawn(Grid::new(2000.0, 0.0)).id();
        let scene_root = app.world_mut().spawn(ChildOf(grid)).id();
        let avatar = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                UsdCameraPose::Avatar,
                Transform::default(),
                ChildOf(scene_root),
            ))
            .id();

        app.update();

        assert!(app.world().get::<MountedCamera>(avatar).is_none());
        assert!(app.world().get::<CameraMountResolved>(avatar).is_none());
    }

    #[test]
    fn authored_nested_camera_never_enters_mounted_follower_path() {
        let mut app = App::new();
        app.add_systems(Update, resolve_camera_mounts);
        let grid = app.world_mut().spawn(Grid::new(2000.0, 0.0)).id();
        let scene_root = app.world_mut().spawn(ChildOf(grid)).id();
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                UsdCameraPose::Authored,
                Transform::default(),
                ChildOf(scene_root),
            ))
            .id();

        app.update();

        assert!(app.world().get::<MountedCamera>(camera).is_none());
        assert!(app.world().get::<CameraMountResolved>(camera).is_none());
    }

    #[test]
    fn mounted_camera_is_the_only_nested_camera_that_gets_a_follower() {
        let mut app = App::new();
        app.add_systems(Update, resolve_camera_mounts);
        let grid = app.world_mut().spawn(Grid::new(2000.0, 0.0)).id();
        let rover = app.world_mut().spawn(ChildOf(grid)).id();
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                UsdCameraPose::Mounted,
                Transform::from_xyz(0.0, 1.0, 2.0),
                ChildOf(rover),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<MountedCamera>(camera).unwrap().mount,
            rover
        );
        assert_eq!(
            app.world().get::<ChildOf>(camera).unwrap().parent(),
            grid,
            "the follower is grid-direct"
        );
    }

    #[test]
    fn mounted_follower_round_trips_the_composed_pose_through_a_rotated_grid() {
        let mut app = App::new();
        app.add_systems(Update, follow_mounted_cameras);
        let root = app
            .world_mut()
            .spawn(Transform::from_rotation(Quat::from_rotation_y(0.35)))
            .id();
        let grid = app
            .world_mut()
            .spawn((
                Grid::new(1_000.0, 100.0),
                Transform::from_rotation(Quat::from_rotation_y(-0.6)),
                ChildOf(root),
            ))
            .id();
        let rover = app
            .world_mut()
            .spawn((
                CellCoord::default(),
                Transform::from_xyz(400.0, 0.0, -400.0).with_rotation(Quat::from_rotation_y(0.8)),
                ChildOf(grid),
            ))
            .id();
        let offset = Transform::from_xyz(0.0, 2.0, 5.0).with_rotation(Quat::from_rotation_x(-0.2));
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                MountedCamera {
                    mount: rover,
                    offset,
                },
                CellCoord::default(),
                Transform::default(),
                ChildOf(grid),
            ))
            .id();

        app.update();

        let world = app.world_mut();
        let mut state: bevy::ecs::system::SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = bevy::ecs::system::SystemState::new(world);
        let (parents, grids, spatial) = state.get(world).unwrap();
        let (mount_position, mount_rotation) =
            lunco_core::coords::world_pose(rover, &parents, &grids, &spatial).unwrap();
        let (camera_position, camera_rotation) =
            lunco_core::coords::world_pose(camera, &parents, &grids, &spatial).unwrap();
        let expected_position = mount_position.0 + mount_rotation.0 * offset.translation.as_dvec3();
        let expected_rotation = mount_rotation.0 * offset.rotation.as_dquat();

        assert!(
            (camera_position.0 - expected_position).length() < 1e-4,
            "camera pose must preserve the mount's composed world position; actual={camera_position:?}, expected={expected_position:?}"
        );
        assert!(
            camera_rotation.0.angle_between(expected_rotation) < 1e-4,
            "camera pose must preserve the mount's composed world rotation"
        );
    }
}
