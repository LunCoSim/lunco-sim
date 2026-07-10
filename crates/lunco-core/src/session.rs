//! Networking authority substrate — always-on, **no wire dependency**.
//!
//! These resources are the seam the optional `lunco-networking` layer drives,
//! and the gate every wire-applied command passes through. In single-player
//! ([`NetworkRole::Standalone`], [`crate::IsServer`]`(true)`) the gate trivially
//! passes and nothing is ever serialized — the *same* codepath as multiplayer
//! (the listen-server model), no second branch. This is D7: the substrate is
//! always compiled in; only the wire is feature-gated.

use crate::commands::{Reject, SessionId};
use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

/// Default WebTransport port for the listen-server / host and for any client
/// address that omits an explicit port. Single source of truth for the `5888`
/// that `--host`, `--connect`, the in-sim Connect panel, the wasm deep-link, and
/// the deploy scripts all default to. Lives in core (no wire dep) so every crate
/// — even ones without a `lunco-networking` dependency (e.g. the workbench
/// Network menu) — can reference one constant.
pub const DEFAULT_HOST_PORT: u16 = 5888;

/// Default HTTP API port when `--api` is passed without an explicit value.
/// Single source of truth for the `4101` the GUI / headless server bins bind to
/// (loopback admin API) — matches the `lunco-server.service` unit and DEPLOY.md.
pub const DEFAULT_API_PORT: u16 = 4101;

/// Which side of the wire is this process? Drives three decisions:
/// capture (`Standalone` never serializes), id minting (`Host` mints
/// [`crate::Provenance::Authoritative`]), and apply (`Host` authorizes).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetworkRole {
    /// Single-process; the wire is inert. Default — single-player is the
    /// zero-config case.
    #[default]
    Standalone,
    /// Listen-server (host-client) or dedicated server. Authoritative: mints
    /// ids, owns authority, ferries snapshots out.
    Host,
    /// Pure client; defers identity + authority to the host, applies snapshots.
    Client,
}

impl NetworkRole {
    pub fn is_host(self) -> bool {
        matches!(self, NetworkRole::Host)
    }
    /// `true` for `Host` and `Client` — i.e. the wire is live and capture should
    /// serialize locally-originated commands.
    pub fn is_networked(self) -> bool {
        !matches!(self, NetworkRole::Standalone)
    }
}

/// Human-readable networking status for the status bar. Always present
/// (default = single-player → the chip stays hidden). The optional
/// `lunco-networking` adapter populates it; the workbench reads it with **no**
/// lightyear / lunco-networking dependency — the same always-on-seam trick the
/// substrate uses everywhere (D7).
#[derive(Resource, Clone, Debug, Default)]
pub struct NetStatus {
    /// Host / Client / Standalone (`Standalone` ⇒ status chip hidden).
    pub role: NetworkRole,
    /// Endpoint label: the host's listen `:PORT`, or the client's `host:port`.
    pub endpoint: String,
    /// Host: count of connected clients. Client: `1` once connected, else `0`.
    pub peers: u32,
    /// Client: handshake completed (session assigned). Always `true` on a host.
    pub connected: bool,
    /// Suggested address to pre-fill the Connect field (page origin on wasm,
    /// `127.0.0.1:5888` on native). Seeded by the `lunco-networking` adapter so
    /// the workbench's Network menu can offer a sensible default with **no**
    /// lunco-networking dependency. Empty until the adapter seeds it.
    pub connect_hint: String,
    /// **Host only**: best-guess `LAN_IP:port` a guest on the same network should
    /// dial, pre-filled into the *Copy invite link* address field. Seeded by the
    /// host adapter (primary non-loopback IPv4). Empty on clients / off-LAN.
    pub invite_hint: String,
    /// **Host only**: this run's self-signed cert digest (bare hex), embedded in
    /// the invite link's `#fragment` so a browser guest can pin a self-signed
    /// host. Empty when serving a CA cert (guests need no digest then).
    pub invite_digest: String,
}

/// UI → wire bridge: "dial this server". Fired by the workbench's **Network**
/// menu (which has no lunco-networking dependency); the optional adapter observes
/// it and re-dispatches the typed `JoinServer` command. No-op when the wire layer
/// isn't compiled in. This is the same always-on-seam trick as [`NetStatus`] (D7).
#[derive(Event, Clone, Debug)]
pub struct NetConnectRequest {
    /// `host:port` — a hostname (`lunica.lunco.space:5888`) or `ip:port`.
    pub address: String,
    /// Optional self-signed cert SHA-256 digest to pin (hex; colons/whitespace
    /// tolerated — paste the host's printed digest as-is). Empty ⇒ normal CA
    /// validation. A **browser** dialing a self-signed LAN/dev host by IP needs
    /// this (it can't skip TLS validation); native bare-IP dials skip validation
    /// and ignore it. The adapter forwards it to `JoinServer`.
    pub digest: String,
}

/// UI → wire bridge: "leave the current session" (return to single-player).
/// Counterpart to [`NetConnectRequest`]; the adapter re-dispatches `LeaveServer`.
#[derive(Event, Clone, Debug, Default)]
pub struct NetDisconnectRequest;

/// A connect request that arrived from an **untrusted deep link** (a clicked
/// `luncosim://connect?…` link, or the web `?connect=…#digest`) and is awaiting
/// the user's confirmation. Unlike the menu's [`NetConnectRequest`] (an explicit
/// in-app click), a link could be planted by a third party to silently redirect
/// the session, so the UI shows a "Connect to X? [Join] [Cancel]" prompt while
/// this is `Some`; only on *Join* does it become a `JoinServer`. The networking
/// adapter seeds it (native arg parse / wasm URL); the UI clears it on either
/// choice. Always-on seam (D7) so the prompt UI carries no networking dep.
#[derive(Resource, Clone, Debug, Default)]
pub struct PendingConnect {
    /// The pending link, or `None` when nothing awaits confirmation.
    pub request: Option<PendingConnectRequest>,
}

/// The address + optional cert digest a [`PendingConnect`] is asking to dial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConnectRequest {
    /// `host:port` — hostname or `ip:port`.
    pub address: String,
    /// Self-signed cert digest to pin, or empty for CA validation.
    pub digest: String,
}

/// This peer's own session id. [`SessionId::LOCAL`] until a client handshake
/// replaces it. Stamped as `origin` on every outgoing mutation so the host can
/// attribute authority.
#[derive(Resource, Clone, Copy, Debug)]
pub struct LocalSession(pub SessionId);

impl Default for LocalSession {
    fn default() -> Self {
        Self(SessionId::LOCAL)
    }
}

