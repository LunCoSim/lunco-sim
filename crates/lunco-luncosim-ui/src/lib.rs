//! Interactive UI shell for the LunCoSim application.
//!
//! This crate owns application-specific presentation and editor composition:
//! egui workbench panels, the interactive USD viewport, tutorial menu, and
//! windowed-only update surface. The simulator core does not depend on these
//! modules, so editing the UI does not rebuild the headless application core.

mod application;
#[cfg(feature = "api-transport")]
mod offscreen;
mod presentation_bridge;
mod save_scenario;
mod terrain_horizon;
mod ui;
#[cfg(all(feature = "networking", not(target_family = "wasm")))]
mod url_scheme;

pub use application::run_gui;
#[cfg(feature = "api-transport")]
pub use offscreen::LunCoSimOffscreenPlugin;
pub(crate) use save_scenario::SaveScenario;

/// Register UI-facing projections after the headless-safe domain plugins.
pub fn register_presentation_bridges(app: &mut bevy::prelude::App) {
    presentation_bridge::register(app);
}
#[cfg(feature = "package-icons")]
pub use ui::WindowIconBytes;
pub use ui::{add_runtime_ui_layer, InitialScenePath, LunCoSimUiConfig, LunCoSimUiPlugin};

/// Run the native package-manager startup hook before CLI dispatch.
///
/// The application binary calls this at process start when its UI composition
/// enables the `updates` feature. Keeping the hook beside the update surface
/// gives the updater one owner and lets headless/UI-shell consumers omit the
/// Velopack dependency entirely.
#[cfg(all(feature = "updates", not(target_arch = "wasm32")))]
pub fn initialize_velopack() {
    velopack::VelopackApp::build().run();
}

/// Rasterized 64x64 RGBA bytes for the packaged native LunCoSim window icon.
#[cfg(feature = "package-icons")]
pub fn window_icon_bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/luncosim-icon.rgba"))
}

pub(crate) fn register_save_scenario_command(app: &mut bevy::prelude::App) {
    save_scenario::register_all_commands(app);
}
