//! Networking authority substrate — always-on, **no wire dependency**.
//!
//! These resources are the seam the optional `lunco-networking` layer drives,
//! and the gate every wire-applied command passes through. In single-player
//! ([`NetworkRole::Standalone`], [`NetworkRole::is_authoritative`]` == true`) the
//! gate trivially passes and nothing is ever serialized — the *same* codepath as multiplayer
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
    /// This peer is the identity + authority owner of its own world: it **mints**
    /// [`GlobalEntityId`](crate::GlobalEntityId)s (the `is_authoritative` gate in
    /// `assign_global_entity_ids`) and authorizes control. `true` for `Host`
    /// (listen/dedicated server) and `Standalone` (single-player is its own
    /// authority); `false` only for a pure `Client`, which defers identity to the
    /// host and pins host-allocated ids via replication.
    ///
    /// **This is THE single source of truth for "am I authoritative".** It used to
    /// be duplicated in a separate `IsServer(bool)` resource set by hand at three
    /// sites (startup / `JoinServer` / `LeaveServer`); the two drifted — a
    /// standalone sandbox was `Standalone` *and* `IsServer(false)`, so runtime
    /// (palette) spawns got no id and a `piloted`-gated lander went dead. Deriving
    /// it here makes that drift unrepresentable.
    pub fn is_authoritative(self) -> bool {
        matches!(self, NetworkRole::Standalone | NetworkRole::Host)
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
            PossessionPolicy::Exclusive => self.owners.get(&gid).is_none_or(|&cur| cur == session),
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

/// **Contact-prediction eligibility:** this non-owned replicated body *may* be
/// promoted to a locally-`Dynamic` [`PredictedDynamic`] body, but **only while an
/// [`OwnedLocally`] body is actually touching it** (`promote_contacting_proxies`).
/// The rest of the time it stays a kinematic snapshot proxy — perfectly synced to
/// authority, no drift.
///
/// Why this exists (the fix for the "predict-all → drift then chaos" bug): the
/// earlier design flipped *every* non-owned rover/prop to `PredictedDynamic` the
/// moment it was seen, so N mutually-colliding Dynamic bodies free-ran local
/// physics reconciled only against a ~0.18 s-stale curve — the solver pushed the
/// pile apart faster than the bounded correction could pull it back → chaos.
/// Kinematic proxies (pose forced by snapshots) provably cannot drift; the ONLY
/// reason to make a body Dynamic is so it *yields* when your owned rover shoves
/// it. So we gate that Dynamic window to the exact interval a shove is happening,
/// against exactly one pusher — the stable regime. On promotion the body gains
/// `PredictedDynamic` (every proxy-driving seam already excludes that marker); on
/// contact-end it loses it and `drive_kinematic_proxies` re-seats it on the
/// authoritative curve.
///
/// Stamped by `maintain_predicted_dynamic` (free props) and
/// `maintain_predicted_vehicles` (remote raycast rovers) on the same eligible set
/// they used to promote outright — cosim/opaque ([`NotPredictable`]), articulated
/// ([`ArticulatedVehicle`], which flips if made Dynamic), owned, and static bodies
/// are all excluded there. Removed when this peer possesses the body (the
/// input-replay [`OwnedLocally`] path takes over). Client-only; harmless elsewhere.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ContactPredictable;

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

/// Which prediction path a divergence sample came from (see [`DivergenceStats`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PredictionKind {
    /// The body this peer owns and drives (`OwnedLocally`) — error measured at the
    /// **acked input seq**, so the client's legitimate latency lead cancels.
    #[default]
    Owned,
    /// A free locally-simulated body (`PredictedDynamic`: props, bumped rocks,
    /// remote rovers under the contact gate) — error measured against the delayed
    /// authoritative curve it is being reconciled onto.
    Free,
}

/// Per-body divergence gauge for one gid.
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyDivergence {
    pub kind: PredictionKind,
    /// Most recent |authority − prediction| (m).
    pub last_m: f32,
    /// Worst value seen since the body started being predicted.
    pub max_m: f32,
    /// Consecutive samples above [`DivergenceStats::warn_m`].
    pub over_streak: u32,
    /// How many times this body was force-rebaselined (hard snap to authority).
    pub rebaselines: u32,
}

/// CLIENT-side desync detection (review N3).
///
/// Before this there was **no way to observe a desync in the field**: the only
/// backstop was a *silent* per-body snap, and the owned-body half of it could be
/// permanently disabled by the stale-ack bug (see [`AppliedInputSeq`]). A client
/// could drift indefinitely and nothing said so — not a log line, not a counter.
///
/// Every client-side reconcile feeds a sample here, so each locally-simulated body
/// carries a live error, a running max, and a rebaseline count. Sustained error
/// past [`Self::warn_m`] is logged once per body per streak — the "I diverged"
/// signal — and the existing snap/teleport paths count themselves as rebaselines.
///
/// **Deliberately not a wire state-hash.** A rolling digest of the host's pose set
/// would tell the client only what each snapshot already tells it, body by body,
/// with authority attached — and a client cannot recompute a host digest for
/// *interpolated* bodies at all (it holds no tick-aligned local state for them; it
/// holds the host's). Comparing local simulation against received authority is the
/// same test, cheaper, and it names the body that diverged. Empty on host/standalone.
#[derive(Resource, Debug)]
pub struct DivergenceStats {
    /// gid → gauge.
    pub bodies: HashMap<u64, BodyDivergence>,
    /// Error (m) above which a sample counts as divergence.
    pub warn_m: f32,
    /// Consecutive over-threshold samples before the client says so out loud.
    pub warn_streak: u32,
}

impl Default for DivergenceStats {
    fn default() -> Self {
        // 1.0 m is comfortably above the reconcile dead-zones (0.40 m) and the
        // measured free-driving prediction error (~13–27 cm per 20 Hz ack), and well
        // below the 6 m gross-desync snap — so a sustained metre of error is real
        // divergence, not noise, and it is reported BEFORE the snap papers over it.
        Self {
            bodies: HashMap::new(),
            warn_m: 1.0,
            warn_streak: 5,
        }
    }
}

impl DivergenceStats {
    /// Record one |authority − prediction| sample for `gid`. Returns `true` exactly
    /// on the sample where the body crosses into a sustained divergence (so the
    /// caller logs once per streak, not once per tick).
    pub fn observe(&mut self, gid: u64, kind: PredictionKind, err_m: f32) -> bool {
        let warn_m = self.warn_m;
        let warn_streak = self.warn_streak;
        let b = self.bodies.entry(gid).or_default();
        b.kind = kind;
        b.last_m = err_m;
        b.max_m = b.max_m.max(err_m);
        if err_m > warn_m {
            b.over_streak += 1;
            b.over_streak == warn_streak
        } else {
            b.over_streak = 0;
            false
        }
    }

