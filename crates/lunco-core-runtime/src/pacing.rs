//! Frame-pacing intent, shared across crates.
//!
//! Winit's `unfocused_mode` is a single global knob that several subsystems have
//! an opinion about, and the last writer each frame wins. The application pacer
//! re-pegs it from the explicit execution mode and active simulation state, so
//! a background realtime Twin gets a bounded cadence while recording/tests can
//! deliberately request Continuous updates.
//!
//! [`FramePacingDemand`] is how a subsystem states its cadence intent instead of
//! fighting over the knob. Overlapping requests are counted independently; callers
//! release only the cadence token they acquired.
//!
//! It lives in `lunco-core-runtime`, shared by animation/capture requesters and
//! the application pacer, without introducing a dependency between them.

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Stable, owner-neutral key for data that a scenario needs before activation.
/// The producer namespace and identity are authored by the domain owner; the
/// runtime only tracks the state published for that exact key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimulationDependencyKey {
    pub owner: String,
    pub identity: String,
}

impl SimulationDependencyKey {
    /// Construct one dependency key, rejecting names that cannot identify an
    /// owner or an owner-scoped input.
    pub fn new(owner: impl Into<String>, identity: impl Into<String>) -> Result<Self, String> {
        let owner = owner.into();
        let identity = identity.into();
        if owner.trim().is_empty() || owner.trim() != owner {
            return Err(
                "simulation dependency owner must be non-empty and have no surrounding whitespace"
                    .to_owned(),
            );
        }
        if identity.trim().is_empty() || identity.trim() != identity {
            return Err(
                "simulation dependency identity must be non-empty and have no surrounding whitespace"
                    .to_owned(),
            );
        }
        Ok(Self { owner, identity })
    }
}

/// Readiness of one owner-published input required by a scenario plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimulationDependencyStatus {
    /// The current owner operation is preparing the requested input.
    Pending { operation_id: u64 },
    /// The owner committed the immutable source revision for this input.
    Ready { source_revision: u64 },
    /// The current owner operation reached a terminal failure.
    Failed {
        operation_id: u64,
        errors: Vec<String>,
    },
}

/// Current readiness facts published by domain owners for scenario admission.
/// This is not a work queue: producers keep their typed jobs and fence results
/// before publishing a terminal state, while scenarios hold their existing
/// preparation boundary until every declared key is ready.
#[derive(Resource, Debug, Default)]
pub struct SimulationDependencyStates {
    owners: BTreeSet<String>,
    states: BTreeMap<SimulationDependencyKey, SimulationDependencyStatus>,
    revision: u64,
}

impl SimulationDependencyStates {
    /// Register a producer namespace. Repeated registration is idempotent.
    pub fn register_owner(&mut self, owner: impl Into<String>) -> Result<bool, String> {
        let owner = owner.into();
        if owner.trim().is_empty() {
            return Err("simulation dependency owner must not be empty".to_owned());
        }
        let inserted = self.owners.insert(owner);
        if inserted {
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(inserted)
    }

    /// Whether an owner namespace has a producer in this application.
    pub fn owner_is_registered(&self, owner: &str) -> bool {
        self.owners.contains(owner)
    }

    /// Publish a state transition. The revision changes only when the visible
    /// state changes, so waiting owners can resume from owner commits without
    /// polling on every simulation tick.
    pub fn publish(
        &mut self,
        key: SimulationDependencyKey,
        status: SimulationDependencyStatus,
    ) -> Result<bool, String> {
        if !self.owners.contains(&key.owner) {
            return Err(format!(
                "simulation dependency owner `{}` is not registered",
                key.owner
            ));
        }
        if self.states.get(&key) == Some(&status) {
            return Ok(false);
        }
        self.states.insert(key, status);
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }

    /// Retire one dependency after its producer's identity leaves scope.
    pub fn retire(&mut self, key: &SimulationDependencyKey) -> bool {
        if self.states.remove(key).is_none() {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Read the currently committed state for an exact owner key.
    pub fn status(&self, key: &SimulationDependencyKey) -> Option<&SimulationDependencyStatus> {
        self.states.get(key)
    }

    /// Revision incremented by every visible state publication or retirement.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// How the host drives the simulation application.
///
/// This is execution policy, not another simulation clock. `Realtime` lets the
/// host derive virtual time from the wall clock; `MaxSpeed` tells a headless
/// host to feed the fixed lattice explicitly and run the Bevy schedule without
/// a wall-clock wait. The fixed timestep, transport rate, and co-simulation
/// barrier remain the same in both modes.
#[derive(
    Resource,
    Reflect,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
)]
#[reflect(Resource)]
pub enum SimulationExecutionMode {
    /// Use the host's normal wall-clock pacing.
    #[default]
    Realtime,
    /// Advance one fixed simulation duration per host update, as fast as the
    /// CPU and causal participants permit.
    MaxSpeed,
}

/// Outstanding cadence requests from systems that animate or record frames.
///
/// Realtime requests select the host's bounded fixed-Hz cadence while unfocused.
/// Continuous requests are reserved for explicit max-speed work such as offline
/// frame recording. Focused windows remain paced by the focused update mode.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct FramePacingDemand {
    realtime: u32,
    continuous: u32,
}

impl FramePacingDemand {
    /// Request the bounded realtime cadence until this request is released.
    pub fn acquire_realtime(&mut self) {
        self.realtime = self.realtime.saturating_add(1);
    }

