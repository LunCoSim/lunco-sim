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

/// A script-authored top-level workbench menu contribution.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchMenu {
    pub label: String,
    pub items: Vec<ScriptWorkbenchMenuItem>,
}

/// One leaf action or submenu in a script-authored workbench menu.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchMenuItem {
    pub label: String,
    pub tooltip: Option<String>,
    pub enabled: bool,
    pub action: Option<ScriptWorkbenchMenuAction>,
    pub children: Vec<ScriptWorkbenchMenuItem>,
}

/// A workbench menu leaf routed through the existing generic Rhai tool hook.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchMenuAction {
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
    /// Replace one provider's complete workbench menu contribution.
    WorkbenchMenus {
        /// Stable owner key; updates replace this provider.
        provider: String,
        /// Workspace Twin identity, or `None` for application assets.
        twin_id: Option<u64>,
        menus: Vec<ScriptWorkbenchMenu>,
    },
    /// Dispatch one selected workbench item through the Rhai tool registry.
    WorkbenchMenuAction {
        tool: String,
        hook: String,
        args: TelemetryValue,
    },
}
