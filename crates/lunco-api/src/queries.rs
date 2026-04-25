//! API query providers — extension point for domain crates to expose
//! list / status endpoints without `lunco-api` taking direct deps on them.
//!
//! ## Why
//!
//! `lunco-api` already has built-in query variants (`ListEntities`,
//! `DiscoverSchema`, `QueryEntity`) that read ECS state and return JSON
//! synchronously. Adding bundled-model / Twin / MSL listing the same way
//! would require `lunco-api` to depend on `lunco-modelica` and
//! `lunco-workspace` — a layering inversion (those crates already depend
//! on `lunco-api` for the executor plugin).
//!
//! Instead, domain crates register an [`ApiQueryProvider`] at startup.
//! When an `ExecuteCommand` request arrives whose `command` matches a
//! registered provider name, the executor calls the provider with
//! `&mut World` access and returns its `ApiResponse` to the transport.
//! Reflect-registered Event commands (the existing fire-and-forget
//! pattern) are unaffected — they take the fallthrough path.
//!
//! ## Provider semantics
//!
//! - **Returns data**, unlike Reflect Event commands which return
//!   `command_accepted`. Use this trait when the caller needs a
//!   structured response.
//! - **Has `&mut World` access** — providers can read any resource and
//!   run any query they need.
//! - **Runs deferred** via `Commands::queue`, so providers execute on a
//!   later command flush, not synchronously inside the observer. This
//!   matches how `CaptureScreenshot` already works.
//!
//! ## Example (will land in P2)
//!
//! ```ignore
//! struct ListBundledProvider;
//! impl ApiQueryProvider for ListBundledProvider {
//!     fn name(&self) -> &'static str { "ListBundled" }
//!     fn execute(&self, _world: &mut World, _params: &serde_json::Value) -> ApiResponse {
//!         let bundled = lunco_modelica::bundled_models();
//!         ApiResponse::ok(serde_json::json!({ "bundled": bundled }))
//!     }
//! }
//!
//! // In a domain crate's plugin build:
//! app.world_mut()
//!     .resource_mut::<ApiQueryRegistry>()
//!     .register(ListBundledProvider);
//! ```

use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use crate::schema::ApiResponse;

/// One registered query — answers a typed request with structured data.
///
/// See module docs for the design rationale.
pub trait ApiQueryProvider: Send + Sync + 'static {
    /// Stable name matched against the `command` field of incoming
    /// `ExecuteCommand` requests. Convention: PascalCase verb-prefixed,
    /// e.g. `"ListBundled"`, `"MslStatus"`, `"ListOpenDocuments"`.
    fn name(&self) -> &'static str;

    /// Run the query against the ECS world. Returning an
    /// [`ApiResponse::Error`] is the right move when params don't
    /// validate or required state is missing.
    ///
    /// Providers MUST NOT block for long — the caller is waiting on a
    /// deferred HTTP response. Cap any blocking work at a few hundred
    /// milliseconds and prefer returning a "not ready yet" response over
    /// blocking on a background task.
    fn execute(
        &self,
        world: &mut World,
        params: &serde_json::Value,
    ) -> ApiResponse;
}

/// Registry of named query providers. Domain crates push impls here at
/// startup via [`Self::register`]; the executor consults it when an
/// `ExecuteCommand` request arrives.
///
/// Stored as Bevy `Resource` so domain plugins can mutate it during
/// `App::build`.
#[derive(Resource, Default)]
pub struct ApiQueryRegistry {
    providers: HashMap<String, Arc<dyn ApiQueryProvider>>,
}

impl ApiQueryRegistry {
    /// Register a provider. Last-writer-wins for duplicate names — the
    /// previous registration is dropped silently. Domain crates own
    /// their query namespaces so collisions in practice mean "you
    /// registered the same plugin twice."
    pub fn register<P: ApiQueryProvider>(&mut self, provider: P) {
        self.providers
            .insert(provider.name().to_string(), Arc::new(provider));
    }

    /// Look up a provider by name. Returns an `Arc` so the caller can
    /// drop the registry borrow before invoking `execute` (which needs
    /// `&mut World`).
    pub fn get(&self, name: &str) -> Option<Arc<dyn ApiQueryProvider>> {
        self.providers.get(name).cloned()
    }

    /// Names of every registered provider. Useful for debug-dumping the
    /// available query surface.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}

/// Plugin that adds the [`ApiQueryRegistry`] resource. Always installed
/// by [`crate::LunCoApiPlugin`]; domain crates do not need to add this
/// plugin themselves — they just mutate the registry.
pub struct ApiQueryRegistryPlugin;

impl Plugin for ApiQueryRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiQueryRegistry>();
    }
}

// ─── ApiVisibility ─────────────────────────────────────────────────────

/// Filter for which Reflect-registered commands are exposed via the
/// external API surface (HTTP transport, MCP `discover_schema`, etc.)
/// while keeping them fully reflectable, observable, and dispatchable
/// **within the app**.
///
/// ## Why a separate filter
///
/// The Bevy `AppTypeRegistry` is the single source of truth for
/// reflected types — every domain plugin's GUI panel, observer, and
/// (per AGENTS.md §4.1) UI command bindings rely on registration. We
/// can't gate sensitive surfaces by *not registering* them: that breaks
/// the in-app dispatch path the GUI itself uses.
///
/// Instead, registration stays unconditional and domain crates push
/// command names that should be hidden from external callers into
/// [`hidden_commands`]. The discovery and executor layers consult this
/// set:
///
/// - [`crate::discover_commands`] omits hidden names from
///   [`crate::ApiSchema`].
/// - The executor rejects hidden commands with
///   [`crate::ApiErrorCode::CommandNotFound`] — the same error a
///   typo'd command name produces, so the surface looks identical to
///   "the command does not exist" from outside.
///
/// ## Default policy
///
/// Empty by default — every Reflect-registered command is visible.
/// Domain crates that ship internal-by-default mutation surfaces add
/// their command names in their plugin `build`. CLI flags or other
/// runtime knobs can clear entries to opt those surfaces in.
///
/// Mutating this resource **after** the API server has started works —
/// future calls observe the new visibility — so a future
/// "live toggle from a privileged channel" feature is reachable
/// without re-architecting the gate.
#[derive(Resource, Default, Debug)]
pub struct ApiVisibility {
    /// Set of Reflect command short names that should be invisible to
    /// external API consumers. The name is the short type path
    /// (`"SetDocumentSource"`), matching what
    /// [`crate::ApiRequest::ExecuteCommand`]'s `command` field carries.
    pub hidden_commands: std::collections::HashSet<String>,
}

impl ApiVisibility {
    /// Hide a command from external API surface. Idempotent.
    pub fn hide(&mut self, name: impl Into<String>) {
        self.hidden_commands.insert(name.into());
    }

    /// Reveal a previously-hidden command. Idempotent — no-op if the
    /// name was never hidden.
    pub fn reveal(&mut self, name: &str) {
        self.hidden_commands.remove(name);
    }

    /// True when the command is hidden from external callers.
    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden_commands.contains(name)
    }
}

/// Plugin that adds the [`ApiVisibility`] resource. Always installed by
/// [`crate::LunCoApiPlugin`].
pub struct ApiVisibilityPlugin;

impl Plugin for ApiVisibilityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiVisibility>();
    }
}
