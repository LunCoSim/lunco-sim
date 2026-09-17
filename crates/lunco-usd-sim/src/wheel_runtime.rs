//! Runtime wheel resynchronization over the authored vehicle parameters.

use avian3d::prelude::{Collider, Friction, RevoluteJoint};
use bevy::asset::AssetId;
use bevy::log::{error, info};
use bevy::math::DVec3;
use bevy::prelude::{Entity, World};
use lunco_mobility::{JointedWheelTire, Suspension, WheelRaycast};
use lunco_usd_bevy_core::{canonical::CanonicalStages, UsdRead, UsdStageAsset};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_sim_authoring::WheelParams;
use lunco_usd_sim_core::PhysicalWheel;
use openusd::sdf::Path as SdfPath;
use std::collections::HashMap;

/// Resolve a wheel's attachment suspension prim via the standard attachment
/// topology. The map belongs to one composed stage, so its keys are stage-local
/// paths; independent instances retain independent topology maps.
pub(crate) fn attachment_suspension_path(
    wheel_path: &str,
    wheel_attachment_targets: &HashMap<String, String>,
) -> Option<SdfPath> {
    wheel_attachment_targets
        .get(wheel_path)
        .and_then(|s| SdfPath::new(s).ok())
}

/// Resolve a wheel's attachment tire prim via the standard attachment
/// topology. The map belongs to one composed stage, so its keys are stage-local
/// paths; independent instances retain independent topology maps.
pub(crate) fn attachment_tire_path(
    wheel_path: &str,
    wheel_attachment_tires: &HashMap<String, String>,
) -> Option<SdfPath> {
    wheel_attachment_tires
        .get(wheel_path)
        .and_then(|s| SdfPath::new(s).ok())
}

/// Attribute families [`resync_wheels_for_stage`] claims from the generic
/// refresh path. Prim-scoped where a name is not wheel-specific:
/// `physxVehicleWheel:mass` is claimed only on a wheel prim — on a chassis it must keep
/// the normal refresh path (mass overrides are rebuilt by `lunco-usd-avian`).
pub(crate) fn claims_edit(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    prim: &SdfPath,
    attr: &str,
) -> bool {
    if attr.starts_with("physxVehicleWheel:") {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAPI");
    }
    if attr == "lunco:wheel:headingAxis" {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAPI");
    }
    if attr.starts_with("lunco:suspension:") || attr.starts_with("physxVehicleSuspension:") {
        return reader.has_api_schema(prim, "PhysxVehicleSuspensionAPI");
    }
    if attr.starts_with("lunco:tire:") || attr.starts_with("physxVehicleTire:") {
        return reader.has_api_schema(prim, "PhysxVehicleTireAPI");
    }
    if matches!(attr, "physics:dynamicFriction" | "physics:staticFriction") {
        return reader.has_api_schema(prim, "PhysxVehicleTireAPI");
    }
    if matches!(
        attr,
        "physxVehicleWheelAttachment:wheel"
            | "physxVehicleWheelAttachment:tire"
            | "physxVehicleWheelAttachment:suspension"
            | "physxVehicleWheelAttachment:index"
    ) {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAttachmentAPI");
    }
    false
}

/// One wheel's re-read result, staged so the `!Send` stage borrow is released
/// before the world is mutated.
struct WheelUpdate {
    entity: Entity,
    physical: bool,
    params: WheelParams,
    collider: Option<Collider>,
}