    /// Release one bounded realtime cadence request.
    pub fn release_realtime(&mut self) {
        self.realtime = self.realtime.saturating_sub(1);
    }

    /// Request continuous updates until this request is released.
    pub fn acquire_continuous(&mut self) {
        self.continuous = self.continuous.saturating_add(1);
    }

    /// Release one continuous update request.
    pub fn release_continuous(&mut self) {
        self.continuous = self.continuous.saturating_sub(1);
    }

    /// Whether at least one subsystem needs the bounded realtime cadence.
    pub fn realtime_wanted(&self) -> bool {
        self.realtime > 0
    }

    /// Whether at least one subsystem explicitly needs continuous updates.
    pub fn continuous_wanted(&self) -> bool {
        self.continuous > 0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FramePacingDemand, SimulationBarrierParticipants, SimulationDependencyKey,
        SimulationDependencyStates, SimulationDependencyStatus,
    };
    use bevy::prelude::Entity;

    #[test]
    fn cadence_requests_are_independent_and_saturating() {
        let mut demand = FramePacingDemand::default();
        demand.acquire_realtime();
        demand.acquire_realtime();
        demand.acquire_continuous();

        assert!(demand.realtime_wanted());
        assert!(demand.continuous_wanted());

        demand.release_realtime();
        assert!(demand.realtime_wanted());
        demand.release_realtime();
        demand.release_realtime();
        assert!(!demand.realtime_wanted());
        assert!(demand.continuous_wanted());
    }

    #[test]
    fn scenario_dependencies_join_the_barrier_and_replace_per_owner() {
        let scenario_a = Entity::from_raw_u32(1).unwrap();
        let scenario_b = Entity::from_raw_u32(2).unwrap();
        let modelica_a = Entity::from_raw_u32(11).unwrap();
        let modelica_b = Entity::from_raw_u32(12).unwrap();
        let independent = Entity::from_raw_u32(13).unwrap();
        let mut participants = SimulationBarrierParticipants::default();
        participants.replace([modelica_a]);

        participants.replace_scenario_plan(
            scenario_a,
            [modelica_a],
            [independent],
            [independent],
            ["EntitiesInRadius".to_owned()],
        );
        participants.replace_scenario_plan(
            scenario_b,
            [modelica_b],
            [],
            [],
            std::iter::empty::<String>(),
        );
        assert!(participants.requires_barrier(modelica_a));
        assert!(participants.requires_barrier(modelica_b));
        assert!(!participants.requires_barrier(independent));
        assert!(participants.scenario_declares_read(scenario_a, independent));
        assert!(participants.scenario_declares_write(scenario_a, independent));
        assert!(participants.scenario_declares_query_read(scenario_a, "EntitiesInRadius"));
        assert!(!participants.scenario_declares_query_read(scenario_a, "Raycast"));

        participants.remove_scenario_dependencies(scenario_a);
        assert!(participants.requires_barrier(modelica_a));
        assert!(participants.requires_barrier(modelica_b));
        assert!(!participants.scenario_declares_read(scenario_a, independent));
        assert!(!participants.scenario_declares_query_read(scenario_a, "EntitiesInRadius"));

        participants.remove_scenario_dependencies(scenario_b);
        assert!(!participants.requires_barrier(modelica_b));
    }

