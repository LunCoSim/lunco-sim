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
use std::collections::HashSet;

use lunco_core::NetworkRole;
use lunco_doc_bevy::JournalResource;
use lunco_twin_journal::{AuthorId, DomainKind, EntryId, EntryKind, JournalEntry};

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

// ── Inbound apply (both roles) ────────────────────────────────────────────────

/// Apply an inbound peer entry into the local journal, merging via
/// `append_remote` (idempotent + convergent). Called by the transport router on
/// **both** roles: a client mirrors the host's edits, and the host merges each
/// client's edits into its journal — from which [`broadcast_journal_entries`]
/// then relays them out to the *other* clients (the host is the fan-out hub, so
/// peer A's edit reaches peer B). Idempotent, so the host re-receiving an entry
/// it already relayed, or a client seeing its own edit echoed back, is a no-op.
pub fn apply_inbound_entry(journal: &JournalResource, msg: &JournalEntryMsg) {
    match serde_json::from_str::<JournalEntry>(&msg.json) {
        Ok(entry) => journal.with_write(|j| j.append_remote(entry)),
        Err(e) => warn!("[journal-plane] bad inbound entry: {e}"),
    }
}

// ── Outbound produce (both roles) ──────────────────────────────────────────────

/// Broadcast newly-appended journal entries so every peer's journal converges.
/// Runs on **both** roles (bidirectional), with role-asymmetric fan-out:
///
/// - **Host** — the relay hub: ships *every* new tail entry (its own authored
///   edits *and* client edits merged in via [`apply_inbound_entry`]) to
///   `NetworkTarget::All`, so peer A's edit reaches peer B. Overlap with the
///   origin peer is harmless (dedup by `EntryId`).
/// - **Client** — ships only its **own** authored entries to the host; it never
///   relays entries it received (the host already holds those and does the
///   fan-out), avoiding needless echo. Foreign entries still advance the cursor.
///
/// Ships the tail past a monotonic cursor (resends from 0 if the log shrank —
/// journal replaced — since peers dedupe by `EntryId`). Late joiners also get
/// the full journal on connect ([`full_journal_msgs`]); overlap is harmless.
/// Reliable `BulkData` lane (edit history, not per-tick state).
pub fn broadcast_journal_entries(
    role: Res<NetworkRole>,
    journal: Option<Res<JournalResource>>,
    mut outbox: ResMut<SyncOutbox>,
    mut sent: Local<usize>,
) {
    if !role.is_networked() {
        return;
    }
    let Some(journal) = journal else {
        return;
    };
    let is_host = role.is_host();
    let me = journal.local_author();
    journal.with_read(|j| {
        let total = j.len();
        if total < *sent {
            *sent = 0; // journal replaced → resend (peers dedupe by EntryId)
        }
        if total == *sent {
            return;
        }
        for entry in j.entries().skip(*sent) {
            // A client relays nothing — only its own authored edits go up to the
            // host, which is the sole fan-out hub. The host ships everything.
            if !is_host && entry.id.author != me {
                continue;
            }
            if let Some(msg) = to_msg(entry) {
                outbox
                    .0
                    .push((SyncChannel::BulkData, SyncEnvelope::JournalEntry(msg)));
            }
        }
        *sent = total;
    });
}

// ── Layer B: journal → scene replay selection (client) ────────────────────────

/// Select the journal Op entries a client should **replay onto its scene** to
/// see the host's live edits: the convergent-ordered
/// ([`merged_order_ids`](lunco_twin_journal::Journal::merged_order_ids)) entries
/// strictly **after** the base `head` (the scenario snapshot the client
/// downloaded), authored by a DIFFERENT peer than `me` (skip the client's own
/// edits — already applied locally), of **USD** domain, not already applied.
/// Returns `(EntryId, op payload)` in apply order. Pure over the journal +
/// inputs (unit-tested); the Bevy driver applies each via
/// `lunco_usd::UsdDocumentRegistry::replay_op` and records the id.
///
/// - `base = None` ⇒ the scenario had no journal head at build (empty history)
///   ⇒ every remote USD op is new.
/// - `base = Some(h)` but `h` not yet in the journal (its full replay hasn't
///   arrived) ⇒ return nothing (defer) rather than risk double-applying the
///   baked history the downloaded files already reflect.
pub fn scene_ops_after(
    journal: &JournalResource,
    base: Option<&EntryId>,
    me: &AuthorId,
    already: &HashSet<EntryId>,
) -> Vec<(EntryId, serde_json::Value)> {
    journal.with_read(|j| {
        let order = j.merged_order_ids();
        let start = match base {
            None => 0,
            Some(h) => match order.iter().position(|id| id == h) {
                Some(i) => i + 1,
                None => return Vec::new(), // base not arrived → defer
            },
        };
        order[start..]
            .iter()
            .filter_map(|id| {
                if already.contains(id) {
                    return None;
                }
                let e = j.get(id)?;
                if &e.id.author == me {
                    return None; // client's own edit — already applied locally
                }
                match &e.kind {
                    EntryKind::Op { domain: DomainKind::Usd, op, .. } => {
                        Some((id.clone(), op.clone()))
                    }
                    _ => None,
                }
            })
            .collect()
    })
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

    #[test]
    fn scene_ops_after_selects_remote_usd_ops_past_base() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, JournalEntry, LifecycleKind, TwinId};

        let me = AuthorId::new("peer-1");
        let journal = JournalResource::new(TwinId::new("t"), me.clone());
        let host = |lam: u64| EntryId { author: AuthorId::new("host"), lamport: lam };
        let mk = |lam: u64, kind: EntryKind| JournalEntry {
            id: host(lam),
            parents: if lam <= 1 { vec![] } else { vec![host(lam - 1)] },
            author: AuthorTag { user: "host".into(), tool: "t".into() },
            at_ms: 0,
            twin: TwinId::new("t"),
            doc: DocumentId::new(1),
            kind,
            change_set: None,
        };
        let usd = |v: i32| EntryKind::Op {
            domain: DomainKind::Usd,
            op: serde_json::json!({ "v": v }),
            inverse: serde_json::json!({}),
        };
        journal.with_write(|j| {
            j.append_remote(mk(1, usd(1)));
            j.append_remote(mk(2, usd(2)));
            j.append_remote(mk(3, EntryKind::Lifecycle(LifecycleKind::Saved))); // not an Op
            j.append_remote(mk(4, usd(4)));
        });
        let none = HashSet::new();
        let lam = |v: &[(EntryId, serde_json::Value)]| v.iter().map(|(id, _)| id.lamport).collect::<Vec<_>>();

        // base = e1 (downloaded snapshot) → apply the newer USD ops (2, 4); the
        // lifecycle entry (3) and the baked base (1) are excluded.
        assert_eq!(lam(&scene_ops_after(&journal, Some(&host(1)), &me, &none)), vec![2, 4]);
        // base = None → every remote USD op is new.
        assert_eq!(lam(&scene_ops_after(&journal, None, &me, &none)), vec![1, 2, 4]);
        // base present but not yet received → defer (don't double-apply history).
        assert!(scene_ops_after(&journal, Some(&host(99)), &me, &none).is_empty());
        // Already-applied ids are skipped.
        let done: HashSet<_> = [host(2)].into_iter().collect();
        assert_eq!(lam(&scene_ops_after(&journal, Some(&host(1)), &me, &done)), vec![4]);
    }
}
