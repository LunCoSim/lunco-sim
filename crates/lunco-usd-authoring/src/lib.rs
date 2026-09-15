//! OpenUSD authoring helpers and schema metadata.
//!
//! The package owns operations over a single authored layer's `sdf::Data`,
//! schema declarations, and conversion to/from USDA. It deliberately does not
//! own document identity, journaling, or runtime composition.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod author;
pub mod schema;
