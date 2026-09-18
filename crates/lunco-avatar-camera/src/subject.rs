//! Subject-binding camera transactions.
//!
//! This module owns the camera half of control and follow transitions. The
//! avatar authority package validates and commits control; it emits the typed
//! [`lunco_camera_core::BindCameraTarget`] or
//! [`lunco_camera_core::ClearCameraBinding`] event for this package to realize.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::{
    CurrentRegionArrival, OrbitReturnBehavior, OrbitUserInput, OrbitViewHistory, OrbitViewReturn,
    RadialArrival,
};
use lunco_camera_core::{
    BindCameraTarget, CameraFollow, CameraPoseLock, ClearCameraBinding, FollowAttitude,
    FollowTarget, FreeFlightCamera, OrbitCamera, SpringArmCamera, SurfaceCamera,
    SurfaceRelativeMode,
};
use lunco_celestial::CelestialBody;
use lunco_celestial_spatial_core::{
    LocalGravityField, surface_axes_for_grid_position, surface_axes_in_grid,
};
use lunco_control_core::ControlLink;
use lunco_core::on_command;
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_environment::GravityBody;
use lunco_spatial::attach::{migrate_to_grid, migrate_to_grid_local_pose};

/// A target that has an authored control profile or a Modelica actuation
/// backend has a meaningful heading for the default camera policy.
type Controllable = bevy::prelude::Or<(
    bevy::prelude::With<lunco_control_core::ControlBinding>,
    bevy::prelude::With<lunco_cosim_core::SimComponent>,
)>;

fn surface_target_frame(
    target_position: DVec3,
    target_rotation: DQuat,
    target_grid: Entity,
    body_entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
) -> Option<(Vec3, Vec3, Vec3, f32)> {
    let (east, north, up) = surface_axes_for_grid_position(
        target_grid,
        target_position,
        body_entity,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let heading =
        lunco_camera_core::math::surface_camera_angles(east, north, up, target_rotation.as_quat())
            .0;
    Some((east, north, up, heading))
}

fn migrate_avatar_to_target_grid(
    commands: &mut Commands,
    avatar: Entity,
    target_grid: Entity,
    final_local_position: DVec3,
    final_rotation: Quat,
    q_grids: &Query<&Grid>,
) -> Result<(), String> {
    if target_grid == Entity::PLACEHOLDER {
        return Err("target has a placeholder Grid frame".to_string());
    }
    let target_grid_ref = q_grids
        .get(target_grid)
        .map_err(|_| format!("target Grid {target_grid:?} is not live"))?;
    migrate_to_grid_local_pose(
        commands,
        avatar,
        target_grid,
        target_grid_ref,
        final_local_position,
        final_rotation.as_dquat(),
    );
    Ok(())
}

fn get_grid_for_entity(
    mut entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
) -> Option<Entity> {
    if q_grids.contains(entity) {
        return Some(entity);
    }
    for _ in 0..lunco_spatial::MAX_HIERARCHY_WALK_DEPTH {
        let parent = q_parents.get(entity).ok()?.parent();
        if q_grids.contains(parent) {
            return Some(parent);
        }
        entity = parent;
    }
    None
}

fn replace_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    message: Option<String>,
) {
    lunco_camera_core::replace_camera_diagnostic(
        diagnostics,
        "avatar-camera",
        "LocalEmbodiment",
        message,
    );
}

fn subject_camera_state_error(camera: Entity) -> String {
    format!("camera {camera:?} is not a complete local camera rig")
}

