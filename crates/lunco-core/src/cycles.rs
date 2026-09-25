//! Bevy ordering labels for the shared runtime cycle vocabulary.
//!
//! The owner-neutral route and execution-context values live in
//! `lunco-runtime-context`, where scripting backends can consume them without
//! depending on the ECS runtime.

use bevy::ecs::schedule::SystemSet;

pub use lunco_runtime_context::{
    RuntimeClock, RuntimeCycle, RuntimeExecutionContext, RuntimePhase, RuntimeProducerStamp,
    RuntimeRoute, RuntimeScope,
};

/// Shared Bevy ordering anchors for runtime cycles.
///
/// These labels order systems in their owning Bevy schedule. They do not
/// create a second scheduler or imply an elapsed-time clock.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeCycleSet {
    /// Scene/Twin lifecycle transaction boundary.
    Lifecycle,
    /// Admit stable identities for entities created by lifecycle projection.
    IdentityAdmission,
    /// Publish identity lookups after identity admission and before simulation.
    EntityIndex,
    /// Fixed simulation cycle.
    Simulation,
    /// Wall-clock avatar/camera interaction cycle.
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
