//! Scope and cadence vocabulary shared by the engine layers.
//!
//! These types are deliberately only contracts.  They do not implement a
//! second event bus or a second Bevy loop; individual owners bind their
//! systems to the appropriate Bevy schedule and use typed events or revisioned
//! resources for hand-off.  Keeping the vocabulary in the dependency-light
//! core prevents application, Twin, and render crates from inventing strings
//! that collide at runtime.

use bevy::ecs::schedule::SystemSet;

/// Lifetime/ownership scope of a runtime resource or event.
///
/// Asset URIs such as `lunco://` and `twin://` remain the identity boundary
/// for persisted content.  This enum is the in-process ownership boundary;
/// it must not be replaced by a free-form string or a name-only registry.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RuntimeScope {
    /// Process-wide engine contracts and invariants.
    Core,
    /// One application/session, including the shell and user preferences.
    Application,
    /// One mounted Twin and its generation-owned runtime state.
    Twin,
}

/// Cadence/lane to which a runtime operation belongs.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RuntimeCycle {
    /// Scene/Twin admission and teardown boundaries.
    Lifecycle,
    /// Deterministic, virtual-time simulation and physics.
    Simulation,
    /// Wall-clock avatar/camera interaction.
    Interaction,
    /// Application command admission and typed command dispatch.
    Command,
    /// Application-scoped one-shot Rhai/REPL evaluation.
    Repl,
    /// Tick-stamped telemetry delivery and retention fan-out.
    Telemetry,
    /// EgUI/application UI painting and typed command emission.
    Ui,
    /// Presentation snapshots, transform hand-off, and render intent.
    Presentation,
    /// Revision/event-driven visual quality work such as baking and LOD.
    Visualization,
}

/// Shared Bevy ordering anchors for the runtime cycles.
///
/// Hosts configure these labels in the schedule they own.  The labels are
/// intentionally not a generic dispatcher: work still runs in its owner's
/// schedule and cannot silently jump from a UI or render pass into physics.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeCycleSet {
    /// Scene/Twin lifecycle transaction boundary.
    Lifecycle,
    /// Fixed simulation cycle.
    Simulation,
    /// Wall-clock interaction cycle.
    Interaction,
    /// Application command admission and typed command dispatch.
    Command,
    /// Application-scoped one-shot Rhai/REPL evaluation.
    Repl,
    /// Bounded delivery of sampled telemetry outside the fixed simulation loop.
    Telemetry,
    /// UI painting cycle.
    Ui,
    /// Presentation/render hand-off cycle.
    Presentation,
    /// Visualization-quality cycle.
    Visualization,
}

/// A typed in-process routing stamp for a revisioned fact or lifecycle event.
///
/// `generation` is the owner generation, not a global event sequence.  A
/// consumer must discard a completion whose generation no longer matches its
/// mounted Twin; this is what prevents stale visual work from a previous Twin
/// from being applied after a reload.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct RuntimeRoute {
    pub scope: RuntimeScope,
    pub cycle: RuntimeCycle,
    pub generation: u64,
}

impl RuntimeRoute {
    /// Construct a process/application route with no Twin generation.
    pub const fn application(cycle: RuntimeCycle) -> Self {
        Self {
            scope: RuntimeScope::Application,
            cycle,
            generation: 0,
        }
    }

    /// Construct a Twin route keyed by its mount generation.
    pub const fn twin(cycle: RuntimeCycle, generation: u64) -> Self {
        Self {
            scope: RuntimeScope::Twin,
            cycle,
            generation,
        }
    }
}

/// Clock family selected by the owner of a runtime invocation.
///
/// A missing clock means the operation is a discrete boundary and has no
/// elapsed-time contract. Callers must not infer a clock from whichever Bevy
/// `Time<T>` resource happens to be available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeClock {
    /// Discrete lifecycle or command boundary with no elapsed-time sample.
    None,
    /// Deterministic fixed simulation time.
    Simulation,
    /// Wall-rooted camera/avatar interaction time.
    Interaction,
    /// Application wall cadence, including one-shot evaluation.
    Application,
    /// Render and visualization cadence.
    Presentation,
}

/// Phase of a synchronous call inside its owning runtime cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimePhase {
    /// The host did not supply a classified phase.
    Unclassified,
    /// Synchronous preparation or initialization.
    Preparation,
    /// Scenario or owner startup.
    Start,
    /// Delivery of one producer-stamped event.
    Event,
    /// Deterministic continuous behavior.
    Behavior,
    /// Teardown or final cleanup.
    Stop,
    /// One-shot Rhai or tool evaluation.
    Evaluation,
    /// Typed command application.
    Command,
}

/// Immutable clock and ordering facts supplied by the active cycle owner.
///
/// Synchronous Rhai functions and nested hooks inherit this value from their
/// caller. `sequence` belongs to the consuming cycle; `producer` preserves an
/// event's origin when it is delivered in a different cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeProducerStamp {
    /// Owner and cycle that produced the event.
    pub route: RuntimeRoute,
    /// Logical sequence at the producer boundary.
    pub sequence: u64,
}

impl RuntimeProducerStamp {
    /// Stamp an event produced in a deterministic simulation tick.
    pub const fn simulation(generation: u64, sequence: u64) -> Self {
        Self {
            route: RuntimeRoute::twin(RuntimeCycle::Simulation, generation),
            sequence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuntimeExecutionContext {
    /// Owner lifetime and active cycle. `None` means the call has no classified
    /// runtime owner; consumers must not infer one from available resources.
    pub route: Option<RuntimeRoute>,
    /// Operation phase within the cycle.
    pub phase: RuntimePhase,
    /// Clock selected by the owning cycle.
    pub clock: RuntimeClock,
    /// Clock time in seconds when this cycle has a time sample.
    pub time_seconds: Option<f64>,
    /// Clock delta in seconds when this cycle has a delta sample.
    pub delta_seconds: Option<f64>,
    /// Stable logical sequence for the consuming cycle.
    pub sequence: Option<u64>,
    /// Producer origin for event-driven calls.
    pub producer: Option<RuntimeProducerStamp>,
}

impl RuntimeExecutionContext {
    /// Context for an operation whose owner has not classified its cycle.
    /// Time-sensitive APIs reject this context instead of guessing.
    pub const fn unclassified() -> Self {
        Self {
            route: None,
            phase: RuntimePhase::Unclassified,
            clock: RuntimeClock::None,
            time_seconds: None,
            delta_seconds: None,
            sequence: None,
            producer: None,
        }
    }

    /// Replace the phase while preserving the owner's cycle and clock sample.
    pub const fn with_phase(self, phase: RuntimePhase) -> Self {
        Self { phase, ..self }
    }

    /// Attach the producer origin of an event delivered in this invocation.
    pub const fn with_producer(self, producer: RuntimeProducerStamp) -> Self {
        Self {
            producer: Some(producer),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twin_routes_are_generation_qualified() {
        assert_ne!(
            RuntimeRoute::twin(RuntimeCycle::Visualization, 1),
            RuntimeRoute::twin(RuntimeCycle::Visualization, 2)
        );
    }

    #[test]
    fn application_routes_do_not_use_a_twin_generation() {
        assert_eq!(RuntimeRoute::application(RuntimeCycle::Ui).generation, 0);
    }

    #[test]
    fn unclassified_context_does_not_invent_an_owner_route() {
        assert_eq!(RuntimeExecutionContext::unclassified().route, None);
    }
}
