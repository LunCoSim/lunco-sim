//! Runtime command reflection and result storage.
//!
//! Pure mutation envelopes and session/result contracts live in
//! `lunco-command-contracts`; this module owns the Bevy-facing command marker,
//! reflected edit intent, and ECS result resources.

use crate::Command;
use bevy::ecs::reflect::ReflectEvent;
use bevy::prelude::{Reflect, Resource};
use bevy::reflect::std_traits::ReflectDefault;
use lunco_command_contracts::{Ack, Reject};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Reflected type marker emitted by [`crate::Command`].
///
/// The API discovery walk uses the same authoritative command abstraction as
/// local observers and Rhai. Keeping this marker in reflected type metadata
/// means commands defined by an external plugin are exposed without a crate
/// name heuristic or per-command registration table.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApiCommandMarker;
/// Whether a mutation command is a **persistent authored edit** (journaled →
/// synced → persisted) or a **transient interactive one** (live change only, not
/// journaled).
///
/// The default is [`Persistent`](Self::Persistent), so API / MCP / scripted
/// callers durably record by default ("journaled by default"). An interactive UI
/// opts into [`Interactive`](Self::Interactive) for a throwaway edit (a test /
/// preview) and sends `Persistent` only on commit.
///
/// This is the explicit form of the interactive/persistent split for **discrete**
/// dual-meaning actions (e.g. `DetachJoint`: interactively pop a joint to test vs.
/// author the scene to have it removed). *Continuous* manipulation (gizmo drag,
/// slider scrub) doesn't need this flag — it uses the `persist_*_to_runtime_layer`
/// observer pattern: the live edit is the interactive form, and a deferred
/// observer journals the committed result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum EditIntent {
    /// Live change only — NOT journaled / synced / persisted. Real-time
    /// manipulation and previews.
    Interactive,
    /// The committed authored edit — journaled, synced, persisted. **Default.**
    #[default]
    Persistent,
}

impl EditIntent {
    /// Does this edit get recorded to the Twin journal (and thus synced/persisted)?
    pub fn is_persistent(self) -> bool {
        matches!(self, EditIntent::Persistent)
    }
}

// ── Internal command outcomes ─────────────────────────────────────────────
//
// A command invoked through a transport (HTTP API, MCP, future wire) gets a
// request id and, if its observer reports one, a terminal outcome the caller
// can poll for. This is the deliberately-minimal model: robotics practice
// (F′ response codes, MAVLink `COMMAND_ACK`, behaviour-tree SUCCESS/FAILURE/
// RUNNING) converges on *one result code + an in-progress state*, not XTCE's
// multi-stage ground-verification pipeline. Richer lifecycles (queued,
// progress, cancel) stay as per-domain state where they already live
// (e.g. experiments' `RunStatus`), not promoted into this substrate.
//
// Distinctions kept (and only these):
// - `Rejected` (never ran — validation/auth/dedup) vs `Failed` (ran, errored):
//   the caller reverts an optimistic edit on `Rejected`, not on `Failed`
//   (MAVLink's `DENIED` vs `FAILED`).
// - `Pending`: accepted, terminal not yet known (async/long-running). MVP
//   handlers are synchronous and never leave a result `Pending`.

/// Terminal (or in-flight) state of a command invocation, keyed by request id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommandOutcome {
    /// Accepted; the observer hasn't reported a terminal state yet.
    Pending,
    /// Ran successfully; carries the [`Ack`] (new generation, optional data).
    Succeeded(Ack),
    /// Never ran — rejected before/at validation. Client should revert.
    Rejected(Reject),
    /// Ran and errored. Client should not revert.
    Failed(String),
}

/// Maximum retained internal outcomes; oldest are evicted FIFO. A simple cap
/// (not a wall-clock TTL) avoids `Instant`/time on wasm and keeps the store
/// bounded.
const MAX_COMMAND_RESULTS: usize = 1024;

/// Internal store of command outcomes, keyed by an in-process command id.
/// Always-on substrate — initialised by `register_core_resources` so
/// result-reporting observers cannot panic on a missing resource. Transport
/// handlers do not expose this store; deferred transport commands answer via
/// their original request correlation.
#[derive(Resource, Default)]
pub struct CommandResults {
    map: HashMap<u64, CommandOutcome>,
    order: VecDeque<u64>,
}

