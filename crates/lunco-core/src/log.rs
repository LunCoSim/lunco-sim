//! Black box logger for simulation events.
//!
//! Logs all commands and telemetry for post-mission analysis.

use bevy::prelude::*;

use crate::telemetry::{TelemetryEvent, SampledParameter};

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
    info!("[BLACKBOX] EVENT: name={}, severity={:?}, data={:?}, ts={:.4}",
        evt.name, evt.severity, evt.data, evt.timestamp);
}

fn log_sampled_parameter(trigger: On<SampledParameter>) {
    let param = trigger.event();
    info!("[BLACKBOX] SAMPLE: name={}, value={:?}, unit={}, ts={:.4}",
        param.name, param.value, param.unit, param.timestamp);
}
