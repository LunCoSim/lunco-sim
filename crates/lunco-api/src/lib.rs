//! # lunco-api — Transport-Agnostic API Core
//!
//! ECS runtime for reflected commands, discovery, entity identity, and
//! telemetry subscriptions. Typed contracts live in `lunco-api-core`, query
//! providers in `lunco-api::queries`, and JSON conversion in the transport codec.
//!
//! ## Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────┐
//! │  lunco-api (transport-agnostic core)                           │
//! │                                                                │
//! │  ApiRegistry   — stable GlobalEntityId ↔ Bevy Entity mapping   │
//! │  ApiExecutor   — ApiRequest → ECS (typed commands, Reflect)    │
//! │  ApiDiscovery  — schema introspection via TypeRegistry         │
//! │  ApiTelemetry  — telemetry subscription + broadcast            │
//! │                                                                │
//! │  Typed API runtime; no wire-format values cross this boundary   │
//! └────────────────────────┬───────────────────────────────────────┘
//!                          │
//!                          ▼
//! ┌────────────────────────────────────────────────────────────────┐
//! │  ECS World                                                     │
//! │  Typed commands (#[Command]) · Resources                        │
//! └────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Principles
//!
//! - **No hardcoded commands**: Commands are discovered via `AppTypeRegistry`
//!   reflection. A host-registered `#[Command]` type is automatically available;
//!   arbitrary reflected internal events are not.
//! - **No hardcoded entity types**: entity identity comes from the runtime
//!   registry; domain-specific reads expose their own typed query providers.
//! - **Typed boundary**: execution and in-process providers use `ApiValue`;
//!   JSON conversion lives in the transport codec.
//! - **Headless-compatible**: No rendering dependencies. Runs on server-only builds.

use bevy::prelude::*;

pub mod discovery;
pub mod executor;
pub mod queries;
pub mod registry;
pub mod session;
pub mod subscription;

// Re-export public types for convenience
pub use discovery::*;
pub use executor::*;
pub use queries::*;
pub use registry::*;
pub use subscription::*;

/// Add `plugin` only if a plugin of the same type isn't already present.
/// `Plugin` is unique by default and a duplicate `add_plugins` panics, so this
/// keeps the transport-free core composable across the API transport plugin and
/// `LunCoScriptingRhaiRuntimePlugin` (which both want it) regardless of add order.
pub fn add_plugin_once<P: Plugin>(app: &mut App, plugin: P) {
    if !app.is_plugin_added::<P>() {
        app.add_plugins(plugin);
    }
}

/// Ensure the **transport-free command core** — the reflect-based command
/// dispatcher ([`ApiExecutorPlugin`]) and entity-id registry
/// ([`ApiEntityRegistryPlugin`]) — is present, without pulling any transport
/// (no HTTP server, no `LunCoApiPlugin`). This is the seam that lets the
/// scripting substrate run `cmd()` **independently of the API**: an app can add
/// `LunCoScriptingRhaiRuntimePlugin` alone and scripts still dispatch every
/// `#[Command]`.
/// Idempotent — safe to call from both plugins.
pub fn ensure_command_core(app: &mut App) {
    add_plugin_once::<ApiExecutorPlugin>(app, ApiExecutorPlugin);
    add_plugin_once::<ApiEntityRegistryPlugin>(app, ApiEntityRegistryPlugin);
}
