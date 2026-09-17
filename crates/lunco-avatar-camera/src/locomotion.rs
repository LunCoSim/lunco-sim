//! Collision-aware local avatar locomotion.
//!
//! This module owns the camera embodiment's movement realization: authored
//! input ports are interpreted as a movement vector, the vector is resolved in
//! the active BigSpace/Avian frame, and the resulting position is written back
//! to the avatar's source Grid. Possession and input projection remain owned by
//! their respective packages.

use avian3d::prelude::{
    Collider, MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitResponse, SpatialQueryFilter,
};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_core::roles::{Avatar, LocalAvatar};
use lunco_avatar_policy::{
    AvatarCollisionSettings, AvatarSoilCollisionPolicy, avatar_soil_collision_policy,
};
use lunco_camera_core::{
    FreeFlightCamera, FreeFlightSettings, SurfaceCamera, SurfaceRelativeMode,
    math::camera_move_direction,
};
use lunco_celestial_spatial_core::{LocalGravityField, gravity_up_in_grid};
use lunco_core::NON_PHYSICAL_QUERY_LAYERS;
use lunco_interaction_core::DragModeActive;
use lunco_port_core::InputPorts;
use lunco_spatial::ActivePhysicsFrame;
use lunco_workspace::WorkspaceResource;

pub(crate) fn report_avatar_policy_error(error: &str, last_error: &mut Option<String>) {
    if last_error.as_deref() != Some(error) {
        warn!("[avatar] collision policy unavailable: {error}");
        *last_error = Some(error.to_string());
    }
}

