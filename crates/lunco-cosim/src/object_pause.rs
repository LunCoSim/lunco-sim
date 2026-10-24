//! Atomic suspension of an authored coupled system.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::*;
use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::SimulationObjectPaused;
use lunco_core::{Command, GlobalEntityId, on_command, register_commands};
use lunco_cosim_core::{SimComponent, SimConnection, SimStatus};
use lunco_modelica_runtime::ModelicaModel;
use lunco_telemetry_core::{Severity, TelemetryEvent, TelemetryValue};

/// Suspend a complete coupled system at a completed communication boundary.
/// Membership follows active dynamic joints, physical ownership and all live
/// simulation connections. Shared port connections therefore share suspension.
/// Supervisors outside that graph remain live to explain or restart the scene.
#[Command]
pub struct PauseSimulationObject {
    /// Dynamic body from which the coupled-system closure is resolved.
    #[authz_target]
    pub target: Entity,
    /// Authored failure, damage or validity condition identity.
    pub condition: String,
    /// Measured values, engineering limits and explanation for Recent Events.
    pub detail: String,
}

fn connect(graph: &mut HashMap<Entity, Vec<Entity>>, a: Entity, b: Entity) {
    graph.entry(a).or_default().push(b);
    graph.entry(b).or_default().push(a);
}

#[on_command(PauseSimulationObject)]
fn on_pause_simulation_object(
    trigger: On<PauseSimulationObject>,
    mut commands: Commands,
    bodies: Query<(Entity, &RigidBody)>,
    entities: Query<Entity>,
    parents: Query<&ChildOf>,
    identities: Query<(
        Option<&GlobalEntityId>,
        Option<&Name>,
        Option<&SimulationObjectPaused>,
    )>,
    links: Query<(Entity, &lunco_physics::PhysicsJointLink), Without<JointDisabled>>,
    all_links: Query<(Entity, &lunco_physics::PhysicsJointLink)>,
    colliders: Query<(Entity, Option<&ColliderOf>), With<Collider>>,
    connections: Query<&SimConnection>,
    mut models: Query<(Entity, &mut ModelicaModel)>,
    mut components: Query<(Entity, &mut SimComponent)>,
) -> Result<Ack, String> {
    let request = trigger.event();
    let result = (|| {
        if request.condition.trim().is_empty() || request.detail.trim().is_empty() {
            return Err("system pause requires a condition identity and an explanation".to_owned());
        }
        if !bodies
            .get(request.target)
            .is_ok_and(|(_, body)| *body == RigidBody::Dynamic)
        {
            return Err("system pause requires a live dynamic body".to_owned());
        }
        let (identity, name, previous) = identities
            .get(request.target)
            .map_err(|_| "system pause target no longer exists".to_owned())?;
        let identity = identity.ok_or("system pause requires a stable object identity")?;
        if previous.is_some() {
            return Ok(Ack::new(OpId::new()));
        }

        let mut graph = HashMap::new();
        // A nested rigid body starts a new ownership boundary even when it
        // remains a USD child after a joint is detached.
        for entity in &entities {
            if bodies.contains(entity) || all_links.contains(entity) {
                continue;
            }
            let mut current = entity;
            while let Ok(parent) = parents.get(current) {
                current = parent.parent();
                if bodies.contains(current) {
                    connect(&mut graph, entity, current);
                    break;
                }
            }
        }
        for (joint, link) in &links {
            for body in [link.body0, link.body1] {
                if bodies
                    .get(body)
                    .is_ok_and(|(_, mode)| *mode == RigidBody::Dynamic)
                {
                    connect(&mut graph, joint, body);
                }
            }
        }
        for connection in &connections {
            connect(&mut graph, connection.start_element, connection.end_element);
        }
        let mut members = HashSet::new();
        let mut pending = vec![request.target];
        while let Some(entity) = pending.pop() {
            if members.insert(entity) {
                if let Some(neighbors) = graph.get(&entity) {
                    pending.extend(neighbors.iter().copied());
                }
            }
        }

        // Nothing changes until every participant is at a completed boundary.
        // In-flight workers retain their existing receipt/clock transaction;
        // dropping a receipt would strand its barrier or publish a partial stop.
        for (entity, model) in &models {
            if members.contains(&entity) && (model.is_stepping || model.is_compiling) {
                return Err(format!(
                    "system pause requires a completed Modelica boundary: {} is busy",
                    model.model_name
                ));
            }
        }
        for (entity, component) in &components {
            if members.contains(&entity) && component.is_stepping {
                return Err(format!(
                    "system pause requires a completed participant boundary: {} is busy",
                    component.model_name
                ));
            }
        }

        let mut dynamic_bodies: Vec<_> = members
            .iter()
            .copied()
            .filter(|entity| {
                bodies
                    .get(*entity)
                    .is_ok_and(|(_, mode)| *mode == RigidBody::Dynamic)
            })
            .collect();
        dynamic_bodies.sort_unstable_by_key(|entity| entity.to_bits());
        lunco_physics::pause_physics_members(
            &dynamic_bodies,
            &all_links,
            &colliders,
            &mut commands,
        );
        for (entity, mut model) in &mut models {
            if members.contains(&entity) {
                model.paused = true;
                model.resume_after_compile = false;
            }
        }
        for (entity, mut component) in &mut components {
            if members.contains(&entity) {
                component.status = SimStatus::Paused;
            }
        }
        let reason = SimulationObjectPaused {
            condition: request.condition.clone(),
            detail: request.detail.clone(),
        };
        let mut ordered: Vec<_> = members.into_iter().collect();
        ordered.sort_unstable_by_key(|entity| entity.to_bits());
        for entity in &ordered {
            commands.entity(*entity).try_insert(reason.clone());
        }
        let label = name
            .map(Name::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("object {}", identity.get()));
        let explanation = format!(
            "{label}: coupled simulation paused ({} dynamic bodies, {} members). {}: {}",
            dynamic_bodies.len(),
            ordered.len(),
            request.condition,
            request.detail,
        );
        commands.trigger(TelemetryEvent {
            name: "simulation-object-paused".to_owned(),
            source: identity.get(),
            severity: Severity::Warning,
            data: TelemetryValue::String(explanation.clone()),
            timestamp: 0.0,
            sim_secs: 0.0,
            sim_tick: 0,
        });
        warn!("[cosim] {explanation}");
        Ok(Ack::new(OpId::new()))
    })();
    if let Err(detail) = &result {
        warn!("[cosim] system pause rejected: {detail}");
    }
    result
}

register_commands!(on_pause_simulation_object);

pub(crate) fn install(app: &mut App) {
    app.register_type::<SimulationObjectPaused>();
    register_all_commands(app);
}