/// Set to `Some(origin)` for the duration of a *wire-applied* command's trigger.
/// Read by:
/// - command **capture** observers → suppress the echo (don't re-serialize a
///   command that just arrived from the wire);
/// - **possession** → skip local camera-binding for a *remote* origin (the host
///   has no camera for a remote player; it only records authority).
///
/// `None` ⇒ the command originated locally on this peer.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct SyncApplyGuard(pub Option<SessionId>);

impl SyncApplyGuard {
    pub fn is_from_sync(&self) -> bool {
        self.0.is_some()
    }
}

/// How [`SessionRegistry::claim`] arbitrates competing possession claims.
///
/// The default models "one user → one rover": a vessel already controlled by
/// another session can't be taken. [`PossessionPolicy::LastWins`] instead lets
/// anyone grab any vessel, stealing it from its previous owner (who is dropped
/// via the client's `enforce_ownership` once the new ownership table
/// propagates). **Host-authoritative** — the host's policy governs the actual
/// claim; clients read it only for an optimistic UI / camera-bind gate
/// ([`SessionRegistry::may_possess`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PossessionPolicy {
    /// First claim wins; not stealable. Default ("one user, one rover").
    #[default]
    Exclusive,
    /// Most recent claim wins; steals the vessel from its prior owner.
    LastWins,
}

/// Server-side ownership: which session may issue authoritative commands against
/// which entity (keyed by [`crate::GlobalEntityId`] raw `u64`). The home of the
/// single [`authorize`] gate. On a pure client this stays empty (the client
/// trusts the host); the client's optimistic local apply is gated by
/// [`SyncApplyGuard`] / [`NetworkRole`], not by ownership.
#[derive(Resource, Default, Debug)]
pub struct SessionRegistry {
    /// rover gid → owning session.
    owners: HashMap<u64, SessionId>,
    /// Arbitration rule consulted by [`Self::claim`] / [`Self::may_possess`].
    /// Survives [`Self::replace_all`] (only the owner table is replaced).
    policy: PossessionPolicy,
}

impl SessionRegistry {
    /// Record `session` as the owner of `gid`, arbitrated by the current
    /// [`PossessionPolicy`]:
    /// - `Exclusive` (default): returns `Err(current_owner)` if a *different*
    ///   session already owns it (not stealable). Idempotent for the same session.
    /// - `LastWins`: always succeeds, stealing the vessel from any prior owner.
    pub fn claim(&mut self, session: SessionId, gid: u64) -> Result<(), SessionId> {
        match self.policy {
            PossessionPolicy::LastWins => {
                self.owners.insert(gid, session);
                Ok(())
            }
            PossessionPolicy::Exclusive => match self.owners.get(&gid) {
                Some(&cur) if cur != session => Err(cur),
                _ => {
                    self.owners.insert(gid, session);
                    Ok(())
                }
            },
        }
    }

    /// Optimistic-bind gate for the *claiming* peer (client camera-bind / UI
    /// enable). `true` if `session` may take `gid` under the current policy.
    /// Host arbitration still happens in [`Self::claim`]; this only avoids an
    /// obviously-doomed optimistic bind under `Exclusive`.
    pub fn may_possess(&self, session: SessionId, gid: u64) -> bool {
        match self.policy {
            PossessionPolicy::LastWins => true,
            PossessionPolicy::Exclusive => {
                self.owners.get(&gid).is_none_or(|&cur| cur == session)
            }
        }
    }

    /// The arbitration rule currently in force.
    pub fn policy(&self) -> PossessionPolicy {
        self.policy
    }

    /// Set the arbitration rule (host-authoritative).
    pub fn set_policy(&mut self, policy: PossessionPolicy) {
        self.policy = policy;
    }

    pub fn owner_of(&self, gid: u64) -> Option<SessionId> {
        self.owners.get(&gid).copied()
    }

    pub fn owns(&self, session: SessionId, gid: u64) -> bool {
        self.owners.get(&gid) == Some(&session)
    }

    /// Free every entity a dropped session held; returns the freed gids so the
    /// caller can release the corresponding `ControllerLink`s (G5).
    pub fn release_session(&mut self, session: SessionId) -> Vec<u64> {
        let freed: Vec<u64> = self
            .owners
            .iter()
            .filter_map(|(&gid, &s)| (s == session).then_some(gid))
            .collect();
        for gid in &freed {
            self.owners.remove(gid);
        }
        freed
    }

    /// Snapshot the full `(gid, session)` table — the host broadcasts this so
    /// clients hold the same authoritative ownership view.
    pub fn snapshot(&self) -> Vec<(u64, u64)> {
        self.owners.iter().map(|(&g, &s)| (g, s.0)).collect()
    }

    /// Replace the whole table from a host broadcast (client side). Idempotent;
    /// the host's table is authoritative.
    pub fn replace_all(&mut self, entries: impl IntoIterator<Item = (u64, SessionId)>) {
        self.owners = entries.into_iter().collect();
    }
}

/// Global mapping of SessionId (raw u64) to username strings.
///
/// On the host, this accumulates user profile updates. The host then
/// broadcasts this mapping to all clients so they can render names above
/// possessed rovers.
#[derive(Resource, Default, Debug, Clone)]
pub struct SessionProfiles {
    pub profiles: HashMap<u64, String>,
    pub colors: HashMap<u64, [u8; 3]>,
}

/// The role defining the user's capabilities in the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AuthorityRole {
    /// Owner has master authority over all space systems and can delegate subsystems.
    Owner,
    /// Operators can command the specific subsystems they have been granted.
    Operator,
    /// Observers have read-only telemetry access.
    Observer,
    /// Autonomous AI agent with restricted subsystem control.
    AiAgent,
}

impl AuthorityRole {
    /// Does holding `self` satisfy a requirement for `required`?
    ///
    /// This is a small **lattice**, not a linear rank: `Owner` satisfies
    /// everything; `Operator` and `AiAgent` each satisfy their own role plus
    /// the read-only `Observer` floor, but are **incomparable** to each other
    /// (an Operator is not an AiAgent, nor vice-versa). `Observer` satisfies
    /// only `Observer`. Centralising the rule here (an exhaustive match on
    /// `self`) means adding a new role is a compile error until its privileges
    /// are stated — it can't silently inherit a permissive tuple arm.
    pub fn satisfies(self, required: AuthorityRole) -> bool {
        use AuthorityRole::*;
        match self {
            Owner => true,
            Operator => matches!(required, Operator | Observer),
            AiAgent => matches!(required, AiAgent | Observer),
            Observer => matches!(required, Observer),
        }
    }
}

/// Information representing a user's active session, ready for Authn/Authz.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserSession {
    /// The unique network session ID.
    pub session_id: SessionId,
    /// The username associated with the session.
    pub username: String,
    /// The active authorization role.
    pub role: AuthorityRole,
    /// Whether the user has completed authentication.
    pub authenticated: bool,
    /// An optional cryptographically secure token/ticket for verification.
    pub token: Option<String>,
}