    #[test]
    fn scenario_runtime_discovered_modelica_access_joins_its_barrier() {
        let scenario = Entity::from_raw_u32(1).unwrap();
        let modelica = Entity::from_raw_u32(11).unwrap();
        let spawned_modelica = Entity::from_raw_u32(12).unwrap();
        let mut participants = SimulationBarrierParticipants::default();
        participants.replace(std::iter::empty());
        participants.replace_modelica_entities([modelica, spawned_modelica]);
        participants.replace_scenario_plan(
            scenario,
            [modelica],
            [],
            [],
            std::iter::empty::<String>(),
        );

        assert!(participants.requires_barrier(modelica));
        assert!(!participants.requires_barrier(spawned_modelica));
        assert_eq!(
            participants.add_scenario_read(scenario, spawned_modelica),
            Some(true)
        );
        assert_eq!(
            participants.add_scenario_write(scenario, spawned_modelica),
            Some(true)
        );
        assert!(participants.scenario_declares_read(scenario, spawned_modelica));
        assert!(participants.scenario_declares_write(scenario, spawned_modelica));
        assert!(participants.requires_barrier(spawned_modelica));

        participants.remove_scenario_dependencies(scenario);
        assert!(!participants.requires_barrier(modelica));
        assert!(!participants.requires_barrier(spawned_modelica));
    }

    #[test]
    fn unresolved_scenario_plan_holds_all_participants_until_commit() {
        let scenario = Entity::from_raw_u32(1).unwrap();
        let modelica_a = Entity::from_raw_u32(11).unwrap();
        let modelica_b = Entity::from_raw_u32(12).unwrap();
        let mut participants = SimulationBarrierParticipants::default();
        participants.replace(std::iter::empty());
        participants.replace_modelica_entities([modelica_a, modelica_b]);

        participants.mark_scenario_plan_pending(scenario);
        assert!(participants.requires_barrier(modelica_a));
        assert!(participants.requires_barrier(modelica_b));

        participants.replace_scenario_plan(
            scenario,
            [modelica_b],
            [],
            [],
            std::iter::empty::<String>(),
        );
        assert!(!participants.requires_barrier(modelica_a));
        assert!(participants.requires_barrier(modelica_b));
    }

    #[test]
    fn scenario_dependency_revisions_follow_owner_state_changes() {
        let key = SimulationDependencyKey::new("sysml.twin-analysis", "twin://school")
            .expect("stable owner key");
        let other = SimulationDependencyKey::new("sysml.twin-analysis", "twin://other")
            .expect("distinct Twin key");
        let mut states = SimulationDependencyStates::default();

        assert!(states.register_owner("sysml.twin-analysis").unwrap());
        assert!(
            states
                .publish(
                    key.clone(),
                    SimulationDependencyStatus::Pending { operation_id: 7 },
                )
                .unwrap()
        );
        let pending_revision = states.revision();
        assert!(
            !states
                .publish(
                    key.clone(),
                    SimulationDependencyStatus::Pending { operation_id: 7 },
                )
                .unwrap()
        );
        assert_eq!(states.revision(), pending_revision);
        assert!(
            states
                .publish(
                    other.clone(),
                    SimulationDependencyStatus::Ready { source_revision: 9 },
                )
                .unwrap()
        );
        assert_eq!(
            states.status(&key),
            Some(&SimulationDependencyStatus::Pending { operation_id: 7 })
        );
        assert!(
            states
                .publish(
                    key.clone(),
                    SimulationDependencyStatus::Ready {
                        source_revision: 12
                    },
                )
                .unwrap()
        );
        assert_eq!(
            states.status(&key),
            Some(&SimulationDependencyStatus::Ready {
                source_revision: 12
            })
        );
        assert!(states.retire(&key));
        assert_eq!(states.status(&key), None);
        assert!(states.status(&other).is_some());
    }