/// Re-derive every spawned wheel of `stage` from the live composed stage, in
/// place. A failed authored read raises a terminal runtime fault and leaves
/// physics held; stale wheel parameters are never retained.
pub(crate) fn resync_wheels_for_stage(world: &mut World, id: AssetId<UsdStageAsset>) {
    let mut rows: Vec<(Entity, String, bool)> = Vec::new();
    {
        let mut q = world.query::<(
            Entity,
            &UsdPrimPath,
            Option<&WheelRaycast>,
            Option<&PhysicalWheel>,
        )>();
        for (e, prim, rc, pw) in q.iter(world) {
            if prim.stage_handle.id() != id || (rc.is_none() && pw.is_none()) {
                continue;
            }
            rows.push((e, prim.path.clone(), pw.is_some()));
        }
    }
    if rows.is_empty() {
        return;
    }

    let mut updates: Vec<WheelUpdate> = Vec::new();
    let mut failures: Vec<(Option<Entity>, String, String)> = Vec::new();
    {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        let mut topology = crate::StageJointTopology::default();
        crate::collect_joint_scan_read(&view, &mut topology);
        for (entity, path, physical) in &rows {
            let Ok(sp) = SdfPath::new(path) else { continue };
            if topology.invalid_wheel_attachments.contains(path) {
                failures.push((
                    Some(*entity),
                    path.clone(),
                    "malformed or ambiguous wheel attachment topology".to_owned(),
                ));
                continue;
            }
            let susp = topology
                .wheel_attachment_targets
                .get(path)
                .and_then(|s| SdfPath::new(s).ok());
            let tire = topology
                .wheel_attachment_tires
                .get(path)
                .and_then(|s| SdfPath::new(s).ok());
            match WheelParams::read(&view, &sp, susp.as_ref(), tire.as_ref()) {
                Ok(params) => {
                    let collider = if *physical {
                        if !view
                            .has_api_schema(&sp, openusd::schemas::physics::tokens::API_RIGID_BODY)
                        {
                            failures.push((
                                Some(*entity),
                                path.clone(),
                                "missing authored PhysicsRigidBodyAPI".to_owned(),
                            ));
                            continue;
                        }
                        match lunco_usd_avian_reader::collider::authored_collider_from_usd(
                            &view, &sp,
                        ) {
                            Ok(collider) => {
                                Some(crate::oriented_wheel_collider(collider, params.axle_axis))
                            }
                            Err(error) => {
                                failures.push((
                                    Some(*entity),
                                    path.clone(),
                                    format!("invalid authored collision geometry: {error}"),
                                ));
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    updates.push(WheelUpdate {
                        entity: *entity,
                        physical: *physical,
                        params,
                        collider,
                    });
                }
                Err(missing) => failures.push((
                    Some(*entity),
                    path.clone(),
                    format!("missing or invalid required attributes: {missing:?}"),
                )),
            }
        }
    }

    for (entity, subject, detail) in failures {
        let first = world
            .get_resource_or_insert_with(lunco_core::RuntimeFaults::default)
            .raise(
                "usd-wheel-resync-invalid",
                entity,
                subject.clone(),
                detail.clone(),
            );
        world
            .get_resource_or_insert_with(lunco_physics::PhysicsHolds::default)
            .set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
        if first {
            error!("[wheel resync] terminal authored-state failure on {subject}: {detail}");
        }
    }

    let wheel_count = updates.len();
    for u in &updates {
        if !u.physical {
            if let Some(mut wheel) = world.get_mut::<WheelRaycast>(u.entity) {
                u.params.apply_to_raycast(&mut wheel);
            }
            if let Some(mut susp) = world.get_mut::<Suspension>(u.entity) {
                u.params.apply_to_suspension(&mut susp);
            }
            if let (Some(susp), Some(mut ray)) = (
                u.params.suspension,
                world.get_mut::<avian3d::prelude::RayCaster>(u.entity),
            ) {
                ray.origin = DVec3::new(
                    0.0,
                    lunco_mobility::strut_offset(susp.rest_length, u.params.radius),
                    0.0,
                );
                ray.max_distance = lunco_mobility::suspension_ray_max_distance(susp.rest_length);
            }
            continue;
        }

        let (old_radius, old_width, axis_rot) = match world.get::<PhysicalWheel>(u.entity) {
            Some(pw) => (pw.wheel_radius, pw.wheel_width, pw.axis_rot),
            None => continue,
        };
        if let Some(mut pw) = world.get_mut::<PhysicalWheel>(u.entity) {
            pw.wheel_radius = u.params.radius as f32;
            pw.wheel_width = u.params.width as f32;
        }
        if let Some(mut mass) = world.get_mut::<avian3d::prelude::Mass>(u.entity) {
            mass.0 = u.params.mass as f32;
        }
        if let Some(mut friction) = world.get_mut::<Friction>(u.entity) {
            friction.dynamic_coefficient = u.params.friction_mu;
            friction.static_coefficient = u.params.friction_mu;
        }
        if let Some(mut tire) = world.get_mut::<JointedWheelTire>(u.entity) {
            tire.radius = u.params.radius;
            tire.axle_inertia = u.params.axle_inertia();
            tire.slip_stiffness = u.params.slip_stiffness;
            tire.lateral_stiffness_graph = u.params.lateral_stiffness_graph;
            tire.min_validated_speed = u.params.min_validated_speed;
            tire.friction_mu = u.params.friction_mu;
            tire.bearing_damping = u.params.bearing_damping;
        }
        world.entity_mut(u.entity).insert((
            crate::physical_wheel_angular_inertia(&u.params, axis_rot),
            avian3d::prelude::NoAutoAngularInertia,
        ));
        if (old_radius as f64 - u.params.radius).abs() > 1e-6
            || (old_width as f64 - u.params.width).abs() > 1e-6
        {
            let Some(collider) = u.collider.as_ref() else {
                continue;
            };
            world.entity_mut(u.entity).insert(collider.clone());
        }
        let mut joint_entity: Option<Entity> = None;
        {
            let mut q = world.query::<(Entity, &RevoluteJoint)>();
            for (je, joint) in q.iter(world) {
                if joint.body2 == u.entity {
                    joint_entity = Some(je);
                    break;
                }
            }
        }
        let Some(je) = joint_entity else { continue };
        if let Some(mut actuator) = world.get_mut::<lunco_cosim::JointTorqueActuator>(je) {
            actuator.brake_torque = u.params.brake_torque_max;
            actuator.rotational_inertia = u.params.axle_inertia();
        }
    }
    info!(
        "[wheel resync] stage {:?}: re-derived {} wheel(s) in place",
        id, wheel_count,
    );
}
