//! Manifest-aware asset download and atomic installation.
//!
//! HTTP retry and resumable byte transfer live in [`lunco_assets_transport`]
//! so lightweight networking consumers do not compile manifest extraction or
//! archive support.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::disallowed_methods)]

pub mod download;