    #[test]
    fn dependency_owner_registration_is_idempotent_and_revisioned() {
        let mut states = SimulationDependencyStates::default();
        assert!(states.register_owner("sysml.twin-analysis").unwrap());
        let registered_revision = states.revision();
        assert!(!states.register_owner("sysml.twin-analysis").unwrap());
        assert_eq!(states.revision(), registered_revision);
        assert!(states.owner_is_registered("sysml.twin-analysis"));
        assert!(!states.owner_is_registered("sysml.doc-analysis"));
        assert!(states.register_owner("").is_err());
        assert!(SimulationDependencyKey::new(" sysml.twin-analysis", "school").is_err());
        assert!(SimulationDependencyKey::new("sysml.twin-analysis", " ").is_err());
    }
}

/// Fixed-step barrier between live simulation participants.
///
/// A participant may execute its solver off-thread, but the deterministic
/// shared simulation must not advance while the result for its next
/// communication point is in flight. The participant bridge raises `held`
/// before dispatching a step and clears it when the result lands. The time
/// spine projects this state onto `Time<Virtual>`, so SimTick, Rhai,
/// controllers, co-simulation propagation, and Avian share one barrier.
///
/// Barrier membership is supplied by the composed simulation topology. The
/// resource starts unresolved, which is deliberately fail-closed while a scene
/// is still being projected. Once the wiring projection has sealed a topology,
/// only participants in the reverse causal closure of a stateful engine sink
/// hold this barrier. A model that has no such path is still stepped and its
/// outputs are held at communication points, but it cannot stall the shared
/// physics clock.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct SimulationBarrier {
    /// Whether the next shared simulation step must wait for a participant result.
    pub held: bool,
    /// Number of live compiled participants measured on the last fixed tick.
    pub active_participants: usize,
    /// Number of live participants whose causal path requires this barrier.
    pub shared_clock_participants: usize,
    /// Largest target/current clock gap on the last fixed tick.
    pub worst_lag_secs: f64,
    /// Participant responsible for `worst_lag_secs`.
    pub worst_entity: Option<Entity>,
}

/// The authoritative set of participants that must synchronize with the
/// shared fixed-step world.
///
/// This is a projection of the resolved simulation graph, not a property of a
/// solver implementation. The USD/co-simulation projection computes the
/// reverse causal closure from stateful sinks (Avian forces, wheel actuators,
/// and joint drives) to their upstream producers. Modelica uses this resource
/// only to decide whether a pending worker result is a shared-clock barrier.
///
/// `topology_ready == false` means the graph is not trustworthy yet. Consumers
/// must then treat every live Modelica participant as coupled. This avoids
/// releasing the world during scene loading merely because the graph has not
/// been projected yet.
#[derive(Resource, Debug, Clone, Default)]
pub struct SimulationBarrierParticipants {
    pub topology_ready: bool,
    pub entities: EntityHashSet,
    /// Live Modelica participants, projected by the co-simulation owner. This
    /// lets generic scripting validate that a declared dependency is covered
    /// by the shared fixed-step barrier without depending on Modelica types.
    pub modelica_entities: EntityHashSet,
    /// Modelica and generic entity access sets named by each active scenario's
    /// simulation_dependencies hook. The scenario owner replaces the full plan
    /// on recompile, detach, or despawn.
    scenario_plans: HashMap<Entity, ScenarioEntityAccess>,
    /// Flattened membership for the per-step barrier read path.
    scenario_participants: EntityHashSet,
    /// A scenario whose source revision is compiling or resolving its
    /// dependency plan. Until the plan is committed, all Modelica participants
    /// are synchronized conservatively.
    pending_scenario_plans: EntityHashSet,
}

#[derive(Debug, Clone, Default)]
struct ScenarioEntityAccess {
    modelica: EntityHashSet,
    reads: EntityHashSet,
    writes: EntityHashSet,
    query_reads: BTreeSet<String>,
}

/// Owner namespace for an operation that must finish before authoritative
/// simulation time advances. Operation ids are allocated by their owner and
/// remain attached to prepared work through its terminal result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SimulationProgressOwner {
    /// Scene load, restart, or clear lifecycle transaction.
    SceneLifecycle,
    /// Runtime USD reference topology admission.
    SceneReferences,
    /// Authored terrain data and collider preparation.
    TerrainPreparation,
    /// USD document source preparation and revision admission.
    DocumentPreparation,
    /// Modelica source/interface preparation.
    ModelicaPreparation,
    /// Rhai parse/import preparation.
    ScriptPreparation,
}

/// Stable owner and operation identity for one simulation-progress hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimulationProgressKey {
    pub owner: SimulationProgressOwner,
    pub operation_id: u64,
}

impl SimulationProgressKey {
    /// Key the hold to the scene lifecycle transaction that owns preparation.
    pub const fn scene_transition(id: lunco_core::SceneTransitionId) -> Self {
        Self {
            owner: SimulationProgressOwner::SceneLifecycle,
            operation_id: id.get(),
        }
    }

    /// Key Modelica preparation to the exact live participant entity.
    ///
    /// A participant has at most one active preparation at a time; worker
    /// results are independently fenced by its `session_id` before this hold
    /// can be released. `Entity::to_bits` includes the generation, so a later
    /// entity reusing the same index cannot release this operation.
    pub fn modelica_participant(entity: Entity) -> Self {
        Self {
            owner: SimulationProgressOwner::ModelicaPreparation,
            operation_id: entity.to_bits(),
        }
    }

