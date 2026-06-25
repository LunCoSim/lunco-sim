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
//! See [docs/architecture/21-domain-usd.md](../../docs/architecture/21-domain-usd.md) for detailed architecture documentation.

use bevy::prelude::*;

// `commands` (document/file verbs) + `ui` (browser/viewport panels) are the
// egui + workbench-shell layer — UI only. `document`/`registry`/`text_edit` are
// egui-free (USD doc model + the core document-lifecycle events).
#[cfg(feature = "ui")]
pub mod commands;
pub mod document;
pub mod registry;
pub mod text_edit;
#[cfg(feature = "ui")]
pub mod ui;

#[cfg(feature = "ui")]
pub use commands::{ApplyUsdOp, UsdCommandsPlugin, USD_DOCUMENT_KIND};
pub use document::{LayerId, UsdChange, UsdDocument, UsdOp};
pub use registry::UsdDocumentRegistry;
pub use lunco_usd_bevy::{FallbackSceneLight, UsdAuthoredLight, UsdPrimPath, UsdStageAsset};
pub use lunco_usd_avian::UsdAvianPlugin;
pub use lunco_usd_sim::UsdSimPlugin;
pub use lunco_usd_sim::cosim::{ClearScene, LoadScene};

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
        app.add_plugins((
            lunco_usd_bevy::UsdBevyPlugin,
            UsdAvianPlugin,
            UsdSimPlugin,
        ));
        // Document/file commands (OpenFile/NewDocument/SaveDocument + the
        // viewport-placeholder/twin-doc observers) pull the egui workbench —
        // UI only. The server triggers `LoadScene` directly (handled by
        // UsdSimPlugin), so it doesn't need these.
        #[cfg(feature = "ui")]
        app.add_plugins(UsdCommandsPlugin);
    }
}