/// Apply one subject-binding camera pose.
///
/// `clear_control` is used only by the explicit `FollowTarget` command. The
/// possession event leaves the already committed `ControlLink` untouched, so
/// authority and presentation remain separate transactions.
fn apply_subject_camera(
    commands: &mut Commands,
    camera: Entity,
    target: Entity,
    camera_transform: &Transform,
    follow: CameraFollow,
    track_heading: bool,
    target_gravity: Option<GravityBody>,
    clear_control: bool,
    q_grids: &Query<&Grid>,
    q_parents: &Query<&ChildOf>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
) -> Result<(), String> {
    let target_grid = get_grid_for_entity(target, q_parents, q_grids)
        .ok_or_else(|| "target has no live Grid frame".to_string())?;
    let (target_position, target_rotation) = lunco_spatial::coords::grid_relative_pose(
        target,
        target_grid,
        q_parents,
        q_grids,
        q_spatial,
    )
    .ok_or_else(|| "target is not spatially reachable from its Grid".to_string())?;

    let (distance, vertical_offset, pitch) = match follow {
        CameraFollow::Orbit => (50.0, 0.0, -0.25),
        CameraFollow::Chase => (25.0, 3.0, -0.25),
        CameraFollow::Heading => (15.0, 2.0, -0.25),
    };
    let surface_frame = target_gravity.and_then(|gravity| {
        surface_target_frame(
            target_position,
            target_rotation,
            target_grid,
            gravity.body_entity,
            q_parents,
            q_grids,
            q_spatial,
        )
    });
    let (current_yaw, current_pitch, _) = camera_transform.rotation.to_euler(EulerRot::YXZ);
    let (yaw, pitch) = if matches!(follow, CameraFollow::Heading) {
        (0.0, pitch)
    } else {
        (current_yaw, current_pitch)
    };
    let rotation = if matches!(follow, CameraFollow::Heading) {
        surface_frame
            .map(|(east, north, up, heading)| {
                lunco_camera_core::math::surface_camera_rotation(
                    east,
                    north,
                    up,
                    heading + yaw,
                    pitch,
                )
            })
            .unwrap_or_else(|| Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0))
    } else {
        Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
    };
    let final_local_position = target_position
        + rotation.mul_vec3(Vec3::Z).as_dvec3() * distance
        + surface_frame
            .map(|(_, _, up, _)| up.as_dvec3())
            .unwrap_or(DVec3::Y)
            * vertical_offset as f64;

    migrate_avatar_to_target_grid(
        commands,
        camera,
        target_grid,
        final_local_position,
        rotation,
        q_grids,
    )?;

    let mut camera_commands = commands.entity(camera);
    camera_commands
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitCamera>()
        .remove::<SurfaceRelativeMode>()
        .remove::<GravityBody>()
        .try_insert(SpringArmCamera {
            target,
            distance,
            yaw,
            pitch,
            damping: match follow {
                CameraFollow::Orbit => None,
                CameraFollow::Chase => Some(0.1),
                CameraFollow::Heading => Some(0.05),
            },
            vertical_offset,
            track_heading,
            attitude: match follow {
                CameraFollow::Orbit => FollowAttitude::WorldLocked,
                CameraFollow::Chase => FollowAttitude::FullAttitude,
                CameraFollow::Heading => FollowAttitude::Heading,
            },
        });
    if matches!(follow, CameraFollow::Heading) {
        if let Some(gravity) = target_gravity {
            camera_commands
                .try_insert(gravity)
                .try_insert(SurfaceRelativeMode);
        }
    }
    if clear_control {
        camera_commands.remove::<ControlLink>();
    }
    Ok(())
}

/// Realize the camera half of an already committed `AcquireControl`.
pub(crate) fn on_bind_camera_target(
    trigger: On<BindCameraTarget>,
    mut commands: Commands,
    q_avatar: Query<
        (&Transform, Option<&SpringArmCamera>, Has<CameraPoseLock>),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
    q_target: Query<(Option<&CameraFollow>, Option<&GravityBody>), Controllable>,
    q_gravity: Query<&GravityBody>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let request = trigger.event();
    let Ok((camera_transform, existing_spring, cinematic_lock)) = q_avatar.get(request.camera)
    else {
        replace_diagnostic(
            &mut diagnostics,
            Some(subject_camera_state_error(request.camera)),
        );
        return;
    };
    replace_diagnostic(&mut diagnostics, None);
    if cinematic_lock {
        return;
    }
    if existing_spring.is_some_and(|arm| arm.target == request.target) {
        return;
    }

    let (follow, track_heading) = q_target
        .get(request.target)
        .map(|(follow, _)| {
            let follow = follow.copied().unwrap_or_default();
            (follow, matches!(follow, CameraFollow::Heading))
        })
        .unwrap_or((CameraFollow::Heading, false));
    let target_gravity = q_gravity.get(request.target).ok().copied();
    if let Err(error) = apply_subject_camera(
        &mut commands,
        request.camera,
        request.target,
        camera_transform,
        follow,
        track_heading,
        target_gravity,
        false,
        &q_grids,
        &q_parents,
        &q_spatial,
    ) {
        warn!(
            camera = ?request.camera,
            target = ?request.target,
            "[possess] camera binding refused: {error}"
        );
        replace_diagnostic(&mut diagnostics, Some(error));
        return;
    }
    info!(camera = ?request.camera, target = ?request.target, "[possess] camera binding committed");
}

