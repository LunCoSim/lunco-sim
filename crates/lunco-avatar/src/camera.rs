use super::*;

/// FreeFlightCamera system: moves the camera in absolute coordinates.
///
/// Only runs when `FreeFlightCamera` is the active camera mode.
/// Position is set by `apply_fly`. This system
/// applies yaw/pitch rotation from user input.
///
/// Note: `FreeFlightCamera` and `SurfaceCamera` are mutually exclusive.
/// `SurfaceCamera` owns the surface-relative rotation policy; this system owns
/// only the ecliptic free-flight rotation.
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
pub(super) fn freeflight_scroll_transit_system(
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
            Has<lunco_camera_core::CameraPoseLock>,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Or<(With<FreeFlightCamera>, With<SurfaceCamera>)>,
            Without<OrbitCamera>,
            Without<SpringArmCamera>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody)>,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    move_and_slide: Option<MoveAndSlide<'_, '_>>,
    collision_settings: Res<AvatarCollisionSettings>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
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
        // Avatar-tagged spawn cameras — same guard as `on_focus_command`).
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
        let pos = grid.grid_position_double(&cell, &tf);
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
