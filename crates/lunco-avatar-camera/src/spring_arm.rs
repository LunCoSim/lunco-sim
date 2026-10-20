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

    for (avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode, mut zoom) in
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

        let filter = collision_filters.filter_for(arm.target, &q_children);
        let probe = tf.rotation.mul_vec3(Vec3::Z).as_dvec3();
        let obstruction = spatial_query
            .as_ref()
            .and_then(|query| {
                query.cast_ray_in_grid(
                    child_of.0,
                    GridPos(target_pos),
                    Dir3::new(probe.as_vec3()).ok()?,
                    arm.distance,
                    true,
                    filter,
                )
            })
            .map(|hit| hit.distance);
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
                    .and_then(|s| Some(vector(s.3?.0.inverse() * s.4?.0)))
                    .unwrap_or(F::Unit),
            ),
            (
                "body_angular_velocity",
                subject
                    .and_then(|s| Some(vector(s.3?.0.inverse() * s.5?.0)))
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
        policy.report(None);
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
        if tf.rotation != next_rotation {
            tf.rotation = next_rotation;
        }

        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * plan.distance;
        let vertical_offset: DVec3 = if surface_mode.is_some() {
            let Some(up) = surface_axes.map(|(_, _, up)| up).or_else(|| {
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            }) else {
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
        // The focus offset itself may cross a ceiling. Resolve it from the
        // subject center before casting the camera boom, with the same filter.
        let focus_length = vertical_offset.length();
        let focus_offset = spatial_query
            .as_ref()
            .and_then(|query| {
                query.cast_ray_in_grid(
                    child_of.0,
                    GridPos(target_pos),
                    Dir3::new(vertical_offset.as_vec3()).ok()?,
                    focus_length,
                    true,
                    filter,
                )
            })
            .map(|hit| vertical_offset * ((hit.distance - 0.1).max(0.0) / focus_length))
            .unwrap_or(vertical_offset);
        let ray_origin = target_pos + focus_offset;
        let desired_pos = ray_origin + offset;
        let ray_dir = (desired_pos - ray_origin).normalize_or(DVec3::Y);
        let ray_len = desired_pos.distance(ray_origin);
        let castable = ray_origin.is_finite() && ray_len.is_finite();
        let hit = match &spatial_query {
            Some(spatial_query) if castable => spatial_query.cast_ray_in_grid(
                child_of.0,
                GridPos(ray_origin),
                Dir3::new(ray_dir.as_vec3()).unwrap_or(Dir3::Y),
                ray_len,
                true,
                filter,
            ),
            _ => None,
        };

        let desired_len = ray_len;
        let target_len = match hit {
            Some(hit_data) => ((hit_data.distance - 0.5).min(desired_len)).max(0.0),
            None => desired_len,
        };
        let Some(current_pos) =
            grid_absolute_seeded(avatar_ent, Some(&cell), &tf, &q_parents, &q_grids)
                .map(|position| position.0)
        else {
            continue;
        };
        let current_len = current_pos.distance(ray_origin);
        // Never ease through geometry. Recovery alone is interpolated.
        let final_len = resolve_camera_arm_length(current_len, target_len, plan.position_alpha);
        let final_pos = ray_origin + ray_dir * final_len;

        let (new_cell, new_tf) =
            local_pose_to_grid_storage(grid, final_pos, tf.rotation.as_dquat());
        cell.set_if_neq(new_cell);
        if tf.translation != new_tf.translation {
            tf.translation = new_tf.translation;
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
