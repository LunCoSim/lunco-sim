//! Avatar camera mode transactions and orbital presentation history.
//!
//! This module owns camera-side focus/return transitions and their transient
//! history. Control authority and scene interaction remain in lunco-avatar.

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::{
    CurrentRegionArrival, OrbitCameraTransition, OrbitReturnBehavior, OrbitUserInput,
    OrbitViewHistory, OrbitViewReturn, RadialArrival,
};
use lunco_camera_core::{
    AdaptiveNearPlane, AnimateOrbitCameraDirection, CameraPoseLock, CameraZoomInput, FocusTarget,
    FreeFlightCamera, OrbitCamera, ReturnFromOrbit, SpringArmCamera, SurfaceCamera,
    SurfaceRelativeMode,
};
use lunco_camera_runtime::CameraInputSettings;
use lunco_celestial::CelestialBody;
use lunco_core::on_command;
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_environment::GravityBody;
use lunco_spatial::attach::migrate_to_grid_local_pose;
use lunco_spatial::coords::pose_in_grid_seeded;

/// Render-clock interpolation from an orbit pose to the saved local camera pose.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct OrbitReturnTransition {
    start_position: DVec3,
    start_rotation: DQuat,
    body_center: Option<DVec3>,
    duration_s: f64,
    elapsed_s: f64,
}

