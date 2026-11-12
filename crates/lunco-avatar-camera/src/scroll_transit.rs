use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::{
    CAMERA_ZOOM_SENSITIVITY, OrbitReturnBehavior, OrbitViewReturn, RadialArrival,
};
use lunco_avatar_policy::{AvatarSoilCollisionPolicy, avatar_soil_collision_policy};
use lunco_camera_core::{
    CameraPoseLock, CameraZoomInput, FreeFlightCamera, OrbitCamera, SpringArmCamera, SurfaceCamera,
    SurfaceRelativeMode, math::zoom_factor,
};
use lunco_camera_runtime::CameraInputSettings;
use lunco_celestial::CelestialBody;
use lunco_celestial::{GeodeticAnchor, SiteAnchor};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_environment::GravityBody;
use lunco_interaction_core::DragModeActive;
use lunco_spatial::coords::GridPos;
use lunco_spatial::coords::grid_absolute_seeded;
use lunco_workspace::WorkspaceResource;

use crate::locomotion::{
    clear_camera_preparation, move_avatar_with_collision, report_avatar_policy_error,
    report_camera_motion_error, write_avatar_grid_position,
};

/// A discrete scroll keeps its admitted endpoint while visual terrain prepares.
/// The camera entity owns the lease; a new input supersedes this request.
#[derive(Component, Clone)]
pub(crate) struct PendingCameraScroll {
    source_grid: Entity,
    frame: Entity,
    source: lunco_terrain_surface::TerrainSurfaceSnapshot,
    start: DVec3,
    end: DVec3,
    frame_start: GridPos,
    frame_end: GridPos,
    rotation: Quat,
    surface_mode: bool,
    settings: lunco_avatar_policy::AvatarCollisionSettings,
    radius_m: f64,
    draw_revision: Option<u64>,
    transition_direction: f32,
    scroll_out: bool,
}

/// Retire pending scroll with its source/mode even when the scene no longer
/// has a site and the transit controller itself has no work to perform.
pub(crate) fn retire_pending_camera_scroll(
    mut commands: Commands,
    pending: Query<(
        Entity,
        &PendingCameraScroll,
        Option<&ChildOf>,
        Has<LocalEmbodiment>,
        Has<FreeFlightCamera>,
        Has<SurfaceCamera>,
        Has<OrbitCamera>,
        Has<SpringArmCamera>,
        Has<CameraPoseLock>,
    )>,
    terrain: lunco_terrain_surface::GridSurfaceQuery,
    settings: Res<lunco_avatar_policy::AvatarCollisionSettings>,
    mut targets: ResMut<lunco_terrain_surface::TerrainCameraTargets>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if pending.is_empty() {
        return;
    }
    let source = terrain.snapshot();
    for (camera, request, parent, local, free, surface, orbit, follow, locked) in &pending {
        let valid = local
            && (free || surface)
            && surface == request.surface_mode
            && !orbit
            && !follow
            && !locked
            && parent.is_some_and(|parent| parent.parent() == request.source_grid)
            && *settings == request.settings
            && source
                .as_ref()
                .is_some_and(|source| source.same_source(&request.source));
        if !valid {
            commands.entity(camera).remove::<PendingCameraScroll>();
            targets.clear(camera);
            clear_camera_preparation(&mut diagnostics, "camera-scroll", &mut None);
        }
    }
}

/// Seed a surface-to-orbit entry. `RadialArrival` replaces these orbital
/// parameters with the camera's live pose in the target's inertial frame.
fn scroll_entry_orbit_camera(target: Entity, radius_m: f64) -> (OrbitCamera, RadialArrival) {
    let camera = OrbitCamera {
        target,
        distance: radius_m * 3.0,
        yaw: 0.0,
        pitch: 0.0,
        damping: None,
        vertical_offset: 0.0,
    };
    (camera, RadialArrival)
}