    /// Note that `gid` was force-rebaselined (hard snap / teleport to authority).
    pub fn note_rebaseline(&mut self, gid: u64) {
        let b = self.bodies.entry(gid).or_default();
        b.rebaselines += 1;
        b.over_streak = 0;
    }

    /// The worst live divergence `(gid, metres)` — the gauge the diagnostics export.
    pub fn worst(&self) -> Option<(u64, f32)> {
        self.bodies
            .iter()
            .max_by(|a, b| a.1.last_m.total_cmp(&b.1.last_m))
            .map(|(&g, b)| (g, b.last_m))
    }

    /// Forget a body (despawn / no longer predicted).
    pub fn forget(&mut self, gid: u64) {
        self.bodies.remove(&gid);
    }
}

/// A reconciliation residual parked on a predicted body, drained a little per
/// fixed tick in **physics space** (`Position`/`Rotation`), never on `Transform`.
///
/// **Why not `Transform`.** The sandbox runs
/// `PhysicsInterpolationPlugin::interpolate_all()`, so `bevy_transform_interpolation`
/// owns every body's `Transform` at render rate and treats ANY external `Transform`
/// write as a teleport — resetting its easing. An offset written there therefore
/// *disabled* interpolation for the corrected body (≈ continuously while driving)
/// and the rover rendered at raw fixed-tick steps: the "jitters while just holding
/// the key" the host never showed. Parking the residual here and nudging
/// `Position`/`Rotation` lets avian writeback + interpolation render it smoothly
/// with no second writer anywhere.
///
/// **Why it lives in `lunco-core`** (review A6): `lunco-networking`'s prediction
/// diagnostics read this component, and that was one of exactly two symbols pulling
/// the whole 13.4k-LOC editor crate into every networking build. The producer/drain
/// systems stay in `lunco-sandbox-edit` (which re-exports the type).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PendingCorrection {
    /// Remaining position delta to apply (world metres).
    pub pos: Vec3,
    /// Remaining orientation delta (applied as `rot * current`).
    pub rot: Quat,
}

impl PendingCorrection {
    /// Residual small enough to drop the component (the drain is done).
    pub fn is_negligible(&self) -> bool {
        self.pos.length_squared() < 1e-8 && self.rot.angle_between(Quat::IDENTITY) < 1e-4
    }
}

/// Client-side unacked input logs keyed by [`crate::GlobalEntityId`] raw `u64`.
/// Populated only for vessels this peer owns + predicts (`OwnedLocally`); the
/// reconcile drops acked frames and replays the rest over the owned avian body.
/// Empty on host/standalone.
#[derive(Resource, Default)]
pub struct OwnedInputLog(pub HashMap<u64, VesselInputLog>);

/// CLIENT-side latest local drive input per owned gid `(throttle, steer)`, captured
/// from the outbound `SetPorts` in `record_control_input`. The render-lead
/// (`lead_owned_rover_render`) reads it to visually anticipate the rover's motion —
/// leading the *rendered* pose forward/turning by ~RTT so driving feels responsive
/// even though physics stays 100% host-authoritative (see the visual-prediction
/// design). Purely presentational; never touches the sim. Empty on host/standalone.
#[derive(Resource, Default)]
pub struct LocalDriveInput(pub HashMap<u64, (f64, f64)>);

/// HOST-side per-tick input jitter buffer for CLIENT-owned (predicted) rovers.
///
/// The owning client emits a **contiguous, per-fixed-tick, `seq`-stamped**
/// `SetPorts` stream (see `drive_from_bindings`). Without this buffer the host
/// applied each forwarded `SetPorts` immediately in `drain_sync_inbox` (`Update` =
/// RENDER cadence) and the port latched — so when the host's render rate lagged
/// the fixed rate it *subsampled* the input stream, integrating a DIFFERENT input
/// sequence than the client predicted with. Harmless for a held (constant) input,
/// but during a turn the changing steer is subsampled → the authoritative yaw
/// diverges from the prediction → the reconcile snap = the post-turn wobble.
///
/// The host consumer ([`BufferedClientInputs::next_for_tick`], run once per
/// `FixedUpdate` tick BEFORE the drive/physics) applies exactly ONE buffered input
/// per tick, in `seq` order, latching the last one when none is newer — so the
/// host integrates the SAME sequence the client did, regardless of its render FPS.
/// `BTreeMap` keeps `seq` order (absorbing reorder on the unreliable
/// `ControlStream`). Empty on client/standalone.
#[derive(Resource, Default)]
pub struct BufferedClientInputs {
    /// gid → (seq → the `SetPorts` writes stamped with that seq), still unapplied.
    pub pending: HashMap<u64, std::collections::BTreeMap<u32, Vec<(String, f64)>>>,
    /// gid → highest `seq` consumed so far (the per-tick cursor).
    pub applied: HashMap<u64, u32>,
    /// gid → the most recently consumed writes, re-applied (latched) on a tick
    /// with no newer input so the consumer stays the body's sole port authority.
    pub last_writes: HashMap<u64, Vec<(String, f64)>>,
}

impl BufferedClientInputs {
    /// Record a forwarded client input (`seq` → `writes`) for `gid`. `seq` 0 is
    /// reserved ("no input") and ignored.
    ///
    /// **Server-side seq validation** (review N1): a `seq` at or below the cursor is
    /// already integrated (a duplicate, or a reorder on the lossy `ControlStream`),
    /// and once a stream is established a `seq` more than [`MAX_SEQ_JUMP`] ahead of
    /// the cursor is not a plausible continuation of a once-per-fixed-tick stream.
    /// Both are dropped. Without the upper check a single `SetPorts { seq: u32::MAX }`
    /// would park the cursor at the top of the range and every genuine input after it
    /// would sit in the queue unconsumed forever.
    ///
    /// The window is NOT applied at cursor 0 (no stream yet): a client's `seq` counter
    /// is per (vessel, client) and does not restart when the vessel changes hands, so
    /// a peer re-possessing a rover it drove earlier legitimately resumes at a high
    /// seq against a freshly-cleared buffer. See `AppliedInputSeq::record`.
    pub fn push(&mut self, gid: u64, seq: u32, writes: Vec<(String, f64)>) {
        if seq == 0 {
            return;
        }
        let cursor = self.cursor(gid);
        if seq <= cursor {
            return; // already integrated (duplicate / reorder behind the cursor)
        }
        // Reference = the highest seq we already know about for this gid (consumed or
        // queued). Zero only before the very first input, where any seq is the
        // stream's baseline.
        let highest = self
            .pending
            .get(&gid)
            .and_then(|m| m.keys().next_back().copied())
            .unwrap_or(0)
            .max(cursor);
        if highest != 0 && seq > highest && seq - highest > MAX_SEQ_JUMP {
            return;
        }
        self.pending.entry(gid).or_default().insert(seq, writes);
    }

