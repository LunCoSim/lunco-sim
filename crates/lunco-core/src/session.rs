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
pub struct WireApplyGuard(pub Option<SessionId>);

impl WireApplyGuard {
    pub fn is_from_wire(&self) -> bool {
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
/// [`WireApplyGuard`] / [`NetworkRole`], not by ownership.
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

/// Suppress the USD loader's automatic [`crate::Provenance::Content`] stamping
/// for a runtime-instanced subtree. Runtime instances get server-allocated
/// identity (root `Authoritative`, children left un-networked) rather than
/// content-derived ids — otherwise two instances of the same asset would derive
/// the **same** id and collide (DESIGN_GAPS B.1 / gap G2). Placed on the spawn
/// root; the loader skips its `Content` stamp for the root and its descendants.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SkipContentStamp;

/// The host replicates this entity's `Transform` to clients via snapshots.
/// Added to runtime-spawned networked roots (rovers). Carried on the client
/// proxy too so tooling can see it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct NetReplicate;

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
}

/// Client-side unacked input logs keyed by [`crate::GlobalEntityId`] raw `u64`.
/// Populated only for vessels this peer owns + predicts (`OwnedLocally`); the
/// reconcile drops acked frames and replays the rest over the owned avian body.
/// Empty on host/standalone.
#[derive(Resource, Default)]
pub struct OwnedInputLog(pub HashMap<u64, VesselInputLog>);

/// Host-side record of the highest input `seq` applied per gid, written when a
/// `DriveRover`/`BrakeRover` is authorized + applied. Stamped into each snapshot's
/// `last_input_seq` so the owning client knows how far the authoritative sim has
/// integrated its inputs (the reconcile ack). Empty on client/standalone.
#[derive(Resource, Default)]
pub struct AppliedInputSeq(pub HashMap<u64, u32>);

/// The single authority gate. Given the `origin` session of a command, the
/// command's short type name, and the target entity's [`crate::GlobalEntityId`]
/// (raw), decide whether the host applies it.
///
/// MVP policy:
/// - `DriveRover` / `BrakeRover` (state-producing control): allowed only if
///   `origin` **owns** the target. Cross-rover control is rejected (G4).
/// - everything else (possession claims, spawns, structural edits): allowed —
///   possession arbitration happens in the handler via [`SessionRegistry::claim`].
///
/// Single-player never reaches here (capture no-ops under `Standalone`).
pub fn authorize(
    reg: &SessionRegistry,
    origin: SessionId,
    type_name: &str,
    target_gid: Option<u64>,
) -> Result<(), Reject> {
    match type_name {
        "DriveRover" | "BrakeRover" => match target_gid {
            Some(gid) if reg.owns(origin, gid) => Ok(()),
            Some(_) => Err(Reject::InvalidOp(format!(
                "session {origin} is not authorized to {type_name} that entity"
            ))),
            None => Err(Reject::InvalidOp(format!("{type_name} has no target"))),
        },
        _ => Ok(()),
    }
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
        let mut reg = SessionRegistry::default();
        reg.claim(A, R1).unwrap();
        // The owner may issue control commands.
        assert!(authorize(&reg, A, "DriveRover", Some(R1)).is_ok());
        assert!(authorize(&reg, A, "BrakeRover", Some(R1)).is_ok());
        // A non-owner may not.
        assert!(authorize(&reg, B, "DriveRover", Some(R1)).is_err());
        // A control command with no target is rejected.
        assert!(authorize(&reg, A, "DriveRover", None).is_err());
        // Possession + structural commands are always allowed (arbitration is in
        // `claim`, not the authority gate).
        assert!(authorize(&reg, B, "PossessVessel", Some(R1)).is_ok());
        assert!(authorize(&reg, B, "SpawnEntity", None).is_ok());
    }
}
