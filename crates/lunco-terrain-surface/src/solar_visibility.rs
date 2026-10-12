//! Physical solar samples at explicitly wired environment-probe sites.
//!
//! This consumes the authoritative f64 surface and the shared framed Sun
//! resolver. It is independent of camera LOD, collider residency, and GPU work.
//! Exact local pose/direction and surface keys fence reuse between fixed ticks.

use crate::oracle::DemHeightField;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_cosim_core::{
    SUN_IRRADIANCE_SOURCE_CONNECTOR, SUN_VISIBILITY_SOURCE_CONNECTOR, SimComponent, SimConnection,
};
use lunco_environment::{
    DirectionSourceId, DirectionTargetId, EnvironmentDirections, EnvironmentProbe,
    ProbeOccluderPaths, ProbeSampling, resolve_direction_for_frame,
};

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
        (Vec<SampleKey>, Vec<DVec3>, DVec3, Vec<Entity>, u64, f64),
    >,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SolarGeometry<'w, 's> {
    colliders: Query<
        'w,
        's,
        (
            Entity,
            &'static avian3d::prelude::Collider,
            &'static avian3d::prelude::Position,
            &'static avian3d::prelude::Rotation,
            &'static lunco_usd_bevy_scene::UsdPrimPath,
        ),
        (
            Without<avian3d::prelude::Sensor>,
            Without<DemHeightField>,
            Without<crate::ColliderTileOf>,
        ),
    >,
    frame: Option<Res<'w, lunco_spatial::ActivePhysicsFrame>>,
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
) -> bool {
    sun.irradiance_w_m2.is_some() && frame.is_some() && preparing.is_empty()
}

/// Publish only demanded visibility ports, before cosimulation propagates wires.
pub(crate) fn publish_sun_visibility(
    rays: Option<Res<EnvironmentDirections>>,
    sun: Res<lunco_environment::SunState>,
    budget: Res<crate::SurfaceOcclusionBudget>,
    parents: Query<&ChildOf>,
    grids: Query<&Grid>,
    spatial: Query<(Option<&CellCoord>, &Transform)>,
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
    geometry: SolarGeometry,
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
    let selected_paths: std::collections::HashSet<_> = probes
        .iter()
        .flat_map(|(_, _, paths, _)| paths.0.iter().cloned())
        .collect();
    let shapes = geometry
        .colliders
        .iter()
        .filter(|(_, _, _, _, path)| selected_paths.contains(&path.path))
        .map(|(entity, collider, position, rotation, _)| {
            lunco_physics::occlusion::CollisionOccluder {
                entity,
                position: position.0,
                rotation: rotation.0,
                shape: collider.shape_scaled().clone(),
            }
        })
        .collect();
    let geometry_sample = memo.geometry_index.update(shapes);
    let mut probe_order = Vec::new();
    let mut paths = std::collections::BTreeSet::new();
    for (entity, _, _, _) in &probes {
        let identity = identities.get(entity).map(|path| path.path.clone());
        let Ok(path) = identity else {
            faults.raise(
                "solar-probe-identity",
                Some(entity),
                "EnvironmentProbe admission",
                "solar probe has no composed USD identity",
            );
            return;
        };
        if !paths.insert(path.clone()) {
            faults.raise(
                "solar-probe-identity",
                Some(entity),
                "EnvironmentProbe admission",
                format!("duplicate solar probe identity {path}"),
            );
            return;
        }
        probe_order.push((path, entity));
    }
    probe_order.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, entity) in probe_order {
        let Ok((probe, sampling, occluder_paths, mut sim)) = probes.get_mut(entity) else {
            continue;
        };

        let demanded = connections.iter().any(|wire| {
            wire.start_element == probe
                && !wire.start_is_input
                && matches!(
                    wire.start_connector.as_str(),
                    SUN_VISIBILITY_SOURCE_CONNECTOR | SUN_IRRADIANCE_SOURCE_CONNECTOR
                )
        });
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
            (|| -> Result<(Vec<SampleKey>, Vec<DVec3>, DVec3, Vec<Entity>, f64), String> {
                if !preparing_terrain.is_empty() {
                    return Err("authored terrain has no admitted surface".into());
                }
                let irradiance = sun
                    .irradiance_w_m2
                    .ok_or("physical solar irradiance unavailable")?;
                if !irradiance.is_finite() || irradiance < 0.0 {
                    return Err("invalid physical solar irradiance".into());
                }
                let direction = resolve_direction_for_frame(
                    &source,
                    probe,
                    rays.as_deref(),
                    &targets,
                    &parents,
                    &grids,
                    &spatial,
                )
                .map_err(|reason| format!("Sun direction unavailable: {reason:?}"))?;
                geometry_sample.clone()?;
                let mut occluders = Vec::with_capacity(occluder_paths.0.len());
                for path in &occluder_paths.0 {
                    let entity = geometry
                        .colliders
                        .iter()
                        .find(|(_, _, _, _, prim)| &prim.path == path)
                        .map(|(entity, _, _, _, _)| entity)
                        .ok_or_else(|| {
                            format!(
                                "optical proxy '{path}' has no admitted collision shape and pose"
                            )
                        })?;
                    occluders.push(entity);
                }
                occluders.sort_unstable();
                if sampling.0.is_empty()
                    || sampling.0.len() > 64
                    || sampling.0.iter().any(|p| !p.is_finite())
                {
                    return Err("solar probe requires 1..64 finite sampling points".into());
                }
                let frame = geometry
                    .frame
                    .as_ref()
                    .ok_or("active physical frame unavailable")?
                    .0;
                let (physical_origin, physical_rotation) =
                    lunco_spatial::coords::pose_in_grid(probe, frame, &parents, &grids, &spatial)
                        .ok_or("probe physical pose unavailable")?;
                let physical_direction = lunco_spatial::coords::unit_direction_between_frames(
                    direction, probe, frame, &parents, &grids, &spatial,
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
                    let (origin, rotation) = lunco_spatial::coords::pose_in_grid(
                        probe, terrain, &parents, &grids, &spatial,
                    )
                    .ok_or_else(|| format!("probe-to-terrain frame unavailable for {terrain:?}"))?;
                    let direction = lunco_spatial::coords::unit_direction_between_frames(
                        direction, probe, terrain, &parents, &grids, &spatial,
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
                                memo.terrain_bounds.entry(key.terrain).or_default(),
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

/// Scene-owned samples and geometry must retire before another Twin loads.
pub(crate) fn clear_solar_visibility(mut memo: ResMut<VisibilityMemo>) {
    *memo = VisibilityMemo::default();
}
