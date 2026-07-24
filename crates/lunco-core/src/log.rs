//! Black box logger for simulation events.
//!
//! Logs all commands and telemetry for post-mission analysis.

use bevy::prelude::*;

use crate::telemetry::{SampledParameter, TelemetryEvent};

/// Plugin that registers logging observers.
pub struct LunCoLogPlugin;

impl Plugin for LunCoLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(log_telemetry_events);
        app.add_observer(log_sampled_parameter);
    }
}

fn log_telemetry_events(trigger: On<TelemetryEvent>) {
    let evt = trigger.event();
    // `cmd:*` and `key:*` are MECHANICAL events: every command the API/UI runs and
    // every key the player taps is republished on this bus so scenarios can
    // `wait_for("cmd:SpawnEntity")`. That makes them continuous, not rare — a
    // scenario writing a port each tick emits `cmd:SetPorts` at frame rate and
    // buries every real line in the terminal. They stay on the bus (scripts still
    // see them); they just stop being default-visible log output. Mission events
    // (touchdown, low-fuel, `emit("rover_deployed")`) keep `info!`.
    if evt.name.starts_with("cmd:")
        || evt.name.starts_with("key:")
        // Link state transitions are still persisted to the black box and
        // delivered to scenarios, but a scene starts several links at once and
        // their routine AOS/LOS chatter obscures actionable load diagnostics.
        || evt.name.starts_with("link.")
    {
        debug!(
            "[BLACKBOX] EVENT: name={}, severity={:?}, data={:?}, ts={:.4}",
            evt.name, evt.severity, evt.data, evt.timestamp
        );
        return;
    }
    info!(
        "[BLACKBOX] EVENT: name={}, severity={:?}, data={:?}, ts={:.4}",
        evt.name, evt.severity, evt.data, evt.timestamp
    );
}

fn log_sampled_parameter(trigger: On<SampledParameter>) {
    let param = trigger.event();
    // `debug!`, not `info!`: a SAMPLE is CONTINUOUS telemetry — the engine emits
    // `engine.fps` / `engine.frame_time` (and any watched port) every frame, so at
    // 100 fps an `info!` here floods the log and, redirected to a file, fills the
    // disk. The black-box RECORD of samples belongs in a telemetry channel/recording,
    // not the tracing log; this line is only a debug aid. Discrete EVENTs stay at
    // `info!` above — they are rare (touchdown, low-fuel) and worth seeing by default.
    debug!(
        "[BLACKBOX] SAMPLE: name={}, value={:?}, unit={}, ts={:.4}",
        param.name, param.value, param.unit, param.timestamp
    );
}
