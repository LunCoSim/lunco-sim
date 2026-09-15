//! Headless OpenUSD operation, assembly, and edit-policy substrate.
//!
//! This crate owns typed USD operations, assembly plans, edit sessions, and
//! pure operation lowerings. The authored document and schema substrate lives
//! in `lunco-usd-document`. This crate deliberately knows nothing about Bevy
//! runtime entities, rendering, physics, simulation, or UI.

pub mod attach;
pub mod commands;
pub mod edit_session;
pub mod material;
pub mod program;
pub mod runtime;
