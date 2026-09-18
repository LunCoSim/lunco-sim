//! Shared telemetry contracts and the typed event bus for LunCoSim.
//!
//! This crate owns the data and lifecycle seam used by producers, API/status
//! consumers, scripting, and the sampling engine. Sampling policy and retained
//! history remain in `lunco-telemetry`.

use bevy::prelude::{App, Commands, On, Plugin};

pub mod telemetry;
pub use telemetry::*;
mod log;
pub use log::LunCoLogPlugin;

/// Installs the shared telemetry reflection and black-box logging boundary.
///
/// Domain crates may emit typed telemetry without depending on the sampler or
/// UI. Applications install this plugin alongside their core runtime.
pub struct LunCoTelemetryCorePlugin;

impl Plugin for LunCoTelemetryCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin)
            .add_observer(project_command_occurrence)
            .add_observer(project_runtime_error)
            .add_observer(project_subsystem_state)
            .register_type::<telemetry::Severity>()
            .register_type::<TelemetryValue>()
            .register_type::<TelemetryEvent>()
            .register_type::<Parameter>()
            .register_type::<SampledParameter>();
    }
}

fn project_command_occurrence(trigger: On<lunco_core::CommandOccurred>, mut commands: Commands) {
    commands.trigger(telemetry::command_telemetry_event(
        trigger.event().name.clone(),
    ));
}

fn project_runtime_error(trigger: On<lunco_core::RuntimeError>, mut commands: Commands) {
    let event = trigger.event();
    telemetry::trigger_error(&mut commands, event.name.clone(), event.message.clone());
}

fn project_subsystem_state(trigger: On<lunco_core::SubsystemStateChanged>, mut commands: Commands) {
    let event = trigger.event();
    commands.trigger(telemetry::TelemetryEvent {
        name: format!("subsystem:{}", event.name),
        source: 0,
        severity: telemetry::Severity::Info,
        data: telemetry::TelemetryValue::Bool(event.on),
        timestamp: 0.0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{ResMut, Resource};

    #[derive(Resource, Default)]
    struct Seen(Vec<TelemetryEvent>);

    fn capture_event(trigger: On<TelemetryEvent>, mut seen: ResMut<Seen>) {
        seen.0.push(trigger.event().clone());
    }

    #[test]
    fn projects_generic_core_facts_without_core_telemetry_types() {
        let mut app = App::new();
        app.add_plugins(LunCoTelemetryCorePlugin)
            .init_resource::<Seen>()
            .add_observer(capture_event);

        app.world_mut().trigger(lunco_core::CommandOccurred {
            name: "SetValue".into(),
        });
        app.world_mut().trigger(lunco_core::RuntimeError {
            name: "load-failed".into(),
            message: "fixture missing".into(),
        });
        app.world_mut().trigger(lunco_core::SubsystemStateChanged {
            name: "thermal".into(),
            on: true,
        });
        app.world_mut().flush();

        let events = &app.world().resource::<Seen>().0;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].name, "cmd:SetValue");
        assert_eq!(events[1].name, "load-failed");
        assert_eq!(events[2].name, "subsystem:thermal");
    }
}
