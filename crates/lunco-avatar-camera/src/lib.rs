//! Avatar-specific camera realization.
//!
//! Generic camera contracts and camera-mode policy live in
//! [`lunco_camera_core`] and [`lunco_camera_runtime`]. This package realizes
//! the avatar's celestial orbital mode, spring-arm mode, and collision-aware
//! local locomotion in explicit BigSpace frames.
//! Possession, focus, and follow authority remain in `lunco-avatar`; this
//! package owns the BigSpace surface/orbit lifecycle and its camera-side
//! transition commands. The generic celestial surface adapter remains in
//! `lunco-camera-celestial`.

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::{
    CAMERA_ZOOM_SENSITIVITY, CurrentRegionArrival, OrbitUserInput, RadialArrival,
    SURFACE_ORBIT_HANDOFF_ALTITUDE_M,
};
use lunco_avatar_core::commands::{FocusTarget, ReturnFromOrbit};
use lunco_avatar_core::roles::{Avatar, LocalAvatar};
use lunco_camera_core::{
    CameraDefaults, CameraPoseLock, CameraUpdateSet, CameraZoomInput, FreeFlightCamera,
    OrbitCamera, SpringArmCamera, SurfaceCamera, SurfaceRelativeMode,
    math::{apply_scroll_zoom, camera_decay_alpha, surface_camera_angles, surface_camera_rotation},
};
use lunco_celestial_spatial::{
    LeaveSurface, LocalGravityField, TeleportToSurface, surface_axes_for_grid_position,
    surface_axes_in_grid,
};
use lunco_core::{CelestialBody, Spacecraft, on_command, register_commands};
use lunco_environment::{GravityBody, GravityProvider};
use lunco_spatial::attach::migrate_to_grid;

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
            .init_resource::<SurfaceModeThreshold>()
            .register_type::<lunco_avatar_policy::AvatarCollisionSettings>()
            .register_type::<SurfaceModeThreshold>()
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
            )
            .add_systems(
                lunco_time::InteractionSchedule,
                reset_avatar_easing_before_spatial_rebase.before(lunco_time::InteractionRestoreSet),
            )
            .add_systems(Update, surface_mode_transition_system);
        register_all_commands(app);
    }
}

// Camera-side lifecycle commands are registered with the same reflection and
// wire-command path as every other command. Their handlers live beside the
// BigSpace realization that owns their spatial invariants.
register_commands!(on_surface_teleport_command, on_leave_surface_command);

/// Hysteresis thresholds for the avatar's surface-relative camera policy.
///
/// The camera enters the body-fixed mode below `engage_altitude` and leaves it
/// above `disengage_altitude`. Keeping the thresholds in the camera adapter
/// makes the policy available to every avatar camera without coupling the
/// command/authority runtime to celestial mode transitions.
#[derive(Resource, Reflect, Clone, Debug)]
#[reflect(Resource)]
pub struct SurfaceModeThreshold {
    /// Altitude in metres below which surface mode engages.
    pub engage_altitude: f64,
    /// Altitude in metres above which surface mode disengages.
    pub disengage_altitude: f64,
}

impl Default for SurfaceModeThreshold {
    fn default() -> Self {
        Self {
            engage_altitude: 50_000.0,
            disengage_altitude: 100_000.0,
        }
    }
}

/// Clear cell-local easing at an avatar BigSpace handoff.
///
/// The generic camera runtime owns easing policy, but only this avatar adapter
/// knows that the samples are BigSpace cell-local. A rebase must therefore
/// discard the old-frame sample before the interaction schedule restores it.
fn reset_avatar_easing_before_spatial_rebase(
    mut q: Query<
        &mut lunco_time::InteractionEased,
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<CameraPoseLock>,
            Or<(Changed<CellCoord>, Changed<ChildOf>)>,
        ),
    >,
) {
    for mut eased in &mut q {
        eased.reset();
    }
}

// ─── Surface Teleport Commands ───────────────────────────────────────────────

