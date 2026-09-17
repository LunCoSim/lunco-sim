use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::OrbitUserInput;
use lunco_avatar_core::commands::ReleaseVessel;
use lunco_avatar_core::roles::{Avatar, LocalAvatar};
use lunco_camera_core::{
    CameraZoomInput, FreeFlightCamera, OrbitCamera, SpringArmCamera, SurfaceCamera,
    SurfaceRelativeMode,
};
use lunco_camera_runtime::{CameraInputSettings, body_orbit_look_scale};
use lunco_celestial::CelestialBody;
use lunco_celestial_spatial::{LocalGravityField, surface_axes_in_grid};
use lunco_control_core::{IntentAnalogState, IntentState, UserIntent};
use lunco_time::{SetTimeTransport, TimeTransport, TransportMode, WorldTime};

// ─── Intent & Input ──────────────────────────────────────────────────────────

/// Captures the avatar's mouse **look** delta (and forwards zoom) into
/// `IntentAnalogState` for the camera behaviour systems.
///
/// Movement (forward/side/up) is NO LONGER read here: it now flows through the
/// shared port path (leafwing `ActionState` → `ControlBinding` → `SetPorts` →
/// FSW `forward`/`side`/`up` → `apply_fly`), exactly like a vessel. This system
/// keeps only the look axis, which stays mouse-direct until the P2 camera decouple.
pub(crate) fn capture_avatar_intent(
    mut q_avatar: Query<
        (Entity, &IntentState, &mut IntentAnalogState),
        (With<Avatar>, With<LocalAvatar>),
    >,
    world: Option<Res<WorldTime>>,
    egui_focus: Res<lunco_control_core::EguiFocus>,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
    mut commands: Commands,
) {
    // Mouse look is a POINTER intent: suppress it while egui holds the pointer so
    // right-dragging over a panel doesn't orbit the scene. (Keyboard focus is
    // irrelevant to look — that gate guards movement/Cancel elsewhere.)
    let pointer_captured = egui_focus.wants_pointer || drag_mode.is_some_and(|drag| drag.active);

    for (entity, intent_state, mut analog) in q_avatar.iter_mut() {
        let mut delta = Vec2::ZERO;
        if !pointer_captured {
            let d = intent_state.axis_pair(&UserIntent::Look);
            if d.length_squared() > 0.00001 {
                delta = d * 10.0;
            }
        }

        analog.look_delta = delta;
        analog.timestamp = world.as_ref().map(|w| w.epoch_jd).unwrap_or_default();

        commands.entity(entity).trigger(|e| {
            let mut a = (*analog).clone();
            a.entity = e;
            a
        });
    }
}

/// Convert Bevy's accumulated mouse-scroll input to line units.
///
/// Bevy preserves the unit supplied by the OS/device. Leafwing's
/// `MouseScrollAxis` exposes only a scalar, so using that axis here loses the
/// distinction and makes pixel-mode touchpads produce enormous zoom deltas.
pub(crate) fn normalized_scroll_delta(scroll: &AccumulatedMouseScroll) -> f32 {
    match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
    }
}

/// Mouse-wheel → per-avatar [`CameraZoomInput`], gated on egui pointer capture.
///
/// Camera zoom is presentation state, not a vessel control port. It consumes the
/// unit-preserving Bevy input at this boundary, then accumulates it per avatar for
/// the active camera behavior to consume + reset.
pub(crate) fn collect_camera_zoom(
    time: Res<Time<Real>>,
    egui_focus: Res<lunco_control_core::EguiFocus>,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut q_avatar: Query<&mut CameraZoomInput, (With<Avatar>, With<LocalAvatar>)>,
) {
    let d = normalized_scroll_delta(&scroll);
    let accepted = !egui_focus.wants_pointer && !drag_mode.is_some_and(|drag| drag.active);
    for mut zoom in q_avatar.iter_mut() {
        // A neutral source frame is meaningful even while the pointer is
        // captured by UI: it completes the previous camera gesture without
        // routing UI-owned wheel input into the scene.
        zoom.ingest(d, accepted, time.delta_secs());
    }
}