fn smooth_transition_fraction(progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn orbit_return_transition_pose(
    transition: &OrbitReturnTransition,
    target_position: DVec3,
    target_rotation: DQuat,
    progress: f64,
) -> Option<(DVec3, DQuat)> {
    if !transition.start_position.is_finite()
        || !target_position.is_finite()
        || !transition.start_rotation.is_finite()
        || !target_rotation.is_finite()
    {
        return None;
    }

    let fraction = smooth_transition_fraction(progress);
    let position = if let Some(center) = transition.body_center {
        let start_offset = transition.start_position - center;
        let target_offset = target_position - center;
        let start_radius = start_offset.length();
        let target_radius = target_offset.length();
        if !center.is_finite()
            || !start_radius.is_finite()
            || !target_radius.is_finite()
            || start_radius <= f64::EPSILON
            || target_radius <= f64::EPSILON
        {
            return None;
        }
        let start_direction = start_offset / start_radius;
        let target_direction = target_offset / target_radius;
        let arc = DQuat::from_rotation_arc(start_direction, target_direction);
        let direction = DQuat::IDENTITY.slerp(arc, fraction) * start_direction;
        let radius = start_radius + (target_radius - start_radius) * fraction;
        center + direction * radius
    } else {
        transition.start_position.lerp(target_position, fraction)
    };
    let rotation = transition
        .start_rotation
        .normalize()
        .slerp(target_rotation.normalize(), fraction)
        .normalize();
    (position.is_finite() && rotation.is_finite()).then_some((position, rotation))
}

fn report_orbit_return_failure(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    camera: Entity,
    message: impl Into<String>,
) {
    let message = message.into();
    warn!(target: "avatar-camera", ?camera, "[orbit return] held: {message}");
    lunco_camera_core::replace_camera_diagnostic(
        diagnostics,
        "avatar-camera",
        "OrbitViewReturn",
        Some(message),
    );
}

fn local_avatar_state_error(requested: Option<Entity>) -> String {
    match requested {
        Some(entity) => format!("requested avatar {entity:?} is not a complete local avatar"),
        None => "the authoritative local embodiment has no complete camera state".to_string(),
    }
}

fn report_orbit_direction_failure(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    camera: Option<Entity>,
    message: impl Into<String>,
) {
    let message = message.into();
    warn!(target: "avatar-camera", ?camera, "[orbit direction] refused: {message}");
    lunco_camera_core::replace_camera_diagnostic(
        diagnostics,
        "avatar-camera",
        "OrbitCameraDirection",
        Some(message),
    );
}

/// Animate the local orbit camera to a new direction without changing its radius.
#[on_command(AnimateOrbitCameraDirection)]
pub(crate) fn on_animate_orbit_camera_direction(
    trigger: On<AnimateOrbitCameraDirection>,
    mut commands: Commands,
    mut q_avatar: Query<
        (&mut OrbitCamera, Has<lunco_camera_core::CameraPoseLock>),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    local_avatar: Option<Res<lunco_embodiment_core::roles::TheLocalEmbodiment>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let camera = match lunco_embodiment_core::roles::resolve_requested_or_local(
        cmd.camera,
        local_avatar.as_deref(),
    ) {
        Ok(camera) => camera,
        Err(message) => {
            report_orbit_direction_failure(&mut diagnostics, cmd.camera, message);
            return;
        }
    };
    let Ok((mut orbit, pose_locked)) = q_avatar.get_mut(camera) else {
        report_orbit_direction_failure(
            &mut diagnostics,
            Some(camera),
            "the requested local camera is not in orbital mode",
        );
        return;
    };
    if pose_locked {
        report_orbit_direction_failure(
            &mut diagnostics,
            Some(camera),
            "the camera pose is owned by another presentation mode",
        );
        return;
    }
    if !cmd.yaw_rad.is_finite()
        || !cmd.pitch_rad.is_finite()
        || cmd.pitch_rad.abs() > std::f64::consts::FRAC_PI_2
        || !cmd.duration_s.is_finite()
        || cmd.duration_s < 0.0
    {
        report_orbit_direction_failure(
            &mut diagnostics,
            Some(camera),
            "yaw, pitch, or transition duration is outside the orbit direction contract",
        );
        return;
    }
    // OrbitCamera stores presentation angles as f32. Keep the conversion at
    // the presentation boundary and leave its f64 orbital radius untouched.
    let yaw = cmd.yaw_rad as f32;
    let pitch = cmd.pitch_rad as f32;
    if !yaw.is_finite() || !pitch.is_finite() {
        report_orbit_direction_failure(
            &mut diagnostics,
            Some(camera),
            "the requested angles exceed the orbit camera's presentation range",
        );
        return;
    }

    let mut camera_commands = commands.entity(camera);
    if cmd.duration_s == 0.0 {
        orbit.yaw = yaw;
        orbit.pitch = pitch;
        camera_commands.remove::<OrbitCameraTransition>();
    } else {
        let from = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let to = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        let Some(transition) = OrbitCameraTransition::new(from, to, cmd.duration_s) else {
            report_orbit_direction_failure(
                &mut diagnostics,
                Some(camera),
                "the requested orbit direction cannot be animated",
            );
            return;
        };
        camera_commands.try_insert(transition);
    }
    camera_commands
        .remove::<(CurrentRegionArrival, RadialArrival)>()
        .try_insert(OrbitUserInput);
    lunco_camera_core::replace_camera_diagnostic(
        &mut diagnostics,
        "avatar-camera",
        "OrbitCameraDirection",
        None,
    );
}

fn remember_user_orbit_pose(
    history: &mut OrbitViewHistory,
    camera: &OrbitCamera,
    q_bodies: &Query<&CelestialBody>,
) {
    let Ok(body) = q_bodies.get(camera.target) else {
        return;
    };
    remember_orbit_pose_for_body(history, camera, body.ephemeris_id);
}

fn remember_orbit_pose_for_body(
    history: &mut OrbitViewHistory,
    camera: &OrbitCamera,
    body_id: i32,
) {
    if !history.remember_camera_pose(body_id, camera) {
        warn!(
            target: "avatar-camera",
            body = body_id,
            "discarding non-finite user orbit pose"
        );
    }
}

/// Retain a user-controlled orbit pose at the single ownership boundary where
/// an orbit mode ends. Every transition that removes or replaces
/// `OrbitCamera` therefore shares the same history write; no command path can
/// accidentally forget one exit route.
fn remember_orbit_camera_on_remove(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world
        .commands()
        .entity(entity)
        .remove::<OrbitCameraTransition>();
    if world.get::<OrbitUserInput>(entity).is_none() {
        return;
    }
    let Some(camera) = world.get::<OrbitCamera>(entity).cloned() else {
        return;
    };
    let Some(body_id) = world
        .get::<CelestialBody>(camera.target)
        .map(|body| body.ephemeris_id)
    else {
        return;
    };
    let Some(mut history) = world.get_mut::<OrbitViewHistory>(entity) else {
        return;
    };
    remember_orbit_pose_for_body(&mut history, &camera, body_id);
    world.commands().entity(entity).remove::<OrbitUserInput>();
}

pub(crate) fn register_orbit_history_hook(app: &mut App) {
    app.world_mut()
        .register_component_hooks::<OrbitCamera>()
        .on_remove(remember_orbit_camera_on_remove);
}
/// Retire per-avatar orbital presentation state with the active Twin. A
/// non-active Twin closing must not disturb the live camera's history.
pub(crate) fn clear_orbit_view_history_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut commands: Commands,
    q_avatar: Query<
        Entity,
        Or<(
            With<OrbitViewHistory>,
            With<OrbitCameraTransition>,
            With<OrbitReturnTransition>,
        )>,
    >,
) {
    if !trigger.event().was_active {
        return;
    }
    for entity in &q_avatar {
        commands.entity(entity).remove::<(
            OrbitViewHistory,
            OrbitUserInput,
            CurrentRegionArrival,
            OrbitCameraTransition,
            OrbitReturnTransition,
            CameraPoseLock,
        )>();
    }
}
/// Install the behavior and frame-owned components captured by one
/// [`OrbitViewReturn`] transaction. Control authority is intentionally not
/// touched here; returning from a view and releasing a vessel are separate
/// domain actions.
fn apply_orbit_return(commands: &mut Commands, avatar: Entity, state: &OrbitViewReturn) {
    let mut entity = commands.entity(avatar);
    entity
        .remove::<SpringArmCamera>()
        .remove::<OrbitCamera>()
        .remove::<FreeFlightCamera>()
        .remove::<SurfaceCamera>()
        .remove::<OrbitViewReturn>()
        .remove::<RadialArrival>()
        .remove::<CurrentRegionArrival>()
        .remove::<OrbitCameraTransition>()
        .remove::<OrbitReturnTransition>()
        .remove::<OrbitUserInput>()
        .remove::<CameraPoseLock>();

    match state.behavior() {
        OrbitReturnBehavior::SpringArm(spring_arm) => {
            entity.try_insert(spring_arm.clone());
        }
        OrbitReturnBehavior::Surface(surface) => {
            entity.try_insert(surface.clone());
        }
        OrbitReturnBehavior::FreeFlight(freeflight) => {
            entity.try_insert(freeflight.clone());
        }
    }
    if let Some(gravity_body) = state.gravity_body() {
        entity.try_insert(gravity_body);
    } else {
        entity.remove::<GravityBody>();
    }
    if state.surface_relative() {
        entity.try_insert(SurfaceRelativeMode);
    } else {
        entity.remove::<SurfaceRelativeMode>();
    }
}