/// Teleports the avatar to a body's surface.
///
/// The camera is parented to the body's surface Grid, not to the Body entity.
/// That keeps the camera in the same body-fixed BigSpace branch as streamed
/// terrain while `SurfaceCamera` derives its orientation from the canonical
/// body-fixed ENU frame. BigSpace origin ownership remains with the persistent
/// OriginAnchor while this camera is migrated into the body-fixed Grid.
#[on_command(TeleportToSurface)]
fn on_surface_teleport_command(
    trigger: On<TeleportToSurface>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &CellCoord,
            &ChildOf,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial_abs: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_globe_lods: Query<&lunco_celestial_spatial::GlobeLod>,
    q_gravity_providers: Query<&GravityProvider>,
    mut field: ResMut<LocalGravityField>,
) {
    let cmd = trigger.event();
    let avatar_ent = cmd.target;

    let body_entity = cmd.body_entity;

    let (body_entity, body_radius) = if let Ok((e, b)) = q_bodies.get(body_entity) {
        debug!("TELEPORT: found body {:?} radius={:.0}m", e, b.radius_m);
        (e, b.radius_m)
    } else {
        warn!(
            "TELEPORT: body entity {:?} not found in q_bodies",
            body_entity
        );
        return;
    };

    if body_entity == Entity::PLACEHOLDER {
        warn!("TELEPORT: no body found");
        return;
    }

    debug!("TELEPORT: triggered for avatar {:?}", avatar_ent);

    // Get camera cell for position lookup
    let Ok((_, _cam_tf, _cam_cell, _cam_child_of, cinematic_lock)) = q_avatar.get(avatar_ent)
    else {
        return;
    };
    if cinematic_lock {
        return;
    }

    // GlobeLod is the authoritative owner of a body's surface Grid. This is
    // deliberately data-driven: adding another celestial body does not add a
    // second Rust-side list of surface-grid marker types.
    let Ok(globe_lod) = q_globe_lods.get(body_entity) else {
        warn!("TELEPORT: body {:?} has no surface LOD Grid", body_entity);
        return;
    };
    let target_grid = globe_lod.surface_grid;
    let Ok(target_grid_ref) = q_grids.get(target_grid) else {
        warn!(
            "TELEPORT: target surface Grid {:?} is not live",
            target_grid
        );
        return;
    };
    debug!(
        "TELEPORT: parenting camera to surface grid {:?}",
        target_grid
    );

    {
        // Resolve the camera pose and its look direction in the same shared
        // BigSpace branch as the body before converting into the destination
        // surface grid.
        let Some((_common_grid, avatar_position, avatar_rotation, body_position, body_rotation)) =
            lunco_spatial::coords::common_grid_poses(
                avatar_ent,
                body_entity,
                &q_parents,
                &q_grids,
                &q_spatial_abs,
            )
        else {
            warn!("TELEPORT: avatar and body have no shared BigSpace Grid");
            return;
        };
        let Some((_, grid_position, grid_to_common, _, body_to_common)) =
            lunco_spatial::coords::common_grid_poses(
                target_grid,
                body_entity,
                &q_parents,
                &q_grids,
                &q_spatial_abs,
            )
        else {
            warn!("TELEPORT: target Grid cannot be composed with the body");
            return;
        };
        let body_to_grid = grid_to_common.inverse() * body_to_common;
        let origin_body = body_rotation.inverse() * (avatar_position - body_position);
        let direction_body = body_rotation.inverse() * (avatar_rotation * Vec3::NEG_Z.as_dvec3());
        let b = origin_body.dot(direction_body);
        let c = origin_body.length_squared() - body_radius * body_radius;
        let discriminant = b * b - c;
        if discriminant < 0.0 {
            warn!("TELEPORT: avatar view does not intersect the body's surface");
            return;
        }
        let root = discriminant.sqrt();
        let Some(t) = [-b - root, -b + root].into_iter().find(|t| *t > 0.0) else {
            warn!("TELEPORT: camera ray does not intersect the body's forward surface");
            return;
        };
        let surface_body_pos = origin_body + direction_body * t;
        let surface_normal = surface_body_pos.normalize_or(DVec3::Y);
        let body_center_in_grid = grid_to_common.inverse() * (body_position - grid_position);
        let surface_local_pos = body_center_in_grid + body_to_grid * surface_body_pos;
        let Some((east, north, up)) = surface_axes_for_grid_position(
            target_grid,
            surface_local_pos,
            body_entity,
            &q_parents,
            &q_grids,
            &q_spatial_abs,
        ) else {
            warn!("TELEPORT: body-fixed tangent frame is not reachable from the target Grid");
            return;
        };

        let (new_cell, new_tf_translation) = target_grid_ref.translation_to_grid(surface_local_pos);

        // Surface gravity from body's GravityProvider
        let surface_g = if let Ok(gp) = q_gravity_providers.get(body_entity) {
            let accel = gp.model.acceleration(surface_body_pos);
            accel.length()
        } else {
            0.0
        };

        // Build the initial attitude from the same body-fixed ENU frame used
        // by SurfaceCamera. No world-axis reference is valid here.
        let surface_rot = surface_camera_rotation(east, north, up, 0.0, -0.2);

        // Parent the camera to the same surface Grid as terrain and rover
        // content. The persistent OriginAnchor tracks the selected camera.
        let local_tf = Transform::from_translation(new_tf_translation).with_rotation(surface_rot);
        migrate_to_grid(&mut commands, avatar_ent, target_grid, new_cell, local_tf);

        commands
            .entity(avatar_ent)
            .try_insert(GravityBody { body_entity })
            .try_insert(SurfaceRelativeMode)
            .try_insert(SurfaceCamera {
                heading: 0.0,
                pitch: -0.2,
            })
            .remove::<FreeFlightCamera>()
            .remove::<OrbitCamera>()
            .remove::<SpringArmCamera>();

        // Update LocalGravityField (world-space "up")
        field.body_entity = Some(body_entity);
        field.body_relative_position = surface_body_pos;
        field.local_up = surface_normal;
        field.surface_g = surface_g;
        let Some((_, body_world_rotation)) =
            lunco_spatial::coords::world_pose(body_entity, &q_parents, &q_grids, &q_spatial_abs)
                .ok()
        else {
            warn!("TELEPORT: body has no complete world BigSpace pose");
            return;
        };
        field.up = body_world_rotation.0 * surface_normal;

        debug!(
            "TELEPORT: done — camera now on surface grid {:?} at alt ~50m",
            target_grid
        );
    }
}