    /// Key terrain preparation to the exact live terrain entity.
    ///
    /// The terrain owner keeps this hold through asynchronous DEM preparation
    /// and releases it only after the authoritative height field/collider is
    /// committed or the operation reaches a terminal failure. Entity
    /// generation prevents a stale result from releasing a later entity that
    /// reused the same index.
    pub fn terrain_preparation(entity: Entity) -> Self {
        Self {
            owner: SimulationProgressOwner::TerrainPreparation,
            operation_id: entity.to_bits(),
        }
    }
}

/// User-visible reason why the causal simulation is waiting for preparation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationProgressBlocker {
    pub key: SimulationProgressKey,
    pub reason: String,
}

/// Reason-keyed admission gate for asynchronous work that changes which state
/// exists at a simulation boundary.
///
/// The gate is event driven: each owner acquires one key when its operation is
/// admitted and releases that exact key after a committed or failed terminal
/// result. Duplicate acquisition is idempotent; a stale completion cannot
/// release another operation's hold. Per-step Modelica causality remains in
/// [`SimulationBarrier`], whose worker handshake is a separate fixed-step
/// synchronization contract.
#[derive(Resource, Debug, Default)]
pub struct SimulationProgress {
    blockers: BTreeMap<SimulationProgressKey, SimulationProgressBlocker>,
}

impl SimulationProgress {
    /// Acquire an operation's admission hold. Returns `true` only when the key
    /// is newly admitted, keeping duplicate lifecycle notifications idempotent.
    pub fn acquire(&mut self, key: SimulationProgressKey, reason: impl Into<String>) -> bool {
        if self.blockers.contains_key(&key) {
            return false;
        }
        self.blockers.insert(
            key,
            SimulationProgressBlocker {
                key,
                reason: reason.into(),
            },
        );
        true
    }

    /// Release only the exact operation that reached its terminal result.
    pub fn release(&mut self, key: SimulationProgressKey) -> bool {
        self.blockers.remove(&key).is_some()
    }

    /// Whether the exact operation currently owns a simulation-progress hold.
    pub fn contains(&self, key: SimulationProgressKey) -> bool {
        self.blockers.contains_key(&key)
    }

    /// Update the visible reason while the same operation remains held.
    pub fn update_reason(&mut self, key: SimulationProgressKey, reason: impl Into<String>) -> bool {
        let Some(blocker) = self.blockers.get_mut(&key) else {
            return false;
        };
        let reason = reason.into();
        if blocker.reason == reason {
            return false;
        }
        blocker.reason = reason;
        true
    }

    /// Whether an admitted operation currently prevents authoritative ticks.
    pub fn is_held(&self) -> bool {
        !self.blockers.is_empty()
    }

    /// Ordered explanations for UI, status, and diagnostics.
    pub fn blockers(&self) -> impl Iterator<Item = &SimulationProgressBlocker> {
        self.blockers.values()
    }
}

impl SimulationBarrierParticipants {
    #[inline]
    pub fn requires_barrier(&self, entity: Entity) -> bool {
        !self.topology_ready
            || (!self.pending_scenario_plans.is_empty() && self.modelica_entities.contains(&entity))
            || self.entities.contains(&entity)
            || self.scenario_participants.contains(&entity)
    }

    /// Whether `scenario` declares `participant` in its committed Rhai plan.
    ///
    /// `entities` and `scenario_participants` are aggregate solver-barrier
    /// membership; neither identifies which scenario reads a participant.
    pub fn scenario_declares_dependency(&self, scenario: Entity, participant: Entity) -> bool {
        self.scenario_plans.get(&scenario).is_some_and(|plan| {
            plan.modelica.contains(&participant) || plan.reads.contains(&participant)
        })
    }

    /// Whether this scenario placed a Modelica entity in the shared access set.
    pub fn scenario_declares_modelica_dependency(
        &self,
        scenario: Entity,
        participant: Entity,
    ) -> bool {
        self.scenario_plans
            .get(&scenario)
            .is_some_and(|plan| plan.modelica.contains(&participant))
    }

    /// Whether this scenario declared a generic live-entity read.
    pub fn scenario_declares_read(&self, scenario: Entity, entity: Entity) -> bool {
        self.scenario_plans
            .get(&scenario)
            .is_some_and(|plan| plan.reads.contains(&entity))
    }