/// Return from an orbital presentation view without changing possession.
///
/// The camera first moves to the captured return grid without changing its
/// world pose, then follows a smooth render-clock path to the saved local pose.
#[on_command(ReturnFromOrbit)]
pub(crate) fn on_return_from_orbit(
    trigger: On<ReturnFromOrbit>,
    mut commands: Commands,
    mut q_avatar: Query<
        (
            &Transform,
            &CellCoord,
            &mut CameraZoomInput,
            &OrbitViewReturn,
            Option<&OrbitCamera>,
            Option<&mut OrbitViewHistory>,
            Has<OrbitUserInput>,
            Has<CameraPoseLock>,
            Option<&OrbitReturnTransition>,
        ),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
    q_bodies: Query<&CelestialBody>,
    settings: Res<CameraInputSettings>,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial_core::OrbitalViewPin>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let camera = trigger.event().camera;
    let Ok((
        transform,
        cell,
        mut zoom,
        return_state,
        current_orbit,
        mut orbit_history,
        orbit_user_input,
        cinematic_lock,
        existing_transition,
    )) = q_avatar.get_mut(camera)
    else {
        return;
    };
    if existing_transition.is_some() {
        return;
    }
    if cinematic_lock {
        report_orbit_return_failure(
            &mut diagnostics,
            camera,
            "the camera pose is owned by another presentation mode",
        );
        return;
    }
    if current_orbit.is_none() {
        report_orbit_return_failure(
            &mut diagnostics,
            camera,
            "the requested local camera is not in orbital mode",
        );
        return;
    }

    let return_state = return_state.clone();
    let target_grid = return_state.parent_grid();
    let Ok(grid) = q_grids.get(target_grid) else {
        report_orbit_return_failure(
            &mut diagnostics,
            camera,
            "the saved local camera grid is no longer available",
        );
        return;
    };
    let Some((start_position, start_rotation)) = pose_in_grid_seeded(
        camera,
        target_grid,
        Some(cell),
        &transform,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        report_orbit_return_failure(
            &mut diagnostics,
            camera,
            "the current orbital camera pose cannot be resolved in the saved local frame",
        );
        return;
    };
    let target_position =
        grid.grid_position_double(&return_state.cell(), &return_state.transform());
    let target_rotation = return_state.transform().rotation.as_dquat();
    if !start_position.is_finite()
        || !start_rotation.is_finite()
        || !target_position.is_finite()
        || !target_rotation.is_finite()
        || !settings.orbit_return_transition_duration_s.is_finite()
        || settings.orbit_return_transition_duration_s <= 0.0
    {
        report_orbit_return_failure(
            &mut diagnostics,
            camera,
            "the saved camera pose or transition duration is invalid",
        );
        return;
    }

    let body_center = if let Some(gravity_body) = return_state.gravity_body() {
        if q_bodies.get(gravity_body.body_entity).is_err() {
            report_orbit_return_failure(
                &mut diagnostics,
                camera,
                "the saved local view body is no longer available",
            );
            return;
        }
        let Some((center, _)) = lunco_spatial::coords::pose_in_grid(
            gravity_body.body_entity,
            target_grid,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            report_orbit_return_failure(
                &mut diagnostics,
                camera,
                "the saved local view body cannot be resolved in its grid",
            );
            return;
        };
        Some(center)
    } else {
        None
    };

    if let Some(camera_pose) = current_orbit {
        if orbit_user_input {
            if let Some(history) = orbit_history.as_deref_mut() {
                remember_user_orbit_pose(history, camera_pose, &q_bodies);
            }
        }
    }
    zoom.begin_mode_transition(None);
    migrate_to_grid_local_pose(
        &mut commands,
        camera,
        target_grid,
        grid,
        start_position,
        start_rotation,
    );
    commands
        .entity(camera)
        .remove::<(
            OrbitCamera,
            OrbitCameraTransition,
            OrbitUserInput,
            RadialArrival,
            CurrentRegionArrival,
        )>()
        .try_insert((
            CameraPoseLock,
            OrbitReturnTransition {
                start_position,
                start_rotation,
                body_center,
                duration_s: settings.orbit_return_transition_duration_s,
                elapsed_s: 0.0,
            },
        ));

    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    commands.trigger(lunco_camera_core::RequestLocalEmbodimentView);
    lunco_camera_core::replace_camera_diagnostic(
        &mut diagnostics,
        "avatar-camera",
        "OrbitViewReturn",
        None,
    );
    info!(
        duration_s = settings.orbit_return_transition_duration_s,
        "ORBITAL EXIT: animating to the saved local camera pose"
    );
}

/// Move a locked camera through its saved local grid, then restore its prior
/// interactive behavior at the exact destination pose.
pub(crate) fn orbit_return_transition_system(
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &mut OrbitReturnTransition,
            &OrbitViewReturn,
            Option<&mut lunco_control_core::IntentAnalogState>,
        ),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            With<CameraPoseLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let dt = time.delta_secs_f64();
    if !dt.is_finite() || dt < 0.0 {
        return;
    }
    for (camera, mut transform, mut cell, child_of, mut transition, return_state, analog_input) in
        &mut q_avatar
    {
        if let Some(mut analog_input) = analog_input {
            analog_input.look_delta = bevy::math::Vec2::ZERO;
        }
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            report_orbit_return_failure(
                &mut diagnostics,
                camera,
                "the active transition grid is no longer available",
            );
            continue;
        };
        transition.elapsed_s = (transition.elapsed_s + dt).min(transition.duration_s);
        let progress = transition.elapsed_s / transition.duration_s;
        let target_position =
            grid.grid_position_double(&return_state.cell(), &return_state.transform());
        let target_rotation = return_state.transform().rotation.as_dquat();
        let Some((position, rotation)) =
            orbit_return_transition_pose(&transition, target_position, target_rotation, progress)
        else {
            report_orbit_return_failure(
                &mut diagnostics,
                camera,
                "the camera return path cannot produce a finite pose",
            );
            continue;
        };

        if progress >= 1.0 {
            cell.set_if_neq(return_state.cell());
            transform.set_if_neq(return_state.transform());
            apply_orbit_return(&mut commands, camera, return_state);
            lunco_camera_core::replace_camera_diagnostic(
                &mut diagnostics,
                "avatar-camera",
                "OrbitViewReturn",
                None,
            );
        } else {
            let (next_cell, next_transform) =
                lunco_spatial::attach::local_pose_to_grid_storage(grid, position, rotation);
            cell.set_if_neq(next_cell);
            transform.set_if_neq(next_transform);
        }
    }
}

