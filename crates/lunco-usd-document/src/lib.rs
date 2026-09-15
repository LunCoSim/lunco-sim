//! Headless authored OpenUSD document substrate.
//!
//! This package owns the send-safe authored document model and typed USD
//! document operations. Reusable authored data/convention types live in
//! `lunco-usd-data`, OpenUSD authoring and schema helpers in
//! `lunco-usd-authoring`, and stage recipes in `lunco-usd-compose`. Runtime
//! projection and operation policy live in the packages that consume these
//! contracts; this package does not install Bevy systems or UI.

pub mod document;
