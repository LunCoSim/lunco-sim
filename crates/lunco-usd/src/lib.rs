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

// `commands` is the headless-safe document/file verb layer (ApplyUsdOp,
// OpenFile/NewDocument/SaveDocument observers, the async load pipeline +
// twin-scene resolver) — egui-free, so server / sandbox / networking bins get
// the full USD document surface. Only the empty-viewport placeholder inside it
// is `ui`-gated. `ui` (browser/viewport panels) is the egui + workbench-shell
// layer. `document`/`registry` are the egui-free USD doc model. Edits author
// through openusd's Stage by SDF path (`lunco_usd_bevy::author`); the old
// `text_edit` byte-splicer and the `edit_target_spike` proof are gone now that
// Phase C2/C3 lands the real Stage-backed authoring.
pub mod commands;
pub mod document;
pub mod registry;
pub mod runtime_persistence;
#[cfg(feature = "ui")]
pub mod ui;

pub use commands::{ApplyUsdOp, UsdCommandsPlugin, USD_DOCUMENT_KIND};
pub use document::{LayerId, UsdChange, UsdDocument, UsdOp};
pub use registry::UsdDocumentRegistry;
pub use lunco_usd_bevy::{FallbackSceneLight, UsdAuthoredLight, UsdPrimPath, UsdStageAsset};
pub use lunco_usd_avian::UsdAvianPlugin;
pub use lunco_usd_sim::UsdSimPlugin;
pub use lunco_usd_sim::NoRenderVisuals;
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
        // Document/file commands (ApplyUsdOp + OpenFile/NewDocument/SaveDocument
        // observers + the async load pipeline + twin-scene resolver) are
        // headless-safe domain-layer wiring — added unconditionally so server /
        // sandbox / networking bins get the full USD document surface. Only the
        // egui browser/viewport panels (`UsdUiPlugin`) stay behind `ui`.
        app.add_plugins(UsdCommandsPlugin);
    }
}
