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
//! - [`follow_mounted_cameras`] — each frame: compose the mount's double-precision
//!   local-grid pose with the authored offset and write the camera's
//!   `CellCoord`+`Transform` in that same Grid.
//!
//! Result: a rover cam at planet-scale distance from the origin renders with
//! the same precision as the free camera — no nested-camera caveat.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_render::SceneCamera;

use crate::camera::UsdCameraPose;

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
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for (cam, child_of, tf, pose) in q_new.iter() {
        if *pose != UsdCameraPose::Mounted {
            continue;
        }
        let parent = child_of.parent();
        if q_grids.contains(parent) {
            // Already grid-direct — nothing to rig, just mark it done.
            commands.entity(cam).try_insert(CameraMountResolved);
            continue;
        }

        // Nested under a moving prim → find the mount's enclosing grid through
        // the canonical BigSpace hierarchy resolver.
        let Some((grid, _)) = lunco_core::coords::ancestor_grid(parent, &q_parents, &q_grids)
        else {
            continue;
        };

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
/// The explicit pose authority is established before this system runs, so this
/// follower is the only writer of transforms for a mounted camera.
pub fn follow_mounted_cameras(
    mut q_cam: Query<
        (
            Entity,
            &MountedCamera,
            &mut CellCoord,
            &mut Transform,
            &ChildOf,
        ),
        (With<SceneCamera>,),
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<MountedCamera>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut commands: Commands,
) {
    for (camera, mounted, mut cell, mut tf, child_of) in q_cam.iter_mut() {
        let Some((mount_grid_entity, mount_grid)) =
            lunco_core::coords::ancestor_grid(mounted.mount, &q_parents, &q_grids)
        else {
            continue;
        };
        let Some((mount_position, mount_rotation)) = lunco_core::coords::grid_relative_pose(
            mounted.mount,
            mount_grid_entity,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            continue;
        };

        // Compose directly in the mount's live Grid frame. Never route this
        // through the heliocentric root: a re-pin or SOI migration must not
        // perturb a camera that is rigidly attached to the rover.
        let camera_position =
            mount_position + mount_rotation * mounted.offset.translation.as_dvec3();
        let camera_rotation = mount_rotation * mounted.offset.rotation.as_dquat();
        let (new_cell, new_translation) = mount_grid.translation_to_grid(camera_position);
        let new_transform =
            Transform::from_translation(new_translation).with_rotation(camera_rotation.as_quat());

        // Follow a mount that migrates grids as one atomic BigSpace operation.
        // The parent, cell, and local transform must never be observed apart.
        if child_of.parent() != mount_grid_entity {
            lunco_core::attach::migrate_to_grid(
                &mut commands,
                camera,
                mount_grid_entity,
                new_cell,
                new_transform,
            );
        } else {
            *cell = new_cell;
            *tf = new_transform;
        }
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
    fn mounted_follower_uses_the_mount_grid_without_root_round_trip() {
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

        let first_local = {
            let world = app.world();
            (
                *world.get::<CellCoord>(camera).unwrap(),
                *world.get::<Transform>(camera).unwrap(),
            )
        };

        // A site pin/rebranch can move the distant root by astronomical-scale
        // amounts. The camera pose in the mount's Grid must not change.
        app.world_mut()
            .entity_mut(root)
            .insert(Transform::from_xyz(-9.0e11, 4.0e11, 7.0e11));
        app.update();

        let second_local = {
            let world = app.world();
            (
                *world.get::<CellCoord>(camera).unwrap(),
                *world.get::<Transform>(camera).unwrap(),
            )
        };
        assert_eq!(first_local.0, second_local.0);
        assert_eq!(first_local.1, second_local.1);

        let world = app.world_mut();
        let mut state: bevy::ecs::system::SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = bevy::ecs::system::SystemState::new(world);
        let (parents, grids, spatial) = state.get(world).unwrap();
        let (mount_position, mount_rotation) =
            lunco_core::coords::grid_relative_pose(rover, grid, &parents, &grids, &spatial)
                .unwrap();
        let expected_position = mount_position + mount_rotation * offset.translation.as_dvec3();
        let expected_rotation = mount_rotation * offset.rotation.as_dquat();
        let camera_position = grids
            .get(grid)
            .unwrap()
            .grid_position_double(&first_local.0, &first_local.1);
        let camera_rotation = first_local.1.rotation.as_dquat();

        assert!(
            (camera_position - expected_position).length() < 1e-4,
            "camera pose must preserve the mount's composed world position; actual={camera_position:?}, expected={expected_position:?}"
        );
        assert!(
            camera_rotation.abs_diff_eq(expected_rotation, 1e-5),
            "camera pose must preserve the mount's composed world rotation: actual={camera_rotation:?}, expected={expected_rotation:?}"
        );
    }

    #[test]
    fn mounted_follower_migrates_atomically_with_the_mount_grid() {
        let mut app = App::new();
        app.add_systems(Update, follow_mounted_cameras);
        let root = app.world_mut().spawn(Transform::default()).id();
        let source_grid = app
            .world_mut()
            .spawn((Grid::new(1_000.0, 0.0), ChildOf(root)))
            .id();
        let target_grid = app
            .world_mut()
            .spawn((Grid::new(1_000.0, 0.0), ChildOf(root)))
            .id();
        let rover = app
            .world_mut()
            .spawn((
                CellCoord::new(2, 0, 0),
                Transform::from_xyz(25.0, 0.0, -10.0),
                ChildOf(target_grid),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                MountedCamera {
                    mount: rover,
                    offset: Transform::from_xyz(0.0, 2.0, 5.0),
                },
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(source_grid),
            ))
            .id();

        app.update();

        let world = app.world();
        assert_eq!(world.get::<ChildOf>(camera).unwrap().parent(), target_grid);
        assert_eq!(world.get::<CellCoord>(camera).unwrap().x, 2);
        assert_eq!(world.get::<CellCoord>(camera).unwrap().y, 0);
        assert_eq!(world.get::<CellCoord>(camera).unwrap().z, 0);
    }
}
