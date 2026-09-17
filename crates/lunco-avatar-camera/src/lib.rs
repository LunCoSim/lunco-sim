//! Avatar-specific camera realization.
//!
//! Generic camera contracts and camera-mode policy live in
//! [`lunco_camera_core`] and [`lunco_camera_runtime`]. This package realizes
//! the avatar's celestial orbital mode, spring-arm mode, and collision-aware
//! local locomotion in explicit BigSpace frames.
//! Possession, focus, and transition commands remain in `lunco-avatar`; the
//! generic celestial surface adapter remains in `lunco-camera-celestial`.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::{
    CAMERA_ZOOM_SENSITIVITY, CurrentRegionArrival, OrbitUserInput, RadialArrival,
    SURFACE_ORBIT_HANDOFF_ALTITUDE_M,
};
use lunco_avatar_core::commands::ReturnFromOrbit;
use lunco_avatar_core::roles::{Avatar, LocalAvatar};
use lunco_camera_core::{
    CameraDefaults, CameraPoseLock, CameraUpdateSet, CameraZoomInput, FreeFlightCamera,
    OrbitCamera, SpringArmCamera, SurfaceCamera,
    math::{apply_scroll_zoom, camera_decay_alpha},
};
use lunco_core::{CelestialBody, Spacecraft};

mod collision;
mod locomotion;
mod scroll_transit;
mod spring_arm;

/// Realizes avatar camera modes that need source-specific spatial adaptation.
///
/// The orbital mode uses the target body's explicit inertial BigSpace frame;
/// the spring arm follows a vessel in its active local frame and filters the
/// followed assembly from its collision query. Free-flight and surface camera
/// modes use the same package's kinematic collision boundary and Grid writer.
pub struct AvatarCelestialCameraPlugin;

impl Plugin for AvatarCelestialCameraPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.init_resource::<CameraDefaults>()
            .init_resource::<lunco_avatar_policy::AvatarCollisionSettings>()
            .init_resource::<lunco_celestial_spatial::ReferenceFrameIndex>()
            .init_resource::<lunco_celestial_spatial::OrbitalViewPin>()
            .register_type::<lunco_avatar_policy::AvatarCollisionSettings>()
            .add_systems(
                PostUpdate,
                (spring_arm::spring_arm_system, orbit_system)
                    .chain()
                    .after(lunco_time::InteractionRenderSet)
                    .before(TransformSystems::Propagate),
            )
            .add_systems(
                lunco_time::InteractionSchedule,
                scroll_transit::freeflight_scroll_transit_system.before(CameraUpdateSet),
            )
            .add_systems(
                lunco_time::InteractionSchedule,
                locomotion::apply_fly.after(CameraUpdateSet),
            );
    }
}

fn orbit_angles_from_arm(direction: bevy::math::DVec3) -> (f32, f32) {
    let direction = direction.normalize_or(bevy::math::DVec3::Z);
    (
        direction.x.atan2(direction.z) as f32,
        (-direction.y.clamp(-1.0, 1.0).asin()) as f32,
    )
}

fn apply_current_region_arrival(
    orbit: &mut OrbitCamera,
    target_orbit: bevy::math::DVec3,
    camera_orbit: bevy::math::DVec3,
    body_radius: f64,
) -> bool {
    let arm = camera_orbit - target_orbit;
    if !arm.is_finite() || arm.length_squared() <= 1.0 || !body_radius.is_finite() {
        return false;
    }
    let distance = body_radius * 3.0;
    if distance <= 0.0 || !distance.is_finite() {
        return false;
    }
    (orbit.yaw, orbit.pitch) = orbit_angles_from_arm(arm);
    orbit.distance = distance;
    true
}