/// Role-Based Access Control registry for all active sessions.
#[derive(Resource, Default, Debug, Clone)]
pub struct SessionRbac {
    pub sessions: HashMap<u64, UserSession>,
}

impl SessionRbac {
    /// Evaluates if a given session is authorized to perform an action requiring a specific role.
    pub fn is_authorized(&self, session_id: SessionId, required_role: AuthorityRole) -> bool {
        let Some(session) = self.sessions.get(&session_id.0) else {
            return false;
        };
        if !session.authenticated {
            return false;
        }
        // A trusted session must carry a **server-issued token**. The host mints one
        // per connection (`on_server_connected`) and for its own Owner session
        // (`setup_host_rbac`); a session that reached the map without a token (e.g. a
        // name-only `UpdateProfile` from an origin the server never issued) is not
        // authorized. This is what stops the token-less Observer→Operator
        // self-promotion (review M2) — authority now requires a credential the
        // server, not the client, created.
        if session.token.is_none() {
            return false;
        }
        // Role-lattice check lives on `AuthorityRole::satisfies` (Owner ⊇ all;
        // Operator/AiAgent ⊇ Observer but mutually incomparable) so the policy
        // is defined once on the type, not hand-rolled per call site.
        session.role.satisfies(required_role)
    }
}


/// Suppress the USD loader's automatic [`crate::Provenance::Content`] stamping
/// for a runtime-instanced subtree. Runtime instances get server-allocated
/// identity (root `Authoritative`, children left un-networked) rather than
/// content-derived ids — otherwise two instances of the same asset would derive
/// the **same** id and collide (design in git history). Placed on the spawn
/// root; the loader skips its `Content` stamp for the root and its descendants.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SkipContentStamp;

/// The host replicates this entity's `Transform` to clients via snapshots.
/// Added to runtime-spawned networked roots (rovers). Carried on the client
/// proxy too so tooling can see it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct NetReplicate;

/// **Delivered chassis motion for client-side animation.** A replicated proxy
/// rover is pinned `Kinematic` with its avian velocity zeroed every frame
/// (`force_kinematic_proxies`), so the local wheel-spin model would see a
/// standstill and never roll. The host's authoritative chassis velocity already
/// rides every snapshot (`SnapshotEntry::lv`/`av`); the client's
/// `interpolate_proxies` stamps it here so the *animation* systems (wheel spin)
/// can read the real ground speed without that velocity ever driving avian
/// integration (which would make the kinematic body glide between snapshots).
///
/// This is the "sync the motion, derive the animation" boundary: chassis pose +
/// velocity are synced; wheel rotation/suspension are recomputed locally from
/// them. Present only on client proxies; absent on host/standalone/owned bodies
/// (which read their live avian velocity instead).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ReplicatedChassisMotion {
    /// Authoritative chassis linear velocity (world m/s), from the latest snapshot.
    pub lin: bevy::math::DVec3,
    /// Authoritative chassis angular velocity (world rad/s), from the latest snapshot.
    pub ang: bevy::math::DVec3,
}

/// **Client-side predict-own marker:** this session owns *and locally predicts*
/// this replicated body (its possessed rover). Maintained only on a client, by
/// `maintain_owned_locally`, from the authoritative [`SessionRegistry`] +
/// [`LocalSession`] — inserted when this peer owns the gid, removed when
/// ownership lapses (released / stolen).
///
/// It is the single classifier the predict-own seams read: the owned body is
/// excluded from kinematic-pinning and snapshot interpolation, runs its own
/// avian + mobility step (so input feels crisp, not `INTERP_DELAY` behind), and
/// is smooth-corrected toward authoritative snapshots instead of hard-applied.
/// **Never present on host/standalone** — those peers simulate every body
/// authoritatively, so no per-body distinction is needed there.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct OwnedLocally;

/// Client-side marker: predict this replicated body's **physics locally** even
/// though this peer supplies it **no input** — a free dynamic prop (a ball /
/// crate) you bump with your rover (design in git history). Like
/// [`OwnedLocally`] it is excluded from kinematic-pinning and snapshot
/// interpolation and runs its own avian step, so a local rover↔prop collision
/// resolves crisply in the same frame. UNLIKE `OwnedLocally` it has no input seq
/// to replay, so it is reconciled by **state** (`reconcile_predicted_dynamic`):
/// each snapshot snaps it to authority if it has grossly diverged, else eases the
/// small error in and seats velocity to authoritative. **Never on host/standalone**
/// (they simulate authoritatively). Cosim-driven bodies (server-only forces) must
/// NEVER get this — they are excluded by the runtime-spawn designation
/// (`maintain_predicted_dynamic`), since cosim props are scene content, not
/// `SkipContentStamp` runtime spawns.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PredictedDynamic;

/// **Opaque-body guard** (design in git history): this body's motion is
/// **not locally computable** — it is driven by forces the client does not run
/// (primarily **cosim / Modelica** bodies: a balloon's buoyancy, a thruster, any
/// `SimComponent`-forced prop). Such a body must be excluded from *every*
/// predicted set even if it is owned or in contact with something predicted, and
/// always falls back to the interpolated proxy path. The discriminator is
/// computability, not ownership (Gap C): predicting a cosim body would replay
/// physics the client can't reproduce, so it would diverge and rubber-band every
/// snapshot. Stamped at the cosim takeover site (where a `SimComponent` is
/// attached to a physics body, in `lunco-usd-sim`'s cosim install) and respected
/// by `maintain_predicted_dynamic` (Phase B). It is the hard backstop that keeps
/// Phase B's structural `SkipContentStamp` guard from being the *only* thing
/// standing between a cosim body and the predictor. Always-on substrate; harmless
/// on host/standalone (nothing predicts there).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct NotPredictable;

/// **Articulated-body guard:** this entity is the ROOT of a multi-body assembly
/// whose child rigid bodies are joined to it by physics joints (a *Physical*
/// rover: chassis + wheels on `RevoluteJoint`s). Its child links ARE now
/// per-link replicated ([`ArticulatedLink`] + [`NetReplicate`]), so a remote
/// client renders the host's true articulation. The root must still **never be
/// single-body predicted** on a client: making only the chassis `Dynamic` and
/// state-reconciling its pose/velocity each snapshot — while the jointed wheels
/// follow their own snapshot streams — violates the joint constraints every
/// snapshot, and the solver injects energy until the rover flips. Instead a
/// remote articulated rover is a fully pose-forced assembly: a *kinematic proxy*
/// chassis AND kinematic-proxy wheels, each driven by its own snapshot stream
/// (the inter-link joints are inert between two kinematic bodies), so it
/// physically cannot flip (`maintain_predicted_vehicles` excludes
/// `With<ArticulatedVehicle>`). Derived declaratively from the USD joint graph
/// (a `PhysicsRevoluteJoint` `physics:body0` target / `PhysicsArticulationRootAPI`
/// prim) by `lunco-usd-sim`'s `process_usd_sim_prims` on both peers; only the
/// client reads it. Single-body (raycast) rovers never carry it and predict normally.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ArticulatedVehicle;

