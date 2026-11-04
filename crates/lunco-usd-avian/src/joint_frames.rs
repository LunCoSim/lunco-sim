//! Notice-driven replacement of native constraints whose composed frames changed.

use super::*;
use lunco_usd_bevy_scene::UsdSceneChangeBatch;
use lunco_usd_bevy_stage::canonical::CanonicalStages;

/// Canonical endpoint facts that produced the current native definition.
/// Comparing f64 facts avoids re-admitting a joint for an unchanged rigid move.
#[derive(Component, Clone, Debug, PartialEq)]
pub(super) struct UsdJointFrames {
    body0: String,
    body1: String,
    position0: DVec3,
    position1: DVec3,
    rotation0: bevy::math::DQuat,
    rotation1: bevy::math::DQuat,
}

impl From<&PendingUsdJoint> for UsdJointFrames {
    fn from(joint: &PendingUsdJoint) -> Self {
        Self {
            body0: joint.body0_path.clone(),
            body1: joint.body1_path.clone(),
            position0: joint.local_pos0,
            position1: joint.local_pos1,
            rotation0: joint.local_rot0,
            rotation1: joint.local_rot1,
        }
    }
}

fn ancestor(changed: &str, dependent: &str) -> bool {
    changed == "/"
        || changed == dependent
        || dependent
            .strip_prefix(changed)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub(super) fn refresh_joint_frames(
    mut changes: MessageReader<UsdSceneChangeBatch>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    joints: Query<(Entity, &UsdPrimPath, &UsdJointFrames)>,
    mut commands: Commands,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
) {
    let mut batches = std::collections::HashMap::<AssetId<UsdStageAsset>, Vec<String>>::new();
    for change in changes.read() {
        batches.entry(change.stage_id).or_default().extend(
            change
                .info_prim_paths
                .iter()
                .chain(&change.resynced_prim_paths)
                .cloned(),
        );
    }
    let mut ordered = joints.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    for (entity, prim, previous) in ordered {
        let Some(changed) = batches.get(&prim.stage_handle.id()) else {
            continue;
        };
        let Some(asset) = stages.get(prim.stage_handle.id()) else {
            continue;
        };
        let (reader, generation) = canonical.reader_for(prim.stage_handle.id(), asset);
        let Ok(path) = SdfPath::new(&prim.path) else {
            continue;
        };
        let targets = reader
            .rel_targets(&path, "physics:body0")
            .into_iter()
            .chain(reader.rel_targets(&path, "physics:body1"))
            .map(|path| path.to_string());
        let dependencies = std::iter::once(prim.path.as_str())
            .chain([previous.body0.as_str(), previous.body1.as_str()])
            .map(str::to_owned)
            .chain(targets)
            .collect::<Vec<_>>();
        if !changed
            .iter()
            .any(|changed| dependencies.iter().any(|path| ancestor(changed, path)))
        {
            continue;
        }
        // A removed/disabled joint belongs to structural/schema projection.
        if !reader.has_prim(&path)
            || !lunco_usd_avian_reader::joint::is_runtime_joint_enabled(&reader, &path)
        {
            continue;
        }
        let Some(pending) = read_joint_spec(&reader, &path) else {
            let endpoints = reader
                .rel_targets(&path, "physics:body0")
                .into_iter()
                .chain(reader.rel_targets(&path, "physics:body1"))
                .map(|target| {
                    match lunco_usd_bevy_stage::world_transform_matrix_d_at(&reader, &target, 0.0) {
                        Ok(matrix) => format!(
                            "{target}: world-transform determinant={}",
                            matrix.determinant()
                        ),
                        Err(error) => format!("{target}: {error}"),
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            let detail = format!(
                "{}: changed joint endpoint requires finite nonsingular composed frames and valid authored anchors; {endpoints}",
                prim.path
            );
            report_physics_runtime_fault(
                Some(&mut faults),
                Some(&mut holds),
                entity,
                prim.path.clone(),
                "usd-physics-joint-invalid",
                detail.clone(),
            );
            commands.trigger(lunco_core::RuntimeError {
                name: "usd-physics-joint-invalid".into(),
                message: detail,
            });
            continue;
        };
        let next = UsdJointFrames::from(&pending);
        if *previous == next {
            continue;
        }
        let previous = (*previous).clone();
        let prim = (*prim).clone();
        commands.queue(move |world: &mut World| {
            if world.get::<UsdJointFrames>(entity) != Some(&previous)
                || world.get::<UsdPrimPath>(entity).is_none_or(|current| {
                    current.stage_handle != prim.stage_handle || current.path != prim.path
                })
                || world
                    .get_non_send::<CanonicalStages>()
                    .is_none_or(|stages| {
                        stages.generation_for(prim.stage_handle.id()) != generation
                    })
            {
                return;
            }
            let was_disabled = world.get::<JointDisabled>(entity).is_some();
            if let Err(error) = lunco_usd_avian_joints::retire_native_joints(world, &[entity]) {
                world.resource_mut::<lunco_core::RuntimeFaults>().raise(
                    "usd-joint-frame-refresh-invalid",
                    Some(entity),
                    &prim.path,
                    &error,
                );
                world
                    .resource_mut::<lunco_physics::PhysicsHolds>()
                    .set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                world.trigger(lunco_core::RuntimeError {
                    name: "usd-joint-frame-refresh-invalid".into(),
                    message: error,
                });
                return;
            }
            let anchors = world
                .get::<lunco_physics::PhysicsJointLink>(entity)
                .map(|link| [link.body0, link.body1]);
            world
                .entity_mut(entity)
                .remove::<lunco_physics::PhysicsJointLink>()
                .remove::<JointDamping>()
                .insert((
                    pending,
                    next,
                    lunco_physics::PhysicsJointPending,
                    lunco_physics::PhysicsJointTopologyPending,
                ));
            if was_disabled {
                world.entity_mut(entity).insert(JointDisabled);
            }
            if let Some(anchors) = anchors {
                for anchor in anchors {
                    if world.get::<ScenePhysicsOwned>(anchor).is_some()
                        && world.get::<UsdPrimPath>(anchor).is_none()
                    {
                        world.despawn(anchor);
                    }
                }
            }
            world
                .resource_mut::<lunco_physics::PhysicsHolds>()
                .set(lunco_physics::PhysicsHolds::BODY_ADMISSION, true);
        });
    }
}
