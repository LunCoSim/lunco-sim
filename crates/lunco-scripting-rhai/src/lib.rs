//! Rhai authoring and discovery surfaces.
//!
//! The world bridge and scenario mechanics live in
//! [`lunco-scripting-rhai-runtime`]. This package owns the API-facing surfaces that
//! describe and diagnose that runtime: catalog discovery, script diagnostics,
//! and dataset queries. Keeping these query providers in a separate production
//! package means editor/query changes do not invalidate the language-neutral
//! scripting crate.

pub mod catalog;
pub mod dataset_queries;
pub mod diagnostics;

use bevy::prelude::*;

/// Installs the Rhai authoring/query providers.
pub struct LunCoScriptingRhaiPlugin;

impl Plugin for LunCoScriptingRhaiPlugin {
    fn build(&self, app: &mut App) {
        diagnostics::register_queries(app);
        dataset_queries::register_queries(app);
        catalog::register_queries(app);
    }
}