/// **Articulated child link:** this entity is a *wheel* (or other jointed child
/// body) of an [`ArticulatedVehicle`] root, and is itself per-link replicated.
/// Derived from the USD joint graph (the prim is a `PhysicsRevoluteJoint`
/// `physics:body1` target) by `lunco-usd-sim`'s `process_usd_sim_prims`; the
/// default membership pass (`apply_net_replication`) then adds [`NetReplicate`].
/// It exists to disambiguate a replicated wheel from the replicated chassis:
/// - `maintain_owned_locally` skips it (a wheel is never owned in the registry —
///   only the chassis gid is claimed — so without this it would strip the
///   `OwnedLocally` that `propagate_owned_to_wheels` mirrors onto owned wheels,
///   and the two systems would fight every frame);
/// - `propagate_owned_to_wheels` selects exactly these links and mirrors the
///   parent chassis's `OwnedLocally` onto them, so an owned rover's wheels run
///   local predicted physics instead of becoming kinematic snapshot proxies.
///
/// Always-on substrate; only the client reads it. Stamped on both peers.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ArticulatedLink;

/// **Replication opt-out:** the USD scene explicitly excludes this body from
/// network replication (`lunco:net:replicate = false` or `lunco:net:authority =
/// "local"`). The membership pass (`apply_net_replication`) skips a body carrying
/// this marker, so it never gets [`NetReplicate`] and stays a purely local,
/// client-side/standalone body. Stamped at USD load by the policy derivation in
/// `lunco-usd-sim` (`derive_net_policy`); the declarative counterpart to the
/// always-on default "every non-static rigid body replicates". Both peers stamp
/// it deterministically from the same authored attribute. See
/// `crates/lunco-networking/USD_REPLICATION_POLICY.md`.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct NetExcluded;

/// Server-side record of a runtime spawn the host must replicate to clients,
/// carrying the catalog id + spawn position so peers can reconstruct the
/// geometry locally (M1) pinned to the host-allocated id. Added to the spawn
/// root on the host; a wire system reads it once the id is minted.
#[derive(Component, Clone, Debug)]
pub struct NetSpawn {
    pub entry_id: String,
    pub position: Vec3,
}

/// One replicated spawn the host told us to instantiate locally, pinned to the
/// host-allocated `gid` (M1 content-reconstruction: geometry loads locally,
/// identity comes from the host). Filled by the wire layer on a client; drained
/// by the spawn domain (`lunco-sandbox-edit`).
#[derive(Clone, Debug)]
pub struct ReplicatedSpawn {
    pub gid: u64,
    pub entry_id: String,
    pub position: Vec3,
}

/// Queue of [`ReplicatedSpawn`]s awaiting local instantiation on a client.
#[derive(Resource, Default)]
pub struct PendingReplicatedSpawns(pub Vec<ReplicatedSpawn>);

/// One replicated transform sample to apply on a client, keyed by
/// [`crate::GlobalEntityId`] raw `u64`. Pushed by the wire layer; applied by an
/// avian-aware system in the spawn domain (which sets avian `Position` so the
/// physics sync doesn't overwrite it).
#[derive(Clone, Copy, Debug)]
pub struct SnapshotSample {
    pub gid: u64,
    /// Host `SimTick` this batch was generated at (60 Hz). The client interpolates
    /// in this tick-derived timebase rather than local receipt time, so bursty /
    /// render-throttled delivery (several 20 Hz snapshots arriving in one frame,
    /// e.g. when the sending peer's window is unfocused) still reconstructs smooth
    /// motion instead of collapsing to one effective sample per burst.
    pub tick: u64,
    pub t: [f32; 3],
    pub r: [f32; 4],
    /// Authoritative linear velocity (avian `LinearVelocity`, f64→f32). Used by
    /// the owned-rover prediction to seat the body for replay; remote bodies
    /// ignore it.
    pub lv: [f32; 3],
    /// Authoritative angular velocity (avian `AngularVelocity`, f64→f32).
    pub av: [f32; 3],
    /// The highest input `seq` the host has applied for this gid (0 = none). The
    /// owning client uses it to drop acked inputs and replay the rest.
    pub last_input_seq: u32,
    /// Authoritative **absolute** position from avian f64 `Position` (gap A). `t`
    /// above is the f32 render-space offset; `pos` is the precise physics truth
    /// the proxy apply seats `Position` from, so lunar/orbital-scale bodies don't
    /// lose precision to f32. Falls back to `t` when the host had no `Position`.
    pub pos: [f64; 3],
    /// big_space `CellCoord` (i64/axis). `[0,0,0]` in the current single-cell
    /// config; carried so replication stays correct once recentering is enabled.
    pub cell: [i64; 3],
}

/// Inbound transform samples awaiting application on a client.
#[derive(Resource, Default)]
pub struct IncomingSnapshots(pub Vec<SnapshotSample>);

/// One sampled vessel input, stamped with a dense per-vessel sequence number and
/// the client `SimTick` it was sampled at. Buffered for client-prediction replay:
/// after a snapshot snaps the owned body to the authoritative state, the reconcile
/// re-applies the still-unacked frames in order to advance back to "now".
#[derive(Clone, Copy, Debug, Default)]
pub struct InputFrame {
    pub seq: u32,
    pub tick: u64,
    pub forward: f64,
    pub steer: f64,
    pub brake: f64,
}

/// Per-vessel input log: a monotonic `seq` counter plus the unacked frames.
#[derive(Default)]
pub struct VesselInputLog {
    /// Next sequence number to assign (monotonic per vessel; `seq` 0 is reserved
    /// for "no input applied yet" in the snapshot ack).
    pub next_seq: u32,
    /// Unacked input frames, oldest first (the reconcile drops `seq <= acked`).
    pub frames: VecDeque<InputFrame>,
    /// Client-only: the most recent `SimTick` at which THIS peer supplied
    /// **nonzero** control input to this vessel. Prediction membership keys off it
    /// (computability, not ownership — see design in git history): a
    /// vessel you possess but are *not actively driving* is dominated by external
    /// forces you can't reproduce locally, so it must fall back to the interpolated
    /// proxy path instead of free-running a local prediction with no correction.
    /// Stamped regardless of `OwnedLocally` so the first nonzero frame can
    /// *bootstrap* prediction. `0` = never driven by this peer.
    pub last_active_tick: u64,
}