/// The body explicitly authored by the loaded site.
fn site_body(
    q_site: &Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_bodies: &Query<(Entity, &CelestialBody)>,
) -> Option<(Entity, f64)> {
    let anchor = q_site.single().ok()?;
    let (ent, body) = q_bodies
        .iter()
        .find(|(_, body)| body.ephemeris_id == anchor.body)?;
    Some((ent, body.radius_m))
}

/// Free-flight scroll transit — the ENTRY half of the scroll loop (the exit
/// half is the ORBITAL SCROLL-THROUGH in `AvatarCelestialCameraPlugin`). On a site-anchored
/// celestial scene, the wheel DOLLIES the free-flight camera along its LOOK
/// direction with an exponential step scaled by altitude (approach slows near
/// the ground, retreat accelerates with height) — "scroll toward what you
/// look at". Once a scroll-OUT carries the camera past the orbital zoom
/// floor, the avatar hands over to the celestial `OrbitCamera` AT ITS
/// CURRENT POSE: [`RadialArrival`] derives the arm from the camera's present
/// position (preserving the current pose), and because the handover altitude
/// equals the orbital floor the arm is already legal — no clamp jump, one
/// continuous gesture from ground to orbit. The descent mirrors it:
/// scroll-through at the floor releases back to free flight (pose parked in
/// the pin on this entry), where scroll-in keeps dollying down.
pub(crate) fn freeflight_scroll_transit_system(
    mut commands: Commands,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &mut CameraZoomInput,
            Option<&Camera>,
            &Projection,
            Option<&SurfaceCamera>,
            Option<&FreeFlightCamera>,
            Option<&GravityBody>,
            Has<SurfaceRelativeMode>,
            Has<CameraPoseLock>,
            Option<&PendingCameraScroll>,
        ),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            Without<Grid>,
            Or<(With<FreeFlightCamera>, With<SurfaceCamera>)>,
            Without<OrbitCamera>,
            Without<SpringArmCamera>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    drag_mode: Option<Res<DragModeActive>>,
    mut collision: crate::locomotion::NavigationCollisionContext,
    camera_input_settings: Res<CameraInputSettings>,
    workspace: Option<Res<WorkspaceResource>>,
    mut policy_error: Local<Option<String>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    // Only meaningful on a site-anchored scene whose solar hierarchy is up.
    let Some((body_ent, radius_m)) = site_body(&q_site, &q_bodies) else {
        return;
    };
    let policy = match avatar_soil_collision_policy(workspace.as_deref()) {
        Ok(policy) => policy,
        Err(error) => {
            report_avatar_policy_error(
                &error,
                &mut policy_error,
                &mut diagnostics,
                "camera-scroll",
            );
            for (_, _, _, _, mut zoom, _, _, _, _, _, _, _, _) in q_avatar.iter_mut() {
                zoom.delta = 0.0;
            }
            return;
        }
    };
    for (
        avatar_ent,
        mut tf,
        mut cell,
        child_of,
        mut zoom,
        cam,
        projection,
        surface_camera,
        freeflight_camera,
        gravity_body,
        surface_relative,
        cinematic_lock,
        pending,
    ) in q_avatar.iter_mut()
    {
        if cinematic_lock {
            commands.entity(avatar_ent).remove::<PendingCameraScroll>();
            collision.targets.clear(avatar_ent);
            continue;
        }
        if zoom.delta == 0.0 && pending.is_none() {
            continue;
        }
        // Only the active render camera transits (scenes carry inactive
        // Embodiment-tagged spawn cameras — same guard as `on_focus_command`).
        if !cam.is_some_and(|c| c.is_active) {
            commands.entity(avatar_ent).remove::<PendingCameraScroll>();
            collision.targets.clear(avatar_ent);
            clear_camera_preparation(&mut diagnostics, "camera-scroll", &mut policy_error);
            zoom.delta = 0.0;
            continue;
        }
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            zoom.delta = 0.0;
            continue;
        };
        let Some((center, _)) = lunco_spatial::coords::pose_in_grid(
            body_ent,
            child_of.parent(),
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            zoom.delta = 0.0;
            continue;
        };
        let Some(pos) = grid_absolute_seeded(avatar_ent, Some(&cell), &tf, &q_parents, &q_grids)
            .map(|position| position.0)
        else {
            zoom.delta = 0.0;
            continue;
        };
        let new_input = zoom.delta != 0.0;
        let (desired_next, transition_direction, scroll_out) = if !new_input {
            let Some(request) = pending else { continue };
            let valid = request.source_grid == child_of.parent()
                && request.start == pos
                && request.rotation == tf.rotation
                && request.surface_mode == surface_camera.is_some()
                && request.settings == *collision.settings
                && collision
                    .active_frame
                    .as_deref()
                    .is_some_and(|frame| frame.0 == request.frame)
                && collision
                    .terrain
                    .snapshot()
                    .is_some_and(|source| source.same_source(&request.source))
                && collision
                    .settings
                    .camera_radius(projection)
                    .is_ok_and(|radius| radius == request.radius_m);
            if !valid {
                commands.entity(avatar_ent).remove::<PendingCameraScroll>();
                collision.targets.clear(avatar_ent);
                clear_camera_preparation(&mut diagnostics, "camera-scroll", &mut policy_error);
                continue;
            }
            if policy == AvatarSoilCollisionPolicy::CollisionEnabled
                && collision.drawn.revision(avatar_ent) == request.draw_revision
            {
                if let Err(reason) = collision.targets.request(
                    avatar_ent,
                    request.source.clone(),
                    request.frame_start,
                    request.frame_end,
                    request.radius_m + request.settings.skin_m,
                ) {
                    commands.entity(avatar_ent).remove::<PendingCameraScroll>();
                    report_camera_motion_error(
                        &reason.into(),
                        &mut policy_error,
                        &mut diagnostics,
                        "camera-scroll",
                    );
                }
                continue;
            }
            (
                request.end,
                request.transition_direction,
                request.scroll_out,
            )
        } else {
            commands.entity(avatar_ent).remove::<PendingCameraScroll>();
            let datum_altitude_m = (pos - center).length() - radius_m;
            let alt = crate::terrain_handoff::clearance_m(
                avatar_ent,
                Some(&cell),
                &tf,
                collision.active_frame.as_deref(),
                &collision.terrain,
                &q_parents,
                &q_grids,
                &q_spatial,
            )
            .unwrap_or(datum_altitude_m);
            let factor = zoom_factor(zoom.delta, CAMERA_ZOOM_SENSITIVITY);
            let direction = zoom.delta;
            zoom.delta = 0.0;
            let step = alt.abs().max(50.0) * (factor - 1.0);
            let fwd = (tf.rotation * Vec3::NEG_Z).as_dvec3();
            (pos - fwd * step, direction, direction < 0.0)
        };
        let next = if policy == AvatarSoilCollisionPolicy::ThroughSoilAllowed {
            collision.targets.clear(avatar_ent);
            desired_next
        } else {
            let next = match move_avatar_with_collision(
                avatar_ent,
                child_of.parent(),
                &cell,
                &tf,
                desired_next - pos,
                collision.active_frame.as_deref().map(|frame| frame.0),
                collision.native.as_ref(),
                &collision.settings,
                projection,
                &collision.terrain,
                collision
                    .budget
                    .as_deref()
                    .map(|budget| budget.max_intervals),
                &collision.drawn,
                &mut collision.targets,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) {
                Ok(position) => position,
                Err(error) => {
                    if let crate::locomotion::CameraMotionError::Preparation { target, .. } = &error
                    {
                        if let (Some(frame), Some(source), Ok(camera_radius)) = (
                            collision.active_frame.as_deref(),
                            collision.terrain.snapshot(),
                            collision.settings.camera_radius(projection),
                        ) {
                            if let Some(to_frame) =
                                lunco_spatial::coords::grid_transform_between_grids(
                                    child_of.parent(),
                                    frame.0,
                                    &q_parents,
                                    &q_grids,
                                    &q_spatial,
                                )
                            {
                                commands.entity(avatar_ent).try_insert(PendingCameraScroll {
                                    source_grid: child_of.parent(),
                                    frame: frame.0,
                                    source,
                                    start: pos,
                                    end: desired_next,
                                    frame_start: GridPos(to_frame.transform_position(pos)),
                                    frame_end: *target,
                                    rotation: tf.rotation,
                                    surface_mode: surface_camera.is_some(),
                                    settings: *collision.settings,
                                    radius_m: camera_radius,
                                    draw_revision: collision.drawn.revision(avatar_ent),
                                    transition_direction,
                                    scroll_out,
                                });
                            }
                        }
                    } else {
                        commands.entity(avatar_ent).remove::<PendingCameraScroll>();
                    }
                    report_camera_motion_error(
                        &error,
                        &mut policy_error,
                        &mut diagnostics,
                        "camera-scroll",
                    );
                    continue;
                }
            };
            next
        };
        commands.entity(avatar_ent).remove::<PendingCameraScroll>();
        if policy_error.take().is_some() {
            lunco_camera_core::replace_camera_diagnostic(
                &mut diagnostics,
                "camera-scroll",
                "CameraNavigation",
                None,
            );
        }
        write_avatar_grid_position(grid, &mut cell, &mut tf, next);

        // Past the orbital floor going OUT -> hand over to the celestial
        // OrbitCamera. Every entry derives the arm from the exact transit pose;
        // per-body history is reserved for explicit focus changes.
        let next_datum_altitude_m = (next - center).length() - radius_m;
        let next_altitude_m = crate::terrain_handoff::clearance_m(
            avatar_ent,
            Some(&cell),
            &tf,
            collision.active_frame.as_deref(),
            &collision.terrain,
            &q_parents,
            &q_grids,
            &q_spatial,
        )
        .unwrap_or(next_datum_altitude_m);
        if scroll_out && next_altitude_m > camera_input_settings.surface_mode_engage_altitude_m {
            let Ok(_) = q_bodies.get(body_ent) else {
                warn!(
                    target = ?body_ent,
                    "SURFACE SCROLL-OUT: site body disappeared before orbit handoff"
                );
                continue;
            };
            let (orbit_camera, radial_arrival) = scroll_entry_orbit_camera(body_ent, radius_m);
            let behavior = surface_camera
                .cloned()
                .map(OrbitReturnBehavior::Surface)
                .or_else(|| {
                    freeflight_camera
                        .cloned()
                        .map(OrbitReturnBehavior::FreeFlight)
                })
                .expect("surface scroll transit requires one camera behavior");
            let mut entity = commands.entity(avatar_ent);
            entity
                .try_insert(OrbitViewReturn::new(
                    child_of.parent(),
                    *cell,
                    *tf,
                    behavior,
                    gravity_body.copied(),
                    surface_relative,
                ))
                .remove::<SpringArmCamera>()
                .remove::<FreeFlightCamera>()
                .remove::<SurfaceCamera>()
                .remove::<SurfaceRelativeMode>()
                .remove::<GravityBody>()
                .try_insert((orbit_camera, radial_arrival));
            zoom.begin_mode_transition(Some(transition_direction));
            info!("SURFACE SCROLL-OUT: entering orbital view from live pose");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scroll_entry_derives_its_pose_from_the_live_camera() {
        let radius_m = 1_737_400.0;
        let (camera, radial_arrival) = scroll_entry_orbit_camera(Entity::PLACEHOLDER, radius_m);

        assert_eq!(camera.target, Entity::PLACEHOLDER);
        assert_eq!(camera.distance, radius_m * 3.0);
        assert_eq!(camera.yaw, 0.0);
        assert_eq!(camera.pitch, 0.0);
        assert!(camera.damping.is_none());
        assert_eq!(camera.vertical_offset, 0.0);
        let _: RadialArrival = radial_arrival;
    }
}
