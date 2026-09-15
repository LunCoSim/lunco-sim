//! Headless authored OpenUSD document substrate.
//!
//! This package owns the send-safe authored layer model, typed USD document
//! operations, OpenUSD authoring helpers, schema metadata, unit conventions,
//! and stage recipes. Runtime projection and operation policy live in the
//! packages that consume these contracts; this package does not install Bevy
//! systems or UI.

pub mod author;
pub mod document;
pub mod metadata;
pub mod recipe;
pub mod schema;
pub mod units;
pub mod usd_data;
