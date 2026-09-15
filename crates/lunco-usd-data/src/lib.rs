//! Reusable, render-free authored USD data contracts.
//!
//! This package is the lower authored-layer boundary shared by document
//! authoring and composed-stage readers. It contains no document lifecycle,
//! journaling, schema registry, or runtime projection policy, so readers that
//! only need data access or USD convention conversion do not pull the full
//! `lunco-usd-document` package.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod metadata;
pub mod units;
pub mod usd_data;
