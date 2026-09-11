//! Production validation for authored assets, loaded USD stages, and Twins.
//!
//! This is a separate runtime crate from [`lunco-scene-commands`]. The command
//! layer owns scene mutation and read verbs; this crate owns the comparatively
//! heavy parse, composition, lint-fact, and Twin namespace inspection paths.
//! Keeping the boundaries separate means changing a spawn command does not
//! recompile validation code, while the headless application can still install
//! the same validation plugin and CLI entry point.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod lint_command;
pub mod twin_lint;
pub mod validate;