/// Leaves the surface and returns to orbit view.
///
/// Opens the same transactional orbit view as every other body-focus path.
/// Spatial placement is owned exclusively by `AvatarCelestialCameraPlugin`, which migrates
/// the avatar to the body's explicit star-fixed
/// [`lunco_celestial::ReferenceFrame::EclipticJ2000`].
#[on_command(LeaveSurface)]
fn on_leave_surface_command(
    trigger: On<LeaveSurface>,
    mut commands: Commands,
    q_avatar: Query<
        (
            Entity,
            Option<&GravityBody>,
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (With<Avatar>, With<LocalAvatar>),
    >,
    mut field: ResMut<LocalGravityField>,
) {
    let avatar_ent = trigger.event().target;
    let Ok((_, gravity_body, cinematic_lock)) = q_avatar.get(avatar_ent) else {
        warn!(?avatar_ent, "LEAVE SURFACE: target is not an avatar");
        return;
    };
    if cinematic_lock {
        return;
    }

    // Find the body we're leaving
    let body_entity = gravity_body
        .map(|gb| gb.body_entity)
        .unwrap_or(Entity::PLACEHOLDER);

    if body_entity == Entity::PLACEHOLDER {
        warn!("LEAVE SURFACE: avatar has no gravity body");
        return;
    }

    commands.trigger(FocusTarget {
        avatar: Some(avatar_ent),
        target: body_entity,
    });

    // Clear gravity field
    field.body_entity = None;
    field.body_relative_position = DVec3::ZERO;
    field.local_up = DVec3::Y;
    field.surface_g = 0.0;
    field.up = DVec3::Y;

    info!("Left surface, opened orbit view around {:?}", body_entity);
}

// ─── Surface Mode Transition ────────────────────────────────────────────────

/// Auto-inserts/removes `SurfaceRelativeMode` based on avatar altitude.
///
/// Uses hysteresis to prevent rapid toggling at the boundary:
/// - Below `engage_altitude` → insert `SurfaceRelativeMode`
/// - Above `disengage_altitude` → remove `SurfaceRelativeMode`
///
/// Altitude is computed as `|body_local_position| - body_radius` from the
/// avatar's `GravityBody` binding. Runs in `Update` so camera systems
/// see the mode change immediately.
fn surface_mode_transition_system(
    q_avatar: Query<
        (
            Entity,
            &Transform,
            &ChildOf,
            Option<&GravityBody>,
            Option<&SurfaceRelativeMode>,
            Option<&SurfaceCamera>,
            Option<&SpringArmCamera>,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<OrbitCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_bodies: Query<&CelestialBody>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    thresholds: Res<SurfaceModeThreshold>,
    field: Res<LocalGravityField>,
    q_site: Query<(), With<lunco_celestial::SiteAnchor>>,
    mut commands: Commands,
) {
    // An orbital view owns the complete camera pose. Surface policy must not
    // mutate a rig while its orbit mode is active.
    let Some((avatar_ent, transform, child_of, maybe_gb, maybe_mode, maybe_sc, maybe_spring)) =
        q_avatar.single().ok()
    else {
        return;
    };

    // Altitude comes from the same body-local position used by gravity and the
    // surface camera. This keeps the transition in the body's authored frame,
    // independent of the active BigSpace parent or camera pose.
    let engage_body = maybe_gb.map(|gb| gb.body_entity);
    let disengage_body = engage_body.or(field.body_entity);
    let altitude_to = |b: Entity| {
        (field.body_entity == Some(b))
            .then_some(field.body_relative_position.length())
            .zip(q_bodies.get(b).ok())
            .map(|(distance, body)| distance - body.radius_m)
    };
    let engage_altitude_m = engage_body.and_then(altitude_to).unwrap_or(f64::MAX);
    let altitude = disengage_body.and_then(altitude_to).unwrap_or(f64::MAX);

    // SurfaceRelativeMode is a coordinate-policy marker, not a camera mode.
    // `SurfaceCamera` owns a free camera's complete surface-relative pose;
    // `SpringArmCamera` owns a followed vessel pose and consumes the marker to
    // choose body-fixed ENU instead of world-Y.
    let camera_is_surface = maybe_sc.is_some();
    let spring_is_surface = maybe_spring.is_some();
    let has_surface_relative_writer = camera_is_surface || spring_is_surface;
    let marker_is_surface = maybe_mode.is_some();

    // An authored site keeps the body-fixed presentation policy through the
    // orbital handoff. Scroll transit changes to the orbital writer at the
    // configured handoff altitude.
    let site_anchored = !q_site.is_empty();

    if has_surface_relative_writer && altitude > thresholds.disengage_altitude && !site_anchored {
        // Too high → leave the surface coordinate policy. A free surface
        // camera changes to free flight; a spring arm remains the same writer
        // and resumes its non-surface heading basis.
        commands.entity(avatar_ent).remove::<SurfaceRelativeMode>();
        if let Some(sc) = maybe_sc {
            // Note: heading→yaw is approximate (different reference frames)
            // but provides a reasonable starting orientation.
            commands
                .entity(avatar_ent)
                .remove::<SurfaceCamera>()
                .try_insert(FreeFlightCamera {
                    yaw: sc.heading,
                    pitch: sc.pitch,
                    damping: None,
                });
        }
    } else if engage_altitude_m < thresholds.engage_altitude {
        // Low enough and explicitly bound to a body → enter surface mode.
        commands.entity(avatar_ent).try_insert(SurfaceRelativeMode);
        // A free camera needs the dedicated surface writer. A spring arm
        // already is the sole writer and derives its ENU orientation itself.
        if !has_surface_relative_writer {
            if let Some((east, north, up)) =
                surface_axes_in_grid(child_of.0, &field, &q_parents, &q_grids, &q_spatial)
            {
                let (heading, pitch) = surface_camera_angles(east, north, up, transform.rotation);
                commands
                    .entity(avatar_ent)
                    .remove::<FreeFlightCamera>()
                    .try_insert(SurfaceCamera { heading, pitch });
            }
        }
    } else if marker_is_surface && !has_surface_relative_writer {
        // A marker without a surface-relative pose owner is not a valid state.
        commands.entity(avatar_ent).remove::<SurfaceRelativeMode>();
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
    use lunco_camera_core::FollowAttitude;

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

    #[test]
    fn surface_policy_preserves_the_possessed_spring_arm_writer() {
        let mut app = App::new();
        app.init_resource::<SurfaceModeThreshold>()
            .insert_resource(LocalGravityField {
                body_entity: None,
                body_relative_position: bevy::math::DVec3::ZERO,
                up: bevy::math::DVec3::Y,
                local_up: bevy::math::DVec3::Y,
                surface_g: 1.0,
            })
            .add_systems(Update, surface_mode_transition_system);

        let body = app
            .world_mut()
            .spawn(CelestialBody {
                name: "test body".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: 100.0,
            })
            .id();
        app.world_mut()
            .resource_mut::<LocalGravityField>()
            .body_entity = Some(body);
        app.world_mut()
            .resource_mut::<LocalGravityField>()
            .body_relative_position = bevy::math::DVec3::Y * 101.0;

        let grid = app.world_mut().spawn(Grid::new(2_000.0, 0.0)).id();
        let target = app.world_mut().spawn_empty().id();
        let avatar = app
            .world_mut()
            .spawn((
                Avatar,
                LocalAvatar,
                Transform::default(),
                CellCoord::ZERO,
                ChildOf(grid),
                GravityBody { body_entity: body },
                SurfaceRelativeMode,
                SpringArmCamera {
                    target,
                    distance: 15.0,
                    yaw: 0.0,
                    pitch: -0.25,
                    damping: None,
                    vertical_offset: 2.0,
                    track_heading: true,
                    attitude: FollowAttitude::Heading,
                },
            ))
            .id();

        app.update();

        assert!(app.world().get::<SpringArmCamera>(avatar).is_some());
        assert!(app.world().get::<SurfaceCamera>(avatar).is_none());
        assert!(app.world().get::<SurfaceRelativeMode>(avatar).is_some());
    }
}
