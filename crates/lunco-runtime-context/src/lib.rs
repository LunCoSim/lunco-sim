//! Owner-neutral runtime cycle, route, and invocation context contracts.
//!
//! These values let the ECS runtime and dependency-light scripting backends
//! exchange the same owner, cycle, clock, phase, and producer facts. They carry
//! no scheduler or clock state themselves.

/// Lifetime/ownership scope of a runtime resource or event.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bevy_reflect::Reflect,
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
    bevy_reflect::Reflect,
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

/// A typed in-process routing stamp for a revisioned fact or lifecycle event.
///
/// `generation` is the owner generation, not a global event sequence. A
/// consumer discards a completion whose generation no longer matches its
/// mounted Twin.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bevy_reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct RuntimeRoute {
    /// Lifetime owner for this route.
    pub scope: RuntimeScope,
    /// Runtime cycle that consumes the routed work.
    pub cycle: RuntimeCycle,
    /// Owner generation; application routes use zero.
    pub generation: u64,
}

impl RuntimeRoute {
    /// Construct a process-wide engine route with no owner generation.
    pub const fn core(cycle: RuntimeCycle) -> Self {
        Self {
            scope: RuntimeScope::Core,
            cycle,
            generation: 0,
        }
    }

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
/// elapsed-time contract. Callers must not infer a clock from whichever time
/// resource happens to be available.
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
    /// Resolution of declared dependencies before live model access.
    DependencyPlan,
    /// Mutable top-level initialization after the dependency plan commits.
    Initialization,
    /// Scenario or owner startup.
    Start,
    /// Delivery of one producer-stamped event.
    Event,
    /// Deterministic continuous behavior.
    Behavior,
    /// One-shot presentation work in the Twin visualization cycle.
    Visualization,
    /// Teardown or final cleanup.
    Stop,
    /// One-shot Rhai or tool evaluation.
    Evaluation,
    /// Typed command application.
    Command,
}

/// Producer identity and sequence for an event delivered in another cycle.
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

/// Immutable clock and ordering facts supplied by the active cycle owner.
///
/// `route: None` means the caller has no classified runtime owner. Time-sensitive
/// APIs reject that context instead of inferring one. `sequence` belongs to the
/// consuming cycle; `producer` preserves an event's origin when delivered in a
/// different cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuntimeExecutionContext {
    /// Owner lifetime and active cycle.
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

/// Why an owner-supplied runtime context cannot be used for an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeExecutionContextError {
    /// An unclassified context contains facts that require a classified owner.
    UnclassifiedContainsOwnerFacts,
    /// A selected clock has no current time sample.
    ClockMissingTimeSample,
    /// A discrete invocation contains a clock time or delta.
    DiscreteContextContainsClockSample,
    /// The selected clock does not belong to the invocation cycle.
    ClockDoesNotMatchCycle,
    /// A Core or Application route carries a non-zero owner generation.
    InvalidRouteGeneration,
    /// The supplied time sample is negative or not finite.
    InvalidTimeSample,
    /// The supplied delta is negative or not finite.
    InvalidDeltaSample,
}

impl std::fmt::Display for RuntimeExecutionContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::UnclassifiedContainsOwnerFacts => {
                "an unclassified context cannot carry owner, clock, sequence, or producer facts"
            }
            Self::ClockMissingTimeSample => "a selected clock requires a time sample",
            Self::DiscreteContextContainsClockSample => {
                "a discrete context cannot carry a time or delta sample"
            }
            Self::ClockDoesNotMatchCycle => "the selected clock does not match the runtime cycle",
            Self::InvalidRouteGeneration => "Core and Application routes must use generation zero",
            Self::InvalidTimeSample => "the time sample must be finite and non-negative",
            Self::InvalidDeltaSample => "the delta sample must be finite and non-negative",
        };
        f.write_str(detail)
    }
}

impl std::error::Error for RuntimeExecutionContextError {}

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

    /// Validate clock samples and ensure a selected clock belongs to its cycle.
    ///
    /// Discrete contexts use [`RuntimeClock::None`] and carry no elapsed-time
    /// values. A selected clock requires a finite, non-negative time sample.
    /// Deltas are optional for event callbacks that retain a clock identity and
    /// current sequence without claiming an exact elapsed interval.
    pub fn validate(self) -> Result<(), RuntimeExecutionContextError> {
        if self.route.is_none() {
            return if self.phase == RuntimePhase::Unclassified
                && self.clock == RuntimeClock::None
                && self.time_seconds.is_none()
                && self.delta_seconds.is_none()
                && self.sequence.is_none()
                && self.producer.is_none()
            {
                Ok(())
            } else {
                Err(RuntimeExecutionContextError::UnclassifiedContainsOwnerFacts)
            };
        }

        if self
            .time_seconds
            .is_some_and(|time| !time.is_finite() || time < 0.0)
        {
            return Err(RuntimeExecutionContextError::InvalidTimeSample);
        }
        if self
            .delta_seconds
            .is_some_and(|delta| !delta.is_finite() || delta < 0.0)
        {
            return Err(RuntimeExecutionContextError::InvalidDeltaSample);
        }

        let Some(route) = self.route else {
            return Err(RuntimeExecutionContextError::UnclassifiedContainsOwnerFacts);
        };
        if route.scope != RuntimeScope::Twin && route.generation != 0 {
            return Err(RuntimeExecutionContextError::InvalidRouteGeneration);
        }
        match self.clock {
            RuntimeClock::None => {
                if self.time_seconds.is_some() || self.delta_seconds.is_some() {
                    return Err(RuntimeExecutionContextError::DiscreteContextContainsClockSample);
                }
            }
            clock => {
                if self.time_seconds.is_none() {
                    return Err(RuntimeExecutionContextError::ClockMissingTimeSample);
                }
                let matches_cycle = clock_matches_cycle(clock, route.cycle);
                if !matches_cycle {
                    return Err(RuntimeExecutionContextError::ClockDoesNotMatchCycle);
                }
            }
        }

        if let Some(producer) = self.producer {
            if producer.route.scope != RuntimeScope::Twin && producer.route.generation != 0 {
                return Err(RuntimeExecutionContextError::InvalidRouteGeneration);
            }
        }

        Ok(())
    }
}