impl CommandResults {
    /// Insert or overwrite an outcome, evicting the oldest entries past the cap.
    pub fn insert(&mut self, id: u64, outcome: CommandOutcome) {
        if !self.map.contains_key(&id) {
            self.order.push_back(id);
        }
        self.map.insert(id, outcome);
        while self.order.len() > MAX_COMMAND_RESULTS {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    /// Record a legacy string-error result as a terminal handler outcome.
    pub fn record(&mut self, id: u64, result: Result<Ack, String>) {
        self.record_handler_result(id, result);
    }

    /// Record a command handler's declared result type as a terminal outcome.
    pub fn record_handler_result(&mut self, id: u64, result: impl Into<CommandOutcome>) {
        self.insert(id, result.into());
    }

    pub fn get(&self, id: u64) -> Option<&CommandOutcome> {
        self.map.get(&id)
    }
}

impl From<Result<Ack, String>> for CommandOutcome {
    fn from(result: Result<Ack, String>) -> Self {
        match result {
            Ok(ack) => Self::Succeeded(ack),
            Err(message) => Self::Failed(message),
        }
    }
}

impl From<Result<Ack, Reject>> for CommandOutcome {
    fn from(result: Result<Ack, Reject>) -> Self {
        match result {
            Ok(ack) => Self::Succeeded(ack),
            Err(rejection) => Self::Rejected(rejection),
        }
    }
}

#[cfg(test)]
mod command_result_tests {
    use super::*;

    #[test]
    fn handler_results_preserve_failure_and_rejection_classes() {
        let mut results = CommandResults::default();
        results.record_handler_result(1, Err::<Ack, _>("handler failed".to_owned()));
        results.record_handler_result(
            2,
            Err::<Ack, _>(Reject::InvalidOp("input was rejected".to_owned())),
        );

        assert!(matches!(
            results.get(1),
            Some(CommandOutcome::Failed(message)) if message == "handler failed"
        ));
        assert!(matches!(
            results.get(2),
            Some(CommandOutcome::Rejected(Reject::InvalidOp(message)))
                if message == "input was rejected"
        ));
    }
}

/// Origin of a reflected command entering the typed command dispatcher.
/// Direct Bevy event triggers have no origin unless their producer routes them
/// through an explicitly classified command boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandOrigin {
    /// HTTP, MCP, or another external API transport submitted this command.
    ApiTransport,
    /// A Rhai evaluation emitted this command. Scenario calls retain their
    /// stable actor identity; application-level evaluations have no actor.
    Rhai {
        /// Typed owner context for the Rhai invocation.
        context: crate::RuntimeExecutionContext,
        /// Stable source actor when the call came from a Twin program.
        actor: Option<crate::GlobalEntityId>,
    },
    /// Local interaction dispatched by a user-facing workbench surface.
    LocalUser {
        /// Session that owns the interaction.
        session_id: lunco_command_contracts::SessionId,
    },
}

/// The request id and origin of the command currently being dispatched. The
/// API dispatcher scopes these facts around the reflected command trigger so
/// command observers can record outcomes and input provenance.
#[derive(Resource, Default, Clone)]
pub struct ActiveCommandId {
    id: Option<u64>,
    origin: Option<CommandOrigin>,
}

impl ActiveCommandId {
    pub fn get(&self) -> Option<u64> {
        self.id
    }

    /// Origin of the currently dispatched reflected command, if classified.
    pub fn origin(&self) -> Option<CommandOrigin> {
        self.origin
    }

    /// Set the command result id without an origin. Used by direct in-process
    /// result tests and command producers that do not use `ApiCommandEvent`.
    pub fn set(&mut self, id: Option<u64>) {
        self.id = id;
        self.origin = None;
    }

    /// Scope a reflected command id and its classified producer origin.
    pub fn set_with_origin(&mut self, id: u64, origin: Option<CommandOrigin>) {
        self.id = Some(id);
        self.origin = origin;
    }

