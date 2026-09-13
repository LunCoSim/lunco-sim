//! # LunCoSim USD System
//!
//! Loads rover definitions from USD (Universal Scene Description) files and maps them to
//! Bevy entities with Avian3D physics and LunCoSim simulation components.
//!
//! ## Architecture
//!
//! This package owns UI-free USD runtime orchestration and document commands.
//! Complete application composition lives in `lunco-usd-bevy-runtime`, while
//! individual visual, physics, and simulation projections remain independently
//! installable.

// `commands` is the headless-safe document/file verb layer (ApplyUsdOp,
// OpenFile/NewDocument/SaveDocument observers, the async load pipeline +
// twin-scene resolver). The browser and viewport presentation lives in
// `lunco-usd-ui`; `document` is the USD document model and the shared
// `DocumentRegistry<UsdDocument>` owns document identity. Edits author through
// OpenUSD's Stage by SDF path (`lunco_usd_core::author`).
pub mod assembly_api;
pub mod commands;
pub mod live_consume;
pub(crate) mod program_runtime;
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
/// Asset-backed OpenUSD assembly. This is the public composition boundary:
/// `lunco-assets` supplies canonical identities and bytes, while this crate
/// interprets USD sublayers, references, payloads, and variants into a stage.
#[cfg(not(target_arch = "wasm32"))]
pub use lunco_usd_compose::compose_file_to_stage;
