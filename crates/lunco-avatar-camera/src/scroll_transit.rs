use avian3d::prelude::MoveAndSlide;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::{
    CAMERA_ZOOM_SENSITIVITY, OrbitReturnBehavior, OrbitViewHistory, OrbitViewReturn, RadialArrival,
    SURFACE_ORBIT_HANDOFF_ALTITUDE_M,
};
use lunco_avatar_policy::{
    AvatarCollisionSettings, AvatarSoilCollisionPolicy, avatar_soil_collision_policy,
};
use lunco_camera_core::{
    CameraPoseLock, CameraZoomInput, FreeFlightCamera, OrbitCamera, SpringArmCamera, SurfaceCamera,
    SurfaceRelativeMode, math::zoom_factor,
};
use lunco_celestial::CelestialBody;
use lunco_celestial::{GeodeticAnchor, SiteAnchor};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_environment::GravityBody;
use lunco_interaction_core::DragModeActive;
use lunco_spatial::ActivePhysicsFrame;
use lunco_spatial::coords::grid_absolute_seeded;
use lunco_workspace::WorkspaceResource;

use crate::locomotion::{
    move_avatar_with_collision, report_avatar_policy_error, write_avatar_grid_position,
};

/// Select the pose for a surface-to-orbit scroll entry. The first entry has no
/// avatar-owned orbital presentation to restore and must derive its arm from
/// the live surface position; a later entry reuses the settled body pose.
fn scroll_entry_orbit_camera(
    target: Entity,
    body: &CelestialBody,
    radius_m: f64,
    history: Option<&OrbitViewHistory>,
) -> (OrbitCamera, bool) {
    let saved_pose = history.and_then(|history| history.pose(body.ephemeris_id));
    let camera = OrbitCamera {
        target,
        distance: saved_pose.map_or(radius_m * 3.0, |pose| pose.distance()),
        yaw: saved_pose.map_or(0.0, |pose| pose.yaw()),
        pitch: saved_pose.map_or(0.0, |pose| pose.pitch()),
        damping: saved_pose.and_then(|pose| pose.damping()),
        vertical_offset: saved_pose.map_or(0.0, |pose| pose.vertical_offset()),
    };
    (camera, saved_pose.is_none())
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
            Option<&SurfaceCamera>,
            Option<&FreeFlightCamera>,
            Option<&OrbitViewHistory>,
            Option<&GravityBody>,
            Has<SurfaceRelativeMode>,
            Has<CameraPoseLock>,
        ),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
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
    active_frame: Option<Res<ActivePhysicsFrame>>,
    move_and_slide: Option<MoveAndSlide<'_, '_>>,
    collision_settings: Res<AvatarCollisionSettings>,
    workspace: Option<Res<WorkspaceResource>>,
    mut policy_error: Local<Option<String>>,
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
            report_avatar_policy_error(&error, &mut policy_error);
            for (_, _, _, _, mut zoom, _, _, _, _, _, _, _) in q_avatar.iter_mut() {
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
        surface_camera,
        freeflight_camera,
        orbit_history,
        gravity_body,
        surface_relative,
        cinematic_lock,
    ) in q_avatar.iter_mut()
    {
        if cinematic_lock {
            continue;
        }
        if zoom.delta == 0.0 {
            continue;
        }
        // Only the active render camera transits (scenes carry inactive
        // Embodiment-tagged spawn cameras — same guard as `on_focus_command`).
        if !cam.is_some_and(|c| c.is_active) {
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
        let alt = (pos - center).length() - radius_m;
        let factor = zoom_factor(zoom.delta, CAMERA_ZOOM_SENSITIVITY);
        let transition_direction = zoom.delta;
        let scroll_out = zoom.delta < 0.0;
        zoom.delta = 0.0;
        let step = alt.abs().max(50.0) * (factor - 1.0);
        let fwd = (tf.rotation * Vec3::NEG_Z).as_dvec3();
        let desired_next = pos - fwd * step;
        let next = if policy == AvatarSoilCollisionPolicy::ThroughSoilAllowed {
            desired_next
        } else {
            let up_direction = if surface_relative {
                (pos - center).normalize_or(DVec3::Y).as_vec3()
            } else {
                Vec3::Y
            };
            let Some(next) = move_avatar_with_collision(
                avatar_ent,
                child_of.parent(),
                &cell,
                &tf,
                desired_next - pos,
                up_direction,
                std::time::Duration::from_secs(1),
                active_frame.as_deref().map(|frame| frame.0),
                move_and_slide.as_ref(),
                &collision_settings,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) else {
                warn_once!(
                    "[avatar] safe collision movement unavailable for scroll transit; movement held"
                );
                continue;
            };
            next
        };
        write_avatar_grid_position(grid, &mut cell, &mut tf, next);

        // Past the orbital floor going OUT -> hand over to the celestial
        // OrbitCamera. A first entry derives the arm from the exact transit pose;
        // a later entry restores the avatar's saved body presentation pose.
        if scroll_out && (next - center).length() - radius_m > SURFACE_ORBIT_HANDOFF_ALTITUDE_M {
            let Ok((_, body)) = q_bodies.get(body_ent) else {
                warn!(
                    target = ?body_ent,
                    "SURFACE SCROLL-OUT: site body disappeared before orbit handoff"
                );
                continue;
            };
            let (orbit_camera, needs_radial_arrival) =
                scroll_entry_orbit_camera(body_ent, body, radius_m, orbit_history);
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
                .try_insert(orbit_camera);
            if needs_radial_arrival {
                entity.try_insert(RadialArrival);
            }
            zoom.begin_mode_transition(Some(transition_direction));
            info!(
                restored = !needs_radial_arrival,
                "SURFACE SCROLL-OUT: entering orbital view"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_scroll_entry_restores_saved_body_pose_without_radial_arrival() {
        let body = CelestialBody {
            name: "Moon".into(),
            ephemeris_id: lunco_celestial::ephemeris_id::MOON,
            radius_m: 1_737_400.0,
        };
        let saved = OrbitCamera {
            target: Entity::PLACEHOLDER,
            distance: 8_000.0,
            yaw: 0.7,
            pitch: -0.3,
            damping: Some(0.2),
            vertical_offset: 4.0,
        };
        let mut history = OrbitViewHistory::default();
        history.remember(
            body.ephemeris_id,
            lunco_avatar_camera_core::OrbitPose::from_camera(&saved)
                .expect("finite saved orbit pose"),
        );

        let (restored, needs_radial_arrival) =
            scroll_entry_orbit_camera(Entity::PLACEHOLDER, &body, body.radius_m, Some(&history));
        assert!(!needs_radial_arrival);
        assert_eq!(restored.target, saved.target);
        assert_eq!(restored.distance, saved.distance);
        assert_eq!(restored.yaw, saved.yaw);
        assert_eq!(restored.pitch, saved.pitch);
        assert_eq!(restored.damping, saved.damping);
        assert_eq!(restored.vertical_offset, saved.vertical_offset);

        let (first_entry, needs_radial_arrival) =
            scroll_entry_orbit_camera(Entity::PLACEHOLDER, &body, body.radius_m, None);
        assert!(needs_radial_arrival);
        assert_eq!(first_entry.distance, body.radius_m * 3.0);
        assert_eq!(first_entry.yaw, 0.0);
        assert_eq!(first_entry.pitch, 0.0);
    }
}
