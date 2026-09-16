use super::*;

pub(super) fn spring_arm_system(
    time: Res<Time<Real>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &mut SpringArmCamera,
            &ChildOf,
            Option<&SurfaceRelativeMode>,
            &mut CameraZoomInput,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<Grid>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            Without<SurfaceCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    gravity: Res<LocalGravityField>,
    q_dragging: Query<(), With<lunco_interaction_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<lunco_physics::GridSpatialQuery>,
    joints: VesselJoints,
    mut collision_filters: Local<VesselCollisionFilterCache>,
    mut topology: VesselCollisionTopology,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }
    let dt = time.delta_secs();
    collision_filters.refresh(&joints, &mut topology);

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode, mut zoom) in
        q_avatar.iter_mut()
    {
        // Skip follow while the target is being dragged by the editor gizmo
        // (marker set by luncosim-edit; never present on a headless server).
        if q_dragging.get(arm.target).is_ok() {
            continue;
        }

        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
        // Possession/follow migration puts the target and avatar in one live
        // body-local BigSpace frame. Keep the render-rate follow solve in that
        // frame as well. A target -> solar root -> avatar-grid round trip makes
        // the camera depend on site pinning and celestial propagation order.
        let Some((target_pos, target_rotation)) = lunco_spatial::coords::grid_relative_pose(
            arm.target, child_of.0, &q_parents, &q_grids, &q_spatial,
        ) else {
            continue;
        };
        let surface_axes = surface_mode.and_then(|_| {
            gravity.body_entity.and_then(|body_entity| {
                surface_axes_for_grid_position(
                    child_of.0,
                    target_pos,
                    body_entity,
                    &q_parents,
                    &q_grids,
                    &q_spatial,
                )
            })
        });

        // Multiplicative zoom using exponential scaling — same formula as
        // ChaseCamera/OrbitCamera so raw pixel scroll deltas stay well-scaled.
        // Scroll up (delta > 0) -> zoom in. Scroll down (delta < 0) -> zoom out.
        apply_scroll_zoom(
            &mut arm.distance,
            &mut zoom.delta,
            CAMERA_ZOOM_SENSITIVITY,
            5.0,
            200.0,
        );

        // Resolve rover heading in double-precision to eliminate quantization
        // jitter. The rover Transform is already render-frame-interpolated by
        // avian's `PhysicsInterpolationPlugin::interpolate_all()` (runs in
        // `RunFixedMainLoop` before Update), so reading it directly here
        // gives a smooth signal — no extra low-pass needed. An additional
        // exp-decay filter would re-introduce jitter under variable frame
        // time because alpha = 1 - exp(-rate*dt) makes the per-frame catch-up
        // step proportional to dt, so the camera's lag wobbles around its
        // mean as frame timing fluctuates.
        // Only steerable vehicles have a meaningful body heading. A freely-
        // rolling rigid body (ball, balloon) tumbles its body frame, so its
        // forward vector flips around as it rolls — deriving heading from it
        // swings the camera wildly. For those, heading is user-only (yaw).
        // Desired orientation — the ONE axis the three follow modes differ on.
        let desired_rot = match arm.attitude {
            // Cockpit frame: full body orientation × user yaw/pitch offset. The
            // camera rolls with the craft (was the separate `ChaseCamera`).
            FollowAttitude::FullAttitude => {
                target_rotation.as_quat() * Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0)
            }
            // Stable external frame: ignore the body's attitude entirely, so a
            // 6-DOF flyer tumbles inside a steady view. World-up, user yaw/pitch
            // (was the celestial `OrbitCamera`, reused wrongly for vessels).
            FollowAttitude::WorldLocked => Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0),
            // Heading-follow: yaw from the body's forward (steerable vehicles),
            // up = surface normal or world-Y.
            FollowAttitude::Heading => {
                let target_heading_d = if arm.track_heading {
                    if let Some((east, north, up)) = surface_axes {
                        surface_camera_angles(east, north, up, target_rotation.as_quat()).0 as f64
                    } else {
                        let target_fwd_d = target_rotation.mul_vec3(Vec3::NEG_Z.as_dvec3());
                        if target_fwd_d.x.abs() > 1e-6 || target_fwd_d.z.abs() > 1e-6 {
                            -target_fwd_d.x.atan2(-target_fwd_d.z)
                        } else {
                            0.0
                        }
                    }
                } else {
                    0.0
                };
                let final_yaw = (target_heading_d + arm.yaw as f64) as f32;
                if let Some((east, north, up)) = surface_axes {
                    surface_camera_rotation(east, north, up, final_yaw, arm.pitch)
                } else {
                    Quat::from_euler(EulerRot::YXZ, final_yaw, arm.pitch, 0.0)
                }
            }
        };

        // Rotation: exponential decay for snappy but smooth heading follow.
        // Frequency 60.0 — snappy without transmitting physics jitter.
        let damping = arm.damping.unwrap_or(defaults.damping);
        let mut next_rotation = tf.rotation;
        next_rotation.smooth_nudge(
            &desired_rot,
            camera_decay_rate(defaults.rotation_rate, damping),
            dt,
        );
        // `Mut<Transform>` is change-detected on mutable dereference, not on
        // value inequality.  Do not wake BigSpace's dirty-subtree walk for a
        // parked camera whose solved pose is already stable.
        if tf.rotation != next_rotation {
            tf.rotation = next_rotation;
        }

        // Desired camera position: behind target along smoothed rotation.
        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * arm.distance;
        let vertical_offset: DVec3 = if surface_mode.is_some() {
            let Some(up) = surface_axes.map(|(_, _, up)| up).or_else(|| {
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            }) else {
                continue;
            };
            up.as_dvec3() * arm.vertical_offset as f64
        } else {
            DVec3::Y * arm.vertical_offset as f64
        };
        let desired_pos = target_pos + offset + vertical_offset;

        // Raycast from rover toward desired camera position.
        // If something blocks (wall, ramp, etc.), place the camera on the
        // SAME SIDE as the rover so the user can see through the obstacle.
        let ray_origin = target_pos;
        let ray_dir = (desired_pos - target_pos).normalize_or(DVec3::Y);
        let ray_len = desired_pos.distance(target_pos);
        // Mask out the TRIGGER layer so the camera doesn't clip on invisible
        // trigger-zone sensors (waypoints etc.).
        //
        // The exclusion set is the whole JOINTED VESSEL — see
        // `vessel_collision_exclusions`.
        // A non-finite origin is not a camera problem to solve, but it IS one this
        // cast cannot survive: obvhs asserts `origin.is_finite()`, so following a
        // vessel whose solve has diverged would panic the compute pool from inside
        // the camera. No hit means the arm simply does not shorten, which is the
        // right behaviour for a frame with nothing valid to look at.
        let castable = ray_origin.is_finite() && ray_len.is_finite();
        let hit = match &spatial_query {
            Some(sq) if castable => {
                let filter = collision_filters.filter_for(arm.target, &q_children);
                sq.cast_ray_in_grid(
                    child_of.0,
                    lunco_spatial::coords::GridPos(ray_origin),
                    bevy::math::Dir3::new(ray_dir.as_vec3()).unwrap_or(bevy::math::Dir3::Y),
                    ray_len,
                    true,
                    filter,
                )
            }
            _ => None,
        };

        // Collision response: only an active obstacle may smooth the arm LENGTH.
        // On a clear ray, use the requested length directly so target translation
        // cannot masquerade as camera-distance motion. The arm DIRECTION (ray_dir)
        // already tracks the user's rotation instantly.
        let desired_len = ray_len;
        let target_len = match hit {
            Some(hit_data) => ((hit_data.distance - 0.5).min(desired_len)).max(0.0),
            None => desired_len,
        };
        let current_pos = grid.grid_position_double(&cell, &tf);
        let current_len = current_pos.distance(target_pos);
        let final_len = resolve_camera_arm_length(
            current_len,
            target_len,
            hit.is_some(),
            defaults.position_rate,
            damping,
            dt,
        );
        let final_pos = target_pos + ray_dir * final_len;

        let (new_cell, new_tf) = grid.translation_to_grid(final_pos);
        cell.set_if_neq(new_cell);
        if tf.translation != new_tf {
            tf.translation = new_tf;
        }
    }
}
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
        // Same exponential the orbit arm uses (`apply_scroll_zoom`), applied
        // to the altitude scale: factor > 1 on scroll-out, < 1 on scroll-in.
        // Clamped to ±25% per FRAME: wheel events batch, and an accumulated
        // delta must never become a teleport-sized step.
        let factor = zoom_factor(zoom.delta, CAMERA_ZOOM_SENSITIVITY);
        let transition_direction = zoom.delta;
        let scroll_out = zoom.delta < 0.0;
        zoom.delta = 0.0;
        // Signed dolly step: negative (forward) on scroll-in. The 50 m floor
        // keeps ground-level scrolling responsive. The same avatar collision
        // owner handles this path, so a wheel gesture cannot tunnel through
        // the terrain while keyboard movement is protected.
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

        // Past the orbital floor going OUT → hand over to the celestial
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
                // The first entry has no user presentation state to restore, so
                // derive its arm from the live surface region for continuity.
                entity.try_insert(RadialArrival);
            }
            // The scroll that crossed the surface/orbit threshold belongs to
            // the surface owner. Do not let later packets from that same
            // wheel gesture reach the newly inserted orbit owner.
            zoom.begin_mode_transition(Some(transition_direction));
            info!(
                restored = !needs_radial_arrival,
                "SURFACE SCROLL-OUT: entering orbital view"
            );
        }
    }
}