#[cfg(test)]
mod orbit_return_transition_tests {
    use super::*;

    #[test]
    fn orbital_return_uses_a_smooth_arc_outside_the_body() {
        let transition = OrbitReturnTransition {
            start_position: DVec3::new(0.0, 0.0, 100.0),
            start_rotation: DQuat::IDENTITY,
            body_center: Some(DVec3::ZERO),
            duration_s: 1.2,
            elapsed_s: 0.0,
        };
        let target_position = DVec3::new(100.0, 0.0, 0.0);
        let at_start =
            orbit_return_transition_pose(&transition, target_position, DQuat::IDENTITY, 0.0)
                .unwrap();
        let midway =
            orbit_return_transition_pose(&transition, target_position, DQuat::IDENTITY, 0.5)
                .unwrap();
        let at_end =
            orbit_return_transition_pose(&transition, target_position, DQuat::IDENTITY, 1.0)
                .unwrap();

        assert_eq!(at_start.0, transition.start_position);
        assert!((at_end.0 - target_position).length() < 1.0e-9);
        assert!((midway.0.length() - 100.0).abs() < 1.0e-9);
        assert!((midway.0 - DVec3::new(100.0, 0.0, 0.0)).length() > 1.0);
    }
}
/// Focuses on a target with an instant transition to OrbitCamera mode.
///
/// Intent-only: this observer picks the orbit *parameters* (target, distance,
/// arrival yaw/pitch) and swaps the behavior component. All spatial placement
/// — explicit inertial-grid selection, cell split and position easing — is owned by
/// `AvatarCelestialCameraPlugin`, which runs at a fixed schedule point on frame-consistent
/// transforms. (An earlier version teleported the avatar here through
/// `world_position_seeded`, which drops the site-anchored solar grids'
/// rotations — landing the camera on a phantom point.)
#[on_command(FocusTarget)]
pub(crate) fn on_focus_command(
    trigger: On<FocusTarget>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &CellCoord,
            &ChildOf,
            Option<&Camera>,
            Option<&OrbitCamera>,
            Option<&OrbitViewHistory>,
            Has<OrbitUserInput>,
            Option<&OrbitViewReturn>,
            Option<&SpringArmCamera>,
            Option<&SurfaceCamera>,
            Option<&FreeFlightCamera>,
            Option<&GravityBody>,
            Has<SurfaceRelativeMode>,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
    mut q_zoom: Query<&mut CameraZoomInput, (With<Embodiment>, With<LocalEmbodiment>)>,
    q_bodies: Query<&CelestialBody>,
    q_body_decls: Query<&lunco_celestial_spatial_core::CelestialBodyDecl>,
    q_body_entities: Query<(Entity, &CelestialBody)>,
    q_children: Query<&Children>,
    local_avatar: Option<Res<lunco_embodiment_core::roles::TheLocalEmbodiment>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let avatar_ent = match lunco_embodiment_core::roles::resolve_requested_or_local(
        cmd.camera,
        local_avatar.as_deref(),
    ) {
        Ok(entity) => entity,
        Err(message) => {
            warn!(target = ?cmd.target, "[focus] refused: {message}");
            lunco_camera_core::replace_camera_diagnostic(
                &mut diagnostics,
                "avatar-camera",
                "LocalEmbodiment",
                Some(message),
            );
            return;
        }
    };
    let Ok((
        avatar_ent,
        cam_tf,
        cam_cell,
        cam_parent,
        _,
        current_orbit,
        orbit_history,
        orbit_user_input,
        return_state,
        spring_arm,
        surface_camera,
        freeflight_camera,
        gravity_body,
        surface_relative,
        cinematic_lock,
    )) = q_avatar.get(avatar_ent)
    else {
        let message = local_avatar_state_error(cmd.camera);
        warn!(target = ?cmd.target, "[focus] refused: {message}");
        lunco_camera_core::replace_camera_diagnostic(
            &mut diagnostics,
            "avatar-camera",
            "LocalEmbodiment",
            Some(message),
        );
        return;
    };
    lunco_camera_core::replace_camera_diagnostic(
        &mut diagnostics,
        "avatar-camera",
        "LocalEmbodiment",
        None,
    );

    // Focus is an interactive camera-mode transition. A cinematic path owns
    // this entity's complete pose, so the command has no valid camera-side
    // effect while the lock is present.
    if cinematic_lock {
        return;
    }

    // Compute distance based on target type.
    let mut distance = 20.0;
    let physical_target = resolve_declared_body(cmd.target, &q_body_decls, &q_body_entities)
        .or_else(|| {
            lunco_spatial::find_descendant_or_self(cmd.target, &q_children, &q_body_entities)
        })
        .unwrap_or(cmd.target);
    let is_body = q_bodies.get(physical_target).is_ok();

    // Already orbiting this very body (clicking the focused globe, re-clicking
    // its view pill): a repeat focus must be a NO-OP. Re-running the swap
    // would discard the current interactive pose and restart its arrival.
    if let Some(orbit) = current_orbit {
        if resolve_declared_body(orbit.target, &q_body_decls, &q_body_entities)
            .or_else(|| {
                lunco_spatial::find_descendant_or_self(orbit.target, &q_children, &q_body_entities)
            })
            .unwrap_or(orbit.target)
            == physical_target
        {
            return;
        }
    }

    // Focus is also a camera-mode handoff. Any wheel delta accumulated before
    // the target switch must not be consumed by the newly focused orbit.
    if let Ok(mut zoom) = q_zoom.get_mut(avatar_ent) {
        zoom.begin_mode_transition(None);
    }
    if let Ok(body) = q_bodies.get(physical_target) {
        distance = body.radius_m * 3.0;
    }

    let (yaw, pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
    let mut next_history = orbit_history.cloned();
    if orbit_user_input {
        if let Some(orbit) = current_orbit {
            let current_target =
                resolve_declared_body(orbit.target, &q_body_decls, &q_body_entities)
                    .or_else(|| {
                        lunco_spatial::find_descendant_or_self(
                            orbit.target,
                            &q_children,
                            &q_body_entities,
                        )
                    })
                    .unwrap_or(orbit.target);
            if let Ok(body) = q_bodies.get(current_target) {
                let history = next_history.get_or_insert_with(OrbitViewHistory::default);
                remember_orbit_pose_for_body(history, orbit, body.ephemeris_id);
            }
        }
    }
    let saved_pose = q_bodies.get(physical_target).ok().and_then(|body| {
        next_history
            .as_ref()
            .and_then(|history| history.pose(body.ephemeris_id))
    });

    let mut ent = commands.entity(avatar_ent);
    if let Some(history) = next_history {
        ent.try_insert(history);
    }
    // First body focus opens one orbit-view transaction. A Moon → Earth switch
    // keeps the original surface snapshot instead of replacing it with the
    // current orbital pose.
    if return_state.is_none() {
        let behavior = if let Some(spring_arm) = spring_arm {
            OrbitReturnBehavior::SpringArm(spring_arm.clone())
        } else if let Some(surface) = surface_camera {
            OrbitReturnBehavior::Surface(surface.clone())
        } else if let Some(freeflight) = freeflight_camera {
            OrbitReturnBehavior::FreeFlight(freeflight.clone())
        } else {
            OrbitReturnBehavior::FreeFlight(FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            })
        };
        ent.try_insert(OrbitViewReturn::new(
            cam_parent.parent(),
            *cam_cell,
            *cam_tf,
            behavior,
            gravity_body.copied(),
            surface_relative,
        ));
    }
    ent.remove::<SpringArmCamera>()
        .remove::<FreeFlightCamera>()
        .remove::<OrbitCameraTransition>()
        // Surface state must go too: the generic surface-camera runtime runs
        // after the celestial orbit writer and would rebuild the rotation as a ground-level
        // tangent frame every frame — the camera orbits the target but looks
        // at the horizon (planet off-screen, view jitters as the arm eases).
        .remove::<SurfaceCamera>()
        .remove::<SurfaceRelativeMode>()
        .remove::<GravityBody>()
        .try_insert(OrbitCamera {
            target: physical_target,
            distance: saved_pose.map_or(distance, |pose| pose.distance()),
            yaw: saved_pose.map_or(yaw, |pose| pose.yaw()),
            pitch: saved_pose.map_or(pitch, |pose| pose.pitch()),
            damping: saved_pose.and_then(|pose| pose.damping()),
            vertical_offset: saved_pose.map_or(0.0, |pose| pose.vertical_offset()),
        });
    ent.remove::<OrbitUserInput>();
    if is_body && saved_pose.is_none() {
        ent.try_insert(CurrentRegionArrival);
    } else {
        ent.remove::<CurrentRegionArrival>();
    }
    info!(
        "FOCUS: avatar={avatar_ent:?} target={:?} (physical {physical_target:?}) body={is_body} distance={:.3e} restored={}",
        cmd.target,
        saved_pose.map_or(distance, |pose| pose.distance()),
        saved_pose.is_some(),
    );
}
fn resolve_declared_body(
    target: Entity,
    declarations: &Query<&lunco_celestial_spatial_core::CelestialBodyDecl>,
    bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<Entity> {
    let decl = declarations.get(target).ok()?;
    bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == decl.naif)
        .map(|(entity, _)| entity)
}

