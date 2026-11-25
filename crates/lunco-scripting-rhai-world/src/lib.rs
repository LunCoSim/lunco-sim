//! Reusable Rhai world and policy runtime.
//!
//! This package owns the high-churn bridge between Rhai and the live Bevy
//! world, the language-neutral scenario driver implementation, authored policy
//! activation, and optional Twin-scoped native hook providers. Application
//! composition, command registration, tools, and timeline persistence remain
//! in [`lunco-scripting-rhai-runtime`](../lunco-scripting-rhai-runtime).

#[cfg(not(target_arch = "wasm32"))]
mod http;
pub mod mission_progress;
#[cfg(feature = "native-plugins")]
pub mod native_plugins;
pub mod policy;
pub mod public_state;
pub mod source_asset;
pub mod tool_libs;
pub mod world_bridge;
