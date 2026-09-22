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
}