/// Follow a target without acquiring control.
#[on_command(FollowTarget)]
pub(crate) fn on_follow_command(
    trigger: On<FollowTarget>,
    mut commands: Commands,
    q_avatar: Query<
        (&Transform, Option<&SpringArmCamera>, Has<CameraPoseLock>),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
    q_target: Query<Entity, Controllable>,
    q_gravity: Query<&GravityBody>,
    local_avatar: Option<Res<lunco_embodiment_core::roles::TheLocalEmbodiment>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let camera = match lunco_embodiment_core::roles::resolve_requested_or_local(
        cmd.camera,
        local_avatar.as_deref(),
    ) {
        Ok(camera) => camera,
        Err(error) => {
            warn!(target = ?cmd.target, "[follow] refused: {error}");
            replace_diagnostic(&mut diagnostics, Some(error));
            return;
        }
    };
    let Ok((camera_transform, existing_spring, cinematic_lock)) = q_avatar.get(camera) else {
        let error = subject_camera_state_error(camera);
        warn!(target = ?cmd.target, "[follow] refused: {error}");
        replace_diagnostic(&mut diagnostics, Some(error));
        return;
    };
    replace_diagnostic(&mut diagnostics, None);
    if cinematic_lock {
        return;
    }
    if existing_spring.is_some_and(|arm| arm.target == cmd.target) {
        return;
    }
    let target_gravity = q_gravity.get(cmd.target).ok().copied();
    let track_heading = q_target.contains(cmd.target);
    if let Err(error) = apply_subject_camera(
        &mut commands,
        camera,
        cmd.target,
        camera_transform,
        CameraFollow::Heading,
        track_heading,
        target_gravity,
        true,
        &q_grids,
        &q_parents,
        &q_spatial,
    ) {
        warn!(target = ?cmd.target, "[follow] refused: {error}");
        replace_diagnostic(&mut diagnostics, Some(error));
    }
}

