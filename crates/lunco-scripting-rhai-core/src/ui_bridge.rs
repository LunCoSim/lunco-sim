//! Typed script-to-UI requests.
//!
//! The scripting layer owns the neutral event contract; an editor or another
//! host may choose how to render it. No domain names or JSON payloads belong in
//! this bridge.

use bevy::prelude::Event;
use lunco_telemetry_core::TelemetryValue;

#[derive(Clone, Debug)]
pub struct ScriptMenuItem {
    pub label: String,
    pub tool: String,
    pub hook: String,
    pub args: TelemetryValue,
}

#[derive(Event, Clone, Debug)]
pub enum ScriptUiRequest {
    ContextMenu {
        screen_position: [f32; 2],
        items: Vec<ScriptMenuItem>,
    },
}