fn orbit_system(
    time: Res<Time<Real>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &mut OrbitCamera,
            &ChildOf,
            &mut CameraZoomInput,
            Has<CurrentRegionArrival>,
            Has<RadialArrival>,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<SpringArmCamera>,
            Without<FreeFlightCamera>,
            Without<SurfaceCamera>,
            Without<CameraPoseLock>,
        ),
    >,
    q_world_grid: Query<Entity, With<lunco_spatial::WorldGrid>>,
    frame_index: Res<lunco_celestial_spatial::ReferenceFrameIndex>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_sc: Query<&Spacecraft>,
    q_dragging: Query<(), With<lunco_interaction_core::GizmoDragging>>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    q_children: Query<&Children>,
    mut commands: Commands,
    mut log_countdown: Local<u32>,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial::OrbitalViewPin>>,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }

    let Ok(root_grid) = q_world_grid.single() else {
        return;
    };
    let dt = time.delta_secs();

    for (
        avatar_ent,
        mut tf,
        mut cell,
        mut orbit,
        child_of,
        mut zoom,
        wants_current_region,
        wants_radial,
    ) in q_avatar.iter_mut()
    {
        if q_dragging.get(orbit.target).is_ok() {
            continue;
        }

        let physical_target =
            lunco_spatial::find_descendant_or_self(orbit.target, &q_children, &q_bodies)
                .unwrap_or(orbit.target);
        let body = q_bodies.get(physical_target).ok().map(|(_, body)| body);
        // Celestial bodies own an explicit star-fixed camera grid. This is the
        // same nested-grid shape as big_space's planets example: body-fixed
        // terrain/vehicles stay under the rotating frame while the camera
        // lives in a co-located inertial sibling.
        let orbit_grid = if let Some(body) = body {
            let Some(entity) =
                frame_index.resolve(lunco_celestial::ReferenceFrame::EclipticJ2000 {
                    center: body.ephemeris_id,
                })
            else {
                warn!(
                    "ORBIT: body {} has no inertial reference frame; refusing an ambiguous camera frame",
                    body.ephemeris_id
                );
                continue;
            };
            entity
        } else {
            root_grid
        };
        let Ok(orbit_grid_ref) = q_grids.get(orbit_grid) else {
            continue;
        };
        let centre_entity = body.map_or(orbit.target, |_| physical_target);
        let Some((target_orbit, _)) = lunco_spatial::coords::pose_in_grid(
            centre_entity,
            orbit_grid,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            continue;
        };
        let Some((cam_orbit, _)) = lunco_spatial::coords::pose_in_grid_seeded(
            avatar_ent,
            orbit_grid,
            Some(&*cell),
            &tf,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            continue;
        };

        if wants_radial {
            let arm = cam_orbit - target_orbit;
            if arm.length_squared() > 1.0 {
                (orbit.yaw, orbit.pitch) = orbit_angles_from_arm(arm);
                orbit.distance = arm.length();
                info!(
                    "ORBIT ARRIVAL: radial yaw={:.2} pitch={:.2} dist={:.3e}",
                    orbit.yaw, orbit.pitch, orbit.distance
                );
            }
            commands.entity(avatar_ent).remove::<RadialArrival>();
        } else if wants_current_region {
            if let Some(body) = body {
                if apply_current_region_arrival(&mut orbit, target_orbit, cam_orbit, body.radius_m)
                {
                    info!(
                        "ORBIT ARRIVAL: current region yaw={:.2} pitch={:.2} dist={:.3e}",
                        orbit.yaw, orbit.pitch, orbit.distance
                    );
                } else {
                    warn!(
                        target = ?orbit.target,
                        "ORBIT ARRIVAL: current camera region is not finite; refusing arrival"
                    );
                }
            }
            commands.entity(avatar_ent).remove::<CurrentRegionArrival>();
        }

        let min_dist = if let Some(body) = body {
            body.radius_m + SURFACE_ORBIT_HANDOFF_ALTITUDE_M
        } else if let Ok(spacecraft) = q_sc.get(orbit.target) {
            (spacecraft.hit_radius_m as f64).max(10.0)
        } else {
            10.0
        };
        let current_len = cam_orbit.distance(target_orbit);
        let surface_exit = body.is_some()
            && orbital_pin.as_ref().is_some_and(|pin| {
                pin.active
                    && zoom.delta > 0.0
                    && orbit.distance <= min_dist * 1.0005
                    && current_len <= min_dist * 1.02
            });
        if surface_exit {
            let transition_direction = zoom.delta;
            zoom.begin_mode_transition(Some(transition_direction));
            commands.trigger(ReturnFromOrbit { target: avatar_ent });
            info!("ORBITAL SCROLL-THROUGH: exiting to surface at current pose");
            continue;
        }

        let zoomed = zoom.delta != 0.0;
        apply_scroll_zoom(
            &mut orbit.distance,
            &mut zoom.delta,
            CAMERA_ZOOM_SENSITIVITY,
            min_dist,
            1.0e11,
        );
        if zoomed {
            commands.entity(avatar_ent).try_insert(OrbitUserInput);
        }

        if let (Some(body), Some(pin)) = (body, orbital_pin.as_mut()) {
            let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
            let direction = rotation.mul_vec3(Vec3::Z).as_dvec3();
            let next_pin = lunco_celestial_spatial::OrbitalViewPin {
                active: true,
                body: body.ephemeris_id,
                dir: direction,
                distance: orbit.distance,
            };
            if **pin != next_pin {
                **pin = next_pin;
            }
        } else if let Some(pin) = orbital_pin.as_mut() {
            if pin.active {
                pin.active = false;
            }
        }

        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let desired_offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * orbit.distance
            + bevy::math::DVec3::Y * orbit.vertical_offset as f64;
        let direction_orbit = desired_offset.normalize_or(bevy::math::DVec3::Z);
        let desired_len = desired_offset.length();
        let final_len = if child_of.parent() != orbit_grid || current_len < 1e-3 {
            desired_len
        } else {
            let damping = orbit.damping.unwrap_or(defaults.damping);
            let alpha = camera_decay_alpha(defaults.position_rate, damping, dt);
            let next = current_len + (desired_len - current_len) * alpha;
            if (next - desired_len).abs() <= desired_len * 1e-9 {
                desired_len
            } else {
                next
            }
        };
        let next_orbit = target_orbit + direction_orbit * final_len;
        let (new_cell, new_translation) = orbit_grid_ref.translation_to_grid(next_orbit);
        let next_transform = Transform::from_translation(new_translation).with_rotation(rotation);
        if child_of.parent() != orbit_grid {
            lunco_spatial::attach::migrate_to_grid(
                &mut commands,
                avatar_ent,
                orbit_grid,
                new_cell,
                next_transform,
            );
        } else {
            cell.set_if_neq(new_cell);
            if tf.translation != new_translation {
                tf.translation = new_translation;
            }
            if tf.rotation != rotation {
                tf.rotation = rotation;
            }
        }

        if *log_countdown == 0 {
            *log_countdown = 240;
            debug!(
                "ORBIT: arm {:.4e}→{:.4e} (cmd {:.3e}) cell=({},{},{}) target=({:.4e},{:.4e},{:.4e})",
                current_len,
                final_len,
                orbit.distance,
                new_cell.x,
                new_cell.y,
                new_cell.z,
                target_orbit.x,
                target_orbit.y,
                target_orbit.z,
            );
        }
        *log_countdown = log_countdown.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_angles_round_trip_the_body_to_camera_arm() {
        for arm in [
            bevy::math::DVec3::Z,
            bevy::math::DVec3::X,
            -bevy::math::DVec3::Z,
            bevy::math::DVec3::new(0.3, 0.8, -0.5).normalize(),
        ] {
            let (yaw, pitch) = orbit_angles_from_arm(arm);
            let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            let reconstructed = rotation.mul_vec3(Vec3::Z).as_dvec3();
            assert!(
                reconstructed.abs_diff_eq(arm.normalize(), 1e-6),
                "arm {arm:?} reconstructed as {reconstructed:?}"
            );
        }
    }

    #[test]
    fn current_region_arrival_uses_the_resolved_radial_direction() {
        let mut orbit = OrbitCamera {
            target: Entity::PLACEHOLDER,
            distance: 1.0,
            yaw: 0.25,
            pitch: 0.5,
            damping: None,
            vertical_offset: 0.0,
        };

        assert!(apply_current_region_arrival(
            &mut orbit,
            bevy::math::DVec3::ZERO,
            bevy::math::DVec3::new(0.0, 10.0, 0.0),
            100.0,
        ));
        assert!((orbit.yaw - 0.0).abs() < 1.0e-6);
        assert!((orbit.pitch + std::f32::consts::FRAC_PI_2).abs() < 1.0e-6);
        assert_eq!(orbit.distance, 300.0);
    }

    #[test]
    fn celestial_orbit_camera_uses_the_explicit_inertial_body_frame() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<CameraDefaults>()
            .init_resource::<lunco_celestial_spatial::ReferenceFrameIndex>()
            .add_systems(First, lunco_celestial_spatial::update_reference_frame_index)
            .add_systems(Update, orbit_system);

        let root_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGrid,
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
            ))
            .id();
        let host_rotation = Quat::from_rotation_y(0.7);
        let host_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::new(75_000_000, 0, 0),
                Transform::from_rotation(host_rotation),
                ChildOf(root_grid),
            ))
            .id();
        let orbit_grid = app
            .world_mut()
            .spawn((
                lunco_celestial::ReferenceFrame::EclipticJ2000 {
                    center: lunco_celestial::ephemeris_id::MOON,
                },
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::new(75_000_000, 0, 0),
                Transform::default(),
                ChildOf(root_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "precision test moon".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                    radius_m: 1_000.0,
                },
                CellCoord::new(100_000, -20_000, 50_000),
                Transform::from_xyz(125.0, -350.0, 700.0),
                ChildOf(host_grid),
            ))
            .id();
        let yaw = 0.35;
        let pitch = -0.2;
        let distance = 10_000.0;
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                CellCoord::ZERO,
                Transform::from_xyz(10.0, 20.0, 30.0),
                ChildOf(host_grid),
                OrbitCamera {
                    target: body,
                    distance,
                    yaw,
                    pitch,
                    damping: None,
                    vertical_offset: 0.0,
                },
                CameraZoomInput::default(),
            ))
            .id();

        app.update();

        let world = app.world();
        assert_eq!(
            world.get::<ChildOf>(avatar).unwrap().parent(),
            orbit_grid,
            "a celestial orbit camera must be a direct child of the target's explicit inertial frame"
        );
        let inertial = world.get::<Grid>(orbit_grid).unwrap();
        let actual_in_inertial = inertial.grid_position_double(
            world.get::<CellCoord>(avatar).unwrap(),
            world.get::<Transform>(avatar).unwrap(),
        );
        let root = world.get::<Grid>(root_grid).unwrap();
        let host_position = root.grid_position_double(
            world.get::<CellCoord>(host_grid).unwrap(),
            world.get::<Transform>(host_grid).unwrap(),
        );
        let host = world.get::<Grid>(host_grid).unwrap();
        let body_local = host.grid_position_double(
            world.get::<CellCoord>(body).unwrap(),
            world.get::<Transform>(body).unwrap(),
        );
        let body_root = host_position + host_rotation.as_dquat() * body_local;
        let arm = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
            .mul_vec3(Vec3::Z)
            .as_dvec3()
            * distance;
        let orbit_origin = root.grid_position_double(
            world.get::<CellCoord>(orbit_grid).unwrap(),
            world.get::<Transform>(orbit_grid).unwrap(),
        );
        let expected_in_inertial = body_root + arm - orbit_origin;
        assert!(
            actual_in_inertial.abs_diff_eq(expected_in_inertial, 1e-3),
            "inertial-grid orbit pose differs: expected {expected_in_inertial:?}, got {actual_in_inertial:?}"
        );
    }
}
