use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::CAMERA_ZOOM_SENSITIVITY;
use lunco_camera_core::{
    CameraDefaults, CameraZoomInput, FollowAttitude, FreeFlightCamera, OrbitCamera,
    SpringArmCamera, SurfaceCamera, SurfaceRelativeMode,
    math::{
        apply_scroll_zoom, camera_decay_alpha, resolve_camera_arm_length, surface_camera_angles,
        surface_camera_rotation,
    },
};
use lunco_celestial_spatial_core::{
    LocalGravityField, gravity_up_in_grid, surface_axes_for_grid_position,
};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_physics::GridSpatialQuery;
use lunco_spatial::attach::local_pose_to_grid_storage;
use lunco_spatial::coords::{GridPos, grid_absolute_seeded};

use lunco_camera_core::SetCameraFollow;
use lunco_camera_core::follow_policy::{FollowFacts as F, FollowPlan, resolve_follow_plan};

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct FollowPolicyContext<'w, 's> {
    terrain: lunco_terrain_surface::GridSurfaceQuery<'w, 's>,
    drawn: lunco_terrain_surface::DrawTerrainQuery<'w, 's>,
    targets: ResMut<'w, lunco_terrain_surface::TerrainCameraTargets>,
    active_frame: Option<Res<'w, lunco_spatial::ActivePhysicsFrame>>,
    collision_settings: Res<'w, lunco_avatar_policy::AvatarCollisionSettings>,
    terrain_budget: Option<Res<'w, lunco_terrain_surface::SurfaceOcclusionBudget>>,
    subjects: Query<
        'w,
        's,
        (
            Option<&'static Name>,
            Option<&'static lunco_camera_core::CameraFollow>,
            Option<&'static lunco_core::GlobalEntityId>,
            Option<&'static avian3d::prelude::Rotation>,
            Option<&'static avian3d::prelude::LinearVelocity>,
            Option<&'static avian3d::prelude::AngularVelocity>,
        ),
    >,
    diagnostics: Option<ResMut<'w, lunco_core::RuntimeDiagnostics>>,
}

impl FollowPolicyContext<'_, '_> {
    fn report(&mut self, error: Option<String>) {
        let previous = self
            .diagnostics
            .as_deref()
            .and_then(|d| d.findings.iter().find(|f| f.producer == "camera-follow"))
            .map(|f| f.message.as_str());
        if previous == error.as_deref() {
            return;
        }
        lunco_camera_core::replace_camera_diagnostic(
            &mut self.diagnostics,
            "camera-follow",
            "SpringArmCamera",
            error,
        );
    }
}

use crate::collision::{VesselCollisionFilterCache, VesselCollisionTopology, VesselJoints};

