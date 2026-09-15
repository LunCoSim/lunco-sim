//! Headless SysML v2 integration for LunCoSim.
//!
//! The runtime crate owns document identity, reversible source edits, Bevy
//! asset loading, and lifecycle/journal integration. Mounted Twins
//! automatically open their indexed `.sysml`/`.kerml` sources through the
//! canonical `twin://` asset authority. Parsing and semantic extraction stay
//! in [`lunco_sysml_ast`], keeping the expensive language implementation out
//! of consumers that only need the document contract.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod document;
mod source_asset;
mod twin_source;

pub use document::{SysmlDocument, SysmlOp};
pub use source_asset::{SysmlSource, SysmlSourceAssetPlugin, SysmlSourceLoader};
pub use twin_source::{PendingSysmlSources, SYSML_TWIN_SOURCE_LOAD_FAILED};

use bevy::prelude::*;

/// Bevy composition for the SysML document/source boundary.
///
/// This plugin is intentionally presentation-free: a UI or server can add it
/// without pulling a renderer. It registers the `.sysml`/`.kerml` text asset
/// and the generic document registry; callers still choose when to open a
/// source through their normal Twin/asset workflow.
pub struct SysmlPlugin;

impl Plugin for SysmlPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SysmlSourceAssetPlugin)
            .init_resource::<lunco_doc_bevy::DocumentRegistry<SysmlDocument>>()
            .init_resource::<PendingSysmlSources>()
            .add_observer(twin_source::request_twin_sysml_sources)
            .add_observer(twin_source::release_twin_sysml_sources)
            .add_systems(
                Update,
                (
                    twin_source::mark_pending_sysml_sources,
                    twin_source::drain_pending_sysml_sources,
                    twin_source::drain_sysml_document_events,
                )
                    .chain(),
            )
            .add_observer(twin_source::sync_workspace_on_sysml_doc_opened)
            .add_observer(twin_source::sync_workspace_on_sysml_doc_closed)
            .add_observer(twin_source::sync_workspace_on_sysml_doc_changed)
            .add_observer(twin_source::sync_workspace_on_sysml_doc_saved);
    }
}
