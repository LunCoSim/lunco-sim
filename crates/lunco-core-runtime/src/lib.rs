//! Bevy runtime mechanisms for the LunCo engine.
//!
//! Stable contracts and data components live in [`lunco_core`]. This package
//! owns the resources and schedules that make those contracts live inside a
//! Bevy application: pacing, subsystem toggles, gate instrumentation, fixed
//! simulation ticks, and rollback/netcode schedule anchors. Scene-lifecycle
//! and diagnostic contracts remain in `lunco_core` because shared domains use
//! them without requiring this runtime plugin.

pub mod cadence;
pub mod gate;
pub mod health;
pub mod pacing;
pub mod subsystems;
pub mod sync;

pub use cadence::{ApplicationCadence, CadenceClock};
pub use health::{ENGINE_HEALTH_HISTORY_LEN, EngineHealthSnapshot, PhysicsHealthSnapshot};
pub use pacing::{
    FramePacingDemand, SimulationBarrier, SimulationBarrierParticipants, SimulationExecutionMode,
    SimulationProgress, SimulationProgressBlocker, SimulationProgressKey, SimulationProgressOwner,
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
            .register_type::<SimTick>()
            .register_type::<health::EngineHealthSnapshot>()
            .register_type::<health::PhysicsHealthSnapshot>();

        register_core_resources(app);
        app.add_observer(record_command_cadence);
        app.add_observer(acquire_scene_progress_hold)
            .add_observer(release_completed_scene_progress_hold)
            .add_observer(release_failed_scene_progress_hold);
        app.add_systems(
            PostUpdate,
            health::publish_engine_health.in_set(lunco_core::RuntimeCycleSet::Presentation),
        );
        app.add_systems(
            lunco_core::SceneTeardown,
            (
                lunco_core::faults::clear_runtime_diagnostics,
                reset_core_scene_state,
            ),
        );
        app.configure_sets(
            lunco_core::SceneTeardown,
            lunco_core::RuntimeCycleSet::Lifecycle,
        );
        subsystems::build_subsystems(app);
        app.configure_sets(
            FixedUpdate,
            (SimTickSet, lunco_core::RuntimeCycleSet::Simulation).chain(),
        )
        .add_systems(FixedUpdate, advance_sim_tick.in_set(SimTickSet));
        app.configure_sets(
            Update,
            (
                lunco_core::RuntimeCycleSet::Lifecycle,
                lunco_core::RuntimeCycleSet::Command,
                lunco_core::RuntimeCycleSet::Repl,
                lunco_core::RuntimeCycleSet::Ui,
            )
                .chain(),
        );
        app.configure_sets(
            PostUpdate,
            (
                lunco_core::RuntimeCycleSet::Presentation,
                lunco_core::RuntimeCycleSet::Ui,
            )
                .chain(),
        );
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
        .init_resource::<SimulationBarrierParticipants>()
        .init_resource::<SimulationProgress>()
        .init_resource::<health::EngineHealthSnapshot>()
        .init_resource::<health::PhysicsHealthSnapshot>()
        .init_resource::<ApplicationCadence>();
}

fn record_command_cadence(
    _trigger: On<lunco_core::CommandOccurred>,
    time: Option<Res<Time<Real>>>,
    mut cadence: ResMut<ApplicationCadence>,
) {
    let Some(time) = time else {
        return;
    };
    cadence.observe_command(&time);
}

fn acquire_scene_progress_hold(
    trigger: On<lunco_core::SceneTransitionStarted>,
    mut progress: ResMut<SimulationProgress>,
) {
    let event = trigger.event();
    let reason = match &event.transition {
        lunco_core::SceneTransition::Load { path, .. } => format!("Loading scene: {path}"),
        lunco_core::SceneTransition::Clear => "Clearing scene".to_owned(),
        lunco_core::SceneTransition::Restart { path, .. } => {
            format!("Restarting scene: {path}")
        }
    };
    let key = SimulationProgressKey::scene_transition(event.id);
    if !progress.acquire(key, reason) {
        bevy::log::warn!(
            "[simulation-progress] duplicate scene hold for transaction {}",
            event.id.get()
        );
    }
}

fn release_completed_scene_progress_hold(
    trigger: On<lunco_core::SceneTransitionCompleted>,
    mut progress: ResMut<SimulationProgress>,
) {
    release_scene_progress_hold(trigger.event().id, &mut progress);
}

fn release_failed_scene_progress_hold(
    trigger: On<lunco_core::SceneTransitionFailed>,
    mut progress: ResMut<SimulationProgress>,
) {
    release_scene_progress_hold(trigger.event().id, &mut progress);
}

fn release_scene_progress_hold(
    id: lunco_core::SceneTransitionId,
    progress: &mut SimulationProgress,
) {
    if !progress.release(SimulationProgressKey::scene_transition(id)) {
        bevy::log::warn!(
            "[simulation-progress] terminal edge has no matching scene hold for transaction {}",
            id.get()
        );
    }
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

    #[test]
    fn scene_progress_is_released_only_by_its_matching_transaction_edge() {
        use lunco_core::{
            SceneTransition, SceneTransitionCompleted, SceneTransitionCoordinator,
            SceneTransitionFailed, SceneTransitionStarted,
        };

        let mut app = App::new();
        app.add_plugins(LunCoCoreRuntimePlugin)
            .init_resource::<SceneTransitionCoordinator>();

        let mut coordinator = SceneTransitionCoordinator::default();
        let first = SceneTransition::load("same.usda", "/World");
        let first_id = coordinator.start(first.clone());
        app.world_mut().trigger(SceneTransitionStarted {
            id: first_id,
            transition: first.clone(),
        });
        assert_eq!(
            app.world()
                .resource::<SimulationProgress>()
                .blockers()
                .count(),
            1
        );

        assert!(coordinator.finish(first_id));
        let second = SceneTransition::load("same.usda", "/World");
        let second_id = coordinator.start(second.clone());
        app.world_mut().trigger(SceneTransitionStarted {
            id: second_id,
            transition: second,
        });

        app.world_mut().trigger(SceneTransitionCompleted {
            id: first_id,
            transition: first,
        });
        let progress = app.world().resource::<SimulationProgress>();
        assert!(progress.is_held());
        assert_eq!(progress.blockers().count(), 1);
        assert_eq!(
            progress.blockers().next().unwrap().key,
            SimulationProgressKey::scene_transition(second_id)
        );

        app.world_mut().trigger(SceneTransitionFailed {
            id: second_id,
            transition: SceneTransition::load("same.usda", "/World"),
            error: "asset failed".to_owned(),
        });
        assert!(!app.world().resource::<SimulationProgress>().is_held());
    }

    #[test]
    fn command_occurrence_advances_only_the_command_clock() {
        let mut app = App::new();
        app.add_plugins(LunCoCoreRuntimePlugin)
            .insert_resource(Time::<Real>::default());

        app.world_mut().trigger(lunco_core::CommandOccurred {
            name: "First".to_owned(),
        });
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_millis(250));
        app.world_mut().trigger(lunco_core::CommandOccurred {
            name: "Second".to_owned(),
        });

        let cadence = app.world().resource::<ApplicationCadence>();
        assert_eq!(cadence.command.sequence, 2);
        assert_eq!(cadence.repl.sequence, 0);
        assert_eq!(cadence.command.interval_secs, Some(0.25));
    }
}
