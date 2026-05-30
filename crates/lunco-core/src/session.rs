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
use std::collections::HashMap;

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

/// Server-side ownership: which session may issue authoritative commands against
/// which entity (keyed by [`crate::GlobalEntityId`] raw `u64`). The home of the
/// single [`authorize`] gate. On a pure client this stays empty (the client
/// trusts the host); the client's optimistic local apply is gated by
/// [`WireApplyGuard`] / [`NetworkRole`], not by ownership.
#[derive(Resource, Default, Debug)]
pub struct SessionRegistry {
    /// rover gid → owning session.
    owners: HashMap<u64, SessionId>,
}

impl SessionRegistry {
    /// Record `session` as the owner of `gid`. **Exclusive**: returns
    /// `Err(current_owner)` if a *different* session already owns it (possession
    /// is not stealable in the MVP). Idempotent for the same session.
    pub fn claim(&mut self, session: SessionId, gid: u64) -> Result<(), SessionId> {
        match self.owners.get(&gid) {
            Some(&cur) if cur != session => Err(cur),
            _ => {
                self.owners.insert(gid, session);
                Ok(())
            }
        }
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
    pub t: [f32; 3],
    pub r: [f32; 4],
}

/// Inbound transform samples awaiting application on a client.
#[derive(Resource, Default)]
pub struct IncomingSnapshots(pub Vec<SnapshotSample>);

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
