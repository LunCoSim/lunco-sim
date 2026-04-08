//! # LunCoSim USD System
//!
//! Loads rover definitions from USD (Universal Scene Description) files and maps them to
//! Bevy entities with Avian3D physics and LunCoSim simulation components.
//!
//! ## Architecture
//!
//! The system consists of three cooperating plugins:
//!
//! - **UsdBevyPlugin** — Spawns child entities for USD prims, attaches meshes + transforms
//! - **UsdAvianPlugin** — Maps USD physics attributes to Avian3D components
//! - **UsdSimPlugin** — Detects simulation schemas and creates wheel/FSW/steering components
//!
//! All three use deferred processing systems that run in the `Update` schedule **after**
//! `sync_usd_visuals`, ensuring assets are fully loaded before any component mapping.
//!
//! See [docs/USD_SYSTEM.md](../../docs/USD_SYSTEM.md) for detailed architecture documentation.

use bevy::prelude::*;

pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
pub use lunco_usd_avian::UsdAvianPlugin;
pub use lunco_usd_sim::UsdSimPlugin;

/// Master plugin that bundles all USD subsystems together.
///
/// Add this single plugin to your app to enable USD asset loading and simulation mapping:
///
/// ```ignore
/// app.add_plugins(UsdPlugins);
/// ```
///
/// This is equivalent to adding all three subsystems individually:
/// - `UsdBevyPlugin` — visual sync (meshes, transforms, hierarchy)
/// - `UsdAvianPlugin` — physics mapping (RigidBody, Collider, Mass, Damping)
/// - `UsdSimPlugin` — simulation mapping (WheelRaycast, FSW, DifferentialDrive)
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((lunco_usd_bevy::UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));
    }
}
