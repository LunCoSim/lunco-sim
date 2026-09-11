//! Shared status and tracked-work infrastructure.
//!
//! This crate owns the renderer-independent status state machine used by
//! loading indicators, diagnostics, console projections, the workbench shell,
//! and headless hosts. It deliberately contains no egui, renderer, or
//! workbench-shell dependency. The status bar is one consumer of this crate,
//! not its owner.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod status_bus;
pub mod tracked_task;