    /// The highest `seq` actually consumed for `gid` so far (0 = none). This — not
    /// `max(seq)` over everything the wire delivered — is the honest reconcile ack:
    /// the host integrates ONE buffered input per fixed tick, so a render frame that
    /// drains K forwarded `SetPorts` has still only run one of them.
    pub fn cursor(&self, gid: u64) -> u32 {
        self.applied.get(&gid).copied().unwrap_or(0)
    }

    /// Forget everything buffered for `gid` (despawn / ownership change).
    pub fn clear_gid(&mut self, gid: u64) {
        self.pending.remove(&gid);
        self.applied.remove(&gid);
        self.last_writes.remove(&gid);
    }

    /// Consume ONE input for `gid` this fixed tick, in `seq` order: the smallest
    /// buffered `seq` past the cursor. Returns the latched last input when none is
    /// newer (so the caller always writes → stays the port authority). When the
    /// backlog exceeds `cap` (host fell behind), skips to the newest to bound added
    /// latency. Returns `None` only before the first input for `gid` ever arrives.
    pub fn next_for_tick(&mut self, gid: u64, cap: usize) -> Option<Vec<(String, f64)>> {
        let cursor = self.applied.get(&gid).copied().unwrap_or(0);
        let target = {
            let Some(map) = self.pending.get(&gid) else {
                return self.last_writes.get(&gid).cloned();
            };
            match map
                .range(cursor.saturating_add(1)..)
                .next()
                .map(|(s, _)| *s)
            {
                None => return self.last_writes.get(&gid).cloned(),
                Some(ns) if map.len() > cap => *map.keys().next_back().unwrap_or(&ns),
                Some(ns) => ns,
            }
        };
        let map = self.pending.get_mut(&gid)?;
        let writes = map.get(&target).cloned();
        // Drop everything up to and including the consumed seq.
        *map = map.split_off(&(target + 1));
        self.applied.insert(gid, target);
        if let Some(w) = &writes {
            self.last_writes.insert(gid, w.clone());
        }
        writes
    }
}

/// One gid's ack watermark **and the owner it belongs to**.
///
/// The owner is what makes the watermark meaningful: an input `seq` stream is
/// per-(vessel, owning peer), and a new owner restarts its stream at 1. See
/// [`AppliedInputSeq`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppliedSlot {
    /// Session whose `seq` stream this watermark tracks (`None` = unowned).
    pub owner: Option<SessionId>,
    /// Highest `seq` from that owner the host has actually **integrated** (0 = none).
    pub seq: u32,
}

/// Largest forward `seq` jump the host will accept in one step.
///
/// The owning client emits one `seq` per fixed tick and the host acks what it
/// integrates, so a legitimate gap is a handful of ticks (bounded by the input
/// jitter buffer's skip-ahead cap). A larger jump is either garbage or hostile —
/// e.g. a single `SetPorts { seq: u32::MAX }`, which under the old "watermark =
/// max(seq)" rule permanently poisoned the gid for *every* future owner (no later
/// seq could ever exceed it, so no ack was ever new again and the owning client's
/// reconcile early-returned forever). Out-of-window jumps are dropped, not clamped:
/// clamping would still advance the watermark past inputs the host never ran.
pub const MAX_SEQ_JUMP: u32 = 1024;

/// Host-side record of the highest input `seq` **integrated** per gid, together
/// with the owner that `seq` belongs to. Stamped into each snapshot's
/// `last_input_seq` so the owning client knows how far the authoritative sim has
/// consumed its inputs (the reconcile ack). Empty on client/standalone.
///
/// **Two invariants this type exists to enforce** (both were violated by the bare
/// `HashMap<u64, u32>` + `*slot = max(*slot, cmd.seq)` it replaces):
///
/// 1. **The watermark is per (gid, owner), and resets on re-possession.** Client A
///    drives rover R to `seq = 5000` and releases; client B possesses R and its
///    `seq` restarts at 1. A gid-only watermark keeps stamping 5000 into every
///    snapshot, so B's `reconcile_owned_prediction` records `last_reconciled =
///    5000`, finds no prediction at that seq, and early-returns on every later ack
///    (all `≤ 5000`) — **B's prediction is never reconciled again, ever**. Slots
///    therefore carry their owner and zero when it changes ([`Self::record`] and
///    [`Self::sync_owners`], the latter catching claim/release/disconnect even
///    before the new owner's first input arrives).
/// 2. **The watermark only moves for a plausible `seq`** ([`MAX_SEQ_JUMP`]).
///
/// Slots are also pruned with the rest of the replication state
/// ([`Self::retain_gids`]) so a long-lived host doesn't leak one per despawned gid.
#[derive(Resource, Default)]
pub struct AppliedInputSeq {
    slots: HashMap<u64, AppliedSlot>,
}

impl AppliedInputSeq {
    /// The ack to stamp into a snapshot for `gid` (0 = nothing integrated yet).
    pub fn ack(&self, gid: u64) -> u32 {
        self.slots.get(&gid).map_or(0, |s| s.seq)
    }

    /// The full slot (owner + seq), for tests/diagnostics.
    pub fn slot(&self, gid: u64) -> AppliedSlot {
        self.slots.get(&gid).copied().unwrap_or_default()
    }

    /// Record that the host **integrated** input `seq` for `gid`, sent by `owner`.
    ///
    /// Resets the watermark when the owner changed (invariant 1) and ignores an
    /// implausible forward jump (invariant 2). `seq == 0` means "no input stream"
    /// (host-local / API drives) and only (re)binds the owner.
    pub fn record(&mut self, gid: u64, owner: Option<SessionId>, seq: u32) {
        let slot = self.slots.entry(gid).or_default();
        if slot.owner != owner {
            *slot = AppliedSlot { owner, seq: 0 };
        }
        // The FIRST seq of a stream is the baseline and is taken as given: a client's
        // `seq` counter is per (vessel, client) and does NOT restart when the vessel
        // changes hands, so a peer that re-possesses a rover it drove earlier resumes
        // at (say) 5001 against a freshly-zeroed slot. Windowing that first sample
        // would refuse its every input for the rest of the session — the very bug
        // this whole mechanism exists to prevent, reintroduced from the other side.
        // Thereafter the window applies, so a wild `seq` cannot vault the watermark
        // out of reach. A hostile FIRST seq (e.g. `u32::MAX`) can only strand the
        // sender's own prediction on the vessel it possesses, and is cleared the
        // moment the vessel changes hands.
        if slot.seq == 0 || (seq > slot.seq && seq - slot.seq <= MAX_SEQ_JUMP) {
            slot.seq = slot.seq.max(seq);
        }
    }

    /// Re-key every slot against the authoritative ownership table, zeroing the
    /// watermark of any gid whose owner changed. This is the half of invariant 1
    /// that `record` alone cannot cover: between A's release and B's first input
    /// the host would otherwise keep stamping A's stale `seq` into snapshots, and
    /// B latches it as `last_reconciled` the moment it starts predicting.
    pub fn sync_owners(&mut self, registry: &SessionRegistry) {
        for (&gid, slot) in self.slots.iter_mut() {
            let owner = registry.owner_of(gid);
            if slot.owner != owner {
                *slot = AppliedSlot { owner, seq: 0 };
            }
        }
    }

