//! OpenUSD light and environment projection for Bevy.
//!
//! This package owns the UsdLux light readers, authored-light markers, and
//! textured dome projection. It depends on the render-free composed USD
//! substrate and is installed by `lunco-usd-bevy`; mesh, material, and stage
//! projection code does not live here.

pub mod dome;
pub mod light;

use bevy::prelude::*;

/// Installs USD light projection and textured dome environment systems.
pub struct UsdLightPlugin;

impl Plugin for UsdLightPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(light::on_usd_light_added)
            .add_plugins(dome::DomePlugin);
    }
}