fn clock_matches_cycle(clock: RuntimeClock, cycle: RuntimeCycle) -> bool {
    match clock {
        RuntimeClock::None => true,
        RuntimeClock::Simulation => cycle == RuntimeCycle::Simulation,
        RuntimeClock::Interaction => cycle == RuntimeCycle::Interaction,
        RuntimeClock::Application => matches!(
            cycle,
            RuntimeCycle::Command | RuntimeCycle::Repl | RuntimeCycle::Telemetry | RuntimeCycle::Ui
        ),
        RuntimeClock::Presentation => {
            matches!(
                cycle,
                RuntimeCycle::Presentation | RuntimeCycle::Visualization
            )
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
    fn core_routes_do_not_use_an_owner_generation() {
        assert_eq!(RuntimeRoute::core(RuntimeCycle::Simulation).generation, 0);
    }

    #[test]
    fn unclassified_context_does_not_invent_an_owner_route() {
        assert_eq!(RuntimeExecutionContext::unclassified().route, None);
        assert!(RuntimeExecutionContext::unclassified().validate().is_ok());
    }

    #[test]
    fn validates_clocked_contexts_against_their_cycle_and_sample() {
        let simulation = RuntimeExecutionContext {
            route: Some(RuntimeRoute::twin(RuntimeCycle::Simulation, 17)),
            phase: RuntimePhase::Event,
            clock: RuntimeClock::Simulation,
            time_seconds: Some(2.5),
            delta_seconds: None,
            sequence: Some(250),
            producer: None,
        };
        assert!(simulation.validate().is_ok());

        let application = RuntimeExecutionContext {
            route: Some(RuntimeRoute::application(RuntimeCycle::Repl)),
            phase: RuntimePhase::Evaluation,
            clock: RuntimeClock::Application,
            time_seconds: Some(8.0),
            delta_seconds: None,
            sequence: Some(3),
            producer: None,
        };
        assert!(application.validate().is_ok());
    }

    #[test]
    fn rejects_a_missing_or_mismatched_clock_sample() {
        let missing_time = RuntimeExecutionContext {
            route: Some(RuntimeRoute::application(RuntimeCycle::Repl)),
            phase: RuntimePhase::Evaluation,
            clock: RuntimeClock::Application,
            time_seconds: None,
            delta_seconds: None,
            sequence: Some(3),
            producer: None,
        };
        assert_eq!(
            missing_time.validate(),
            Err(RuntimeExecutionContextError::ClockMissingTimeSample)
        );

        let wrong_cycle = RuntimeExecutionContext {
            route: Some(RuntimeRoute::application(RuntimeCycle::Ui)),
            phase: RuntimePhase::Evaluation,
            clock: RuntimeClock::Simulation,
            time_seconds: Some(8.0),
            delta_seconds: Some(0.01),
            sequence: Some(3),
            producer: None,
        };
        assert_eq!(
            wrong_cycle.validate(),
            Err(RuntimeExecutionContextError::ClockDoesNotMatchCycle)
        );
    }

    #[test]
    fn rejects_invalid_samples_and_clock_data_on_discrete_contexts() {
        let invalid_time = RuntimeExecutionContext {
            route: Some(RuntimeRoute::core(RuntimeCycle::Simulation)),
            phase: RuntimePhase::Behavior,
            clock: RuntimeClock::Simulation,
            time_seconds: Some(f64::NAN),
            delta_seconds: Some(0.01),
            sequence: Some(1),
            producer: None,
        };
        assert_eq!(
            invalid_time.validate(),
            Err(RuntimeExecutionContextError::InvalidTimeSample)
        );

        let discrete_with_time = RuntimeExecutionContext {
            route: Some(RuntimeRoute::twin(RuntimeCycle::Lifecycle, 2)),
            phase: RuntimePhase::Preparation,
            clock: RuntimeClock::None,
            time_seconds: Some(1.0),
            delta_seconds: None,
            sequence: None,
            producer: None,
        };
        assert_eq!(
            discrete_with_time.validate(),
            Err(RuntimeExecutionContextError::DiscreteContextContainsClockSample)
        );
    }
}
