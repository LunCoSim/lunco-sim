//! Refresh composed physical mount metadata at the live USD lifecycle boundary.

use super::*;
use lunco_cosim_core::{ForceActuator, TorqueActuator};
use lunco_mobility::{RaycastMassContribution, WheelBodyMount};
use lunco_usd_bevy_stage::UsdRead;

type MountedPhysics = Or<(
    With<ForceActuator>,
    With<TorqueActuator>,
    With<WheelBodyMount>,
    With<PhysicalWheel>,
    With<JointedWheelTire>,
    With<RaycastMassContribution>,
)>;

fn changed_ancestor(changed: &str, source: &str) -> bool {
    changed == "/"
        || changed == source
        || source
            .strip_prefix(changed)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn relative_to_body(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
    body: Entity,
    stage: bevy::asset::AssetId<UsdStageAsset>,
    prims: &Query<&UsdPrimPath>,
    parents: &Query<&ChildOf>,
    previews: &Query<(), With<UsdPreviewOnly>>,
) -> Result<Transform, String> {
    let body_prim = prims
        .get(body)
        .map_err(|_| "physical mount owner is no longer projected")?;
    if body_prim.stage_handle.id() != stage || is_preview_only(body, parents, previews) {
        return Err("physical mount owner belongs to another stage or a preview".into());
    }
    let body_path = SdfPath::new(&body_prim.path).map_err(|error| error.to_string())?;
    if !reader.has_api_schema(&body_path, "PhysicsRigidBodyAPI") {
        return Err(format!(
            "{} is no longer a PhysicsRigidBodyAPI owner",
            body_prim.path
        ));
    }
    lunco_usd_bevy_stage::transform_in_body_frame(reader, &body_path, path).ok_or_else(|| {
        format!(
            "cannot derive finite body-local transform relative to {}",
            body_prim.path
        )
    })
}

pub(super) fn report_invalid_mount(
    commands: &mut Commands,
    faults: &mut lunco_core::RuntimeFaults,
    entity: Entity,
    path: &str,
    reason: impl std::fmt::Display,
) {
    let message = format!("{path}: invalid physical mount: {reason}");
    faults.raise(
        "usd-physical-mount-invalid",
        Some(entity),
        "USD physical mount",
        message.clone(),
    );
    commands.trigger(lunco_core::RuntimeError {
        name: "usd-physical-mount-invalid".into(),
        message,
    });
    commands.queue(|world: &mut World| {
        if let Some(mut holds) = world.get_resource_mut::<lunco_physics::PhysicsHolds>() {
            holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
        }
    });
}

/// Notice-driven: no scene traversal or composed transform reads on idle frames.
/// Native local transforms are compared before writing, so moving a complete
/// assembly does not dirty its unchanged wheel or mass ownership metadata.
pub(super) fn refresh_mounted_physics(
    mut changes: MessageReader<UsdSceneChangeBatch>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    prims: Query<&UsdPrimPath>,
    parents: Query<&ChildOf>,
    previews: Query<(), With<UsdPreviewOnly>>,
    mut mounted: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&mut ForceActuator>,
            Option<&mut TorqueActuator>,
            Option<&mut WheelBodyMount>,
            Option<&mut PhysicalWheel>,
            Option<&mut JointedWheelTire>,
            Option<&mut RaycastMassContribution>,
        ),
        MountedPhysics,
    >,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut commands: Commands,
) {
    let mut batches: Vec<(bevy::asset::AssetId<UsdStageAsset>, u64, Vec<String>)> = Vec::new();
    for change in changes.read() {
        let index = match batches
            .iter()
            .position(|(stage, _, _)| *stage == change.stage_id)
        {
            Some(index) => index,
            None => {
                batches.push((change.stage_id, change.stage_generation, Vec::new()));
                batches.len() - 1
            }
        };
        let (_, generation, paths) = &mut batches[index];
        *generation = (*generation).max(change.stage_generation);
        paths.extend(
            change
                .info_prim_paths
                .iter()
                .chain(&change.resynced_prim_paths)
                .cloned(),
        );
    }
    for (stage_id, generation, mut changed) in batches {
        changed.sort();
        changed.dedup();
        let mut endpoints = mounted
            .iter()
            .filter(|(entity, path, ..)| {
                path.stage_handle.id() == stage_id && !is_preview_only(*entity, &parents, &previews)
            })
            .map(|(entity, path, ..)| (path.path.clone(), entity))
            .collect::<Vec<_>>();
        endpoints.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, entity) in endpoints {
            let Ok((_, prim, force, torque, wheel, physical, tire, mass)) = mounted.get_mut(entity)
            else {
                continue;
            };
            let affected = changed.iter().any(|changed| {
                changed_ancestor(changed, &prim.path)
                    || wheel
                        .as_ref()
                        .and_then(|mount| prims.get(mount.body).ok())
                        .is_some_and(|owner| changed_ancestor(changed, &owner.path))
                    || mass
                        .as_ref()
                        .and_then(|mount| prims.get(mount.owner).ok())
                        .is_some_and(|owner| changed_ancestor(changed, &owner.path))
            });
            if !affected {
                continue;
            }
            // Prepare every dependent description before changing this endpoint.
            let update = (|| -> Result<_, String> {
                let asset = stages
                    .get(stage_id)
                    .ok_or("physical mount stage asset is absent")?;
                let (reader, current) = canonical.reader_for(stage_id, asset);
                if current < generation {
                    return Err(
                        "physical mount notice has no current composed stage generation".into(),
                    );
                }
                let path = SdfPath::new(&prim.path).map_err(|error| error.to_string())?;
                if !reader.has_prim(&path) {
                    return Err("mounted USD prim is absent".into());
                }
                if (physical.is_some() || tire.is_some()) && wheel.is_none() {
                    return Err("physical wheel has lost its authored body mount".into());
                }
                let force = force.as_ref().map(|_| {
                    lunco_usd_actuation::force_actuator_from_usd(&reader, &path)
                        .ok_or("force actuator requires a rigid-body ancestor, finite local transform/direction and positive maximum force".to_owned())
                }).transpose()?;
                let torque = torque.as_ref().map(|_| {
                    lunco_usd_actuation::torque_actuator_from_usd(&reader, &path)
                        .ok_or("torque actuator requires a rigid-body ancestor, finite local transform/axis and positive maximum torque".to_owned())
                }).transpose()?;
                let wheel_local = wheel
                    .as_ref()
                    .map(|mount| {
                        relative_to_body(
                            &reader, &path, mount.body, stage_id, &prims, &parents, &previews,
                        )
                    })
                    .transpose()?;
                let vehicle_local = physical
                    .as_ref()
                    .map(|_| {
                        vehicle_mount_transform(&reader, &path)
                            .map(|(_, local)| local)
                            .ok_or("physical wheel has no valid enclosing vehicle mount".to_owned())
                    })
                    .transpose()?;
                let mass_local = mass
                    .as_ref()
                    .map(|mount| {
                        relative_to_body(
                            &reader,
                            &path,
                            mount.owner,
                            stage_id,
                            &prims,
                            &parents,
                            &previews,
                        )
                    })
                    .transpose()?;
                Ok((force, torque, wheel_local, vehicle_local, mass_local))
            })();
            let (new_force, new_torque, wheel_local, vehicle_local, mass_local) = match update {
                Ok(update) => update,
                Err(reason) => {
                    report_invalid_mount(&mut commands, &mut faults, entity, &prim.path, reason);
                    continue;
                }
            };
            if let (Some(mut old), Some(new)) = (force, new_force) {
                if old.local_position != new.local_position
                    || old.direction_local != new.direction_local
                    || old.max_force_n != new.max_force_n
                {
                    *old = new;
                }
            }
            if let (Some(mut old), Some(new)) = (torque, new_torque) {
                if old.axis_local != new.axis_local || old.max_torque_nm != new.max_torque_nm {
                    *old = new;
                }
            }
            if let (Some(mut old), Some(local)) = (wheel, wheel_local) {
                if old.local != local {
                    old.local = local;
                }
            }
            if let (Some(mut old), Some(local)) = (physical, vehicle_local) {
                if old.mount_local != local.translation {
                    old.mount_local = local.translation;
                }
            }
            if let (Some(mut old), Some(local)) = (tire, wheel_local) {
                let heading = VehicleFrame::forward(GridRot(local.rotation.as_dquat()));
                if old.heading_local != heading {
                    old.heading_local = heading;
                }
            }
            if let (Some(mut old), Some(local)) = (mass, mass_local) {
                if old.local != local {
                    old.local = local;
                }
            }
        }
    }
}