/// Client-side unacked input logs keyed by [`crate::GlobalEntityId`] raw `u64`.
/// Populated only for vessels this peer owns + predicts (`OwnedLocally`); the
/// reconcile drops acked frames and replays the rest over the owned avian body.
/// Empty on host/standalone.
#[derive(Resource, Default)]
pub struct OwnedInputLog(pub HashMap<u64, VesselInputLog>);

/// Host-side record of the highest input `seq` applied per gid, written when a
/// `SetPorts` control command is authorized + applied. Stamped into each snapshot's
/// `last_input_seq` so the owning client knows how far the authoritative sim has
/// integrated its inputs (the reconcile ack). Empty on client/standalone.
#[derive(Resource, Default)]
pub struct AppliedInputSeq(pub HashMap<u64, u32>);

/// What a command demands of its caller. The default is a **fully open
/// sandbox**: any authenticated session (Observer floor) may issue it with no
/// ownership requirement. Tightening a command for RBAC is a matter of changing
/// its policy — the [`authorize`] gate, the [`AuthorityRole`] lattice, and the
/// command handlers never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandPolicy {
    /// Minimum role on the [`AuthorityRole`] lattice the caller must satisfy.
    pub min_role: AuthorityRole,
    /// Whether the caller must **own** the command's target entity. When `true`,
    /// control of a *non-owned* target additionally demands the elevated
    /// `Operator` role (and is then rejected by the ownership check anyway).
    pub ownership_gated: bool,
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self::OPEN
    }
}

impl CommandPolicy {
    /// Open sandbox: any authenticated session, no ownership requirement.
    pub const OPEN: Self = Self { min_role: AuthorityRole::Observer, ownership_gated: false };
    /// Ownership-gated control (e.g. `SetPorts`): the owner may act at the
    /// Observer floor; a non-owner is rejected.
    pub const OWNED_CONTROL: Self = Self { min_role: AuthorityRole::Observer, ownership_gated: true };
}

/// Well-known capability keys for authorization gates that aren't a single
/// reflected command but still want one policy substrate — currently the
/// avatar-relay paths (`TutorStatus`/`StudentStatus`/`SharePerspective`), which
/// seize or redirect peers' avatar camera + input.
///
/// They are resolved through the same [`CommandPolicyRegistry`] as real commands
/// (keyed by these strings), so an operator tightens "who may teach / share a
/// perspective" exactly as they would tighten any command. Absent from the
/// registry by default → [`CommandPolicy::OPEN`], so every authenticated peer may
/// relay (matching the open-sandbox default). The string values intentionally
/// match the wire envelope / command type names so the command and relay paths of
/// the same concept share one policy entry.
pub mod capability {
    /// Relay a `TutorStatus` (locks/seizes targeted peers' avatar input + camera).
    pub const TUTOR_STATUS: &str = "TutorStatus";
    /// Relay a `StudentStatus` (mirrors a student's view to an observing tutor).
    pub const STUDENT_STATUS: &str = "StudentStatus";
    /// Relay a `SharePerspective` (one-shot snaps peers' camera to the sender's view).
    pub const SHARE_PERSPECTIVE: &str = "SharePerspective";
    /// Apply an inbound **journal edit** (an authored document change on the
    /// journal plane) to the host's authoritative history. Gates *who may edit
    /// the shared scene/model*, resolved through [`CommandPolicyRegistry`] like
    /// any command. Absent from the default registry → [`super::CommandPolicy::OPEN`]
    /// (open-sandbox: any authenticated peer may edit). A deployment tightens
    /// collaborative editing to, e.g., `Operator` via
    /// [`CommandPolicyRegistry::set_override`](super::CommandPolicyRegistry::set_override)
    /// (`"JournalEdit"`) — read-only viewers then can't mutate shared content.
    pub const JOURNAL_EDIT: &str = "JournalEdit";
    /// Ingest an inbound **asset offer** (a client-imported asset written into
    /// the host's shared twin, then redistributed to every peer). The content-
    /// plane sibling of [`JOURNAL_EDIT`]: gates *who may contribute bytes to
    /// the shared scenario*, resolved through [`CommandPolicyRegistry`] like
    /// any command. Absent from the default registry →
    /// [`super::CommandPolicy::OPEN`] (open-sandbox: any authenticated peer may
    /// import); tighten via
    /// [`CommandPolicyRegistry::set_override`](super::CommandPolicyRegistry::set_override)
    /// (`"AssetOffer"`).
    pub const ASSET_OFFER: &str = "AssetOffer";
}

/// The [`lunco_hooks`] id of the optional **scripted authorization** hook.
///
/// When a hook is registered under this id, [`authorize`] consults it *after* the
/// built-in role+ownership decision — but only on the ALLOW path, and only to
/// **further restrict** (defense in depth): the final decision is `built-in AND
/// hook`. This makes RBAC policy authorable in rhai/Python ("policy→rhai") for
/// custom rules — per-capability allowlists, rate/time gates — without ever
/// weakening the compiled floor. The hook receives a map
/// `{ session, capability, target, owns_target, role }` and returns a bool
/// (`true` = allow). It fails **closed**: a missing bool or a fault denies (a
/// registered-but-broken security policy must not silently wave requests through).
/// Absent hook ⇒ behaviour is byte-for-byte the pre-hook gate.
pub const AUTHORIZE_HOOK: &str = "rbac.authorize";

/// The [`lunco_hooks`] id of the **control-authority takeover** policy (spec 034).
///
/// When one actor tries to possess a vessel **another session already owns**, the
/// possession path ([`SessionRegistry::claim`] is `Exclusive` by default and would
/// refuse) asks this hook whether the takeover is allowed. The rule — e.g. "a human
/// may take a vessel from an autopilot (`AiAgent`), but an autopilot may not take
/// one a human holds" — is authored in **rhai**, not Rust, so a deployment tunes it
/// without recompiling (`policy→rhai`). The hook receives a map
/// `{ taker, taker_role, owner, owner_role, target }` and returns a bool (`true` =
/// the taker may steal, so the prior owner is released first). It fails **closed**:
/// absent hook or a non-bool/faulting policy ⇒ no takeover (the vessel stays with
/// its current owner). See [`may_take_control`].
pub const CONTROL_AUTHORITY_HOOK: &str = "control.authority.take";

/// Hook id for the **boot-entry** policy: consulted once at app startup to decide
/// what the launch should do — onboard (start a tutorial), load a scene, resume,
/// or nothing (let the app load its default). The policy is a pure decision:
/// `ctx` in → a `#{ command, params }` map (dispatched generically) or `()` out.
/// Authored in `assets/scripting/policy/boot.rhai`; hot-rewritable by this id.
pub const BOOT_HOOK: &str = "boot.entry";

