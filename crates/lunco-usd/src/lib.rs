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
//! - **UsdSimPlugin** — Detects simulation schemas and creates wheel/FSW/joint components
//!
//! All three use deferred processing systems that run in the `Update` schedule **after**
//! `sync_usd_visuals`, ensuring assets are fully loaded before any component mapping.
//!
//! See [docs/architecture/21-domain-usd.md](../../docs/architecture/21-domain-usd.md) for detailed architecture documentation.

use bevy::prelude::*;

// `commands` is the headless-safe document/file verb layer (ApplyUsdOp,
// OpenFile/NewDocument/SaveDocument observers, the async load pipeline +
// twin-scene resolver). The browser and viewport presentation lives in
// `lunco-usd-ui`; `document` is the USD document model and the shared
// `DocumentRegistry<UsdDocument>` owns document identity. Edits author through
// OpenUSD's Stage by SDF path (`lunco_usd_core::author`).
pub mod assembly_api;
pub mod commands;
pub mod live_consume;
/// Lowering a material edit into a real UsdShade network (`Material` +
/// `UsdPreviewSurface` + `material:binding`). Crate-agnostic op builder — the
/// Inspector, the command API and scripting all author materials through it, so
/// none of them can reinvent the non-standard "shader inputs on a geom prim"
/// spelling.
pub mod registry;
pub mod runtime_persistence;
pub mod twin_projection;

pub use commands::{
    ApplyUsdOp, ApplyUsdOps, AttachProgram, CommitUsdProposal, CreateUsdProposal,
    ReviewUsdProposal, UsdCommandsPlugin, UsdProposalReviewAction, USD_DOCUMENT_KIND,
};
// The document and operation model is owned by `lunco-usd-core`; this crate
// exposes runtime integration and command/plugin APIs.
pub use lunco_usd_avian::{
    BigSpacePhysicsBridgePlugin, ShouldBeDynamic, UsdAvianPlugin, UsdCollisionFilter,
};
pub use lunco_usd_bevy::{UsdAuthoredLight, UsdPrimPath};
/// Asset-backed OpenUSD assembly. This is the public composition boundary:
/// `lunco-assets` supplies canonical identities and bytes, while this crate
/// interprets USD sublayers, references, payloads, and variants into a stage.
#[cfg(not(target_arch = "wasm32"))]
pub use lunco_usd_compose::compose_file_to_stage;
pub use lunco_usd_sim::cosim::{ClearScene, LoadScene, RestartScene};
pub use lunco_usd_sim::UsdSimPlugin;
pub use lunco_usd_sim::{GroundColliderPending, UsdSimProcessed};

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
/// - `UsdSimPlugin` — simulation mapping (WheelRaycast, FSW, authored ports)
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((lunco_usd_bevy::UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));
        // Document/file commands (ApplyUsdOp + OpenFile/NewDocument/SaveDocument
        // observers + the async load pipeline + twin-scene resolver) are
        // headless-safe domain-layer wiring. The egui browser/viewport panels
        // are installed separately by `lunco-usd-ui`.
        app.add_plugins(UsdCommandsPlugin);
    }
}
