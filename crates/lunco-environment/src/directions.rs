//! Universal direction targets and their scalar cosim projection.
//!
//! A requested source identifier resolves either to one explicitly tagged
//! target entity or to one explicitly supplied framed ray (used for authored
//! directional lights). Every probe computes its own target bearing from its
//! position and axes. Point targets use BigSpace-relative positions; ray
//! sources use the same frame-rotation operation. A duplicate target, a ray
//! and point with the same id, or an unresolved frame is an error and publishes
//! no value.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;

use lunco_spatial::coords::UnitDirection3;

pub const SUN_DIRECTION_SOURCE: &str = "sun";
pub const EARTH_DIRECTION_SOURCE: &str = "earth";

pub use lunco_cosim_core::DirectionSourceId;

/// Composed-scene classification for whether celestial bodies own the Sun
/// direction source. The component is absent until the active scene's
/// composition has been inspected; a static authored-light ray must not be
/// published before that decision is available.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CelestialSourceClassification {
    /// Whether the composed scene declares at least one celestial body source.
    pub has_source: bool,
}

/// Explicit target identity attached to a non-celestial position-bearing entity.
#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct DirectionTargetId(DirectionSourceId);

impl DirectionTargetId {
    pub fn new(value: &str) -> Option<Self> {
        DirectionSourceId::parse(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A unit direction together with the frame in which its components live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramedDirection {
    pub frame: Entity,
    pub direction: UnitDirection3,
}

/// Explicit infinite-range direction rays. Finite targets are entities tagged
/// with [`DirectionTargetId`] and are resolved separately for each probe.
#[derive(Resource, Debug, Default)]
pub struct EnvironmentDirections {
    rays: BTreeMap<DirectionSourceId, FramedDirection>,
    revision: u64,
}

impl EnvironmentDirections {
    /// Set or withdraw one explicitly framed direction ray.
    pub fn set_named(&mut self, source: &str, sample: Option<FramedDirection>) -> bool {
        let Some(source) = DirectionSourceId::parse(source) else {
            return false;
        };
        let changed = match sample {
            Some(sample) => self.rays.insert(source, sample) != Some(sample),
            None => self.rays.remove(&source).is_some(),
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    pub fn get_named(&self, source: &str) -> Option<FramedDirection> {
        self.rays.get(&DirectionSourceId::parse(source)?).copied()
    }

    fn ray(&self, source: &DirectionSourceId) -> Option<FramedDirection> {
        self.rays.get(source).copied()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn clear(&mut self) {
        if !self.rays.is_empty() {
            self.rays.clear();
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

/// Composed-wire demand for framed direction sources on one environment
/// probe. The connector stem is the target's authored `DirectionTargetId`.
#[derive(Component, Debug, Default, Clone, PartialEq, Eq)]
pub struct DirectionSourceRequirements(pub BTreeSet<DirectionSourceId>);

/// Why a requested target direction could not be converted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectionResolutionError {
    Missing,
    Ambiguous,
    FrameUnavailable,
    Coincident,
}

/// Resolve and express a named direction source in `target_frame`.
///
/// Point targets are computed per consumer from BigSpace positions. Framed
/// rays are rotated from their declared frame. A source must resolve to exactly
/// one of those representations; duplicate identities and mixed point/ray
/// definitions fail visibly.
pub fn resolve_direction_for_frame<F: bevy::ecs::query::QueryFilter>(
    source: &DirectionSourceId,
    target_frame: Entity,
    rays: Option<&EnvironmentDirections>,
    q_targets: &Query<(Entity, &DirectionTargetId)>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&big_space::prelude::Grid>,
    q_spatial: &Query<(Option<&big_space::prelude::CellCoord>, &Transform), F>,
) -> Result<UnitDirection3, DirectionResolutionError> {
    let mut targets = q_targets
        .iter()
        .filter_map(|(entity, target)| (target.as_str() == source.as_str()).then_some(entity));
    let first_target = targets.next();
    if targets.next().is_some()
        || (first_target.is_some() && rays.and_then(|r| r.ray(source)).is_some())
    {
        return Err(DirectionResolutionError::Ambiguous);
    }

    if let Some(target) = first_target {
        let displacement = lunco_spatial::coords::relative_position_to_entity_in_entity_frame(
            target_frame,
            target,
            q_parents,
            q_grids,
            q_spatial,
        )
        .ok_or(DirectionResolutionError::FrameUnavailable)?;
        return UnitDirection3::normalized(displacement)
            .ok_or(DirectionResolutionError::Coincident);
    }

    let ray = rays
        .and_then(|rays| rays.ray(source))
        .ok_or(DirectionResolutionError::Missing)?;
    lunco_spatial::coords::unit_direction_between_frames(
        ray.direction,
        ray.frame,
        target_frame,
        q_parents,
        q_grids,
        q_spatial,
    )
    .ok_or(DirectionResolutionError::FrameUnavailable)
}

fn direction_diagnostic(
    code: &str,
    subject: String,
    message: String,
) -> lunco_core::RuntimeDiagnostic {
    lunco_core::RuntimeDiagnostic {
        code: code.to_string(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: "environment-directions".to_string(),
        subject,
        message,
    }
}

/// Publish every demanded direction directly to probe cosim outputs. Each
/// probe resolves finite target entities in its own BigSpace-relative frame;
/// no body-specific local projection is retained between provider and model.
pub fn publish_direction_sources_to_cosim(
    rays: Option<Res<EnvironmentDirections>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_targets: Query<(Entity, &DirectionTargetId)>,
    mut q_probes: Query<
        (
            Entity,
            Option<&DirectionSourceRequirements>,
            Option<&mut lunco_cosim_core::SimComponent>,
        ),
        With<crate::EnvironmentProbe>,
    >,
    q_connections: Query<&lunco_cosim_core::SimConnection>,
    q_consumers: Query<&lunco_cosim_core::SimComponent, Without<crate::EnvironmentProbe>>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let mut findings = Vec::new();
    let mut ambiguous_sources = BTreeSet::new();
    let mut invalid_frames = BTreeSet::new();
    let mut coincident_targets = BTreeSet::new();
    let mut missing_sources = BTreeSet::new();
    let mut missing_interfaces = 0;

    for (probe, requirements, sim) in &mut q_probes {
        let Some(mut sim) = sim else {
            if let Some(requirements) = requirements {
                missing_interfaces += requirements.0.len();
            }
            continue;
        };

        let requirements = requirements.map(|required| &required.0);
        let existing_sources: BTreeSet<_> = sim
            .outputs
            .keys()
            .filter_map(|connector| DirectionSourceId::from_mount_connector(connector))
            .collect();
        let mut source_ids = existing_sources;
        if let Some(requirements) = requirements {
            source_ids.extend(requirements.iter().cloned());
        }

        for source in source_ids {
            let demanded = requirements.is_some_and(|required| required.contains(&source));
            let converted = if !demanded {
                None
            } else {
                match resolve_direction_for_frame(
                    &source,
                    probe,
                    rays.as_deref(),
                    &q_targets,
                    &q_parents,
                    &q_grids,
                    &q_spatial,
                ) {
                    Ok(direction) => Some(direction),
                    Err(DirectionResolutionError::Ambiguous) => {
                        ambiguous_sources.insert(source.clone());
                        fault_running_direction_consumer(
                            probe,
                            &source,
                            "source resolves to multiple targets",
                            &q_connections,
                            &q_consumers,
                            &mut faults,
                        );
                        None
                    }
                    Err(DirectionResolutionError::FrameUnavailable) => {
                        invalid_frames.insert(source.clone());
                        fault_running_direction_consumer(
                            probe,
                            &source,
                            "BigSpace cannot resolve the source and probe frames",
                            &q_connections,
                            &q_consumers,
                            &mut faults,
                        );
                        None
                    }
                    Err(DirectionResolutionError::Coincident) => {
                        coincident_targets.insert(source.clone());
                        fault_running_direction_consumer(
                            probe,
                            &source,
                            "target position coincides with the probe origin",
                            &q_connections,
                            &q_consumers,
                            &mut faults,
                        );
                        None
                    }
                    Err(DirectionResolutionError::Missing) => {
                        missing_sources.insert(source.clone());
                        fault_running_direction_consumer(
                            probe,
                            &source,
                            "no target position or framed ray is registered",
                            &q_connections,
                            &q_consumers,
                            &mut faults,
                        );
                        None
                    }
                }
            };

            let connectors = [
                source.mount_connector('x').expect("validated axis"),
                source.mount_connector('y').expect("validated axis"),
                source.mount_connector('z').expect("validated axis"),
            ];
            if let Some(components) = converted.map(UnitDirection3::components) {
                for (connector, value) in
                    connectors
                        .into_iter()
                        .zip([components.x, components.y, components.z])
                {
                    sim.outputs.insert(connector, value);
                }
            } else {
                for connector in connectors {
                    sim.outputs.remove(&connector);
                }
            }
        }
    }

    for source in ambiguous_sources {
        findings.push(direction_diagnostic(
            "direction-source-ambiguous",
            source.as_str().to_string(),
            format!(
                "direction source `{}` resolves to multiple target entities or both a target entity and a framed ray",
                source.as_str()
            ),
        ));
    }
    for source in invalid_frames {
        findings.push(direction_diagnostic(
            "direction-frame-invalid",
            source.as_str().to_string(),
            format!(
                "BigSpace cannot resolve the requested direction between source `{}` and an environment probe",
                source.as_str()
            ),
        ));
    }
    for source in coincident_targets {
        findings.push(direction_diagnostic(
            "direction-target-coincident",
            source.as_str().to_string(),
            format!(
                "direction target `{}` is at the probe origin, so its bearing is undefined",
                source.as_str()
            ),
        ));
    }
    for source in missing_sources {
        findings.push(direction_diagnostic(
            "direction-source-missing",
            source.as_str().to_string(),
            format!(
                "one or more environment probes require direction source `{}` but no matching target or framed ray exists",
                source.as_str()
            ),
        ));
    }
    if missing_interfaces > 0 {
        findings.push(direction_diagnostic(
            "direction-probe-interface-missing",
            "EnvironmentProbe".to_string(),
            format!("{missing_interfaces} demanded direction source(s) have no SimComponent output interface"),
        ));
    }
    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("environment-directions", findings);
    }
}

fn fault_running_direction_consumer(
    probe: Entity,
    source: &DirectionSourceId,
    cause: &str,
    q_connections: &Query<&lunco_cosim_core::SimConnection>,
    q_consumers: &Query<&lunco_cosim_core::SimComponent, Without<crate::EnvironmentProbe>>,
    faults: &mut Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let has_live_consumer = q_connections.iter().any(|connection| {
        if connection.start_element != probe || connection.start_is_input {
            return false;
        }
        let Some(required) = DirectionSourceId::from_mount_connector(&connection.start_connector)
        else {
            return false;
        };
        if &required != source {
            return false;
        }
        q_consumers
            .get(connection.end_element)
            .is_ok_and(|consumer| {
                matches!(
                    consumer.status,
                    lunco_cosim_core::SimStatus::Running | lunco_cosim_core::SimStatus::Paused
                )
            })
    });
    if !has_live_consumer {
        return;
    }
    let detail = format!(
        "direction source `{}` is unavailable for a running co-simulation consumer: {cause}",
        source.as_str()
    );
    if let Some(faults) = faults.as_deref_mut() {
        faults.raise(
            "environment-direction-unavailable",
            Some(probe),
            "EnvironmentProbe direction",
            detail,
        );
    }
}