/// Per-command authorization policy, resolved by [`authorize`].
///
/// The registry is the single seam through which RBAC is introduced later
/// **without touching the gate**:
/// - `base` is the compile-time policy a command declares. Today it carries only
///   the MVP ownership-gated control commands; in future the `#[Command]` derive
///   (CQ-705) will populate it so policy lives on the type, not a stringly-typed
///   match here.
/// - `overrides` is the deployment-level knob, consulted *before* `base`, so an
///   operator can lock a specific server down (or open it up) at runtime without
///   recompiling any command. Empty by default = open sandbox.
///
/// A command absent from both resolves to [`CommandPolicy::OPEN`], so the current
/// "everything works by default" behavior is preserved exactly.
#[derive(Resource, Debug, Clone)]
pub struct CommandPolicyRegistry {
    base: HashMap<&'static str, CommandPolicy>,
    overrides: HashMap<String, CommandPolicy>,
}

impl Default for CommandPolicyRegistry {
    fn default() -> Self {
        let mut base = HashMap::new();
        // MVP policy, preserved: direct-control commands require ownership of
        // their target (G4). Everything else stays OPEN.
        base.insert("SetPorts", CommandPolicy::OWNED_CONTROL);
        // H2 Fix: gate tutor/relay capabilities to Operator role by default
        base.insert(capability::TUTOR_STATUS, CommandPolicy { min_role: AuthorityRole::Operator, ownership_gated: false });
        base.insert(capability::SHARE_PERSPECTIVE, CommandPolicy { min_role: AuthorityRole::Operator, ownership_gated: false });
        Self { base, overrides: HashMap::new() }
    }
}

impl CommandPolicyRegistry {
    /// Effective policy for a command type: `overrides` → `base` → [`CommandPolicy::OPEN`].
    pub fn policy_for(&self, type_name: &str) -> CommandPolicy {
        if let Some(p) = self.overrides.get(type_name) {
            return *p;
        }
        self.base.get(type_name).copied().unwrap_or_default()
    }

    /// Declare a command's compile-time policy (the future `#[Command]`-derive seam).
    ///
    // TODO(rbac step 4 / CQ-705): drive this from a `#[command(min_role = ..,
    // ownership_gated)]` attribute in `lunco-command-macro` so policy lives on the
    // command type and auto-populates `base` at startup, instead of being set here
    // by hand. This `register()` entry point is the hook the derive will call.
    pub fn register(&mut self, type_name: &'static str, policy: CommandPolicy) {
        self.base.insert(type_name, policy);
    }

    /// Deployment knob: tighten or loosen a command at runtime without recompiling.
    pub fn set_override(&mut self, type_name: impl Into<String>, policy: CommandPolicy) {
        self.overrides.insert(type_name.into(), policy);
    }

    /// Drop a runtime override, falling back to the command's declared policy.
    pub fn clear_override(&mut self, type_name: &str) {
        self.overrides.remove(type_name);
    }
}

/// The single authority gate. Given the `origin` session of a command, the
/// command's short type name, and the target entity's [`crate::GlobalEntityId`]
/// (raw), decide whether the host applies it.
///
/// Policy is **data**, resolved from [`CommandPolicyRegistry`] — this function
/// encodes only the *mechanism* (role lattice + ownership), never the per-command
/// policy. By default every command resolves to [`CommandPolicy::OPEN`], so an
/// authenticated session may do anything; RBAC is introduced by registering or
/// overriding policies, not by editing this gate.
///
/// Single-player never reaches here (capture no-ops under `Standalone`).
pub fn authorize(
    reg: &SessionRegistry,
    rbac: &SessionRbac,
    policies: &CommandPolicyRegistry,
    origin: SessionId,
    type_name: &str,
    target_gid: Option<u64>,
) -> Result<(), Reject> {
    let policy = policies.policy_for(type_name);
    let owns_target = target_gid.is_some_and(|gid| reg.owns(origin, gid));

    // Role requirement. Ownership-gated control of a *non-owned* target demands
    // the elevated Operator role (then rejected by the ownership check below); an
    // owner needs only the command's declared `min_role`. This keeps a legitimate
    // owner that connected as Observer (and never sent an `UpdateProfile`) able to
    // control what it owns.
    let required_role = if policy.ownership_gated && !owns_target {
        AuthorityRole::Operator
    } else {
        policy.min_role
    };

    if !rbac.is_authorized(origin, required_role) {
        return Err(Reject::InvalidOp(format!(
            "session {origin} is not authorized: lacks role {required_role:?}"
        )));
    }

    // Ownership gate. Commands that aren't ownership-gated pass at the role floor
    // above — their arbitration (if any) happens in the handler (e.g.
    // [`SessionRegistry::claim`]).
    let baseline: Result<(), Reject> = if policy.ownership_gated {
        match target_gid {
            Some(gid) if reg.owns(origin, gid) => Ok(()),
            Some(_) => Err(Reject::InvalidOp(format!(
                "session {origin} is not authorized to {type_name} that entity"
            ))),
            None => Err(Reject::InvalidOp(format!("{type_name} has no target"))),
        }
    } else {
        Ok(())
    };
    baseline?;

    // The compiled floor allowed this. If a scripted authorization policy is
    // registered ([`AUTHORIZE_HOOK`], "policy→rhai"), it may FURTHER restrict —
    // never grant. Absent hook ⇒ no-op (identical to the pre-hook gate).
    authz_hook_gate(rbac, origin, type_name, target_gid, owns_target)
}

/// Consult the optional scripted authorization hook ([`AUTHORIZE_HOOK`]). Only
/// reached once the built-in gate already allowed, so the hook can only tighten.
/// Fails **closed** (a registered hook that faults or returns a non-bool denies).
fn authz_hook_gate(
    rbac: &SessionRbac,
    origin: SessionId,
    type_name: &str,
    target_gid: Option<u64>,
    owns_target: bool,
) -> Result<(), Reject> {
    use lunco_hooks::HookValue;
    let role = rbac
        .sessions
        .get(&origin.0)
        .map(|s| format!("{:?}", s.role))
        .unwrap_or_else(|| "None".to_string());
    let ctx = HookValue::map([
        ("session", HookValue::Int(origin.0 as i64)),
        ("capability", HookValue::str(type_name)),
        ("target", HookValue::Int(target_gid.map(|g| g as i64).unwrap_or(-1))),
        ("owns_target", HookValue::Bool(owns_target)),
        ("role", HookValue::str(role)),
    ]);
    match lunco_hooks::invoke(AUTHORIZE_HOOK, &[ctx]) {
        // No hook registered → unchanged behaviour.
        None => Ok(()),
        // Hook allowed.
        Some(Ok(v)) if v.as_bool() == Some(true) => Ok(()),
        // Hook denied (or returned a non-bool → fail closed).
        Some(Ok(_)) => Err(Reject::InvalidOp(format!(
            "session {origin} denied {type_name} by authorization policy"
        ))),
        // Hook faulted → fail closed (a broken security policy must not pass).
        Some(Err(e)) => Err(Reject::InvalidOp(format!(
            "session {origin} denied {type_name}: authorization policy error: {e}"
        ))),
    }
}

