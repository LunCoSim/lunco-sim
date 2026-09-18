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

    /// Record a handler's `Result<Ack, String>` as a terminal outcome.
    /// `Ok` → [`CommandOutcome::Succeeded`], `Err` → [`CommandOutcome::Failed`]
    /// (a handler that ran and errored — not a pre-execution `Rejected`).
    pub fn record(&mut self, id: u64, result: Result<Ack, String>) {
        let outcome = match result {
            Ok(ack) => CommandOutcome::Succeeded(ack),
            Err(msg) => CommandOutcome::Failed(msg),
        };
        self.insert(id, outcome);
    }

    pub fn get(&self, id: u64) -> Option<&CommandOutcome> {
        self.map.get(&id)
    }
}

/// The request id of the command currently being dispatched, set by the
/// transport dispatcher immediately around the observer trigger so the
/// `#[on_command]` wrapper can record its outcome under the right id.
/// `None` for in-process triggers (UI `commands.trigger`) — those aren't
/// polled, so their result handlers simply don't record.
#[derive(Resource, Default)]
pub struct ActiveCommandId(Option<u64>);

impl ActiveCommandId {
    pub fn get(&self) -> Option<u64> {
        self.0
    }
    pub fn set(&mut self, id: Option<u64>) {
        self.0 = id;
    }
}

/// Commands a **client-scoped script** is allowed to issue — the presentation /
/// client-local surface (HUD, notifications, camera framing), which only ever
/// mutate *this peer's* view and never authoritative sim state.
///
/// A predicting client must not run scripts that mutate shared state (they'd
/// double-apply / fight replication), so scripting blocks a client-scoped
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
    /// Register a command name as safe for a client-scoped script to issue.
    pub fn allow(&mut self, name: impl Into<String>) {
        self.client_local.insert(name.into());
    }
    /// True if a client-scoped script may issue the command named `name`.
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
/// client-scoped script to issue (see [`ClientCommandPolicy`]). Call it from the
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
}

impl Default for SpawnEntity {
    fn default() -> Self {
        Self {
            entry_id: String::new(),
            position: [0.0; 3],
            rotation: None,
        }
    }
}
