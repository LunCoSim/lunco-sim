//! Physical solar samples at explicitly wired environment-probe sites.
//!
//! This consumes the authoritative f64 surface and the shared framed Sun
//! resolver. It is independent of camera LOD, collider residency, and GPU work.
//! Exact local pose/direction and surface keys fence reuse between fixed ticks.

use crate::oracle::DemHeightField;
use avian3d::prelude::{Collider, Position, RigidBodyColliders, Rotation, Sensor};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_cosim_core::{
    SUN_IRRADIANCE_SOURCE_CONNECTOR, SUN_VISIBILITY_SOURCE_CONNECTOR, SimComponent, SimConnection,
};
use lunco_environment::{
    DirectionResolutionError, DirectionSourceId, DirectionTargetId, EnvironmentDirections,
    EnvironmentProbe, ProbeOccluderPaths, ProbeSampling, resolve_direction_for_frame,
};
use lunco_physics::SourceColliders;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, PartialEq)]
struct SampleKey {
    terrain: Entity,
    surface: u64,
    origin: DVec3,
    direction: DVec3,
    point: usize,
}

#[derive(Resource, Default)]
pub(crate) struct VisibilityMemo {
    geometry_index: lunco_physics::occlusion::CollisionOcclusionIndex,
    terrain_bounds: std::collections::HashMap<Entity, crate::SurfaceOcclusionCache>,
    samples: std::collections::HashMap<
        Entity,
        (Vec<SampleKey>, Vec<DVec3>, DVec3, Arc<[Entity]>, u64, f64),
    >,
}

/// Membership is shared by initialization sampling and the fixed simulation clock.
#[derive(Resource, Default)]
pub(crate) struct SolarMembership {
    valid: bool,
    demand: HashSet<Entity>,
    probe_order: Vec<Entity>,
    participants: HashMap<Entity, Result<Arc<[Entity]>, String>>,
    colliders: Vec<Entity>,
    candidates: HashSet<Entity>,
}

impl lunco_core::SceneResource for VisibilityMemo {}
impl lunco_core::SceneResource for SolarMembership {}

