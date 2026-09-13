//! Headless OpenUSD document and authoring substrate.
//!
//! This crate owns the serializable USD layer model, typed mutations, schema
//! metadata, authoring helpers, and pure operation lowerings. It deliberately
//! knows nothing about Bevy runtime entities, rendering, physics, simulation,
//! or UI. [`lunco-usd`](../lunco-usd) owns those runtime integrations.

pub mod attach;
pub mod author;
pub mod commands;
pub mod document;
pub mod edit_session;
pub mod material;
pub mod metadata;
pub mod program;
pub mod recipe;
pub mod schema;
pub mod units;
pub mod usd_data;

/// Send-safe authored USD layer data used by document and authoring APIs.
pub type UsdData = openusd::sdf::Data;

pub use document::{LayerId, UsdChange, UsdDocument, UsdOp, UsdReferenceArc, UsdReferenceListOp};
pub use metadata::AttrUiHint;
pub use recipe::StageRecipe;
pub use units::{
    ConventionTransform, StageMetadataReader, StageMetrics, StageMetricsError, UpAxis,
};
pub use usd_data::UsdDataExt;
