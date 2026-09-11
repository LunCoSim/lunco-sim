//! Interactive UI shell for the LunCoSim application.
//!
//! This crate owns application-specific presentation and editor composition:
//! egui workbench panels, the interactive USD viewport, tutorial menu, and
//! windowed-only update surface. The simulator core does not depend on these
//! modules, so editing the UI does not rebuild the headless application core.

mod save_scenario;
mod ui;

pub(crate) use save_scenario::SaveScenario;
pub use ui::{
    add_runtime_ui_layer, InitialScenePath, LunCoSimUiConfig, LunCoSimUiPlugin, WindowIconBytes,
};

/// Rasterized 64x64 RGBA bytes for the native LunCoSim window icon.
pub fn window_icon_bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/luncosim-icon.rgba"))
}

pub(crate) fn register_save_scenario_command(app: &mut bevy::prelude::App) {
    save_scenario::register_all_commands(app);
}
