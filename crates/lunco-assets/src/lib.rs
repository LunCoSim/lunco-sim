//! Native dataset provisioning runtime.
//!
//! The lightweight asset identity, source, and storage APIs live in
//! [`lunco-assets-core`]. Dataset declarations, registry state, and lifecycle
//! commands live in [`lunco-assets-datasets`]. This package owns only the
//! worker lifecycle that composes the explicit `lunco-assets-download` and
//! `lunco-assets-processing` operations. Keep ordinary runtime consumers on
//! the lighter registry and transport packages; add this package only at an
//! application boundary that provisions data.

#![allow(clippy::disallowed_methods)]

pub mod datasets;
