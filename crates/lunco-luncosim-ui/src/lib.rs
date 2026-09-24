//! Interactive UI shell for the LunCoSim application.
//!
//! This crate owns application-specific presentation and editor composition:
//! egui workbench panels, the interactive USD viewport, Rhai-contributed
//! application menus, and windowed-only update surface. The simulator core does not depend on these
//! modules, so editing the UI does not rebuild the headless application runtime.

mod application;
mod camera;
#[cfg(feature = "api-transport")]
mod offscreen;
mod save_scenario;
mod ui;
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod url_scheme;

pub use application::run_gui;
#[cfg(feature = "api-transport")]
pub use offscreen::LunCoSimOffscreenPlugin;
pub(crate) use save_scenario::SaveScenario;
#[cfg(feature = "package-icons")]
pub use ui::WindowIconBytes;
pub use ui::{InitialScenePath, LunCoSimUiConfig, LunCoSimUiPlugin, add_runtime_ui_layer};

/// Rasterized 64x64 RGBA bytes for the packaged native LunCoSim window icon.
#[cfg(feature = "package-icons")]
pub fn window_icon_bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/luncosim-icon.rgba"))
}

pub(crate) fn register_save_scenario_command(app: &mut bevy::prelude::App) {
    save_scenario::register_all_commands(app);
}