    /// The gids whose recorded owner no longer matches the registry — i.e. the
    /// vessels that changed hands since the last sync. Read before
    /// [`Self::sync_owners`] (which clears the difference) so the caller can also
    /// drop the previous owner's queued-but-unintegrated inputs.
    pub fn changed_owner_gids(&self, registry: &SessionRegistry) -> Vec<u64> {
        self.slots
            .iter()
            .filter(|(&gid, slot)| slot.owner != registry.owner_of(gid))
            .map(|(&gid, _)| gid)
            .collect()
    }

    /// Drop the slot for one gid (despawn).
    pub fn clear_gid(&mut self, gid: u64) {
        self.slots.remove(&gid);
    }

    /// Keep only the slots whose gid passes `keep` — the prune that runs alongside
    /// the snapshot diff-cache so the map tracks the live set.
    pub fn retain_gids(&mut self, keep: impl Fn(u64) -> bool) {
        self.slots.retain(|&gid, _| keep(gid));
    }

    /// Number of live slots (prune/leak assertions).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether any gid has a watermark.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

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
    pub const OPEN: Self = Self {
        min_role: AuthorityRole::Observer,
        ownership_gated: false,
    };
    /// Ownership-gated control (e.g. `SetPorts`): the owner may act at the
    /// Observer floor; a non-owner is rejected.
    pub const OWNED_CONTROL: Self = Self {
        min_role: AuthorityRole::Observer,
        ownership_gated: true,
    };
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

/// Which entities the **control path is currently down** to: commands issued now
/// would not reach them.
///
/// Generic on purpose, and the name says only what is true: *the path is down*. It
/// is NOT a comms concept and there is no comms vocabulary here (doc 49 §1) — a
/// radio blackout, a jammer, a dead receiver, a severed harness or an OBC fault all
/// mean the same thing to a command, and `lunco-core` cannot depend on
/// `lunco-celestial` to learn about any of them anyway.
///
/// **The core never decides this.** Nothing in Rust concludes "no link ⇒ no
/// control": that is a mission's call, and a store-and-forward or delayed-command
/// mission would disagree. Whoever knows the domain sets it — the Space School
/// scenario reads real `LinkState` via `can_reach` and calls [`SetControlPath`],
/// which is exactly doc 49's split (kernel = geometry, script = meaning) applied one
/// layer up.
///
/// Read into the [`AUTHORIZE_HOOK`] ctx as `target_control_path_down`, so an
/// authored policy can refuse commands to an unreachable vessel. Keyed by
/// [`crate::GlobalEntityId`] (raw), like [`SessionRegistry`] — a gid outlives the
/// `Entity` and is what the gate already speaks.
///
/// The verb that writes it is `SetControlPath`, in `lunco-controller` (beside
/// `drive_from_bindings`, the path it gates): the `#[Command]` macro expands to
/// `lunco_core::…` paths, so a command cannot be declared inside this crate.
#[derive(Resource, Debug, Clone, Default)]
pub struct ControlPathRegistry {
    down: std::collections::HashSet<u64>,
}

impl ControlPathRegistry {
    /// Mark the control path to `gid` down (`true`) or restored (`false`).
    pub fn set(&mut self, gid: u64, down: bool) {
        if down {
            self.down.insert(gid);
        } else {
            self.down.remove(&gid);
        }
    }

    /// Is the control path to `gid` down? Unknown ⇒ `false`: a vessel nobody has
    /// said anything about is reachable, so this resource existing changes nothing
    /// until a mission uses it.
    pub fn is_down(&self, gid: u64) -> bool {
        self.down.contains(&gid)
    }

    /// Forget every blackout — e.g. on scene teardown, so a stale one cannot
    /// silently disable a vessel in the next scene.
    pub fn clear(&mut self) {
        self.down.clear();
    }
}

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
        base.insert(
            capability::TUTOR_STATUS,
            CommandPolicy {
                min_role: AuthorityRole::Operator,
                ownership_gated: false,
            },
        );
        base.insert(
            capability::SHARE_PERSPECTIVE,
            CommandPolicy {
                min_role: AuthorityRole::Operator,
                ownership_gated: false,
            },
        );
        Self {
            base,
            overrides: HashMap::new(),
        }
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
    paths: &ControlPathRegistry,
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
    authorize_policy(rbac, paths, origin, type_name, target_gid, owns_target)
}

