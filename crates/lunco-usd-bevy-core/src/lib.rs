//! Headless composed-USD substrate shared by visual and domain projections.
//!
//! This package owns the reader contract, live composed view, send-safe
//! projection snapshot, composition entry points, and USD-only policy helpers.
//! It deliberately contains no mesh, light, camera, window, renderer, or UI
//! projection. `lunco-usd-bevy` is the visual adapter built on this substrate.

pub mod animation;
pub mod live_edit;
pub mod mount;
pub mod point_instancer;
pub mod program;
