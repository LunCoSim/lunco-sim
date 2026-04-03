use bevy::prelude::*;
use crate::architecture::{CommandMessage, CommandResponse};
use crate::telemetry::{TelemetryEvent, SampledParameter};

pub struct LunCoLogPlugin;

impl Plugin for LunCoLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(log_commands);
        app.add_observer(log_responses);
        app.add_observer(log_telemetry_events);
        app.add_observer(log_sampled_parameters);
    }
}

/// Black Box logger that records all outgoing commands.
fn log_commands(trigger: On<CommandMessage>) {
    let cmd = trigger.event();
    info!("[BLACKBOX] COMMAND: id={}, target={:?}, name={}, args={:?}, source={:?}", 
        cmd.id, cmd.target, cmd.name, cmd.args, cmd.source);
}

/// Black Box logger that records all command responses (ACK/NACK).
fn log_responses(trigger: On<CommandResponse>) {
    let resp = trigger.event();
    info!("[BLACKBOX] RESPONSE: cmd_id={}, status={:?}", resp.command_id, resp.status);
}

/// Black Box logger for discrete telemetry events.
fn log_telemetry_events(trigger: On<TelemetryEvent>) {
    let evt = trigger.event();
    info!("[BLACKBOX] EVENT: name={}, severity={:?}, data={:?}, ts={:.4}", 
        evt.name, evt.severity, evt.data, evt.timestamp);
}

/// Black Box logger for continuous sampled parameters.
fn log_sampled_parameters(trigger: On<SampledParameter>) {
    let param = trigger.event();
    // We use debug for sampled params to avoid spamming info logs in normal runs
    debug!("[BLACKBOX] TM: name={}, value={:?}, unit={}, ts={:.4}", 
        param.name, param.value, param.unit, param.timestamp);
}
