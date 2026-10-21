//! Explicit fixed-cycle admission through the shared simulation runner.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{AppSceneExt, Command, SceneResource, on_command, register_commands};

/// Manual admission demand. This is a count of fixed cycles, not another clock.
/// Scene preparation and participant barriers remain mandatory for every cycle.
#[derive(Resource, Debug, Default)]
pub struct SimulationStepControl {
    held: bool,
    remaining: u32,
    pub(crate) active_cycle: bool,
}

impl SceneResource for SimulationStepControl {}

impl SimulationStepControl {
    /// Whether automatic simulation advancement is held by explicit control.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Whether the shared fixed runner is currently applying a complete cycle.
    pub fn is_cycle_active(&self) -> bool {
        self.active_cycle
    }

    /// Explicit fixed cycles still awaiting admission.
    pub fn remaining(&self) -> u32 {
        self.remaining
    }

    /// Validate the complete request before changing hold state or queued demand.
    pub fn request(
        &mut self,
        hold: Option<bool>,
        steps: Option<u32>,
        transport_running: bool,
    ) -> Result<(), String> {
        let held = hold.unwrap_or(self.held);
        let remaining = if hold == Some(false) {
            0
        } else {
            self.remaining
        };
        let remaining = match steps {
            Some(0) => return Err("simulation step count must be positive".into()),
            Some(_) if !held && transport_running => {
                return Err("hold or pause the simulation before requesting fixed cycles".into());
            }
            Some(count) => remaining
                .checked_add(count)
                .ok_or("simulation step queue is full")?,
            None => remaining,
        };
        self.held = held;
        self.remaining = remaining;
        Ok(())
    }

    pub(crate) fn consume(&mut self) {
        self.remaining -= 1;
    }
    pub(crate) fn cancel(&mut self) {
        self.remaining = 0;
    }
}

/// Hold/release automatic simulation advancement or request exact fixed cycles.
/// Each requested cycle uses the same physics, Modelica and scenario schedule.
#[Command(default)]
pub struct StepSimulation {
    /// Explicit manual hold. Releasing it also retires unspent step requests.
    pub hold: Option<bool>,
    /// Positive number of shared fixed cycles to admit while held or paused.
    pub steps: Option<u32>,
}

#[on_command(StepSimulation)]
fn on_step_simulation(
    trigger: On<StepSimulation>,
    transport: Res<crate::TimeTransport>,
    mut control: ResMut<SimulationStepControl>,
) -> Result<Ack, String> {
    let command = trigger.event();
    if let Err(error) = control.request(command.hold, command.steps, transport.is_running()) {
        warn!("[time] rejected shared simulation step request: {error}");
        return Err(error);
    }
    Ok(Ack::new(OpId::new()))
}

pub(crate) fn install(app: &mut App) {
    app.init_scene_resource::<SimulationStepControl>();
    register_all_commands(app);
}

register_commands!(on_step_simulation);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_lifecycle_retires_manual_demand_after_removal_callbacks() {
        #[derive(Component)]
        struct TerminalRequester;
        let mut app = App::new();
        install(&mut app);
        app.add_observer(|_: On<Remove<TerminalRequester>>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                let mut control = world.resource_mut::<SimulationStepControl>();
                control.request(Some(true), Some(7), true).unwrap();
                control.active_cycle = true;
            });
        });
        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<SimulationStepControl>()
                .request(Some(true), Some(4), true)
                .unwrap();
            app.world_mut()
                .spawn((lunco_core::SceneOwned, TerminalRequester));
            lunco_core::run_scene_teardown(app.world_mut());
            let control = app.world().resource::<SimulationStepControl>();
            assert!(!control.is_held());
            assert!(!control.is_cycle_active());
            assert_eq!(control.remaining(), 0);
        }
    }
}
