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
use std::collections::BTreeMap;

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
    use super::FramePacingDemand;

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
    /// USD document source preparation and revision admission.
    DocumentPreparation,
    /// Modelica source/interface preparation.
    ModelicaPreparation,
    /// Rhai parse/import preparation.
    ScriptPreparation,
    /// SysML source-set analysis and revision admission.
    SysmlAnalysis,
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
        !self.topology_ready || self.entities.contains(&entity)
    }

    pub fn replace(&mut self, entities: impl IntoIterator<Item = Entity>) {
        self.entities.clear();
        self.entities.extend(entities);
        self.topology_ready = true;
    }
}
