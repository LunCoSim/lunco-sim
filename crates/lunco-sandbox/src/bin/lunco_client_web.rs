//! LunCo Client — Web entry point.
//!
//! Web-optimized version of the full celestial simulation client.
//! Loads Earth, planets, orbital mechanics with UI and workbench docking.

use bevy::prelude::*;
use big_space::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub fn main() {}

#[cfg(not(target_arch = "wasm32"))]
pub fn main() {
    run();
}

/// Returns BigSpace plugins configured for the current platform.
fn get_big_space_plugins() -> impl bevy::prelude::PluginGroup {
    #[cfg(not(target_arch = "wasm32"))]
    {
        BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>()
    }
    #[cfg(target_arch = "wasm32")]
    {
        BigSpaceDefaultPlugins.build()
    }
}

/// Browser entry point. Called automatically when the WASM module loads.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub fn run() {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>().set(WindowPlugin {
            primary_window: Some(Window {
                title: "LunCo Client".into(),
                resolution: bevy::window::WindowResolution::new(1280, 720),
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                prevent_default_event_handling: true,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(get_big_space_plugins())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(lunco_celestial::CelestialPlugin)
        .add_plugins(lunco_environment::EnvironmentPlugin)
        .add_plugins(lunco_mobility::LunCoMobilityPlugin)
        .add_plugins(lunco_controller::LunCoControllerPlugin)
        .add_plugins(lunco_avatar::LunCoAvatarPlugin)
        .add_plugins(lunco_workbench::WorkbenchPlugin);

    // Transform/visibility propagation is owned entirely by big_space
    // (`BigSpaceDefaultPlugins` via `get_big_space_plugins`). The custom
    // `global_transform_propagation_system` that previously ran here —
    // twice per frame, in PreUpdate and PostUpdate — was removed
    // (2026-06-02): it fought big_space's propagation and corrupted
    // `GlobalTransform` on every entity (camera roll in surface mode),
    // mirroring the fix already applied to `main.rs` and `sandbox.rs`.

    #[cfg(not(target_arch = "wasm32"))]
    app.run();
}
