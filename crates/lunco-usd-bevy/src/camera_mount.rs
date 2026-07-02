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
//! - [`resolve_camera_mounts`] — once per camera: a `Camera3d` whose `ChildOf`
//!   is not a `Grid` is reparented to the mount's grid and given a
//!   [`MountedCamera`]; a grid-direct camera is just marked resolved.
//! - [`follow_mounted_cameras`] — each frame: write the mount's double-precision
//!   world pose × offset back into the camera's grid-local `CellCoord`+`Transform`.
//!
//! Result: a rover cam at planet-scale distance from the origin renders with
//! the same precision as the free camera — no nested-camera caveat.

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

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

/// Realise nested `def Camera`s as grid-direct mount followers; leave
/// grid-direct cameras (top-level scene cameras, the avatar eye) untouched.
/// Runs once per camera. Retries next frame if the mount's grid isn't spawned
/// yet (async scene load).
pub fn resolve_camera_mounts(
    q_new: Query<(Entity, &ChildOf, &Transform), (With<Camera3d>, Without<CameraMountResolved>)>,
    q_is_grid: Query<(), With<Grid>>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for (cam, child_of, tf) in q_new.iter() {
        let parent = child_of.parent();
        if q_is_grid.contains(parent) {
            // Already grid-direct — nothing to rig, just mark it done.
            commands.entity(cam).insert(CameraMountResolved);
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
        commands.entity(cam).insert((
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
pub fn follow_mounted_cameras(
    mut q_cam: Query<(&MountedCamera, &mut CellCoord, &mut Transform, &ChildOf), With<Camera3d>>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<MountedCamera>>,
    q_grids: Query<&Grid>,
) {
    for (mounted, mut cell, mut tf, child_of) in q_cam.iter_mut() {
        let Ok((m_cell, m_tf)) = q_spatial.get(mounted.mount) else {
            continue;
        };
        let m_cell = m_cell.copied().unwrap_or_default();
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            continue;
        };

        // Mount world pose: position in double precision; rotation is
        // precision-safe (a quaternion doesn't accumulate cell-offset error).
        let mount_world: DVec3 = grid.grid_position_double(&m_cell, m_tf);
        let mount_rot = m_tf.rotation;

        // Camera world pose = mount · offset.
        let cam_world = mount_world + (mount_rot * mounted.offset.translation).as_dvec3();
        let cam_rot = mount_rot * mounted.offset.rotation;

        // Back into the camera's grid (cell + local transform).
        let (new_cell, new_local) = grid.translation_to_grid(cam_world);
        *cell = new_cell;
        tf.translation = new_local;
        tf.rotation = cam_rot;
    }
}