pub(crate) fn spring_arm_system(
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
            &Projection,
        ),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            Without<Grid>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            Without<SurfaceCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    gravity: Res<LocalGravityField>,
    q_dragging: Query<(), With<lunco_interaction_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<GridSpatialQuery>,
    joints: VesselJoints,
    mut collision_filters: Local<VesselCollisionFilterCache>,
    mut topology: VesselCollisionTopology,
    mut policy: FollowPolicyContext,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }
    let dt = time.delta_secs();
    collision_filters.refresh(&joints, &mut topology);

    for (avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode, mut zoom, projection) in
        q_avatar.iter_mut()
    {
        if q_dragging.get(arm.target).is_ok() {
            continue;
        }

        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
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

        apply_scroll_zoom(
            &mut arm.distance,
            &mut zoom.delta,
            CAMERA_ZOOM_SENSITIVITY,
            5.0,
            200.0,
        );

        let mut filter = collision_filters
            .filter_for(arm.target, &q_children)
            .clone();
        filter
            .excluded_entities
            .extend(policy.terrain.native_terrain_colliders());
        let radius = match policy.collision_settings.camera_radius(projection) {
            Ok(radius) => radius,
            Err(error) => {
                policy.report(Some(error));
                continue;
            }
        };
        let Some(native) = spatial_query.as_ref() else {
            policy.report(Some("camera follow has no native volume query".into()));
            continue;
        };
        let Some(frame) = policy.active_frame.as_deref().map(|frame| frame.0) else {
            policy.report(Some("camera follow has no active physics frame".into()));
            continue;
        };
        let Some(to_physics) = lunco_spatial::coords::grid_transform_between_grids(
            child_of.0, frame, &q_parents, &q_grids, &q_spatial,
        ) else {
            policy.report(Some("camera follow cannot resolve physics frame".into()));
            continue;
        };
        let Some(to_source) = lunco_spatial::coords::grid_transform_between_grids(
            frame, child_of.0, &q_parents, &q_grids, &q_spatial,
        ) else {
            policy.report(Some("camera follow cannot resolve source frame".into()));
            continue;
        };
        let shape = match crate::locomotion::camera_collider(radius) {
            Ok(shape) => shape,
            Err(error) => {
                policy.report(Some(error));
                continue;
            }
        };
        let probe = match Dir3::new(tf.rotation * Vec3::Z) {
            Ok(probe) => probe,
            Err(_) => {
                policy.report(Some(
                    "camera follow has an invalid obstruction direction".into(),
                ));
                continue;
            }
        };
        let obstruction = match native.cast_shape_in_grid(
            child_of.0,
            &shape,
            GridPos(target_pos),
            Quat::IDENTITY,
            probe,
            arm.distance,
            policy.collision_settings.skin_m,
            &filter,
        ) {
            Ok(hit) => hit.map(|hit| f64::from(hit.distance)),
            Err(error) => {
                policy.report(Some(error));
                continue;
            }
        };
        let damping = arm.damping.unwrap_or(defaults.damping);
        let base = FollowPlan {
            distance: arm.distance,
            yaw: f64::from(arm.yaw),
            pitch: f64::from(arm.pitch),
            height: f64::from(arm.vertical_offset),
            focus_x: 0.0,
            focus_z: 0.0,
            attitude: arm.attitude,
            track_heading: arm.track_heading,
            rotation_alpha: camera_decay_alpha(defaults.rotation_rate, damping, dt),
            position_alpha: camera_decay_alpha(defaults.position_rate, damping, dt),
        };
        let subject = policy.subjects.get(arm.target).ok();
        let vector = |v: DVec3| F::Array(vec![F::Float(v.x), F::Float(v.y), F::Float(v.z)]);
        let facts = F::map([
            ("settings", base.facts()),
            ("frame", F::str("subject_grid")),
            (
                "body_velocity",
                subject
                    .and_then(|s| Some(vector((s.3?.0.inverse() * s.4?.0).as_dvec3())))
                    .unwrap_or(F::Unit),
            ),
            (
                "body_angular_velocity",
                subject
                    .and_then(|s| Some(vector((s.3?.0.inverse() * s.5?.0).as_dvec3())))
                    .unwrap_or(F::Unit),
            ),
            (
                "subject_id",
                subject
                    .and_then(|s| s.2)
                    .map(|id| F::UInt(id.get()))
                    .unwrap_or(F::Unit),
            ),
            (
                "subject_name",
                subject
                    .and_then(|s| s.0)
                    .map(|n| F::str(n.as_str()))
                    .unwrap_or(F::Unit),
            ),
            (
                "subject_follow",
                subject
                    .and_then(|s| s.1)
                    .map(|f| {
                        F::str(match f {
                            lunco_camera_core::CameraFollow::Heading => "heading",
                            lunco_camera_core::CameraFollow::Orbit => "orbit",
                            lunco_camera_core::CameraFollow::Chase => "chase",
                        })
                    })
                    .unwrap_or(F::Unit),
            ),
            ("subject_forward", vector(target_rotation * DVec3::NEG_Z)),
            ("subject_up", vector(target_rotation * DVec3::Y)),
            (
                "obstruction_distance",
                obstruction.map(F::Float).unwrap_or(F::Unit),
            ),
            ("collision_available", F::Bool(spatial_query.is_some())),
            ("delta_seconds", F::Float(f64::from(dt))),
        ]);
        let context = lunco_core::RuntimeExecutionContext {
            target_twin_id: None,
            route: Some(lunco_core::RuntimeRoute::application(
                lunco_core::RuntimeCycle::Presentation,
            )),
            phase: lunco_core::RuntimePhase::Preparation,
            clock: lunco_core::RuntimeClock::Presentation,
            time_seconds: Some(time.elapsed_secs_f64()),
            delta_seconds: Some(f64::from(dt)),
            sequence: None,
            producer: None,
        };
        let plan = match resolve_follow_plan(facts, base, context) {
            Ok(plan) => plan,
            Err(error) => {
                policy.report(Some(error));
                continue;
            }
        };
        // Angles narrow only at the render quaternion boundary.
        let yaw = plan.yaw.rem_euclid(std::f64::consts::TAU) as f32;
        let pitch = plan.pitch as f32;
        let desired_rot = match plan.attitude {
            FollowAttitude::FullAttitude => {
                target_rotation.as_quat() * Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
            }
            FollowAttitude::WorldLocked => Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0),
            FollowAttitude::Heading => {
                let target_heading_d = if plan.track_heading {
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
                let final_yaw = (target_heading_d + plan.yaw) as f32;
                if let Some((east, north, up)) = surface_axes {
                    surface_camera_rotation(east, north, up, final_yaw, pitch)
                } else {
                    Quat::from_euler(EulerRot::YXZ, final_yaw, pitch, 0.0)
                }
            }
        };

        let next_rotation = tf.rotation.slerp(desired_rot, plan.rotation_alpha as f32);
        let offset = next_rotation.mul_vec3(Vec3::Z).as_dvec3() * plan.distance;
        let vertical_offset: DVec3 = if surface_mode.is_some() {
            let Some(up) = surface_axes.map(|(_, _, up)| up).or_else(|| {
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            }) else {
                policy.report(Some("camera follow has no surface-relative up axis".into()));
                continue;
            };
            up.as_dvec3() * plan.height
        } else {
            DVec3::Y * plan.height
        };
        let lateral_focus = surface_axes
            .map(|(east, north, _)| {
                east.as_dvec3() * plan.focus_x - north.as_dvec3() * plan.focus_z
            })
            .unwrap_or(DVec3::new(plan.focus_x, 0.0, plan.focus_z));
        let vertical_offset = vertical_offset + lateral_focus;
        let Some(current_pos) =
            grid_absolute_seeded(avatar_ent, Some(&cell), &tf, &q_parents, &q_grids)
                .map(|position| position.0)
        else {
            policy.report(Some(
                "camera follow cannot resolve its current grid pose".into(),
            ));
            continue;
        };
        let resolved = (|| -> Result<DVec3, crate::locomotion::CameraMotionError> {
            let solve = |origin: DVec3, delta: DVec3| {
                crate::locomotion::cast_camera_source_step(
                    to_physics.transform_position(origin),
                    to_physics.transform_vector(delta),
                    radius,
                    &policy.collision_settings,
                    &policy.terrain,
                    policy
                        .terrain_budget
                        .as_deref()
                        .map_or(0, |budget| budget.max_intervals),
                    native,
                    &filter,
                )
                .map(|motion| to_source.transform_position(motion.position))
            };
            let ray_origin = solve(target_pos, vertical_offset)?;
            let desired_pos = solve(ray_origin, offset)?;
            let boom = desired_pos - ray_origin;
            let target_len = boom.length();
            let current_len = current_pos.distance(ray_origin);
            let final_len = resolve_camera_arm_length(current_len, target_len, plan.position_alpha);
            let candidate = if target_len > 0.0 {
                ray_origin + boom * (final_len / target_len)
            } else {
                ray_origin
            };
            // The boom certifies visibility from the subject. A separate sweep
            // certifies the camera's actual damped/swinging travel this frame.
            crate::locomotion::resolve_camera_segment(
                avatar_ent,
                frame,
                to_physics.transform_position(current_pos),
                to_physics.transform_vector(candidate - current_pos),
                radius,
                &policy.collision_settings,
                &policy.terrain,
                policy
                    .terrain_budget
                    .as_deref()
                    .map_or(0, |budget| budget.max_intervals),
                &policy.drawn,
                &mut policy.targets,
                native,
                &filter,
            )
            .map(|point| to_source.transform_position(point))
        })();
        let final_pos = match resolved {
            Ok(position) => {
                policy.report(None);
                position
            }
            Err(crate::locomotion::CameraMotionError::Invalid(error)) => {
                policy.report(Some(error));
                continue;
            }
            Err(crate::locomotion::CameraMotionError::Preparation { reason, .. }) => {
                if let Some(diagnostics) = policy.diagnostics.as_deref_mut() {
                    diagnostics.replace_producer(
                        "camera-follow",
                        [lunco_core::RuntimeDiagnostic {
                            code: "camera-terrain-preparing".into(),
                            severity: lunco_core::DiagnosticSeverity::Info,
                            producer: "camera-follow".into(),
                            subject: "SpringArmCamera".into(),
                            message: reason,
                        }],
                    );
                }
                continue;
            }
        };

        // The boom's translation and orientation form one admitted rig pose.
        // Preparation must not advance the orbit while its movement is held.
        let (new_cell, new_tf) =
            local_pose_to_grid_storage(grid, final_pos, next_rotation.as_dquat());
        cell.set_if_neq(new_cell);
        if tf.translation != new_tf.translation || tf.rotation != new_tf.rotation {
            tf.translation = new_tf.translation;
            tf.rotation = new_tf.rotation;
        }
    }
}