/// Move the avatar's capsule through the active physics frame and return the
/// resulting position in the avatar's source Grid.
///
/// The avatar is a client-local camera embodiment rather than a dynamic rigid
/// body. `MoveAndSlide` is therefore the kinematic boundary. The canonical
/// BigSpace transform helpers keep both the query origin and result in
/// `ActivePhysicsFrame` even when the camera Grid is nested or rotated.
pub(crate) fn move_avatar_with_collision(
    avatar: Entity,
    source_grid: Entity,
    cell: &CellCoord,
    transform: &Transform,
    desired_delta: DVec3,
    up_direction: Vec3,
    delta_time: std::time::Duration,
    active_frame: Option<Entity>,
    move_and_slide: Option<&MoveAndSlide<'_, '_>>,
    collision_settings: &AvatarCollisionSettings,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
) -> Option<DVec3> {
    let move_and_slide = move_and_slide?;
    let active_frame = active_frame?;
    let delta_secs = delta_time.as_secs_f64();
    if !delta_secs.is_finite() || delta_secs <= 0.0 {
        return None;
    }
    if !collision_settings.radius_m.is_finite()
        || !collision_settings.capsule_length_m.is_finite()
        || collision_settings.radius_m <= 0.0
        || collision_settings.capsule_length_m < 0.0
    {
        return None;
    }

    let source_grid_ref = q_grids.get(source_grid).ok()?;
    let source_position = source_grid_ref.grid_position_double(cell, transform);
    if !source_position.is_finite() || !desired_delta.is_finite() || !up_direction.is_finite() {
        return None;
    }
    let source_to_physics = lunco_spatial::coords::grid_transform_between_grids(
        source_grid,
        active_frame,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let physics_to_source = lunco_spatial::coords::grid_transform_between_grids(
        active_frame,
        source_grid,
        q_parents,
        q_grids,
        q_spatial,
    )?;
    let physics_position = source_to_physics.transform_position(source_position);
    let physics_delta = source_to_physics.transform_vector(desired_delta);
    let physics_up = source_to_physics
        .transform_vector(up_direction.as_dvec3())
        .normalize_or(DVec3::Y);
    if !physics_position.is_finite()
        || !physics_delta.is_finite()
        || !physics_up.is_finite()
        || physics_up.length_squared() <= f64::EPSILON
    {
        return None;
    }

    // The camera may pitch, but the avatar body stays upright in the local
    // surface/world-up direction. A capsule has no meaningful yaw, so this
    // shortest-arc rotation is the complete shape orientation contract.
    let shape_rotation = DQuat::from_rotation_arc(DVec3::Y, physics_up);
    let velocity = physics_delta / delta_secs;
    let shape = Collider::capsule(
        collision_settings.radius_m,
        collision_settings.capsule_length_m,
    );
    let mut filter = SpatialQueryFilter::from_excluded_entities([avatar]);
    filter.mask = avian3d::prelude::LayerMask(!NON_PHYSICAL_QUERY_LAYERS);
    let output = move_and_slide.move_and_slide(
        &shape,
        physics_position,
        shape_rotation,
        velocity,
        delta_time,
        &MoveAndSlideConfig::default(),
        &filter,
        |_| MoveAndSlideHitResponse::Accept,
    );
    if !output.position.is_finite() {
        return None;
    }
    let result = physics_to_source.transform_position(output.position);
    result.is_finite().then_some(result)
}

/// Apply one complete Grid-absolute position without duplicating the
/// `CellCoord`/local-Transform split at each avatar movement entry point.
pub(crate) fn write_avatar_grid_position(
    grid: &Grid,
    cell: &mut CellCoord,
    transform: &mut Transform,
    position: DVec3,
) {
    let (new_cell, new_transform) = grid.translation_to_grid(position);
    if *cell != new_cell {
        *cell = new_cell;
    }
    if transform.translation != new_transform {
        transform.translation = new_transform;
    }
}

/// Realize the avatar's authored flight-controller ports as local movement.
///
/// The controller reads `forward`, `side`, `up`, and `speed_boost` from the
/// shared port surface. It runs on the wall-clock interaction schedule so the
/// local camera remains usable while virtual simulation time is paused.
pub(crate) fn apply_fly(
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &InputPorts,
            &FreeFlightSettings,
            Has<FreeFlightCamera>,
            Has<SurfaceCamera>,
            Option<&SurfaceRelativeMode>,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    gravity: Res<LocalGravityField>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    drag_mode: Option<Res<DragModeActive>>,
    active_frame: Option<Res<ActivePhysicsFrame>>,
    move_and_slide: Option<MoveAndSlide<'_, '_>>,
    collision_settings: Res<AvatarCollisionSettings>,
    workspace: Option<Res<WorkspaceResource>>,
    mut policy_error: Local<Option<String>>,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    let policy = match avatar_soil_collision_policy(workspace.as_deref()) {
        Ok(policy) => policy,
        Err(error) => {
            report_avatar_policy_error(&error, &mut policy_error);
            return;
        }
    };
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for (
        entity,
        mut tf,
        mut cell,
        child_of,
        inputs,
        flight_settings,
        has_freeflight,
        has_surface_camera,
        surface_mode,
    ) in q_avatar.iter_mut()
    {
        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
        let current_pos = grid.grid_position_double(&cell, &tf);
        if !has_freeflight && !has_surface_camera && !ctrl_pressed {
            continue;
        }

        let forward = inputs.cmd("forward") as f32;
        let side = inputs.cmd("side") as f32;
        let elevation = inputs.cmd("up") as f32;
        let deadzone = flight_settings.input_deadzone as f32;
        let boost = if inputs.cmd("speed_boost") > flight_settings.boost_threshold {
            flight_settings.boost_multiplier
        } else {
            1.0
        };
        if forward.abs() < deadzone && side.abs() < deadzone && elevation.abs() < deadzone {
            continue;
        }

        let up_dir = if surface_mode.is_some() {
            let Some(up) =
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            else {
                continue;
            };
            up
        } else {
            Vec3::Y
        };
        let move_vec = camera_move_direction(&tf, forward, side, elevation, up_dir);
        let desired_delta =
            move_vec.as_dvec3() * flight_settings.speed_mps * boost * time.delta_secs_f64();
        let next_pos = if policy == AvatarSoilCollisionPolicy::ThroughSoilAllowed {
            current_pos + desired_delta
        } else {
            let Some(next_pos) = move_avatar_with_collision(
                entity,
                child_of.parent(),
                &cell,
                &tf,
                desired_delta,
                up_dir,
                time.delta(),
                active_frame.as_deref().map(|frame| frame.0),
                move_and_slide.as_ref(),
                &collision_settings,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) else {
                warn_once!("[avatar] safe collision movement unavailable; movement held");
                continue;
            };
            next_pos
        };
        write_avatar_grid_position(grid, &mut cell, &mut tf, next_pos);
    }
}