/// Initializes avatar entities that lack a behavior component.
///
/// Inserts `FreeFlightCamera` as the default behavior with the entity's
/// current transform orientation.
///
/// `Without<CameraPoseLock>` is load-bearing, not hygiene: a path-driven
/// camera has no interactive mode, and this initializer must never create one
/// after the authored path has claimed pose ownership.
pub(crate) fn avatar_init_system(
    mut commands: Commands,
    q_avatar: Query<
        (Entity, &Transform, Option<&OrbitViewHistory>),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            With<Projection>,
            With<lunco_render::SceneCamera>,
            Without<SpringArmCamera>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            // SurfaceCamera is a complete interactive mode, not an absent
            // behavior component. Without this guard init would reinsert
            // FreeFlightCamera over it on the next Update tick.
            Without<SurfaceCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_proj: Query<
        (Entity, &Projection),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            Without<AdaptiveNearPlane>,
            With<Projection>,
            With<lunco_render::SceneCamera>,
        ),
    >,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    for (entity, tf, history) in q_avatar.iter() {
        if history.is_none() {
            commands
                .entity(entity)
                .try_insert(OrbitViewHistory::default());
        }

        // An authored standard USD camera already owns its projection, camera
        // presentation profile, exposure, and initial look-at transform. The
        // avatar owner adds only the generic interactive movement substrate;
        // Rhai selects richer behavior through typed camera commands.
        let mut avatar = commands.entity(entity);
        let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
        avatar.try_insert(FreeFlightCamera {
            yaw,
            pitch,
            damping: None,
        });
    }
    let mut failure = None;
    for (entity, projection) in q_proj.iter() {
        if let Projection::Perspective(projection) = projection {
            if let Some(source) =
                AdaptiveNearPlane::new(f64::from(projection.near), f64::from(projection.far))
            {
                commands.entity(entity).try_insert(source);
            } else {
                failure = Some(format!(
                    "cannot seed adaptive clipping from invalid source projection on {entity:?}"
                ));
            }
        }
    }
    let previous = diagnostics
        .as_deref()
        .and_then(|diagnostics| {
            diagnostics
                .findings
                .iter()
                .find(|finding| finding.producer == "camera-clip-source")
        })
        .map(|finding| finding.message.as_str());
    if previous != failure.as_deref() {
        lunco_camera_core::replace_camera_diagnostic(
            &mut diagnostics,
            "camera-clip-source",
            "LocalEmbodiment",
            failure,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_avatar_camera_core::OrbitPose;
    use lunco_camera_core::FollowAttitude;
    use lunco_celestial_spatial_core::LocalGravityField;
    use lunco_control_core::ControlLink;

    #[test]
    fn generic_avatar_clipping_seeds_once_and_preserves_authored_source() {
        let mut app = App::new();
        app.add_systems(Update, avatar_init_system);
        let source = AdaptiveNearPlane::new(0.01, 1_000_000.0).unwrap();
        let authored = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                Transform::IDENTITY,
                lunco_render::SceneCamera::default(),
                Projection::Perspective(PerspectiveProjection::default()),
                source,
            ))
            .id();
        let generic = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                Transform::IDENTITY,
                lunco_render::SceneCamera::default(),
                Projection::Perspective(PerspectiveProjection {
                    near: 0.02,
                    far: 500.0,
                    ..default()
                }),
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<AdaptiveNearPlane>(authored).unwrap(),
            source
        );
        let seeded = app.world().get::<AdaptiveNearPlane>(generic).unwrap();
        assert_eq!(seeded.source_near_m, f64::from(0.02_f32));
        assert_eq!(seeded.source_far_m, 500.0);
        app.update();
        assert_eq!(
            *app.world().get::<AdaptiveNearPlane>(authored).unwrap(),
            source
        );
    }

    #[test]
    fn animate_orbit_camera_direction_preserves_radius_and_rejects_bad_pitch() {
        let mut app = App::new();
        app.init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_observer(on_animate_orbit_camera_direction);
        let target = app.world_mut().spawn_empty().id();
        let camera = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                OrbitCamera {
                    target,
                    distance: 1_900_000.0,
                    yaw: -0.4,
                    pitch: 0.15,
                    damping: None,
                    vertical_offset: 12.0,
                },
            ))
            .id();

        app.world_mut().trigger(AnimateOrbitCameraDirection {
            camera: Some(camera),
            yaw_rad: 0.8,
            pitch_rad: -0.3,
            duration_s: 1.5,
        });
        app.world_mut().flush();

        let orbit = app.world().get::<OrbitCamera>(camera).unwrap();
        assert_eq!(orbit.distance, 1_900_000.0);
        assert_eq!(orbit.yaw, -0.4);
        assert_eq!(orbit.pitch, 0.15);
        assert_eq!(orbit.vertical_offset, 12.0);
        assert!(app.world().get::<OrbitCameraTransition>(camera).is_some());

        app.world_mut().trigger(AnimateOrbitCameraDirection {
            camera: Some(camera),
            yaw_rad: 0.0,
            pitch_rad: 2.0,
            duration_s: 0.0,
        });
        app.world_mut().flush();

        let orbit = app.world().get::<OrbitCamera>(camera).unwrap();
        assert_eq!(orbit.distance, 1_900_000.0);
        assert_eq!(orbit.yaw, -0.4);
        assert_eq!(orbit.pitch, 0.15);
        assert!(app.world().get::<OrbitCameraTransition>(camera).is_some());
        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert!(diagnostics.findings.iter().any(|finding| {
            finding.producer == "avatar-camera"
                && finding.subject == "OrbitCameraDirection"
                && finding
                    .message
                    .contains("outside the orbit direction contract")
        }));
    }

    #[test]
    fn focus_refuses_an_explicit_non_local_avatar_without_entity_order_fallback() {
        let mut app = App::new();
        app.init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_observer(on_focus_command);

        let requested_avatar = app.world_mut().spawn(Embodiment).id();
        let target = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(FocusTarget {
            camera: Some(requested_avatar),
            target,
        });
        app.world_mut().flush();

        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert_eq!(diagnostics.findings.len(), 1);
        assert_eq!(diagnostics.findings[0].producer, "avatar-camera");
        assert!(diagnostics.findings[0].message.contains("requested avatar"));
        assert!(app.world().get::<OrbitCamera>(requested_avatar).is_none());
    }

    #[test]
    fn moon_earth_surface_round_trip_preserves_the_original_surface_transaction() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .init_resource::<lunco_celestial_spatial_core::OrbitalViewPin>()
            .init_resource::<CameraInputSettings>()
            .init_resource::<Time<Real>>()
            .add_observer(on_focus_command)
            .add_observer(on_return_from_orbit)
            .add_systems(Update, orbit_return_transition_system);

        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
            ))
            .id();
        let moon = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "Moon".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                    radius_m: 1_737_400.0,
                },
                ChildOf(surface_grid),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let earth = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "Earth".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::EARTH,
                    radius_m: 6_371_000.0,
                },
                ChildOf(surface_grid),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let original_cell = CellCoord::new(1_737, -7, 13);
        let original_transform = Transform::from_xyz(175.0, 23.0, -440.0)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.3, -0.4, 0.0));
        let original_surface = SurfaceCamera {
            heading: 0.3,
            pitch: -0.4,
        };
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                Camera {
                    is_active: true,
                    ..default()
                },
                original_cell,
                original_transform,
                ChildOf(surface_grid),
                original_surface.clone(),
                GravityBody { body_entity: moon },
                SurfaceRelativeMode,
            ))
            .id();

        app.world_mut().trigger(FocusTarget {
            camera: Some(avatar),
            target: moon,
        });
        app.world_mut().flush();
        let first_snapshot = app.world().get::<OrbitViewReturn>(avatar).unwrap().clone();
        assert_eq!(first_snapshot.parent_grid(), surface_grid);
        assert_eq!(first_snapshot.cell(), original_cell);
        assert!(matches!(
            first_snapshot.behavior(),
            OrbitReturnBehavior::Surface(_)
        ));
        assert!(
            app.world().get::<CurrentRegionArrival>(avatar).is_some(),
            "surface-to-body focus must resolve the current region instead of using a fixed arrival"
        );

        app.world_mut().trigger(FocusTarget {
            camera: Some(avatar),
            target: earth,
        });
        app.world_mut().flush();
        let second_snapshot = app.world().get::<OrbitViewReturn>(avatar).unwrap();
        assert_eq!(second_snapshot.parent_grid(), first_snapshot.parent_grid());
        assert_eq!(second_snapshot.cell(), first_snapshot.cell());
        assert!(
            second_snapshot
                .transform()
                .translation
                .abs_diff_eq(first_snapshot.transform().translation, 1e-6)
        );
        assert_eq!(
            app.world().get::<OrbitCamera>(avatar).unwrap().target,
            earth
        );
        assert!(
            app.world().get::<CurrentRegionArrival>(avatar).is_some(),
            "a body without saved user pose must resolve from the current region"
        );

        app.world_mut().trigger(ReturnFromOrbit { camera: avatar });
        app.world_mut().flush();
        assert!(app.world().get::<CameraPoseLock>(avatar).is_some());
        assert!(app.world().get::<OrbitViewReturn>(avatar).is_some());
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs_f64(1.2));
        app.update();
        let world = app.world();
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), surface_grid);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), original_cell);
        let restored = world.get::<Transform>(avatar).unwrap();
        assert!(
            restored
                .translation
                .abs_diff_eq(original_transform.translation, 1e-6)
        );
        assert!(
            restored
                .rotation
                .abs_diff_eq(original_transform.rotation, 1e-6)
        );
        assert_eq!(
            world.get::<SurfaceCamera>(avatar).unwrap().heading,
            original_surface.heading
        );
        assert_eq!(world.get::<GravityBody>(avatar).unwrap().body_entity, moon);
        assert!(world.get::<SurfaceRelativeMode>(avatar).is_some());
    }

    #[test]
    fn user_orbit_pose_is_restored_per_body_after_switching_targets() {
        let mut app = App::new();
        register_orbit_history_hook(&mut app);
        app.add_observer(on_focus_command);

        let grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
            ))
            .id();
        let moon = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: 1_737_400.0,
            })
            .id();
        let earth = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Earth".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::EARTH,
                radius_m: 6_371_000.0,
            })
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                CellCoord::ZERO,
                Transform::from_xyz(0.0, 0.0, 100.0),
                ChildOf(grid),
                OrbitCamera {
                    target: moon,
                    distance: 8_000.0,
                    yaw: 0.7,
                    pitch: -0.3,
                    damping: Some(0.2),
                    vertical_offset: 4.0,
                },
                OrbitViewHistory::default(),
                OrbitUserInput,
            ))
            .id();

        app.world_mut().trigger(FocusTarget {
            camera: Some(avatar),
            target: earth,
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<OrbitCamera>(avatar).unwrap().target,
            earth,
            "body switch must replace the active orbit target"
        );

        let stored = app
            .world()
            .get::<OrbitViewHistory>(avatar)
            .expect("orbit history remains avatar-owned")
            .pose(lunco_celestial::ephemeris_id::MOON)
            .expect("leaving Moon orbit stores the user-controlled pose");
        assert_eq!(
            stored,
            OrbitPose::from_camera(&OrbitCamera {
                target: moon,
                distance: 8_000.0,
                yaw: 0.7,
                pitch: -0.3,
                damping: Some(0.2),
                vertical_offset: 4.0,
            })
            .unwrap()
        );

        app.world_mut().trigger(FocusTarget {
            camera: Some(avatar),
            target: moon,
        });
        app.world_mut().flush();
        let restored = app.world().get::<OrbitCamera>(avatar).unwrap();
        assert_eq!(restored.target, moon);
        assert_eq!(restored.yaw, 0.7);
        assert_eq!(restored.pitch, -0.3);
        assert_eq!(restored.distance, 8_000.0);
        assert_eq!(restored.damping, Some(0.2));
        assert_eq!(restored.vertical_offset, 4.0);
        assert!(
            app.world().get::<CurrentRegionArrival>(avatar).is_none(),
            "a saved body pose must not be replaced by a new arrival"
        );
    }

    #[test]
    fn direct_orbit_mode_removal_records_and_clears_user_pose() {
        let mut app = App::new();
        register_orbit_history_hook(&mut app);

        let body = app
            .world_mut()
            .spawn(CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
            })
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                OrbitCamera {
                    target: body,
                    distance: 12_000.0,
                    yaw: 1.1,
                    pitch: -0.25,
                    damping: Some(0.15),
                    vertical_offset: 7.0,
                },
                OrbitViewHistory::default(),
                OrbitUserInput,
            ))
            .id();

        app.world_mut().entity_mut(avatar).remove::<OrbitCamera>();
        app.world_mut().flush();

        let history = app.world().get::<OrbitViewHistory>(avatar).unwrap();
        assert_eq!(
            history.pose(lunco_celestial::ephemeris_id::MOON),
            Some(
                OrbitPose::from_camera(&OrbitCamera {
                    target: body,
                    distance: 12_000.0,
                    yaw: 1.1,
                    pitch: -0.25,
                    damping: Some(0.15),
                    vertical_offset: 7.0,
                })
                .unwrap()
            )
        );
        assert!(app.world().get::<OrbitUserInput>(avatar).is_none());
    }

    #[test]
    fn active_twin_close_clears_orbit_history_without_touching_inactive_close() {
        let mut app = App::new();
        app.add_observer(clear_orbit_view_history_on_twin_closed);
        let avatar = app
            .world_mut()
            .spawn((Embodiment, LocalEmbodiment, OrbitViewHistory::default()))
            .id();

        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(1),
            root: std::path::PathBuf::from("/inactive"),
            was_active: false,
        });
        app.world_mut().flush();
        assert!(app.world().get::<OrbitViewHistory>(avatar).is_some());

        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: lunco_workspace::TwinId::new(2),
            root: std::path::PathBuf::from("/active"),
            was_active: true,
        });
        app.world_mut().flush();
        assert!(app.world().get::<OrbitViewHistory>(avatar).is_none());
    }

    #[test]
    fn possessed_orbit_view_round_trip_preserves_control_and_spring_arm() {
        let mut app = App::new();
        app.init_resource::<lunco_core_session::SyncApplyGuard>()
            .init_resource::<LocalGravityField>()
            .init_resource::<lunco_celestial_spatial_core::OrbitalViewPin>()
            .init_resource::<CameraInputSettings>()
            .init_resource::<Time<Real>>()
            .add_observer(on_focus_command)
            .add_observer(on_return_from_orbit)
            .add_systems(Update, orbit_return_transition_system);

        let root_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();

        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(root_grid),
            ))
            .id();
        let orbit_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(root_grid),
            ))
            .id();
        let moon = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "Moon".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                    radius_m: 1_737_400.0,
                },
                ChildOf(surface_grid),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let rover = app.world_mut().spawn_empty().id();
        let original_cell = CellCoord::new(1_737, -2, 5);
        let original_transform = Transform::from_xyz(31.0, 12.0, -17.0)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, -0.2, 0.0));
        let original_spring = SpringArmCamera {
            target: rover,
            distance: 14.0,
            yaw: 0.25,
            pitch: -0.35,
            damping: Some(0.4),
            vertical_offset: 2.0,
            track_heading: true,
            attitude: FollowAttitude::Heading,
        };
        let avatar = app
            .world_mut()
            .spawn((
                Embodiment,
                LocalEmbodiment,
                Camera {
                    is_active: true,
                    ..default()
                },
                original_cell,
                original_transform,
                ChildOf(surface_grid),
                original_spring.clone(),
                ControlLink { target: rover },
                GravityBody { body_entity: moon },
                SurfaceRelativeMode,
            ))
            .id();

        app.world_mut().trigger(FocusTarget {
            camera: Some(avatar),
            target: moon,
        });
        app.world_mut().flush();
        assert!(matches!(
            app.world()
                .get::<OrbitViewReturn>(avatar)
                .unwrap()
                .behavior(),
            OrbitReturnBehavior::SpringArm(_)
        ));
        assert_eq!(
            app.world().get::<ControlLink>(avatar).unwrap().target,
            rover,
            "entering a presentation view must not release control"
        );

        app.world_mut().entity_mut(avatar).insert((
            ChildOf(orbit_grid),
            CellCoord::new(-50_000, 20_000, 9_000),
            Transform::from_xyz(700.0, -600.0, 500.0),
        ));
        app.world_mut().trigger(ReturnFromOrbit { camera: avatar });
        app.world_mut().flush();

        assert!(app.world().get::<CameraPoseLock>(avatar).is_some());
        assert!(app.world().get::<OrbitViewReturn>(avatar).is_some());
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs_f64(0.6));
        app.update();
        assert!(app.world().get::<CameraPoseLock>(avatar).is_some());
        assert!(app.world().get::<SpringArmCamera>(avatar).is_none());
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs_f64(0.6));
        app.update();

        let world = app.world();
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), surface_grid);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), original_cell);
        let restored_transform = world.get::<Transform>(avatar).unwrap();
        assert!(
            restored_transform
                .translation
                .abs_diff_eq(original_transform.translation, 1e-6)
        );
        assert!(
            restored_transform
                .rotation
                .abs_diff_eq(original_transform.rotation, 1e-6)
        );
        let restored_spring = world.get::<SpringArmCamera>(avatar).unwrap();
        assert_eq!(restored_spring.target, original_spring.target);
        assert_eq!(restored_spring.distance, original_spring.distance);
        assert_eq!(restored_spring.yaw, original_spring.yaw);
        assert_eq!(restored_spring.pitch, original_spring.pitch);
        assert_eq!(
            world.get::<ControlLink>(avatar).unwrap().target,
            rover,
            "returning from a presentation view must preserve possession"
        );
        assert!(world.get::<OrbitCamera>(avatar).is_none());
        assert!(world.get::<OrbitViewReturn>(avatar).is_none());
        assert!(
            !world
                .resource::<lunco_celestial_spatial_core::OrbitalViewPin>()
                .active
        );
    }
}
#[test]
fn avatar_init_does_not_reinsert_freeflight_over_surface_camera() {
    let mut app = App::new();
    app.add_systems(Update, avatar_init_system);

    let avatar = app
        .world_mut()
        .spawn((
            Embodiment,
            LocalEmbodiment,
            Transform::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            SurfaceCamera {
                heading: 0.0,
                pitch: -0.2,
            },
        ))
        .id();

    let camera_less_avatar = app
        .world_mut()
        .spawn((
            Embodiment,
            LocalEmbodiment,
            Transform::default(),
            Projection::Perspective(PerspectiveProjection::default()),
        ))
        .id();

    app.update();

    assert!(app.world().get::<SurfaceCamera>(avatar).is_some());
    assert!(
        app.world().get::<FreeFlightCamera>(avatar).is_none(),
        "camera initialization must not create two mutually-exclusive modes"
    );
    assert!(
        app.world()
            .get::<FreeFlightCamera>(camera_less_avatar)
            .is_none(),
        "camera behavior requires an authored SceneCamera intent"
    );
    assert!(
        app.world()
            .get::<lunco_render::SceneCamera>(camera_less_avatar)
            .is_none(),
        "avatar initialization must not fabricate missing camera intent"
    );
    assert!(
        app.world()
            .get::<AdaptiveNearPlane>(camera_less_avatar)
            .is_none(),
        "camera precision policy requires authored SceneCamera intent"
    );
}
