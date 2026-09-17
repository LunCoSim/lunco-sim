//! Backend-neutral local and replicated embodiment contracts.
//!
//! An embodiment is an entity that can be presented as the local operator or a
//! replicated remote operator. It is deliberately independent of cameras,
//! vessels, input devices, physics, and application UI. Embodiment, AI operator,
//! inspection, and networking packages compose these roles with their own
//! behavior contracts.

pub mod roles;