pub(crate) fn register_solar_membership(app: &mut App) {
    fn watch<T: Component>(app: &mut App) {
        app.add_observer(|_: On<Add<T>>, mut m: ResMut<SolarMembership>| m.valid = false);
        app.add_observer(|_: On<Remove<T>>, mut m: ResMut<SolarMembership>| m.valid = false);
    }
    fn watch_candidate<T: Component>(app: &mut App) {
        app.add_observer(|event: On<Add<T>>, mut m: ResMut<SolarMembership>| {
            if m.candidates.contains(&event.entity) {
                m.valid = false;
            }
        });
        app.add_observer(|event: On<Remove<T>>, mut m: ResMut<SolarMembership>| {
            if m.candidates.contains(&event.entity) {
                m.valid = false;
            }
        });
    }
    use lunco_core::AppSceneExt;
    app.init_scene_resource::<SolarMembership>();
    app.init_scene_resource::<VisibilityMemo>();
    watch::<EnvironmentProbe>(app);
    watch::<ProbeSampling>(app);
    watch::<SimComponent>(app);
    watch::<ProbeOccluderPaths>(app);
    watch::<SimConnection>(app);
    watch_candidate::<RigidBodyColliders>(app);
    watch_candidate::<SourceColliders>(app);
    watch_candidate::<Collider>(app);
    watch_candidate::<Sensor>(app);
    watch_candidate::<Position>(app);
    watch_candidate::<Rotation>(app);
    watch_candidate::<DemHeightField>(app);
    watch_candidate::<crate::ColliderTileOf>(app);
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SolarGeometry<'w, 's> {
    membership: ResMut<'w, SolarMembership>,
    changed: Query<'w, 's, (), Or<(Changed<SimConnection>, Changed<ProbeOccluderPaths>)>>,
    changed_sources:
        Query<'w, 's, Entity, Or<(Changed<RigidBodyColliders>, Changed<SourceColliders>)>>,
    prims: Res<'w, lunco_usd_bevy_scene::prim_index::UsdPrimIndex>,
    sources: Query<
        'w,
        's,
        (
            Option<&'static avian3d::prelude::RigidBodyColliders>,
            Option<&'static lunco_physics::SourceColliders>,
        ),
    >,
    colliders: Query<
        'w,
        's,
        (
            Entity,
            &'static avian3d::prelude::Collider,
            &'static avian3d::prelude::Position,
            &'static avian3d::prelude::Rotation,
        ),
        (
            Without<avian3d::prelude::Sensor>,
            Without<DemHeightField>,
            Without<crate::ColliderTileOf>,
        ),
    >,
}

/// Initialization samples wait for the physical providers' preparation.
/// Once admitted, malformed sampling or topology fails at the normal owner.
pub(crate) fn initialization_sample_ready(
    sun: Res<lunco_environment::SunState>,
    frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    preparing: Query<
        (),
        (
            With<crate::terrain::DemTerrainRequest>,
            Without<DemHeightField>,
        ),
    >,
    rays: Option<Res<EnvironmentDirections>>,
    poses: lunco_physics::SimulationPoseQuery,
    targets: Query<(Entity, &DirectionTargetId)>,
    probes: Query<Entity, With<EnvironmentProbe>>,
    connections: Query<&SimConnection>,
) -> bool {
    if sun.irradiance_w_m2.is_none() || frame.is_none() || !preparing.is_empty() {
        return false;
    }
    // Irradiance can be published before the celestial provider has resolved
    // the active physical frame. Initialization must not latch a runtime fault
    // for that preparation state. Other resolution errors remain terminal.
    let source = DirectionSourceId::parse(lunco_environment::SUN_DIRECTION_SOURCE)
        .expect("canonical Sun source id");
    probes.iter().all(|probe| {
        !solar_sample_demanded(probe, &connections)
            || resolve_direction_for_frame(&source, probe, rays.as_deref(), &targets, &poses)
                != Err(DirectionResolutionError::FrameUnavailable)
    })
}

fn solar_sample_demanded(probe: Entity, connections: &Query<&SimConnection>) -> bool {
    connections
        .iter()
        .filter_map(solar_wire_probe)
        .any(|source| source == probe)
}

fn solar_wire_probe(wire: &SimConnection) -> Option<Entity> {
    (!wire.start_is_input
        && matches!(
            wire.start_connector.as_str(),
            SUN_VISIBILITY_SOURCE_CONNECTOR | SUN_IRRADIANCE_SOURCE_CONNECTOR
        ))
    .then_some(wire.start_element)
}

/// Publish only demanded visibility ports, before cosimulation propagates wires.
pub(crate) fn publish_sun_visibility(
    rays: Option<Res<EnvironmentDirections>>,
    sun: Res<lunco_environment::SunState>,
    budget: Res<crate::SurfaceOcclusionBudget>,
    poses: lunco_physics::SimulationPoseQuery,
    targets: Query<(Entity, &DirectionTargetId)>,
    identities: Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    terrains: Query<(Entity, &DemHeightField)>,
    preparing_terrain: Query<
        Entity,
        (
            With<crate::terrain::DemTerrainRequest>,
            Without<DemHeightField>,
        ),
    >,
    connections: Query<&SimConnection>,
    mut probes: Query<
        (
            Entity,
            &ProbeSampling,
            &ProbeOccluderPaths,
            &mut SimComponent,
        ),
        With<EnvironmentProbe>,
    >,
    mut geometry: SolarGeometry,
    mut memo: ResMut<VisibilityMemo>,
    mut diagnostics: ResMut<lunco_core::RuntimeDiagnostics>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    let source = DirectionSourceId::parse(lunco_environment::SUN_DIRECTION_SOURCE)
        .expect("canonical Sun source id");
    let mut findings = Vec::new();
    let mut remaining_intervals = budget.max_tick_intervals;
    memo.samples.retain(|probe, _| probes.contains(*probe));
    memo.terrain_bounds
        .retain(|terrain, _| terrains.contains(*terrain));
    for cache in memo.terrain_bounds.values_mut() {
        cache.begin_batch();
    }
    if !geometry.membership.valid
        || geometry.prims.is_changed()
        || !geometry.changed.is_empty()
        || geometry
            .changed_sources
            .iter()
            .any(|source| geometry.membership.candidates.contains(&source))
    {
        geometry.membership.valid = false;
        let mut ordered = std::collections::BTreeMap::new();
        let demanded_probes: HashSet<_> = connections.iter().filter_map(solar_wire_probe).collect();
        let mut participants = std::collections::HashMap::new();
        let mut shapes = HashSet::new();
        let mut candidates = HashSet::new();
        for (probe, _, paths, _) in &probes {
            let Ok(identity) = identities.get(probe) else {
                faults.raise(
                    "solar-probe-identity",
                    Some(probe),
                    "EnvironmentProbe admission",
                    "solar probe has no composed USD identity",
                );
                return;
            };
            if ordered.insert(identity.path.clone(), probe).is_some() {
                faults.raise(
                    "solar-probe-identity",
                    Some(probe),
                    "EnvironmentProbe admission",
                    format!("duplicate solar probe identity {}", identity.path),
                );
                return;
            }
            if !demanded_probes.contains(&probe) {
                continue;
            }
            let members = (|| {
                let mut selected = Vec::new();
                for path in &paths.0 {
                    let mut matched_source = false;
                    for &source in geometry.prims.in_stage(identity.stage_handle.id(), path) {
                        candidates.insert(source);
                        let Ok((body, cooked)) = geometry.sources.get(source) else {
                            continue;
                        };
                        let count = selected.len();
                        for native in lunco_physics::collider_candidates(source, body, cooked) {
                            candidates.insert(native);
                            let Ok((entity, ..)) = geometry.colliders.get(native) else {
                                continue;
                            };
                            shapes.insert(entity);
                            selected.push(entity);
                        }
                        if selected.len() != count {
                            if matched_source {
                                return Err(format!(
                                    "optical proxy '{path}' has multiple admitted sources"
                                ));
                            }
                            matched_source = true;
                        }
                    }
                    if !matched_source {
                        return Err(format!(
                            "optical proxy '{path}' has no admitted collision shape and pose"
                        ));
                    }
                }
                selected.sort_unstable();
                selected.dedup();
                Ok(Arc::from(selected))
            })();
            participants.insert(probe, members);
        }
        let mut colliders: Vec<_> = shapes.into_iter().collect();
        colliders.sort_unstable();
        *geometry.membership = SolarMembership {
            valid: true,
            demand: demanded_probes,
            participants,
            colliders,
            candidates,
            probe_order: ordered.into_values().collect(),
        };
    }
    let shapes = geometry
        .membership
        .colliders
        .iter()
        .map(|&id| {
            let (entity, collider, position, rotation) = geometry
                .colliders
                .get(id)
                .map_err(|_| format!("optical collider {id:?} lost its admitted shape or pose"))?;
            Ok(lunco_physics::occlusion::CollisionOccluder {
                entity,
                position: position.0,
                rotation: rotation.0.as_dquat(),
                shape: collider.shape_scaled().clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>();
    let geometry_sample = shapes.and_then(|shapes| memo.geometry_index.update(shapes));
    for &entity in &geometry.membership.probe_order {
        let Ok((probe, sampling, _, mut sim)) = probes.get_mut(entity) else {
            continue;
        };

        let demanded = geometry.membership.demand.contains(&probe);
        if !demanded {
            memo.samples.remove(&probe);
            if sim
                .bypass_change_detection()
                .outputs
                .remove(SUN_VISIBILITY_SOURCE_CONNECTOR)
                .is_some()
            {
                sim.set_changed();
            }
            if sim
                .bypass_change_detection()
                .outputs
                .remove(SUN_IRRADIANCE_SOURCE_CONNECTOR)
                .is_some()
            {
                sim.set_changed();
            }
            continue;
        }
        let sample =
            (|| -> Result<(Vec<SampleKey>, Vec<DVec3>, DVec3, Arc<[Entity]>, f64), String> {
                if !preparing_terrain.is_empty() {
                    return Err("authored terrain has no admitted surface".into());
                }
                let irradiance = sun
                    .irradiance_w_m2
                    .ok_or("physical solar irradiance unavailable")?;
                if !irradiance.is_finite() || irradiance < 0.0 {
                    return Err("invalid physical solar irradiance".into());
                }
                let direction =
                    resolve_direction_for_frame(&source, probe, rays.as_deref(), &targets, &poses)
                        .map_err(|reason| format!("Sun direction unavailable: {reason:?}"))?;
                geometry_sample.clone()?;
                let occluders = geometry
                    .membership
                    .participants
                    .get(&probe)
                    .cloned()
                    .ok_or_else(|| "demanded probe has no admitted USD identity".to_string())??;
                if sampling.0.is_empty()
                    || sampling.0.len() > 64
                    || sampling.0.iter().any(|p| !p.is_finite())
                {
                    return Err("solar probe requires 1..64 finite sampling points".into());
                }
                let (physical_origin, physical_rotation) =
                    poses.pose(probe).ok_or("probe physical pose unavailable")?;
                let physical_origin = physical_origin.0;
                let physical_rotation = physical_rotation.0;
                let physical_direction = lunco_spatial::coords::UnitDirection3::normalized(
                    physical_rotation * direction.components(),
                )
                .ok_or("Sun physical frame unavailable")?
                .components();
                let origins: Vec<_> = sampling
                    .0
                    .iter()
                    .map(|p| physical_origin + physical_rotation * *p)
                    .collect();
                let mut keys = Vec::with_capacity(terrains.iter().len() * sampling.0.len());
                for (terrain, field) in &terrains {
                    let (terrain_origin, terrain_rotation) =
                        poses.pose(terrain).ok_or_else(|| {
                            format!("terrain physical pose unavailable for {terrain:?}")
                        })?;
                    let inverse = terrain_rotation.0.inverse();
                    let origin = inverse * (physical_origin - terrain_origin.0);
                    let rotation = (inverse * physical_rotation).normalize();
                    let direction = lunco_spatial::coords::UnitDirection3::normalized(
                        inverse * physical_direction,
                    )
                    .ok_or_else(|| format!("Sun-to-terrain frame unavailable for {terrain:?}"))?
                    .components();
                    for (point, offset) in sampling.0.iter().enumerate() {
                        keys.push(SampleKey {
                            terrain,
                            surface: field.0.surface_key(),
                            origin: origin + rotation * *offset,
                            direction,
                            point,
                        });
                    }
                }
                keys.sort_by(|a, b| {
                    a.point
                        .cmp(&b.point)
                        .then(a.surface.cmp(&b.surface))
                        .then(a.origin.x.total_cmp(&b.origin.x))
                        .then(a.origin.y.total_cmp(&b.origin.y))
                        .then(a.origin.z.total_cmp(&b.origin.z))
                        .then(a.direction.x.total_cmp(&b.direction.x))
                        .then(a.direction.y.total_cmp(&b.direction.y))
                        .then(a.direction.z.total_cmp(&b.direction.z))
                });
                if let Some((
                    previous,
                    previous_origins,
                    previous_direction,
                    previous_occluders,
                    revision,
                    visibility,
                )) = memo.samples.get(&probe)
                {
                    if *previous == keys
                        && *previous_origins == origins
                        && *previous_direction == physical_direction
                        && *previous_occluders == occluders
                        && *revision == memo.geometry_index.revision()
                    {
                        return Ok((keys, origins, physical_direction, occluders, *visibility));
                    }
                }
                let mut lit = 0;
                for (point, origin) in origins.iter().enumerate() {
                    if memo
                        .geometry_index
                        .occludes(*origin, physical_direction, &occluders)?
                    {
                        continue;
                    }
                    let mut blocked = false;
                    for key in keys.iter().filter(|key| key.point == point) {
                        let (_, field) = terrains.get(key.terrain).map_err(|e| e.to_string())?;
                        if field
                            .0
                            .occludes_ray(
                                key.origin,
                                key.direction,
                                *budget,
                                &mut remaining_intervals,
                                memo.terrain_bounds.entry(key.terrain).or_insert_with(|| {
                                    let mut cache = crate::SurfaceOcclusionCache::default();
                                    cache.begin_batch();
                                    cache
                                }),
                            )
                            .map_err(|reason| {
                                format!(
                                    "sample {point} on {:?}, origin {:?}, direction {:?}: {reason}",
                                    key.terrain, key.origin, key.direction
                                )
                            })?
                        {
                            blocked = true;
                            break;
                        }
                    }
                    if !blocked {
                        lit += 1;
                    }
                }
                let visibility = lit as f64 / origins.len() as f64;
                Ok((keys, origins, physical_direction, occluders, visibility))
            })();
        match sample {
            Ok((keys, origins, physical_direction, occluders, visibility)) => {
                let revision = memo.geometry_index.revision();
                memo.samples.insert(
                    probe,
                    (
                        keys,
                        origins,
                        physical_direction,
                        occluders,
                        revision,
                        visibility,
                    ),
                );
                if sim.outputs.get(SUN_VISIBILITY_SOURCE_CONNECTOR).copied() != Some(visibility) {
                    sim.bypass_change_detection()
                        .outputs
                        .insert(SUN_VISIBILITY_SOURCE_CONNECTOR.to_string(), visibility);
                    sim.set_changed();
                }
                let irradiance = sun.irradiance_w_m2.expect("validated physical sample");
                if sim.outputs.get(SUN_IRRADIANCE_SOURCE_CONNECTOR).copied() != Some(irradiance) {
                    sim.bypass_change_detection()
                        .outputs
                        .insert(SUN_IRRADIANCE_SOURCE_CONNECTOR.into(), irradiance);
                    sim.set_changed();
                }
            }
            Err(message) => {
                memo.samples.remove(&probe);
                if sim
                    .bypass_change_detection()
                    .outputs
                    .remove(SUN_VISIBILITY_SOURCE_CONNECTOR)
                    .is_some()
                {
                    sim.set_changed();
                }
                if sim
                    .bypass_change_detection()
                    .outputs
                    .remove(SUN_IRRADIANCE_SOURCE_CONNECTOR)
                    .is_some()
                {
                    sim.set_changed();
                }
                findings.push(lunco_core::RuntimeDiagnostic {
                    code: "solar-sample-unavailable".into(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "terrain-sun-visibility".into(),
                    subject: format!("{probe:?}"),
                    message: message.clone(),
                });
                faults.raise(
                    "solar-sample-unavailable",
                    Some(probe),
                    "EnvironmentProbe Sun visibility",
                    message,
                );
                break;
            }
        }
    }
    diagnostics.replace_producer("terrain-sun-visibility", findings);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lunco_core_runtime::SimTick;
    use lunco_physics::{PhysicsPoseSeeded, PrescribedSimulationPose};
    use lunco_spatial::coords::{GridPos, GridRot};

    #[test]
    fn initialization_waits_for_physical_frames_without_hiding_invalid_sources() {
        let mut world = World::new();
        let frame = world
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                Transform::default(),
            ))
            .id();
        world.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        world.insert_resource(SimTick(0));
        let mut sun = lunco_environment::SunState::default();
        sun.publish_physical(Some(1361.0)).unwrap();
        world.insert_resource(sun);
        let source = world
            .spawn((
                DirectionTargetId::new("sun").unwrap(),
                PrescribedSimulationPose::Pending,
                Transform::default(),
                ChildOf(frame),
            ))
            .id();
        let probe = world
            .spawn((EnvironmentProbe, Transform::default(), ChildOf(frame)))
            .id();
        world.spawn(SimConnection {
            start_element: probe,
            start_connector: SUN_VISIBILITY_SOURCE_CONNECTOR.into(),
            start_is_input: false,
            end_element: probe,
            end_connector: "visibility".into(),
            scale: 1.0,
            offset: 0.0,
        });
        assert!(
            !world.run_system_once(initialization_sample_ready).unwrap(),
            "pending celestial pose must keep initialization pending"
        );
        world
            .entity_mut(source)
            .insert(PrescribedSimulationPose::Sample {
                frame,
                tick: SimTick(0),
                position: GridPos(DVec3::X * 100.0),
                rotation: GridRot(bevy::math::DQuat::IDENTITY),
            });
        assert!(world.run_system_once(initialization_sample_ready).unwrap());
        world
            .entity_mut(probe)
            .insert(avian3d::prelude::RigidBody::Dynamic);
        assert!(
            !world.run_system_once(initialization_sample_ready).unwrap(),
            "unseeded native observer must keep initialization pending"
        );
        world.entity_mut(probe).insert(PhysicsPoseSeeded);
        assert!(world.run_system_once(initialization_sample_ready).unwrap());
        world.entity_mut(source).remove::<DirectionTargetId>();
        assert!(
            world.run_system_once(initialization_sample_ready).unwrap(),
            "missing authored source must reach the publisher's diagnostic, not wait forever"
        );
    }
}