/// Restore the local camera after a possession release.
pub(crate) fn on_clear_camera_binding(
    trigger: On<ClearCameraBinding>,
    mut commands: Commands,
    mut q_avatar: Query<
        (
            &mut Transform,
            &mut CellCoord,
            Option<&SurfaceCamera>,
            &ChildOf,
            Option<&OrbitViewReturn>,
            Option<&OrbitCamera>,
            Option<&mut OrbitViewHistory>,
            Has<OrbitUserInput>,
            Has<CameraPoseLock>,
        ),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    q_bodies: Query<&CelestialBody>,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial_core::OrbitalViewPin>>,
    gravity: Res<LocalGravityField>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
) {
    let camera = trigger.event().camera;
    let Ok((
        mut transform,
        mut cell,
        surface,
        child_of,
        return_state,
        current_orbit,
        mut orbit_history,
        orbit_user_input,
        cinematic_lock,
    )) = q_avatar.get_mut(camera)
    else {
        return;
    };
    if cinematic_lock {
        return;
    }
    if orbit_user_input {
        if let (Some(orbit), Some(history)) = (current_orbit, orbit_history.as_deref_mut()) {
            if let Ok(body) = q_bodies.get(orbit.target) {
                if !history.remember_camera_pose(body.ephemeris_id, orbit) {
                    warn!(
                        target: "avatar-camera",
                        body = body.ephemeris_id,
                        "discarding non-finite user orbit pose"
                    );
                }
            }
        }
    }
    let return_state = return_state.cloned();
    if let Some(state) = &return_state {
        if child_of.parent() == state.parent_grid() {
            cell.set_if_neq(state.cell());
            transform.set_if_neq(state.transform());
        } else {
            migrate_to_grid(
                &mut commands,
                camera,
                state.parent_grid(),
                state.cell(),
                state.transform(),
            );
        }
    }
    let rotation = return_state
        .as_ref()
        .map(|state| state.transform().rotation)
        .unwrap_or(transform.rotation);
    let (yaw, pitch) = if surface.is_some() {
        surface_axes_in_grid(
            child_of.parent(),
            &gravity,
            &q_parents,
            &q_grids,
            &q_spatial,
        )
        .map(|(east, north, up)| {
            lunco_camera_core::math::surface_camera_angles(east, north, up, rotation)
        })
        .unwrap_or_else(|| {
            let (yaw, pitch, _) = rotation.to_euler(EulerRot::YXZ);
            (yaw, pitch)
        })
    } else {
        let (yaw, pitch, _) = rotation.to_euler(EulerRot::YXZ);
        (yaw, pitch)
    };

    let mut camera_commands = commands.entity(camera);
    camera_commands
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitViewReturn>()
        .remove::<RadialArrival>()
        .remove::<CurrentRegionArrival>()
        .remove::<OrbitUserInput>();
    if let Some(state) = return_state {
        match state.behavior().clone() {
            OrbitReturnBehavior::SpringArm(_) => {
                camera_commands.try_insert(FreeFlightCamera {
                    yaw,
                    pitch,
                    damping: None,
                });
            }
            OrbitReturnBehavior::Surface(surface) => {
                camera_commands.try_insert(surface);
            }
            OrbitReturnBehavior::FreeFlight(freeflight) => {
                camera_commands.try_insert(freeflight);
            }
        }
        if let Some(gravity_body) = state.gravity_body() {
            camera_commands.try_insert(gravity_body);
        } else {
            camera_commands.remove::<GravityBody>();
        }
        if state.surface_relative() {
            camera_commands.try_insert(SurfaceRelativeMode);
        } else {
            camera_commands.remove::<SurfaceRelativeMode>();
        }
    } else if surface.is_some() {
        camera_commands.try_insert(SurfaceCamera {
            heading: yaw,
            pitch,
        });
    } else {
        camera_commands.try_insert(FreeFlightCamera {
            yaw,
            pitch,
            damping: None,
        });
    }
    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    info!(camera = ?camera, "[release] restored local camera binding");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbital_release_restores_pose_and_mode_in_one_transition() {
        let mut app = App::new();
        app.init_resource::<LocalGravityField>()
            .add_observer(on_clear_camera_binding);

        let root_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGrid,
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::new(17, -4, 8),
                Transform::from_rotation(Quat::from_rotation_y(0.7)),
                ChildOf(root_grid),
            ))
            .id();
        let body = app.world_mut().spawn_empty().id();
        let return_cell = CellCoord::new(12, -3, 9);
        let return_transform = Transform::from_xyz(125.0, -42.0, 9.0)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.4, -0.25, 0.0));
        let return_surface = SurfaceCamera {
            heading: 0.4,
            pitch: -0.25,
        };
        app.insert_resource(lunco_celestial_spatial_core::OrbitalViewPin {
            active: true,
            body: lunco_celestial::ephemeris_id::MOON,
            dir: DVec3::Z,
            distance: 5_000_000.0,
        });
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                CellCoord::new(-80_000, 30_000, 20_000),
                Transform::from_xyz(700.0, -600.0, 500.0),
                ChildOf(root_grid),
                OrbitViewReturn::new(
                    surface_grid,
                    return_cell,
                    return_transform,
                    OrbitReturnBehavior::Surface(return_surface.clone()),
                    Some(GravityBody { body_entity: body }),
                    true,
                ),
                OrbitCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 5_000_000.0,
                    yaw: 0.2,
                    pitch: -0.1,
                    damping: None,
                    vertical_offset: 0.0,
                },
                OrbitUserInput,
            ))
            .id();
        app.world_mut()
            .trigger(ClearCameraBinding { camera: avatar });
        app.world_mut().flush();

        let world = app.world();
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), surface_grid);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), return_cell);
        let restored = world.get::<Transform>(avatar).unwrap();
        assert!(
            restored
                .translation
                .abs_diff_eq(return_transform.translation, 1e-6)
        );
        assert!(
            restored
                .rotation
                .abs_diff_eq(return_transform.rotation, 1e-6)
        );
        assert_eq!(
            world.get::<SurfaceCamera>(avatar).unwrap().heading,
            return_surface.heading
        );
        assert_eq!(world.get::<GravityBody>(avatar).unwrap().body_entity, body);
        assert!(world.get::<SurfaceRelativeMode>(avatar).is_some());
        assert!(
            !world
                .resource::<lunco_celestial_spatial_core::OrbitalViewPin>()
                .active
        );
        assert!(world.get::<OrbitCamera>(avatar).is_none());
        assert!(world.get::<FreeFlightCamera>(avatar).is_none());
        assert!(world.get::<OrbitViewReturn>(avatar).is_none());
    }
}
