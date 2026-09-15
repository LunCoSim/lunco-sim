//! Backend-neutral avatar contracts.
//!
//! This package owns the ECS components and typed commands that describe an
//! avatar's camera and possession state. [`lunco-avatar`] owns the systems
//! that interpret those contracts; render, USD, networking, and scene-camera
//! packages can therefore share the contracts without compiling that system
//! implementation.

pub mod camera;
pub mod commands;
pub mod lifecycle;