/// Applies look deltas from `IntentAnalogState` to whichever behavior
/// component is currently active on the avatar.
///
/// When CTRL is held (momentary free-flight overlay), look deltas are
/// applied directly to the Transform rotation since the behavior systems
/// (SpringArmCamera/OrbitCamera) are skipped during this time.
///
/// In surface mode, CTRL+look applies yaw around `local_up` and pitch around
/// the yawed-right axis, matching the surface-relative camera orientation.
pub(crate) fn avatar_behavior_input_system(
    q_avatar: Query<
        (&IntentAnalogState, Option<&SurfaceRelativeMode>),
        (With<Avatar>, With<LocalAvatar>),
    >,
    mut q_spring: Query<
        &mut SpringArmCamera,
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    mut q_orbit: Query<
        (Entity, &mut OrbitCamera),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    mut q_freeflight: Query<
        &mut FreeFlightCamera,
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    mut q_surface: Query<
        &mut SurfaceCamera,
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    mut q_tf: Query<
        (&mut Transform, &CellCoord, &ChildOf),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    settings: Res<CameraInputSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    gravity: Res<LocalGravityField>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_children: Query<&Children>,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
    mut commands: Commands,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    let Some((analog, surface_mode)) = q_avatar.single().ok() else {
        return;
    };
    let look_delta = analog.look_delta;
    if look_delta.length_squared() < 0.0001 {
        return;
    }

    let delta_yaw = -look_delta.x * settings.look_radians_per_pointer_unit;
    let delta_pitch = -look_delta.y * settings.look_radians_per_pointer_unit;
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    if ctrl_pressed {
        // Momentary free-flight: apply look deltas directly to Transform.
        if let Ok((mut tf, _cell, child_of)) = q_tf.single_mut() {
            let next_rotation = if surface_mode.is_some() {
                let up_v =
                    surface_axes_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
                        .map(|(_, _, up)| up)
                        .unwrap_or(Vec3::Y);
                let yaw_q = Quat::from_axis_angle(up_v, delta_yaw);
                let right: Vec3 = *tf.right();
                let right_yawed = yaw_q.mul_vec3(right);
                let pitch_q = Quat::from_axis_angle(right_yawed, delta_pitch);
                pitch_q * yaw_q * tf.rotation
            } else {
                // Ecliptic: YXZ euler decomposition
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                Quat::from_euler(
                    EulerRot::YXZ,
                    yaw + delta_yaw,
                    (pitch + delta_pitch).clamp(-1.5, 1.5),
                    0.0,
                )
            };
            if tf.rotation != next_rotation {
                tf.rotation = next_rotation;
            }
        }
    } else {
        // Normal mode: apply to the active behavior component.
        if let Some(mut arm) = q_spring.iter_mut().next() {
            (arm.yaw, arm.pitch) = look_angles(arm.yaw, arm.pitch, look_delta, &settings, 1.0);
        }
        if let Some((entity, mut orbit)) = q_orbit.iter_mut().next() {
            let physical_target =
                lunco_spatial::find_descendant_or_self(orbit.target, &q_children, &q_bodies)
                    .unwrap_or(orbit.target);
            let scale = q_bodies.get(physical_target).map_or(1.0, |(_, body)| {
                body_orbit_look_scale(orbit.distance, body.radius_m, &settings)
            }) as f32;
            (orbit.yaw, orbit.pitch) =
                look_angles(orbit.yaw, orbit.pitch, look_delta, &settings, scale);
            commands.entity(entity).try_insert(OrbitUserInput);
        }
        if let Some(mut ff) = q_freeflight.iter_mut().next() {
            (ff.yaw, ff.pitch) = look_angles(ff.yaw, ff.pitch, look_delta, &settings, 1.0);
        }
        if let Some(mut sc) = q_surface.iter_mut().next() {
            (sc.heading, sc.pitch) = look_angles(sc.heading, sc.pitch, look_delta, &settings, 1.0);
        }
    }
}

/// Apply one semantic Look intent to a camera's yaw/pitch state.
///
/// All pointer buttons are resolved by the controller into this intent before
/// reaching here. Keeping the angle conversion in one function makes the
/// right-button path identical for free-flight, orbit, spring-arm, and surface
/// cameras; no camera mode is allowed to reinterpret a raw mouse button.
pub(crate) fn look_angles(
    yaw: f32,
    pitch: f32,
    look_delta: Vec2,
    settings: &CameraInputSettings,
    scale: f32,
) -> (f32, f32) {
    let scale = scale.max(0.0);
    let yaw = yaw + -look_delta.x * settings.look_radians_per_pointer_unit * scale;
    let pitch =
        (pitch - look_delta.y * settings.look_radians_per_pointer_unit * scale).clamp(-1.5, 1.5);
    (yaw, pitch)
}

pub(crate) fn avatar_global_hotkeys(
    q_avatar: Query<&IntentState, (With<Avatar>, With<LocalAvatar>)>,
    transport: Option<Res<TimeTransport>>,
    mut commands: Commands,
) {
    for intent_state in q_avatar.iter() {
        if intent_state.just_pressed(&UserIntent::Pause) {
            if let Some(transport) = transport.as_deref() {
                commands.trigger(SetTimeTransport {
                    playing: Some(matches!(transport.mode, TransportMode::Paused)),
                    ..default()
                });
            }
        }
    }
}

/// The semantic cancel intent releases the current avatar camera/control mode.
pub(crate) fn avatar_escape_possession(
    q_avatar: Query<
        (Entity, &IntentState),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Or<(
                With<lunco_cosim_core::ControlLink>,
                With<SpringArmCamera>,
                With<OrbitCamera>,
            )>,
        ),
    >,
    cursor_mode: lunco_core::CursorModeActive,
    mut commands: Commands,
) {
    if cursor_mode.any() {
        return;
    }
    for (entity, intent) in q_avatar.iter() {
        if intent.just_pressed(&UserIntent::Cancel) {
            commands.trigger(ReleaseVessel { target: entity });
        }
    }
}

/// Run condition for keyboard-owned avatar actions.
pub(crate) fn scene_keyboard_active(focus: Res<lunco_control_core::EguiFocus>) -> bool {
    !focus.wants_keyboard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_units_are_normalized_before_zoom() {
        let line = AccumulatedMouseScroll {
            delta: Vec2::new(0.0, 1.0),
            unit: MouseScrollUnit::Line,
        };
        let pixel = AccumulatedMouseScroll {
            delta: Vec2::new(0.0, MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR),
            unit: MouseScrollUnit::Pixel,
        };
        assert_eq!(normalized_scroll_delta(&line), 1.0);
        assert_eq!(normalized_scroll_delta(&pixel), 1.0);
    }

    #[test]
    fn semantic_look_intent_rotates_camera_angles() {
        let settings = CameraInputSettings {
            look_radians_per_pointer_unit: 0.01,
            ..default()
        };
        let (yaw, pitch) = look_angles(0.0, 0.0, Vec2::new(10.0, -5.0), &settings, 1.0);

        assert!((yaw + 0.1).abs() < 1.0e-6);
        assert!((pitch - 0.05).abs() < 1.0e-6);
    }
}