/// The rig owns live tuning; selection and possession remain unchanged.
#[lunco_core::on_command(SetCameraFollow)]
pub(crate) fn on_set_camera_follow(
    trigger: On<SetCameraFollow>,
    local: Option<Res<lunco_embodiment_core::roles::TheLocalEmbodiment>>,
    mut rigs: Query<
        (&mut SpringArmCamera, Has<lunco_camera_core::CameraPoseLock>),
        (With<Embodiment>, With<LocalEmbodiment>),
    >,
) -> Result<lunco_core::CommandOutcome, String> {
    let request = trigger.event();
    let camera =
        lunco_embodiment_core::roles::resolve_requested_or_local(request.camera, local.as_deref())?;
    let (mut rig, locked) = rigs
        .get_mut(camera)
        .map_err(|_| "requested camera is not a live follow rig")?;
    if locked {
        return Err("camera pose is owned by another presentation mode".into());
    }
    // Validate every field before committing any of them. f32 is the explicit
    // render-rig boundary; no simulation or persisted parameter is narrowed.
    for value in [
        request.distance_m,
        request.height_m,
        request.yaw_rad,
        request.pitch_rad,
        request.damping,
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_finite() || value.abs() > f32::MAX as f64 {
            return Err(
                "follow settings must be finite and representable by the render rig".into(),
            );
        }
    }
    if request.distance_m.is_some_and(|n| n <= 0.0)
        || request
            .pitch_rad
            .is_some_and(|n| n.abs() >= std::f64::consts::FRAC_PI_2)
        || request.damping.is_some_and(|n| !(0.0..1.0).contains(&n))
    {
        return Err("invalid follow distance, pitch or damping range".into());
    }
    let attitude = match request.mode.as_deref() {
        None => None,
        Some("stabilized") => Some((FollowAttitude::Heading, false)),
        Some("heading") => Some((FollowAttitude::Heading, true)),
        Some("body") => Some((FollowAttitude::FullAttitude, false)),
        _ => return Err("follow mode must be stabilized, heading or body".into()),
    };
    if let Some(n) = request.distance_m {
        rig.distance = n;
    }
    if let Some(n) = request.height_m {
        rig.vertical_offset = n as f32;
    }
    if let Some(n) = request.yaw_rad {
        rig.yaw = n.rem_euclid(std::f64::consts::TAU) as f32;
    }
    if let Some(n) = request.pitch_rad {
        rig.pitch = n as f32;
    }
    if let Some(n) = request.damping {
        rig.damping = Some(n as f32);
    }
    if let Some((attitude, track)) = attitude {
        rig.attitude = attitude;
        rig.track_heading = track;
    }
    Ok(lunco_core::CommandOutcome::Succeeded(Default::default()))
}