/// Consult the optional scripted authorization hook ([`AUTHORIZE_HOOK`]) ALONE,
/// without the role/ownership floor [`authorize`] applies first.
///
/// **Why this is public and separate.** The floor is a *wire* concern: it decides
/// whether a remote session may touch a vessel it may not own. The LOCAL keyboard
/// path has deliberately looser rules — `drive_from_bindings` drives an unpossessed
/// vessel (owner `None`), and the free avatar self-drives an entity with no gid at
/// all — so putting the full [`authorize`] on it would deny both and break ordinary
/// local play. What the local path *must* respect is the authored POLICY, so that a
/// rule like "refuse tele-op while the control path is down" binds every command
/// route rather than only the ones that happen to cross the network.
///
/// Only reached once any applicable floor has allowed, so the hook can only tighten.
/// Fails **closed** (a registered hook that faults or returns a non-bool denies).
pub fn authorize_policy(
    rbac: &SessionRbac,
    paths: &ControlPathRegistry,
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
        (
            "target",
            HookValue::Int(target_gid.map(|g| g as i64).unwrap_or(-1)),
        ),
        ("owns_target", HookValue::Bool(owns_target)),
        ("role", HookValue::str(role)),
        // Precomputed FACT, not a lookup the policy could do itself: the hook engine
        // is a bare `Engine::new()` with no world bridge (no `find`/`query`), which
        // is what keeps policies pure and deterministic. So a fact a policy needs
        // must arrive in the ctx — the same reason `link.rs` passes `terrain_blocked`
        // in rather than letting the verdict march the terrain itself.
        (
            "target_control_path_down",
            HookValue::Bool(target_gid.is_some_and(|g| paths.is_down(g))),
        ),
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

/// **The** answer to "may `session` take control of `gid` right now?" — the single
/// predicate the whole possession path must ask.
///
/// It is [`SessionRegistry::may_possess`] (free, or already ours, or `LastWins`) OR an
/// authored takeover of a *different* session's vessel via [`may_take_control`]. Both
/// halves are the rule; either alone is not.
///
/// WHY THIS EXISTS: `PossessVessel` is handled by two observers — the authority leg
/// (`record_possession_authority`, which claims/takes over) and the bind leg
/// (`on_possess_command`, which attaches the camera + `ControllerLink`). They each used
/// to decide possession for themselves, and only the authority leg knew about takeover.
/// They agreed *only* because the authority leg happens to be registered first, so the
/// bind leg re-derived its answer from an already-updated table. Registered the other way
/// round, the bind leg would refuse a takeover the authority leg had just granted: the log
/// would say "session N possesses entity" while the cockpit stayed empty and the vessel
/// undrivable — a silent, order-dependent split-brain. One predicate, both legs, no
/// ordering assumption.
///
/// Deliberately permissive on an *unknown* vessel (no owner recorded): a client's table is
/// a replicated copy that can lag its own claim, and single-player's is empty until the
/// authority leg runs. Refusing there would block the local bind waiting for a round trip.
pub fn may_control(
    registry: &SessionRegistry,
    rbac: &SessionRbac,
    session: SessionId,
    gid: u64,
) -> bool {
    if registry.may_possess(session, gid) {
        return true;
    }
    match registry.owner_of(gid) {
        Some(owner) => may_take_control(rbac, session, owner, gid),
        None => true,
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

    // ── The control-path fact and the policy that reads it ───────────────────
    //
    // `lunco_hooks` is a PROCESS-GLOBAL registry and `authorize` consults it on
    // every call, so a hook registered by one test is visible to every other test
    // running in parallel. Serialise on the registry, not just on each other.
    fn hook_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A stand-in for the Summer Space School's `sim/tutorials/teleop_policy.rhai`
    /// (authored in that TWIN, not this repo): refuse `SetPorts` to a vessel whose
    /// control path is down, exempt autonomy, allow everything else.
    struct TeleopPolicy;
    impl lunco_hooks::ScriptHook for TeleopPolicy {
        fn invoke(&self, args: &[lunco_hooks::HookValue]) -> lunco_hooks::HookResult {
            let ctx = args
                .first()
                .cloned()
                .unwrap_or(lunco_hooks::HookValue::Unit);
            let get = |k: &str| ctx.get(k).cloned().unwrap_or(lunco_hooks::HookValue::Unit);
            if get("capability").as_str() != Some("SetPorts") {
                return Ok(lunco_hooks::HookValue::Bool(true));
            }
            if get("role").as_str() == Some("AiAgent") {
                return Ok(lunco_hooks::HookValue::Bool(true));
            }
            let down = get("target_control_path_down").as_bool().unwrap_or(false);
            Ok(lunco_hooks::HookValue::Bool(!down))
        }
    }

    /// `may_control` is the ONE predicate both `PossessVessel` legs ask, so it must
    /// agree with `may_possess` on the easy cases and additionally honour takeover.
    ///
    /// The bug it guards: the bind leg used to call `may_possess` alone, which refuses
    /// ANY vessel another session owns, while the authority leg granted takeovers via
    /// the rhai policy. They agreed only by registration order. If the bind leg says no
    /// to a takeover the authority leg said yes to, you get authority with no camera —
    /// the log reads "session N possesses entity" and the cockpit is empty.
    #[test]
    fn may_control_agrees_with_the_authority_leg_on_a_policy_takeover() {
        let _g = hook_lock();
        let (reg, rbac, _) = owner_of_r1();

        // Free vessel → anyone may take it. Both predicates already agree here.
        assert!(may_control(&reg, &rbac, B, R2));
        assert!(reg.may_possess(B, R2));
        // Our own vessel → still ours.
        assert!(may_control(&reg, &rbac, A, R1));

        // R1 is owned by A. `may_possess` alone refuses B outright...
        assert!(!reg.may_possess(B, R1), "precondition: exclusive refusal");
        // ...and with NO takeover policy registered, may_control fails closed too.
        lunco_hooks::unregister(CONTROL_AUTHORITY_HOOK);
        assert!(
            !may_control(&reg, &rbac, B, R1),
            "no policy ⇒ fail closed, current owner keeps the vessel"
        );

        // With a policy that ALLOWS the takeover, may_control must say yes — this is
        // exactly where the old `may_possess`-only bind leg said no and split the brain.
        struct AllowTakeover;
        impl lunco_hooks::ScriptHook for AllowTakeover {
            fn invoke(&self, _args: &[lunco_hooks::HookValue]) -> lunco_hooks::HookResult {
                Ok(lunco_hooks::HookValue::Bool(true))
            }
        }
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: CONTROL_AUTHORITY_HOOK.to_string(),
            backend: "rust".into(),
            deterministic: true,
            hook: std::sync::Arc::new(AllowTakeover),
        });
        assert!(
            may_control(&reg, &rbac, B, R1),
            "policy allows takeover ⇒ BOTH legs must allow it"
        );
        lunco_hooks::unregister(CONTROL_AUTHORITY_HOOK);
    }

    fn owner_of_r1() -> (SessionRegistry, SessionRbac, CommandPolicyRegistry) {
        let mut reg = SessionRegistry::default();
        reg.claim(A, R1).unwrap();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(
            A.0,
            UserSession {
                session_id: A,
                username: "Player A".into(),
                role: AuthorityRole::Operator,
                authenticated: true,
                token: Some("srv-token-a".into()),
            },
        );
        (reg, rbac, CommandPolicyRegistry::default())
    }

    /// A blackout with NO policy registered changes nothing. The fact is inert on
    /// its own — refusing is a policy's decision, never the core's.
    #[test]
    fn control_path_down_alone_refuses_nothing() {
        let _g = hook_lock();
        lunco_hooks::unregister(AUTHORIZE_HOOK);
        let (reg, rbac, pol) = owner_of_r1();
        let mut paths = ControlPathRegistry::default();
        paths.set(R1, true);

        assert!(
            authorize(&reg, &rbac, &pol, &paths, A, "SetPorts", Some(R1)).is_ok(),
            "no policy registered ⇒ the gate is unchanged, blackout or not"
        );
    }

    /// The school policy's actual job: refuse the human's drive in a blackout, and
    /// only the drive.
    #[test]
    fn policy_refuses_drive_while_control_path_is_down() {
        let _g = hook_lock();
        let (mut reg, rbac, pol) = owner_of_r1();
        // A owns R2 as well. The per-target assertion below needs a SECOND vessel A
        // may legitimately drive — an unowned one would be refused by the ownership
        // floor before the policy is ever consulted, and the test would pass for
        // entirely the wrong reason.
        reg.claim(A, R2).unwrap();
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: AUTHORIZE_HOOK.to_string(),
            backend: "rust".into(),
            deterministic: true,
            hook: std::sync::Arc::new(TeleopPolicy),
        });

        // Link up: the owner drives.
        let up = ControlPathRegistry::default();
        let drive_up = authorize(&reg, &rbac, &pol, &up, A, "SetPorts", Some(R1));

        // Link down: the same owner, same vessel, refused.
        let mut down = ControlPathRegistry::default();
        down.set(R1, true);
        let drive_down = authorize(&reg, &rbac, &pol, &down, A, "SetPorts", Some(R1));
        // …but only DRIVING is refused — a blackout must not lock a student out of
        // possessing, looking around, or the tutorial's own commands.
        let possess_down = authorize(&reg, &rbac, &pol, &down, A, "PossessVessel", Some(R1));
        // And a DIFFERENT vessel is unaffected: the fact is per-target.
        let other_down = authorize(&reg, &rbac, &pol, &down, A, "SetPorts", Some(R2));

        lunco_hooks::unregister(AUTHORIZE_HOOK);

        assert!(drive_up.is_ok(), "link up ⇒ the owner drives");
        assert!(drive_down.is_err(), "link down ⇒ tele-op is refused");
        assert!(
            possess_down.is_ok(),
            "a blackout refuses driving, not everything"
        );
        assert!(other_down.is_ok(), "the blackout is per-target, not global");
    }

    /// `authorize_policy` is what the LOCAL keyboard path calls. It must apply the
    /// same policy WITHOUT the ownership floor — `drive_from_bindings` legitimately
    /// drives an unpossessed vessel, and inheriting the floor would break local play.
    #[test]
    fn local_gate_applies_policy_without_the_ownership_floor() {
        let _g = hook_lock();
        let (reg, rbac, pol) = owner_of_r1();
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: AUTHORIZE_HOOK.to_string(),
            backend: "rust".into(),
            deterministic: true,
            hook: std::sync::Arc::new(TeleopPolicy),
        });

        let mut down = ControlPathRegistry::default();
        down.set(R2, true);

        // R2 is UNOWNED. The full gate refuses it on ownership alone...
        let full = authorize(&reg, &rbac, &pol, &down, A, "SetPorts", Some(R2));
        // ...but the local path must still be able to drive an unpossessed vessel,
        // so it consults the policy only — which here refuses R2 for the blackout,
        // not for the ownership.
        let local_blacked = authorize_policy(&rbac, &down, A, "SetPorts", Some(R2), false);
        let clear = ControlPathRegistry::default();
        let local_clear = authorize_policy(&rbac, &clear, A, "SetPorts", Some(R2), false);

        lunco_hooks::unregister(AUTHORIZE_HOOK);

        assert!(full.is_err(), "the wire floor refuses an unowned target");
        assert!(
            local_blacked.is_err(),
            "the local path still honours the policy"
        );
        assert!(
            local_clear.is_ok(),
            "…and with no blackout the local path drives an unpossessed vessel — \
             applying the ownership floor here would have broken ordinary local play"
        );
    }

    /// Autonomy keeps driving in a blackout. That is the lesson — the rover is on
    /// its own, not disabled.
    #[test]
    fn policy_exempts_autonomy_from_the_blackout() {
        let _g = hook_lock();
        let mut reg = SessionRegistry::default();
        reg.claim(B, R1).unwrap();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(
            B.0,
            UserSession {
                session_id: B,
                username: "autopilot".into(),
                role: AuthorityRole::AiAgent,
                authenticated: true,
                token: Some("srv-token-b".into()),
            },
        );
        let pol = CommandPolicyRegistry::default();
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: AUTHORIZE_HOOK.to_string(),
            backend: "rust".into(),
            deterministic: true,
            hook: std::sync::Arc::new(TeleopPolicy),
        });

        let mut down = ControlPathRegistry::default();
        down.set(R1, true);
        let ai_drive = authorize(&reg, &rbac, &pol, &down, B, "SetPorts", Some(R1));

        lunco_hooks::unregister(AUTHORIZE_HOOK);
        assert!(ai_drive.is_ok(), "an AiAgent drives through a blackout");
    }

    #[test]
    fn control_path_registry_is_per_gid_and_clearable() {
        let mut paths = ControlPathRegistry::default();
        assert!(
            !paths.is_down(R1),
            "unknown ⇒ reachable, so the resource is inert by default"
        );
        paths.set(R1, true);
        assert!(paths.is_down(R1));
        assert!(!paths.is_down(R2));
        paths.set(R1, false);
        assert!(!paths.is_down(R1), "restoring must actually clear it");
        paths.set(R1, true);
        paths.clear();
        assert!(
            !paths.is_down(R1),
            "clear() drops every blackout (scene teardown)"
        );
    }

    #[test]
    fn default_policy_is_exclusive() {
        assert_eq!(
            SessionRegistry::default().policy(),
            PossessionPolicy::Exclusive
        );
    }

    // ── Input-ack watermark (review N1) ───────────────────────────────────────

    /// **THE re-possession bug.** Client A drives rover R up to `seq = 5000` and
    /// releases; client B possesses R and its `seq` stream restarts at 1. Under the
    /// old gid-only `max(seq)` watermark the host kept stamping 5000 into every
    /// snapshot forever — B's reconcile latched 5000 as `last_reconciled`, found no
    /// prediction at that seq, and early-returned on every later (lower) ack: B's
    /// prediction was never reconciled again, and the rover it was driving drifted
    /// without bound. No attacker, no packet loss — just two players and one rover.
    #[test]
    fn ack_watermark_resets_when_a_vessel_changes_hands() {
        let mut reg = SessionRegistry::default();
        let mut applied = AppliedInputSeq::default();

        // A possesses R1 and drives it a long way into its seq stream.
        reg.claim(A, R1).expect("A claims R1");
        applied.record(R1, Some(A), 4000);
        applied.record(R1, Some(A), 5000);
        assert_eq!(applied.ack(R1), 5000);

        // A releases; B possesses. The watermark must NOT survive the handover —
        // this is what `sync_applied_seq_owners` calls on the host each time the
        // ownership table changes, i.e. BEFORE B has sent a single input.
        reg.release_session(A);
        reg.claim(B, R1).expect("B claims R1");
        applied.sync_owners(&reg);
        assert_eq!(
            applied.ack(R1),
            0,
            "a re-possessed vessel must not keep acking the previous owner's seq"
        );
        assert_eq!(applied.slot(R1).owner, Some(B));

        // B's stream starts at 1 and is acked normally from there.
        applied.record(R1, Some(B), 1);
        assert_eq!(applied.ack(R1), 1);
        applied.record(R1, Some(B), 2);
        assert_eq!(applied.ack(R1), 2);
    }

    /// The other half of the same invariant: even without a `sync_owners` pass, the
    /// first input from a NEW owner rebinds the slot instead of being compared
    /// against the previous owner's watermark.
    #[test]
    fn ack_watermark_rebinds_on_first_input_from_a_new_owner() {
        let mut applied = AppliedInputSeq::default();
        applied.record(R1, Some(A), 5000);
        applied.record(R1, Some(B), 1);
        assert_eq!(applied.ack(R1), 1, "B's seq stream is its own, not A's");
        assert_eq!(applied.slot(R1).owner, Some(B));
    }

    /// A hostile (or corrupt) `SetPorts { seq: u32::MAX }` used to poison the gid
    /// permanently for every future owner: nothing could ever exceed the watermark
    /// again, so no ack was ever "new" and the owner's reconcile early-returned for
    /// the rest of the process's life. An implausible jump is now simply dropped.
    #[test]
    fn ack_watermark_ignores_an_implausible_seq_jump() {
        let mut applied = AppliedInputSeq::default();
        applied.record(R1, Some(A), 5);
        applied.record(R1, Some(A), u32::MAX);
        assert_eq!(applied.ack(R1), 5, "u32::MAX must not become the watermark");
        // A jump just past the window is refused; one inside it is accepted.
        applied.record(R1, Some(A), 5 + MAX_SEQ_JUMP + 1);
        assert_eq!(applied.ack(R1), 5);
        applied.record(R1, Some(A), 5 + MAX_SEQ_JUMP);
        assert_eq!(applied.ack(R1), 5 + MAX_SEQ_JUMP);
    }

    /// **The other side of the same coin, and it bit this fix in review.** A client's
    /// `seq` counter is per (vessel, client) and does NOT restart when the vessel
    /// changes hands — so a peer that re-possesses a rover it drove earlier resumes at
    /// 5001 against a freshly-zeroed slot. If the plausibility window were applied to
    /// that first sample it would refuse the returning owner's every input for the
    /// rest of the session: the exact class of permanent, silent breakage N1 is about,
    /// just from the opposite direction. The first seq of a stream is the baseline.
    #[test]
    fn a_returning_owner_resumes_its_own_seq_stream() {
        let mut reg = SessionRegistry::default();
        let mut applied = AppliedInputSeq::default();
        let mut buf = BufferedClientInputs::default();

        // A drove R1 up to 5000, then B took it, then A takes it back.
        reg.claim(A, R1).unwrap();
        applied.record(R1, Some(A), 5000);
        reg.release_session(A);
        reg.claim(B, R1).unwrap();
        applied.sync_owners(&reg);
        buf.clear_gid(R1);
        reg.release_session(B);
        reg.claim(A, R1).unwrap();
        applied.sync_owners(&reg);
        buf.clear_gid(R1);

        // A's client picks up where ITS counter left off — far past the zeroed slot.
        buf.push(R1, 5001, vec![("throttle".into(), 1.0)]);
        buf.push(R1, 5002, vec![("throttle".into(), 1.0)]);
        assert!(
            buf.next_for_tick(R1, 8).is_some(),
            "the returning owner must be able to drive"
        );
        applied.record(R1, Some(A), buf.cursor(R1));
        assert_eq!(applied.ack(R1), 5001);
        buf.next_for_tick(R1, 8);
        applied.record(R1, Some(A), buf.cursor(R1));
        assert_eq!(applied.ack(R1), 5002, "and to keep being acked");
    }

    /// The watermark map is pruned on the same live set as the rest of the
    /// replication state — gids are never reused, so without this a long-lived host
    /// leaks one slot per despawned vessel forever.
    #[test]
    fn ack_watermarks_are_pruned_with_the_live_set() {
        let mut applied = AppliedInputSeq::default();
        applied.record(R1, Some(A), 3);
        applied.record(R2, Some(B), 7);
        assert_eq!(applied.len(), 2);
        applied.retain_gids(|gid| gid == R1); // R2 despawned
        assert_eq!(applied.len(), 1);
        assert_eq!(applied.ack(R2), 0);
        assert_eq!(applied.ack(R1), 3);
    }

    // ── Per-tick input consumption (review N2) ────────────────────────────────

    /// **The ack must be what the host INTEGRATED, not what it received.** The host
    /// drains the wire on the RENDER clock, so a frame can deliver K of the client's
    /// per-fixed-tick inputs at once; physics still runs exactly one of them per
    /// fixed tick. `cursor` is that "one per tick" watermark — acking `max(seq)` at
    /// receive time claimed K inputs applied when one had been, the client dropped
    /// K−1 predicted frames it had really simulated, and the divergence scaled with
    /// how much the input CHANGED across those frames (i.e. it showed up on turns
    /// and stops — the post-turn oscillation).
    #[test]
    fn buffered_inputs_are_consumed_one_per_tick_and_the_ack_follows() {
        let mut buf = BufferedClientInputs::default();
        let mut applied = AppliedInputSeq::default();
        // One render frame delivers three ticks' worth of steering input.
        buf.push(R1, 1, vec![("steer".into(), 0.0)]);
        buf.push(R1, 2, vec![("steer".into(), 0.5)]);
        buf.push(R1, 3, vec![("steer".into(), 1.0)]);
        assert_eq!(buf.cursor(R1), 0, "nothing integrated yet");

        for expected_seq in 1..=3u32 {
            let writes = buf.next_for_tick(R1, 8).expect("one input per fixed tick");
            assert_eq!(buf.cursor(R1), expected_seq);
            applied.record(R1, Some(A), buf.cursor(R1));
            assert_eq!(
                applied.ack(R1),
                expected_seq,
                "the ack must name the seq physics actually ran this tick"
            );
            // …and each tick sees its OWN steer value, not just the last one.
            let steer = writes.iter().find(|(n, _)| n == "steer").map(|(_, v)| *v);
            assert_eq!(steer, Some((expected_seq as f64 - 1.0) * 0.5));
        }
        // Nothing new: the last input latches (the consumer stays the port authority)
        // and the ack does NOT advance past what was integrated.
        assert!(buf.next_for_tick(R1, 8).is_some());
        assert_eq!(applied.slot(R1).seq, 3);
    }

    /// A vessel that changes hands must not hand the new owner the PREVIOUS owner's
    /// queued-but-unintegrated inputs — that would be a control leak, not just a
    /// stale ack. (`sync_applied_seq_owners` calls `clear_gid` for exactly this.)
    #[test]
    fn buffered_inputs_are_dropped_when_a_vessel_changes_hands() {
        let mut buf = BufferedClientInputs::default();
        buf.push(R1, 1, vec![("throttle".into(), 1.0)]);
        buf.push(R1, 2, vec![("throttle".into(), 1.0)]);
        buf.clear_gid(R1);
        assert!(
            buf.next_for_tick(R1, 8).is_none(),
            "A's inputs must not drive B's vessel"
        );
        assert_eq!(buf.cursor(R1), 0);
    }

    // ── Desync gauge (review N3) ──────────────────────────────────────────────

    /// A body that keeps diverging past the threshold raises the "I diverged" signal
    /// exactly ONCE per streak (so the caller logs, rather than spamming per tick),
    /// and a body that returns to tolerance resets — with the max preserved.
    #[test]
    fn divergence_gauge_signals_once_per_sustained_streak() {
        let mut stats = DivergenceStats::default();
        let over = stats.warn_m + 0.5;
        let mut signals = 0;
        for _ in 0..(stats.warn_streak * 3) {
            if stats.observe(R1, PredictionKind::Owned, over) {
                signals += 1;
            }
        }
        assert_eq!(
            signals, 1,
            "one signal per sustained divergence, not one per tick"
        );
        assert_eq!(stats.worst(), Some((R1, over)));

        // Back in tolerance → streak resets, max is remembered.
        assert!(!stats.observe(R1, PredictionKind::Owned, 0.01));
        assert_eq!(stats.bodies[&R1].over_streak, 0);
        assert_eq!(stats.bodies[&R1].max_m, over);
        // A hard snap to authority is a rebaseline, and it is counted (it used to be
        // entirely silent — the netcode's loudest symptom, invisible in the field).
        stats.note_rebaseline(R1);
        assert_eq!(stats.bodies[&R1].rebaselines, 1);
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
        // No mission has declared a blackout: the gate must behave exactly as before.
        let paths = ControlPathRegistry::default();
        let mut reg = SessionRegistry::default();
        reg.claim(A, R1).unwrap();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(
            A.0,
            UserSession {
                session_id: A,
                username: "Player A".to_string(),
                role: AuthorityRole::Operator,
                authenticated: true,
                token: Some("srv-token-a".to_string()),
            },
        );
        rbac.sessions.insert(
            B.0,
            UserSession {
                session_id: B,
                username: "Player B".to_string(),
                role: AuthorityRole::Operator,
                authenticated: true,
                token: Some("srv-token-b".to_string()),
            },
        );

        // The owner may issue control commands.
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "SetPorts", Some(R1)).is_ok());
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "SetPorts", Some(R1)).is_ok());
        // A non-owner may not.
        assert!(authorize(&reg, &rbac, &pol, &paths, B, "SetPorts", Some(R1)).is_err());
        // A control command with no target is rejected.
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "SetPorts", None).is_err());
        // Possession + structural commands are always allowed (arbitration is in
        // `claim`, not the authority gate).
        assert!(authorize(&reg, &rbac, &pol, &paths, B, "PossessVessel", Some(R1)).is_ok());
        assert!(authorize(&reg, &rbac, &pol, &paths, B, "SpawnEntity", None).is_ok());

        // An authenticated Observer that *owns* the rover may still drive it:
        // ownership is the gate, not the Operator role. (A client connects as
        // Observer and may never send an UpdateProfile to be promoted.)
        let mut observer_rbac = SessionRbac::default();
        for s in [A, B] {
            observer_rbac.sessions.insert(
                s.0,
                UserSession {
                    session_id: s,
                    username: "Observer".to_string(),
                    role: AuthorityRole::Observer,
                    authenticated: true,
                    token: Some(format!("srv-token-{}", s.0)),
                },
            );
        }
        // Owner-Observer A may drive what it owns, and possess/structural commands.
        assert!(authorize(&reg, &observer_rbac, &pol, &paths, A, "SetPorts", Some(R1)).is_ok());
        assert!(authorize(
            &reg,
            &observer_rbac,
            &pol,
            &paths,
            A,
            "PossessVessel",
            Some(R1)
        )
        .is_ok());
        // An authenticated non-owner is still rejected by the ownership gate.
        assert!(authorize(&reg, &observer_rbac, &pol, &paths, B, "SetPorts", Some(R1)).is_err());

        // The authenticated FLOOR remains: an UNauthenticated session is rejected
        // even for an owned entity (RBAC infra stays wired, just not role-gated).
        let mut unauth_rbac = SessionRbac::default();
        unauth_rbac.sessions.insert(
            A.0,
            UserSession {
                session_id: A,
                username: "Player A".to_string(),
                role: AuthorityRole::Observer,
                authenticated: false,
                token: None,
            },
        );
        assert!(authorize(&reg, &unauth_rbac, &pol, &paths, A, "SetPorts", Some(R1)).is_err());

        // M2: a session that is `authenticated` but carries NO server-issued token
        // is rejected even for an entity it owns. This is the gate that stops a
        // name-only `UpdateProfile` from minting authority — a credential the
        // server (not the client) created is now required.
        let mut tokenless_rbac = SessionRbac::default();
        tokenless_rbac.sessions.insert(
            A.0,
            UserSession {
                session_id: A,
                username: "Player A".to_string(),
                role: AuthorityRole::Operator,
                authenticated: true,
                token: None,
            },
        );
        assert!(authorize(&reg, &tokenless_rbac, &pol, &paths, A, "SetPorts", Some(R1)).is_err());
        assert!(authorize(
            &reg,
            &tokenless_rbac,
            &pol,
            &paths,
            A,
            "PossessVessel",
            Some(R1)
        )
        .is_err());
    }

    #[test]
    fn unregistered_command_is_open_by_default() {
        // The RBAC-readiness invariant: a command with no declared policy resolves
        // to OPEN, so any authenticated Observer may issue it with no target. This
        // keeps "everything works by default" true while the gate is data-driven.
        let pol = CommandPolicyRegistry::default();
        // No mission has declared a blackout: the gate must behave exactly as before.
        let paths = ControlPathRegistry::default();
        assert_eq!(pol.policy_for("SomeBrandNewCommand"), CommandPolicy::OPEN);

        let reg = SessionRegistry::default();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(
            A.0,
            UserSession {
                session_id: A,
                username: "Observer".to_string(),
                role: AuthorityRole::Observer,
                authenticated: true,
                token: Some("srv-token-a".to_string()),
            },
        );
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "SomeBrandNewCommand", None).is_ok());
    }

    #[test]
    fn override_tightens_a_command_without_touching_the_gate() {
        // The RBAC switch: an operator locks `SpawnEntity` down to `Operator` at
        // runtime. The gate code is unchanged — only data in the registry differs.
        let mut pol = CommandPolicyRegistry::default();
        let paths = ControlPathRegistry::default();
        pol.set_override(
            "SpawnEntity",
            CommandPolicy {
                min_role: AuthorityRole::Operator,
                ownership_gated: false,
            },
        );

        let reg = SessionRegistry::default();
        let mut rbac = SessionRbac::default();
        rbac.sessions.insert(
            A.0,
            UserSession {
                session_id: A,
                username: "Observer".to_string(),
                role: AuthorityRole::Observer,
                authenticated: true,
                token: Some("srv-token-a".to_string()),
            },
        );
        rbac.sessions.insert(
            B.0,
            UserSession {
                session_id: B,
                username: "Operator".to_string(),
                role: AuthorityRole::Operator,
                authenticated: true,
                token: Some("srv-token-b".to_string()),
            },
        );

        // Observer is rejected for the tightened command…
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "SpawnEntity", None).is_err());
        // …a still-open command (PossessVessel) is unaffected…
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "PossessVessel", None).is_ok());
        // …and an Operator passes the tightened command.
        assert!(authorize(&reg, &rbac, &pol, &paths, B, "SpawnEntity", None).is_ok());

        // Clearing the override restores open-by-default.
        pol.clear_override("SpawnEntity");
        assert!(authorize(&reg, &rbac, &pol, &paths, A, "SpawnEntity", None).is_ok());
    }
    // NOTE: the scripted-authorization-hook test lives in
    // `tests/authz_hook.rs` (its own test binary), because it registers under the
    // process-global `AUTHORIZE_HOOK` id — doing so in this binary would race the
    // other `authorize()` unit tests running on parallel threads.
}