    /// Whether this scenario declared a generic live-entity write.
    pub fn scenario_declares_write(&self, scenario: Entity, entity: Entity) -> bool {
        self.scenario_plans
            .get(&scenario)
            .is_some_and(|plan| plan.writes.contains(&entity))
    }

    /// Whether this scenario declares a provider that reads a broad owner
    /// snapshot during simulation.
    pub fn scenario_declares_query_read(&self, scenario: Entity, name: &str) -> bool {
        self.scenario_plans
            .get(&scenario)
            .is_some_and(|plan| plan.query_reads.contains(name))
    }

    pub fn replace(&mut self, entities: impl IntoIterator<Item = Entity>) {
        self.entities.clear();
        self.entities.extend(entities);
        self.topology_ready = true;
    }

    /// Replace the current Modelica population while preserving the independent
    /// causal graph and scenario dependency contributions.
    pub fn replace_modelica_entities(&mut self, entities: impl IntoIterator<Item = Entity>) {
        self.modelica_entities.clear();
        self.modelica_entities.extend(entities);
        self.rebuild_scenario_participants();
    }

    /// Hold all Modelica participants while one scenario source revision is
    /// being compiled and its dependency hook is resolved.
    pub fn mark_scenario_plan_pending(&mut self, scenario: Entity) {
        self.pending_scenario_plans.insert(scenario);
    }

    /// Commit one scenario's resolved access plan and release its admission
    /// hold. Directional entries join the barrier when they are Modelica
    /// participants in the current projection.
    pub fn replace_scenario_plan(
        &mut self,
        scenario: Entity,
        modelica: impl IntoIterator<Item = Entity>,
        reads: impl IntoIterator<Item = Entity>,
        writes: impl IntoIterator<Item = Entity>,
        query_reads: impl IntoIterator<Item = String>,
    ) {
        self.scenario_plans.insert(
            scenario,
            ScenarioEntityAccess {
                modelica: modelica.into_iter().collect(),
                reads: reads.into_iter().collect(),
                writes: writes.into_iter().collect(),
                query_reads: query_reads.into_iter().collect(),
            },
        );
        self.pending_scenario_plans.remove(&scenario);
        self.rebuild_scenario_participants();
    }

    /// Add one runtime-discovered read to a committed scenario plan.
    ///
    /// This covers entity identities materialized by an ordered simulation
    /// command after the static dependency plan was resolved.
    pub fn add_scenario_read(&mut self, scenario: Entity, entity: Entity) -> Option<bool> {
        let inserted = self.scenario_plans.get_mut(&scenario)?.reads.insert(entity);
        self.rebuild_scenario_participants();
        Some(inserted)
    }

    /// Add one runtime-discovered write to a committed scenario plan.
    pub fn add_scenario_write(&mut self, scenario: Entity, entity: Entity) -> Option<bool> {
        let inserted = self
            .scenario_plans
            .get_mut(&scenario)?
            .writes
            .insert(entity);
        self.rebuild_scenario_participants();
        Some(inserted)
    }

    /// Remove one scenario's dependency contribution and pending-plan hold.
    pub fn remove_scenario_dependencies(&mut self, scenario: Entity) {
        self.scenario_plans.remove(&scenario);
        self.pending_scenario_plans.remove(&scenario);
        self.rebuild_scenario_participants();
    }

    /// Clear all scenario contributions after a shared scripting contract is
    /// replaced. The next lifecycle pass will rebuild them from current source.
    pub fn clear_scenario_dependencies(&mut self) {
        self.scenario_plans.clear();
        self.scenario_participants.clear();
        self.pending_scenario_plans.clear();
    }

    /// Whether this entity is a live Modelica participant in the latest
    /// co-simulation projection.
    pub fn is_modelica_participant(&self, entity: Entity) -> bool {
        self.modelica_entities.contains(&entity)
    }

    fn rebuild_scenario_participants(&mut self) {
        self.scenario_participants.clear();
        for plan in self.scenario_plans.values() {
            self.scenario_participants
                .extend(plan.modelica.iter().copied());
            self.scenario_participants.extend(
                plan.reads
                    .iter()
                    .filter(|entity| self.modelica_entities.contains(*entity))
                    .copied(),
            );
            self.scenario_participants.extend(
                plan.writes
                    .iter()
                    .filter(|entity| self.modelica_entities.contains(*entity))
                    .copied(),
            );
        }
    }
}