/// Ask the rhai control-authority policy ([`CONTROL_AUTHORITY_HOOK`]) whether
/// `taker` may take a vessel currently owned by `owner`. Used by the possession
/// path when the vessel is owned by a *different* session, so the built-in
/// `Exclusive` refusal can be overridden by an authored takeover rule (spec 034).
///
/// The decision is entirely the rhai policy's — this only marshals the context and
/// **fails closed**: with no hook registered, or a policy that faults or returns a
/// non-bool, it returns `false` (no takeover; the current owner keeps the vessel).
pub fn may_take_control(
    rbac: &SessionRbac,
    taker: SessionId,
    owner: SessionId,
    target_gid: u64,
) -> bool {
    use lunco_hooks::HookValue;
    let role_of = |s: SessionId| {
        rbac.sessions
            .get(&s.0)
            .map(|u| format!("{:?}", u.role))
            .unwrap_or_else(|| "None".to_string())
    };
    let ctx = HookValue::map([
        ("taker", HookValue::Int(taker.0 as i64)),
        ("taker_role", HookValue::str(role_of(taker))),
        ("owner", HookValue::Int(owner.0 as i64)),
        ("owner_role", HookValue::str(role_of(owner))),
        ("target", HookValue::Int(target_gid as i64)),
    ]);
    matches!(
        lunco_hooks::invoke(CONTROL_AUTHORITY_HOOK, &[ctx]),
        Some(Ok(v)) if v.as_bool() == Some(true)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::SessionId;

    const A: SessionId = SessionId(1);
    const B: SessionId = SessionId(2);
    const R1: u64 = 0xA1;
    const R2: u64 = 0xB2;

    #[test]
    fn default_policy_is_exclusive() {
        assert_eq!(SessionRegistry::default().policy(), PossessionPolicy::Exclusive);
    }

    #[test]
    fn authority_role_lattice() {
        use AuthorityRole::*;
        // Owner satisfies everything.
        for r in [Owner, Operator, Observer, AiAgent] {
            assert!(Owner.satisfies(r), "Owner should satisfy {r:?}");
        }
        // Operator and AiAgent each satisfy themselves + the Observer floor…
        assert!(Operator.satisfies(Operator));
        assert!(Operator.satisfies(Observer));
        assert!(AiAgent.satisfies(AiAgent));
        assert!(AiAgent.satisfies(Observer));
        // …but are incomparable to each other and never grant Owner.
        assert!(!Operator.satisfies(AiAgent));
        assert!(!AiAgent.satisfies(Operator));
        assert!(!Operator.satisfies(Owner));
        assert!(!AiAgent.satisfies(Owner));
        // Observer is the floor: only satisfies Observer.
        assert!(Observer.satisfies(Observer));
        assert!(!Observer.satisfies(Operator));
        assert!(!Observer.satisfies(AiAgent));
        assert!(!Observer.satisfies(Owner));
    }

    #[test]
    fn exclusive_first_claim_wins_and_blocks_others() {
        let mut reg = SessionRegistry::default();
        assert!(reg.claim(A, R1).is_ok());
        assert_eq!(reg.owner_of(R1), Some(A));
        // A different session cannot take an owned vessel.
        assert_eq!(reg.claim(B, R1), Err(A));
        assert_eq!(reg.owner_of(R1), Some(A)); // unchanged
        // Same session re-claim is idempotent.
        assert!(reg.claim(A, R1).is_ok());
        assert_eq!(reg.owner_of(R1), Some(A));
    }

    #[test]
    fn exclusive_may_possess_reflects_ownership() {
        let mut reg = SessionRegistry::default();
        assert!(reg.may_possess(A, R1)); // free
        reg.claim(A, R1).unwrap();
        assert!(reg.may_possess(A, R1)); // owns it
        assert!(!reg.may_possess(B, R1)); // taken by another
        assert!(reg.may_possess(B, R2)); // a different, free rover
    }

    #[test]
    fn lastwins_steals_and_always_permits() {
        let mut reg = SessionRegistry::default();
        reg.set_policy(PossessionPolicy::LastWins);
        reg.claim(A, R1).unwrap();
        assert_eq!(reg.owner_of(R1), Some(A));
        // B steals it — claim succeeds and ownership flips.
        assert!(reg.claim(B, R1).is_ok());
        assert_eq!(reg.owner_of(R1), Some(B));
        // may_possess is unconditionally true under LastWins.
        assert!(reg.may_possess(A, R1));
        assert!(reg.may_possess(B, R1));
    }

    #[test]
    fn release_session_frees_all_its_vessels_only() {
        let mut reg = SessionRegistry::default();
        reg.claim(A, R1).unwrap();
        reg.claim(A, R2).unwrap();
        reg.claim(B, 0xC3).unwrap();
        let mut freed = reg.release_session(A);
        freed.sort_unstable();
        assert_eq!(freed, vec![R1, R2]);
        assert_eq!(reg.owner_of(R1), None);
        assert_eq!(reg.owner_of(R2), None);
        assert_eq!(reg.owner_of(0xC3), Some(B)); // B untouched
        // Freed vessel is now claimable by anyone, even under Exclusive.
        assert!(reg.claim(B, R1).is_ok());
    }

    #[test]
    fn owns_is_exact_session_match() {
        let mut reg = SessionRegistry::default();
        reg.claim(A, R1).unwrap();
        assert!(reg.owns(A, R1));
        assert!(!reg.owns(B, R1));
        assert!(!reg.owns(A, R2)); // unowned
    }

    #[test]
    fn replace_all_preserves_policy() {
        // The host broadcasts only the owner table; a client's replace_all must
        // NOT reset the arbitration policy (it's a separate field).
        let mut reg = SessionRegistry::default();
        reg.set_policy(PossessionPolicy::LastWins);
        reg.replace_all([(R1, A), (R2, B)]);
        assert_eq!(reg.policy(), PossessionPolicy::LastWins); // survived
        assert_eq!(reg.owner_of(R1), Some(A));
        assert_eq!(reg.owner_of(R2), Some(B));
    }

    #[test]
    fn snapshot_roundtrips_host_to_client() {
        let mut host = SessionRegistry::default();
        host.claim(A, R1).unwrap();
        host.claim(B, R2).unwrap();
        let mut client = SessionRegistry::default();
        client.replace_all(host.snapshot().into_iter().map(|(g, s)| (g, SessionId(s))));
        assert_eq!(client.owner_of(R1), Some(A));
        assert_eq!(client.owner_of(R2), Some(B));
    }

    #[test]
    fn authorize_drive_requires_ownership() {
        let pol = CommandPolicyRegistry::default();
        let mut reg = SessionRegistry::default();
        reg.claim(A, R1).unwrap();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(A.0, UserSession {
            session_id: A,
            username: "Player A".to_string(),
            role: AuthorityRole::Operator,
            authenticated: true,
            token: Some("srv-token-a".to_string()),
        });
        rbac.sessions.insert(B.0, UserSession {
            session_id: B,
            username: "Player B".to_string(),
            role: AuthorityRole::Operator,
            authenticated: true,
            token: Some("srv-token-b".to_string()),
        });

        // The owner may issue control commands.
        assert!(authorize(&reg, &rbac, &pol, A, "SetPorts", Some(R1)).is_ok());
        assert!(authorize(&reg, &rbac, &pol, A, "SetPorts", Some(R1)).is_ok());
        // A non-owner may not.
        assert!(authorize(&reg, &rbac, &pol, B, "SetPorts", Some(R1)).is_err());
        // A control command with no target is rejected.
        assert!(authorize(&reg, &rbac, &pol, A, "SetPorts", None).is_err());
        // Possession + structural commands are always allowed (arbitration is in
        // `claim`, not the authority gate).
        assert!(authorize(&reg, &rbac, &pol, B, "PossessVessel", Some(R1)).is_ok());
        assert!(authorize(&reg, &rbac, &pol, B, "SpawnEntity", None).is_ok());

        // An authenticated Observer that *owns* the rover may still drive it:
        // ownership is the gate, not the Operator role. (A client connects as
        // Observer and may never send an UpdateProfile to be promoted.)
        let mut observer_rbac = SessionRbac::default();
        for s in [A, B] {
            observer_rbac.sessions.insert(s.0, UserSession {
                session_id: s,
                username: "Observer".to_string(),
                role: AuthorityRole::Observer,
                authenticated: true,
                token: Some(format!("srv-token-{}", s.0)),
            });
        }
        // Owner-Observer A may drive what it owns, and possess/structural commands.
        assert!(authorize(&reg, &observer_rbac, &pol, A, "SetPorts", Some(R1)).is_ok());
        assert!(authorize(&reg, &observer_rbac, &pol, A, "PossessVessel", Some(R1)).is_ok());
        // An authenticated non-owner is still rejected by the ownership gate.
        assert!(authorize(&reg, &observer_rbac, &pol, B, "SetPorts", Some(R1)).is_err());

        // The authenticated FLOOR remains: an UNauthenticated session is rejected
        // even for an owned entity (RBAC infra stays wired, just not role-gated).
        let mut unauth_rbac = SessionRbac::default();
        unauth_rbac.sessions.insert(A.0, UserSession {
            session_id: A,
            username: "Player A".to_string(),
            role: AuthorityRole::Observer,
            authenticated: false,
            token: None,
        });
        assert!(authorize(&reg, &unauth_rbac, &pol, A, "SetPorts", Some(R1)).is_err());

        // M2: a session that is `authenticated` but carries NO server-issued token
        // is rejected even for an entity it owns. This is the gate that stops a
        // name-only `UpdateProfile` from minting authority — a credential the
        // server (not the client) created is now required.
        let mut tokenless_rbac = SessionRbac::default();
        tokenless_rbac.sessions.insert(A.0, UserSession {
            session_id: A,
            username: "Player A".to_string(),
            role: AuthorityRole::Operator,
            authenticated: true,
            token: None,
        });
        assert!(authorize(&reg, &tokenless_rbac, &pol, A, "SetPorts", Some(R1)).is_err());
        assert!(authorize(&reg, &tokenless_rbac, &pol, A, "PossessVessel", Some(R1)).is_err());
    }

    #[test]
    fn unregistered_command_is_open_by_default() {
        // The RBAC-readiness invariant: a command with no declared policy resolves
        // to OPEN, so any authenticated Observer may issue it with no target. This
        // keeps "everything works by default" true while the gate is data-driven.
        let pol = CommandPolicyRegistry::default();
        assert_eq!(pol.policy_for("SomeBrandNewCommand"), CommandPolicy::OPEN);

        let reg = SessionRegistry::default();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(A.0, UserSession {
            session_id: A,
            username: "Observer".to_string(),
            role: AuthorityRole::Observer,
            authenticated: true,
            token: Some("srv-token-a".to_string()),
        });
        assert!(authorize(&reg, &rbac, &pol, A, "SomeBrandNewCommand", None).is_ok());
    }

    #[test]
    fn override_tightens_a_command_without_touching_the_gate() {
        // The RBAC switch: an operator locks `SpawnEntity` down to `Operator` at
        // runtime. The gate code is unchanged — only data in the registry differs.
        let mut pol = CommandPolicyRegistry::default();
        pol.set_override(
            "SpawnEntity",
            CommandPolicy { min_role: AuthorityRole::Operator, ownership_gated: false },
        );

        let reg = SessionRegistry::default();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(A.0, UserSession {
            session_id: A,
            username: "Observer".to_string(),
            role: AuthorityRole::Observer,
            authenticated: true,
            token: Some("srv-token-a".to_string()),
        });
        rbac.sessions.insert(B.0, UserSession {
            session_id: B,
            username: "Operator".to_string(),
            role: AuthorityRole::Operator,
            authenticated: true,
            token: Some("srv-token-b".to_string()),
        });

        // Observer is rejected for the tightened command…
        assert!(authorize(&reg, &rbac, &pol, A, "SpawnEntity", None).is_err());
        // …a still-open command (PossessVessel) is unaffected…
        assert!(authorize(&reg, &rbac, &pol, A, "PossessVessel", None).is_ok());
        // …and an Operator passes the tightened command.
        assert!(authorize(&reg, &rbac, &pol, B, "SpawnEntity", None).is_ok());

        // Clearing the override restores open-by-default.
        pol.clear_override("SpawnEntity");
        assert!(authorize(&reg, &rbac, &pol, A, "SpawnEntity", None).is_ok());
    }
    // NOTE: the scripted-authorization-hook test lives in
    // `tests/authz_hook.rs` (its own test binary), because it registers under the
    // process-global `AUTHORIZE_HOOK` id — doing so in this binary would race the
    // other `authorize()` unit tests running on parallel threads.
}
