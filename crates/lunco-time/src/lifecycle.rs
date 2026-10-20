//! Discrete simulation transport facts, independent of temporary admission holds.

use bevy::prelude::*;
use lunco_core::{AppSceneExt, SceneResource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationLifecycleTransition {
    Started,
    Paused,
    Unpaused,
    Stopped,
}

impl SimulationLifecycleTransition {
    pub fn telemetry_name(self) -> &'static str {
        match self {
            Self::Started => "SIMULATION_STARTED",
            Self::Paused => "SIMULATION_PAUSED",
            Self::Unpaused => "SIMULATION_UNPAUSED",
            Self::Stopped => "SIMULATION_STOPPED",
        }
    }
}

/// Published only at an authoritative transport or simulation lifecycle edge.
#[derive(Event, Debug, Clone, Copy)]
pub struct SimulationLifecycleChanged {
    pub transition: SimulationLifecycleTransition,
}

#[derive(Resource, Default)]
pub(crate) struct SimulationLifecycle {
    started: bool,
    // A one-shot terminal notice crosses teardown so consumers receive it
    // after their outgoing scene history is cleared. No scene data survives.
    pending_stop: bool,
}

impl SceneResource for SimulationLifecycle {
    fn clear_scene(&mut self) {
        self.pending_stop |= self.started;
        self.started = false;
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_scene_resource::<SimulationLifecycle>()
        .add_systems(
            First,
            flush_stop.run_if(resource_changed::<SimulationLifecycle>),
        );
}

pub(crate) fn transport_changed(
    lifecycle: &SimulationLifecycle,
    before: crate::TransportMode,
    after: crate::TransportMode,
    commands: &mut Commands,
) {
    if before == after || !lifecycle.started {
        return;
    }
    commands.trigger(SimulationLifecycleChanged {
        transition: match after {
            crate::TransportMode::Playing => SimulationLifecycleTransition::Unpaused,
            crate::TransportMode::Paused => SimulationLifecycleTransition::Paused,
        },
    });
}

/// The first automatic tick, rather than scene projection or a worker receipt,
/// proves that the simulation has started. Explicit single steps stay manual.
pub(crate) fn start(world: &mut World) {
    let state = world.resource::<SimulationLifecycle>();
    if state.started && !state.pending_stop {
        return;
    }
    let mut state = world.resource_mut::<SimulationLifecycle>();
    let stopped = std::mem::take(&mut state.pending_stop);
    let started = !state.started;
    state.started = true;
    drop(state);
    if stopped {
        world.trigger(SimulationLifecycleChanged {
            transition: SimulationLifecycleTransition::Stopped,
        });
    }
    if started {
        world.trigger(SimulationLifecycleChanged {
            transition: SimulationLifecycleTransition::Started,
        });
    }
}

fn flush_stop(mut state: ResMut<SimulationLifecycle>, mut commands: Commands) {
    if state.pending_stop {
        state.pending_stop = false;
        commands.trigger(SimulationLifecycleChanged {
            transition: SimulationLifecycleTransition::Stopped,
        });
    }
}
