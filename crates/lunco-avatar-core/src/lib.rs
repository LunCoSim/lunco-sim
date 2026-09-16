//! Backend-neutral avatar commands and lifecycle contracts.
//!
//! This package owns the avatar ECS role components, typed commands, and transient
//! notification contracts that describe avatar-facing state. Reusable camera state lives in
//! [`lunco-camera-core`]; [`lunco-avatar`] owns the systems that interpret both
//! contracts. Render, USD, networking, and scene-camera packages can therefore
//! share the focused contract package they need while the avatar system
//! implementation remains in [`lunco-avatar`].

pub mod commands;
pub mod lifecycle;
pub mod notifications;
pub mod roles;
