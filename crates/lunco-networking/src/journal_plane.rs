//! The **journal replication plane** — one of the networking replication
//! mechanisms (see the plane taxonomy in `NETWORKING_ASSET_SYNC_DESIGN.md`).
//!
//! It ships authored Twin-journal entries host→client so peers converge on the
//! same **mergeable edit history** (`Journal::append_remote`, which merges
//! divergent branches deterministically). This plane is deliberately separate
//! from the others, by replication *semantics*:
//!
//! - **Command** plane — ephemeral control/structural *intent* (DriveRover…),
//!   replayed once. Authored document edits do NOT ride it.
//! - **State** plane — continuous physics pose/velocity, overwrite + interpolate.
//! - **Content** plane — immutable file bytes by CID.
//! - **Journal** plane (this) — authored, mergeable document *history*.
//!
//! The module owns the plane's wire type, its outbound producer
//! ([`broadcast_journal_entries`]), its inbound apply ([`apply_inbound_entry`]),
//! the late-joiner full replay ([`full_journal_msgs`]), and peer-identity
//! stamping. The transport ferry (`sync::drain_sync_inbox`) only *routes* the
//! `JournalEntry` envelope here — it holds no journal logic itself.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use lunco_core::NetworkRole;
use lunco_doc_bevy::JournalResource;
use lunco_twin_journal::{AuthorId, JournalEntry};

use crate::sync::{SyncEnvelope, SyncOutbox};
use lunco_core::SyncChannel;

/// Host → client: one Twin-journal entry, carried as **JSON text** (not the
/// typed [`JournalEntry`]) because it rides the positional `bincode` codec,
/// which can't (de)serialize the `serde_json::Value` inside `EntryKind::Op` —
/// the same reason `SyncCommand` carries its payload as a string. The client
/// `serde_json::from_str`s it and feeds `Journal::append_remote` (merge).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntryMsg {
    /// `serde_json::to_string(&JournalEntry)`.
    pub json: String,
}

// ── Peer identity ─────────────────────────────────────────────────────────────

/// Peer-unique journal author for a client session. Keeps `(author, lamport)`
/// entry ids globally distinct from the host and other clients — without this,
/// two peers mint colliding ids and `append_remote` dedups a remote entry as a
/// local one. (Session-scoped for now; a stable per-install id would survive
/// reconnects — a later refinement.)
pub fn peer_author(session: u64) -> AuthorId {
    AuthorId::new(format!("peer-{session}"))
}

/// The host's stable journal author. One host per session, so a fixed id is
/// unique against every `peer-<n>` client.
pub fn host_author() -> AuthorId {
    AuthorId::new("host")
}

/// Host startup: stamp the host's peer-unique journal author so its authored
/// edits are attributable and never collide with a client's. Clients stamp
/// `peer-<session>` on handshake instead (see `sync::drain_sync_inbox`). No-op
/// off the host / when no journal is present. Idempotent.
pub fn stamp_host_journal_author(role: Res<NetworkRole>, journal: Option<Res<JournalResource>>) {
    if !role.is_host() {
        return;
    }
    if let Some(j) = journal {
        if j.local_author() != host_author() {
            j.set_local_author(host_author());
        }
    }
}

// ── Wire (de)serialization ──────────────────────────────────────────────────

fn to_msg(entry: &JournalEntry) -> Option<JournalEntryMsg> {
    serde_json::to_string(entry)
        .ok()
        .map(|json| JournalEntryMsg { json })
}

/// All current journal entries as wire messages, in log order — the full replay
/// a late joiner needs on connect (the server streams these to the new peer).
pub fn full_journal_msgs(journal: &JournalResource) -> Vec<JournalEntryMsg> {
    journal.with_read(|j| j.entries().filter_map(to_msg).collect())
}

// ── Inbound apply (client) ────────────────────────────────────────────────────

/// Apply an inbound host entry into the local journal, merging via
/// `append_remote` (idempotent + convergent). Called by the transport router
/// only on a client; the host is the sole source in this one-way phase.
pub fn apply_inbound_entry(journal: &JournalResource, msg: &JournalEntryMsg) {
    match serde_json::from_str::<JournalEntry>(&msg.json) {
        Ok(entry) => journal.with_write(|j| j.append_remote(entry)),
        Err(e) => warn!("[journal-plane] bad entry from host: {e}"),
    }
}

// ── Outbound produce (host) ────────────────────────────────────────────────────

/// Host: broadcast newly-appended journal entries to all clients so their
/// journals converge with the host's. Ships the tail of the log past a
/// monotonic cursor (resends from 0 if the log shrank — journal replaced —
/// since clients dedupe by `EntryId`). Late joiners also get the full journal on
/// connect ([`full_journal_msgs`]); the overlap is harmless (idempotent).
/// Reliable `BulkData` lane (edit history, not per-tick state). Host-only.
pub fn broadcast_journal_entries(
    role: Res<NetworkRole>,
    journal: Option<Res<JournalResource>>,
    mut outbox: ResMut<SyncOutbox>,
    mut sent: Local<usize>,
) {
    if !role.is_host() {
        return;
    }
    let Some(journal) = journal else {
        return;
    };
    journal.with_read(|j| {
        let total = j.len();
        if total < *sent {
            *sent = 0; // journal replaced → resend (clients dedupe by EntryId)
        }
        if total == *sent {
            return;
        }
        for entry in j.entries().skip(*sent) {
            if let Some(msg) = to_msg(entry) {
                outbox
                    .0
                    .push((SyncChannel::BulkData, SyncEnvelope::JournalEntry(msg)));
            }
        }
        *sent = total;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_and_host_authors_are_distinct() {
        assert_ne!(peer_author(1), host_author());
        assert_ne!(peer_author(1), peer_author(2));
        assert_eq!(peer_author(7), peer_author(7));
    }
}