    /// Scope a local interaction origin without creating a request id.
    pub fn set_origin(&mut self, origin: Option<CommandOrigin>) {
        self.id = None;
        self.origin = origin;
    }
}

/// Commands a **client-targeted script** is allowed to issue — the presentation /
/// client-local surface (HUD, notifications, camera framing), which only ever
/// mutate *this peer's* view and never authoritative sim state.
///
/// A predicting client must not run scripts that mutate shared state (they'd
/// double-apply / fight replication), so scripting blocks a client-targeted
/// scenario's `cmd()` calls by default (deny-all). A command opts INTO the
/// client-local surface by name via [`MarkClientLocalExt::mark_client_local`],
/// contributed by the command's OWN crate at plugin build — so the classification
/// stays a dynamic registry, not a hardcoded list, and no low crate has to depend
/// on a UI crate to know a HUD command is client-local.
///
/// Keyed by `short_type_path` (the same string `cmd("Name", …)` dispatches on and
/// [`declare_channel`] keys the wire router on).
#[derive(Resource, Default)]
pub struct ClientCommandPolicy {
    client_local: std::collections::HashSet<String>,
}

impl ClientCommandPolicy {
    /// Register a command name as safe for a client-targeted script to issue.
    pub fn allow(&mut self, name: impl Into<String>) {
        self.client_local.insert(name.into());
    }
    /// True if a client-targeted script may issue the command named `name`.
    pub fn allows(&self, name: &str) -> bool {
        self.client_local.contains(name)
    }
    /// The command names currently on the client-local surface. Lets the wire
    /// layer cross-check the client-local ⊆ non-networked invariant at startup
    /// (a client-scriptable command must never ride a networked channel).
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.client_local.iter().map(String::as_str)
    }
}

/// App extension: mark a command type as **client-local** — safe for a
/// client-targeted script to issue (see [`ClientCommandPolicy`]). Call it from the
/// plugin of the crate that DEFINES the command, next to its `register_command`,
/// so the client-local surface is assembled from each crate's own declarations.
pub trait MarkClientLocalExt {
    fn mark_client_local<C: bevy::reflect::TypePath>(&mut self) -> &mut Self;
}

impl MarkClientLocalExt for bevy::app::App {
    fn mark_client_local<C: bevy::reflect::TypePath>(&mut self) -> &mut Self {
        if !self.world().contains_resource::<ClientCommandPolicy>() {
            self.init_resource::<ClientCommandPolicy>();
        }
        self.world_mut()
            .resource_mut::<ClientCommandPolicy>()
            .allow(C::short_type_path().to_string());
        self
    }
}

/// Spawn an independent entity from the catalog at a given world position.
///
/// **Why the type lives in `lunco-core` and the handler does not.** `SpawnEntity`
/// is a *wire* command: `lunco-networking` declares its channel
/// (`declare_channel::<SpawnEntity>`), which needs nothing but the type. The
/// handler (`on_spawn_entity_command`) lives with the catalog it spawns from, in
/// `lunco-scene-commands`. Keeping the *definition* here is what lets the networking crate
/// drop its dependency on the 13.4k-LOC editor — an edge that used to drag the
/// whole editor closure (→ modelica → workspace → doc-bevy) into every networking
/// build for exactly two symbols (review A6).
///
/// `reflect_default` semantics: API/rhai callers may omit optional fields — a
/// missing `rotation` defaults to `None` (→ identity). Position is always
/// expressed in the current semantic physics frame; callers
/// never pass a Bevy grid entity or perform BigSpace hierarchy conversion
/// themselves.
#[Command(reflect_default)]
pub struct SpawnEntity {
    /// The independent catalog entry ID (e.g. "ball_dynamic", "skid_rover").
    pub entry_id: String,
    /// Position in the active physics frame, in metres. Kept as f64 through
    /// command transport and frame conversion; narrowing occurs only at the
    /// final scene-root-local Bevy `Transform` boundary.
    pub position: [f64; 3],
    /// Rotation in the active physics frame as an `(x, y, z, w)` unit
    /// quaternion (optional; omitted → identity). Kept as f64 across the
    /// command boundary for the same reason as `position`; Bevy's f32
    /// [`bevy::prelude::Quat`] is a render/local-transform representation, not a
    /// simulation-frame interchange type.
    pub rotation: Option<[f64; 4]>,
    /// Stable producer identity required for raw-file runtime admission from
    /// API, direct typed, and actorless Rhai callers. Twin Rhai uses its actor
    /// identity and omits this field.
    pub producer_id: Option<u64>,
}

impl Default for SpawnEntity {
    fn default() -> Self {
        Self {
            entry_id: String::new(),
            position: [0.0; 3],
            rotation: None,
            producer_id: None,
        }
    }
}
