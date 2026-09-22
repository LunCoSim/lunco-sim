//! Bevy runtime mechanisms for the LunCo engine.
//!
//! Stable contracts and data components live in [`lunco_core`]. This package
//! owns the resources and schedules that make those contracts live inside a
//! Bevy application: pacing, subsystem toggles, gate instrumentation, fixed
//! simulation ticks, and rollback/netcode schedule anchors. Scene-lifecycle
//! and diagnostic contracts remain in `lunco_core` because shared domains use
//! them without requiring this runtime plugin.

pub mod gate;
pub mod pacing;
pub mod subsystems;
pub mod sync;

pub use pacing::{
    KeepAwake, SimulationBarrier, SimulationBarrierParticipants, SimulationExecutionMode,
};
pub use sync::LockExt;

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// Fixed simulation rate used by the shared Bevy and network clock.
pub const FIXED_HZ: f64 = 60.0;

/// Seconds represented by one fixed simulation tick.
pub const SECS_PER_TICK: f64 = 1.0 / FIXED_HZ;

/// Monotonic discrete simulation tick.
#[derive(
    Resource,
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Resource)]
pub struct SimTick(pub u64);

/// Fixed-update ordering anchor for the authoritative simulation tick.
///
/// Consumers that record or publish state keyed by simulation time must run
/// after this set.  That makes the tick boundary explicit instead of relying
/// on incidental system insertion order.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimTickSet;

impl SimTick {
    /// Signed tick distance `self - other`, wrapping-safe.
    pub fn wrapping_diff(self, other: SimTick) -> i64 {
        self.0.wrapping_sub(other.0) as i64
    }
}

/// Ordering anchor for fixed-step control propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlDacSet;

/// Schedule used to replay one deterministic actuation tick.
#[derive(ScheduleLabel, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RollbackReplay;

/// True only while the client is re-simulating an owned entity.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackInProgress(pub bool);

/// Run condition for systems that must not execute during rollback replay.
pub fn not_rolling_back(rb: Option<Res<RollbackInProgress>>) -> bool {
    !rb.is_some_and(|r| r.0)
}

/// Ordering anchor for the client-netcode update pipeline.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetcodeSet {
    /// Instantiate host-replicated spawns.
    InstantiateSpawns,
    /// Run client prediction after replicated spawns exist.
    Predict,
}

/// Bevy plugin that installs the generic runtime substrate.
pub struct LunCoCoreRuntimePlugin;

impl Plugin for LunCoCoreRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<lunco_core::PhysicsPoseAuthoritative>()
            .register_type::<lunco_core::ModelStateRevision>()
            .register_type::<lunco_core::MobilityRoot>()
            .register_type::<lunco_core::GlobalEntityId>()
            .register_type::<lunco_core::Provenance>()
            .register_type::<SimTick>();

        register_core_resources(app);
        app.add_systems(
            lunco_core::SceneTeardown,
            (
                lunco_core::faults::clear_runtime_diagnostics,
                reset_core_scene_state,
            ),
        );
        subsystems::build_subsystems(app);
        app.configure_sets(FixedUpdate, SimTickSet)
            .add_systems(FixedUpdate, advance_sim_tick.in_set(SimTickSet));
        app.init_resource::<RollbackInProgress>();
    }
}

fn register_core_resources(app: &mut App) {
    app.init_resource::<SimTick>()
        .init_resource::<lunco_core::SceneMountState>()
        .init_resource::<lunco_core::CommandResults>()
        .init_resource::<lunco_core::ActiveCommandId>()
        .init_resource::<lunco_core::RuntimeFaults>()
        .init_resource::<lunco_core::RuntimeDiagnostics>()
        .init_resource::<SimulationBarrier>()
        .init_resource::<SimulationBarrierParticipants>();
}

fn reset_core_scene_state(mut rollback: ResMut<RollbackInProgress>) {
    rollback.0 = false;
}

fn advance_sim_tick(mut tick: ResMut<SimTick>, vtime: Option<Res<Time<Virtual>>>) {
    let running = vtime.is_some_and(|time| !time.is_paused() && time.relative_speed_f64() > 0.0);
    if running {
        tick.0 = tick.0.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_tick_advances_only_while_virtual_time_runs() {
        let mut app = App::new();
        app.init_resource::<SimTick>()
            .add_systems(FixedUpdate, advance_sim_tick)
            .insert_resource(Time::<Virtual>::default());

        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 1);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 1);
    }
}
