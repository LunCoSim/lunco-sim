//! The **journal replication plane** — one of the networking replication
//! mechanisms (see the plane taxonomy in `NETWORKING_ASSET_SYNC_DESIGN.md`).
//!
//! It ships authored Twin-journal entries host→client so peers converge on the
//! same **mergeable edit history** (`Journal::append_remote`, which merges
//! divergent branches deterministically). This plane is deliberately separate
//! from the others, by replication *semantics*:
//!
//! - **Command** plane — ephemeral control/structural *intent* (SetPorts and
//!   ReleaseControl…), replayed once. Authored document edits do NOT ride it.
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

use lunco_core_session::NetworkRole;
use lunco_doc_bevy::JournalResource;
use lunco_storage::{FileStorage, Storage, StorageHandle};
use lunco_twin_journal::{AuthorId, DomainKind, EntryId, EntryKind, JournalEntry};

use crate::sync::{SyncEnvelope, SyncOutbox};
use lunco_command_contracts::SyncChannel;

/// Host → client: one Twin-journal entry, carried as **JSON text** (not the
/// typed [`JournalEntry`]) because it rides the positional `bincode` codec,
/// which can't (de)serialize the `serde_json::Value` inside `EntryKind::Op` —
/// the same reason `SyncCommand` carries its payload as a string. The client
/// `serde_json::from_str`s it and feeds `Journal::append_remote` (merge).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntryMsg {
    pub scope: crate::scope::WireSceneScope,
    /// `serde_json::to_string(&JournalEntry)`.
    pub json: String,
}

// ── Peer identity ─────────────────────────────────────────────────────────────

/// This peer's **local journal author** — durable across restarts and useful for
/// standalone/offline authoring. A network handshake replaces it with the
/// server-issued connection author before networked entries are applied, so a
/// payload cannot impersonate another live peer. Precedence:
///
/// 1. `LUNCO_PEER_ID` env override — distinct ids for multiple instances on ONE
///    machine (tests, `net_smoke`, `run_host_client`), which otherwise share the
///    persisted install id and would collide.
/// 2. A per-install id persisted in the user config dir (`identity/peer_id`),
///    minted once from fresh entropy and reused forever — the real-product path.
/// 3. A fresh random id if the config dir can't be read/written (never collides
///    within a run; just not durable — logged).
pub fn local_author_id() -> AuthorId {
    let override_id = std::env::var("LUNCO_PEER_ID").ok();
    if let Some(author) = author_from_override(override_id.as_deref()) {
        return author;
    }
    AuthorId::new(persisted_install_id())
}

fn author_from_override(value: Option<&str>) -> Option<AuthorId> {
    let value = value?.trim();
    (!value.is_empty()).then(|| AuthorId::new(value))
}

/// Canonical journal author for a live network connection. The server chooses
/// this value and sends it in the handshake; payloads may not choose an author.
pub fn author_for_session(session: lunco_command_contracts::SessionId) -> String {
    format!("net-session-{:016x}", session.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn persisted_install_id() -> String {
    let path = lunco_settings::user_config_dir()
        .join("identity")
        .join("peer_id");
    let storage = FileStorage::new();
    let handle = StorageHandle::File(path.clone());
    if let Ok(existing) = storage.read_sync(&handle) {
        let Ok(existing) = String::from_utf8(existing) else {
            return fresh_install_id(&storage, &path, &handle);
        };
        let id = existing.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    fresh_install_id(&storage, &path, &handle)
}

#[cfg(not(target_arch = "wasm32"))]
fn fresh_install_id(
    storage: &FileStorage,
    path: &std::path::Path,
    handle: &StorageHandle,
) -> String {
    let fresh = format!("peer-{:016x}", lunco_id::random_u64());
    if let Some(parent) = path.parent() {
        let parent_handle = StorageHandle::File(parent.to_path_buf());
        if let Err(error) = storage.ensure_directory_sync(&parent_handle) {
            warn!(
                "[journal-plane] could not prepare install-id directory {}: {error}",
                parent.display()
            );
        }
    }
    // The identity file is a small local bootstrap record. It is not a network
    // credential: live network journal entries are rebound to the server-issued
    // connection author in `apply_inbound_entry`.
    if let Err(error) = storage.write_sync(handle, fresh.as_bytes()) {
        warn!(
            "[journal-plane] could not persist install id to {}: {error}",
            path.display()
        );
    } else {
        info!(
            "[journal-plane] minted install id {fresh} at {}",
            path.display()
        );
    }
    fresh
}

#[cfg(target_arch = "wasm32")]
fn persisted_install_id() -> String {
    // TODO(web-identity): persist via localStorage/OPFS so a browser peer keeps a
    // stable identity across reloads. For now a per-page-load id (stable within a
    // session, not across reloads).
    format!("peer-{:016x}", lunco_id::random_u64())
}

/// Stamp this peer's local install id ([`local_author_id`]) as the journal's
/// initial author. Networked clients receive a connection-bound author in the
/// handshake; standalone sessions keep this durable local identity.
pub fn stamp_local_journal_author(journal: Option<Res<JournalResource>>) {
    if let Some(j) = journal {
        let me = local_author_id();
        if j.local_author() != me {
            j.set_local_author(me);
        }
    }
}

// ── Wire (de)serialization ──────────────────────────────────────────────────

fn to_msg(
    entry: &JournalEntry,
    scope: lunco_core_session::ReplicationScope,
) -> Option<JournalEntryMsg> {
    serde_json::to_string(entry)
        .ok()
        .map(|json| JournalEntryMsg {
            scope: crate::scope::wire_scope(scope),
            json,
        })
}

/// All current journal entries as wire messages, in log order — the full replay
/// a late joiner needs on connect (the server streams these to the new peer).
pub fn full_journal_msgs(
    journal: &JournalResource,
    scope: lunco_core_session::ReplicationScope,
) -> Vec<JournalEntryMsg> {
    journal.with_read(|j| {
        j.entries()
            .filter_map(|entry| to_msg(entry, scope))
            .collect()
    })
}

// ── Inbound apply (both roles) ────────────────────────────────────────────────

/// Apply an inbound peer entry into the local journal, merging via
/// `append_remote` (idempotent + convergent). Called by the transport router on
/// **both** roles: a client mirrors the host's edits, and the host merges each
/// client's edits into its journal — from which [`broadcast_journal_entries`]
/// then relays them out to the *other* clients (the host is the fan-out hub, so
/// peer A's edit reaches peer B). Idempotent, so the host re-receiving an entry
/// it already relayed, or a client seeing its own edit echoed back, is a no-op.
pub fn apply_inbound_entry(
    journal: &JournalResource,
    msg: &JournalEntryMsg,
    canonical_author: Option<AuthorId>,
    scope: lunco_core_session::ReplicationScope,
) -> Option<EntryId> {
    Some(merge_wire_entry(
        journal,
        decode_wire_entry(msg, scope)?,
        canonical_author,
    ))
}

fn decode_wire_entry(
    msg: &JournalEntryMsg,
    scope: lunco_core_session::ReplicationScope,
) -> Option<JournalEntry> {
    if crate::scope::internal_scope(msg.scope) != Some(scope) {
        warn!("[journal-plane] rejected entry from a mismatched scene owner");
        return None;
    }
    match serde_json::from_str(&msg.json) {
        Ok(entry) => Some(entry),
        Err(error) => {
            warn!("[journal-plane] bad inbound entry: {error}");
            None
        }
    }
}
fn merge_wire_entry(
    journal: &JournalResource,
    mut entry: JournalEntry,
    canonical_author: Option<AuthorId>,
) -> EntryId {
    if let Some(author) = canonical_author {
        entry.id.author = author.clone();
        entry.author.user = author.0;
        // The host's admitted journal owns storage identity; remote native
        // roots are peer-local and never choose the host persistence target.
        entry.twin = journal.with_read(|journal| journal.twin().clone());
    }
    let id = entry.id.clone();
    journal.with_write(|journal| journal.append_remote(entry));
    id
}

/// A remote Twin's merge state cannot share entry-ID slots with a different
/// mounted Twin or the local persistent journal.
#[derive(Resource, Default)]
pub struct ReplicatedJournal {
    mirror: Option<(
        lunco_core_session::ReplicationScope,
        Entity,
        JournalResource,
    )>,
    local_tail: Option<(
        lunco_core_session::ReplicationScope,
        Entity,
        lunco_twin_journal::TwinId,
        usize,
    )>,
}
impl ReplicatedJournal {
    pub fn for_owner(
        &self,
        scope: lunco_core_session::ReplicationScope,
        connection: Option<Entity>,
    ) -> Option<JournalResource> {
        let (owner, transport, journal) = self.mirror.as_ref()?;
        (*owner == scope && Some(*transport) == connection).then(|| journal.clone())
    }
    pub fn admit_local_tail(
        &mut self,
        scope: lunco_core_session::ReplicationScope,
        connection: Entity,
        journal: &JournalResource,
    ) {
        let (identity, length) =
            journal.with_read(|journal| (journal.twin().clone(), journal.len()));
        self.local_tail = Some((scope, connection, identity, length));
    }
    pub fn local_tail_for(
        &self,
        scope: lunco_core_session::ReplicationScope,
        connection: Option<Entity>,
        journal: &JournalResource,
    ) -> Option<usize> {
        let (owner, transport, identity, cursor) = self.local_tail.as_ref()?;
        (*owner == scope
            && Some(*transport) == connection
            && journal.with_read(|journal| journal.twin() == identity))
        .then_some(*cursor)
    }
    pub fn retire(&mut self, scope: lunco_core_session::ReplicationScope) {
        if self
            .mirror
            .as_ref()
            .is_some_and(|(owner, _, _)| *owner == scope)
        {
            self.mirror = None;
        }
        if self
            .local_tail
            .as_ref()
            .is_some_and(|(owner, _, _, _)| *owner == scope)
        {
            self.local_tail = None;
        }
    }
    pub fn clear(&mut self) {
        self.mirror = None;
        self.local_tail = None;
    }
    pub fn apply(
        &mut self,
        msg: &JournalEntryMsg,
        scope: lunco_core_session::ReplicationScope,
        connection: Entity,
        local_author: AuthorId,
    ) -> Option<EntryId> {
        Some(self.merge(
            decode_wire_entry(msg, scope)?,
            scope,
            connection,
            local_author,
        ))
    }
    fn merge(
        &mut self,
        entry: JournalEntry,
        scope: lunco_core_session::ReplicationScope,
        connection: Entity,
        local_author: AuthorId,
    ) -> EntryId {
        if self.for_owner(scope, Some(connection)).is_none() {
            self.mirror = Some((
                scope,
                connection,
                JournalResource::new(entry.twin.clone(), local_author.clone()),
            ));
        }
        let journal = &self.mirror.as_ref().expect("mirror admitted above").2;
        journal.with_write(|journal| journal.set_local_author(local_author));
        merge_wire_entry(journal, entry, None)
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
    facts: crate::scope::SceneScopeFacts,
    application: Res<crate::scope::ApplicationJournalBinding>,
    connection: Res<lunco_core_session::ClientConnection>,
    scene: Res<lunco_core_session::ReplicatedScene>,
    remote: Res<crate::scenario_sync::RemoteScenarioManifest>,
    journal: Option<Res<JournalResource>>,
    mut outbox: ResMut<SyncOutbox>,
    mut mirror: ResMut<ReplicatedJournal>,
    mut sent: Local<usize>,
    mut sent_owner: Local<
        Option<(
            lunco_core_session::ReplicationScope,
            Option<Entity>,
            lunco_twin_journal::TwinId,
        )>,
    >,
) {
    if !role.is_networked() {
        return;
    }
    let Some(journal) = journal else {
        return;
    };
    let scope = if role.is_host() {
        facts.host_journal_scope(&journal, &application)
    } else {
        facts.client_journal_scope(
            connection.0,
            scene.0.as_ref(),
            remote.host_twin,
            &journal,
            &application,
        )
    };
    let Some(scope) = scope else {
        return;
    };
    let owner = (
        scope,
        connection.0,
        journal.with_read(|journal| journal.twin().clone()),
    );
    if sent_owner.as_ref() != Some(&owner) {
        // A downloaded scene shares the application journal resource. Its
        // existing tail was not authored under this remote mount and must not
        // be relabeled as new scene work. The host's bound journal and a
        // scene-free Application journal retain their full replay semantics.
        *sent = if !role.is_host() && matches!(scope, lunco_core_session::ReplicationScope::Twin(_))
        {
            let Some(cursor) = mirror.local_tail_for(scope, connection.0, &journal) else {
                warn!(
                    "[journal-plane] client local journal tail is not admitted to the live scene"
                );
                return;
            };
            cursor
        } else {
            0
        };
        *sent_owner = Some(owner);
    }
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
            if let Some(msg) = to_msg(entry, scope) {
                if !is_host && matches!(scope, lunco_core_session::ReplicationScope::Twin(_)) {
                    if let Some(connection) = connection.0 {
                        mirror.merge(entry.clone(), scope, connection, me.clone());
                    }
                }
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
/// `lunco_usd_commands::DocumentRegistry::<UsdDocument>::replay_op` and records the id.
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
    domain_ops_after(journal, base, me, already, DomainKind::Usd)
}

/// The domain-parameterized core of [`scene_ops_after`]: select the not-yet-applied
/// `Op` entries of a GIVEN `domain`, authored by a peer other than `me`, strictly
/// after `base`, **in convergent [`merged_order_ids`](lunco_twin_journal::Journal::merged_order_ids)
/// order** — so the selection honors the active [`MergeStrategy`] (default or a
/// scripted merge policy) identically for every domain.
///
/// This is the single strategy-honoring selection every document domain must
/// route its journal replay through. USD does today ([`scene_ops_after`]); when
/// networked **Modelica** replay is wired (the deferred multi-doc / cross-peer
/// `DocumentId` follow-up — see `lunco_luncosim::replay_scenario_journal`), it MUST
/// select via `domain_ops_after(.., DomainKind::Modelica)` and feed
/// [`lunco_modelica`]'s `replay_op`, NOT iterate raw `entries()` (insertion order),
/// or Modelica state would diverge under a scripted merge policy. The generic
/// policy mechanism is tested by `lunco-twin-journal`; this adapter keeps only
/// the domain filtering and replay boundary.
pub fn domain_ops_after(
    journal: &JournalResource,
    base: Option<&EntryId>,
    me: &AuthorId,
    already: &HashSet<EntryId>,
    domain: DomainKind,
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
                    EntryKind::Op { domain: d, op, .. } if *d == domain => {
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
    fn local_author_id_respects_env_override() {
        // The env override is how multiple instances on one machine (tests,
        // net_smoke) get distinct stable authors. The pure parser keeps the
        // test independent of process-global environment mutation.
        assert_eq!(
            author_from_override(Some(" peer-override-xyz ")),
            Some(AuthorId::new("peer-override-xyz"))
        );
        assert_eq!(author_from_override(Some("  ")), None);
    }

    #[test]
    fn host_rebinds_inbound_entry_to_connection_author() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, TwinId};

        let twin = TwinId::new("canonical-author");
        let source = JournalResource::new(twin.clone(), AuthorId::new("attacker"));
        source.with_write(|j| {
            j.append_local(
                AuthorTag {
                    user: "victim".into(),
                    tool: "spoofed".into(),
                },
                DocumentId::new(1),
                EntryKind::Op {
                    domain: DomainKind::Usd,
                    op: serde_json::json!({ "v": 1 }),
                    inverse: serde_json::json!({}),
                },
                None,
            );
        });
        let msg = full_journal_msgs(&source, lunco_core_session::ReplicationScope::Application)
            .pop()
            .expect("source entry");
        let target = JournalResource::new(twin, AuthorId::new("host"));
        apply_inbound_entry(
            &target,
            &msg,
            Some(AuthorId::new("net-session-0000000000000007")),
            lunco_core_session::ReplicationScope::Application,
        )
        .expect("valid canonical entry");

        target.with_read(|j| {
            let entry = j.entries().next().expect("canonical entry");
            assert_eq!(
                entry.id.author,
                AuthorId::new("net-session-0000000000000007")
            );
            assert_eq!(entry.author.user, "net-session-0000000000000007");
        });
    }

    /// End-to-end simulation of the bidirectional plane across THREE peers
    /// (host + two clients) using the real plane functions — `full_journal_msgs`
    /// (the host's fan-out / late-joiner replay), `apply_inbound_entry` (the
    /// inbound merge, both roles), `peer_author`/`host_author` (the collision
    /// fix), and `scene_ops_after` (Layer B selection) — plus the real
    /// `append_remote` merge. No transport: entries are routed by hand exactly
    /// as the ferry does, so it deterministically proves convergence + correct
    /// scene-op selection without booting two networked apps.
    #[test]
    fn bidirectional_round_trip_converges_and_selects_foreign_ops() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, TwinId};

        let twin = TwinId::new("t");
        let host = JournalResource::new(twin.clone(), AuthorId::new("host"));
        let c1 = JournalResource::new(twin.clone(), AuthorId::new("peer-1"));
        let c2 = JournalResource::new(twin, AuthorId::new("peer-2"));

        // Author a local USD op on `j` (EntryId.author = j's local_author).
        let author_usd = |j: &JournalResource, v: i32| {
            j.with_write(|jj| {
                jj.append_local(
                    AuthorTag::for_tool("test"),
                    DocumentId::new(1),
                    EntryKind::Op {
                        domain: DomainKind::Usd,
                        op: serde_json::json!({ "v": v }),
                        inverse: serde_json::json!({}),
                    },
                    None,
                )
            })
        };
        // The ferry: deliver every entry currently in `from` into `to` (merge).
        let deliver = |from: &JournalResource, to: &JournalResource| {
            for msg in full_journal_msgs(from, lunco_core_session::ReplicationScope::Application) {
                apply_inbound_entry(
                    to,
                    &msg,
                    None,
                    lunco_core_session::ReplicationScope::Application,
                )
                .expect("valid application entry");
            }
        };

        // Host authors two edits, fans out to both clients (host → All).
        author_usd(&host, 1);
        author_usd(&host, 2);
        deliver(&host, &c1);
        deliver(&host, &c2);

        // Client 1 authors an edit and sends it UP to the host (client → host);
        // the host then RELAYS its whole log to client 2 (host = fan-out hub).
        author_usd(&c1, 3);
        deliver(&c1, &host);
        deliver(&host, &c2);
        // And the host's relay reaches client 1 too (its own edit echoes back —
        // idempotent, a no-op) — model the full broadcast to All.
        deliver(&host, &c1);

        // All three peers converge on the identical merged order (3 entries).
        let order = |j: &JournalResource| j.with_read(|jj| jj.merged_order_ids());
        assert_eq!(order(&host).len(), 3, "host has all three edits");
        assert_eq!(order(&host), order(&c1), "c1 converged with host");
        assert_eq!(order(&host), order(&c2), "c2 converged with host");

        let none = HashSet::new();
        let vals = |ops: &[(EntryId, serde_json::Value)]| {
            ops.iter()
                .map(|(_, v)| v["v"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };
        // Client 2 authored nothing → it replays ALL three edits (host's 1,2 +
        // client 1's 3), in convergent order.
        assert_eq!(
            vals(&scene_ops_after(&c2, None, &AuthorId::new("peer-2"), &none)),
            vec![1, 2, 3]
        );
        // Client 1 authored edit 3 → it is EXCLUDED (already applied locally);
        // only the host's two remote edits replay.
        assert_eq!(
            vals(&scene_ops_after(&c1, None, &AuthorId::new("peer-1"), &none)),
            vec![1, 2]
        );
        // The host sees client 1's edit (author != host), not its own two.
        assert_eq!(
            vals(&scene_ops_after(&host, None, &AuthorId::new("host"), &none)),
            vec![3]
        );
    }

    /// A host that opens a twin whose journal was authored by SOMEONE ELSE must
    /// not replay that saved history: the `.usda` files it just loaded already
    /// contain it. Basing on the head captured at load (what the manifest
    /// advertises) is what makes that true — `base = None` + the `author != me`
    /// filter does NOT, because every entry looks foreign.
    ///
    /// This is the `LUNCO_PEER_ID=local-host` crash: 982 stale entries replayed
    /// over already-baked files, churning rovers until avian panicked on an
    /// orphaned wheel joint (`assert!(island.joint_count > 0)`).
    #[test]
    fn host_does_not_replay_saved_history_authored_by_another_peer() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, TwinId};

        let twin = TwinId::new("t");
        // The twin's history was written by `peer-old` (another machine/session).
        let saved = JournalResource::new(twin.clone(), AuthorId::new("peer-old"));
        let author_usd = |j: &JournalResource, v: i32| {
            j.with_write(|jj| {
                jj.append_local(
                    AuthorTag::for_tool("test"),
                    DocumentId::new(1),
                    EntryKind::Op {
                        domain: DomainKind::Usd,
                        op: serde_json::json!({ "v": v }),
                        inverse: serde_json::json!({}),
                    },
                    None,
                )
            })
        };
        author_usd(&saved, 1);
        author_usd(&saved, 2);

        // The host boots with a DIFFERENT local author id and loads those files.
        let me = AuthorId::new("local-host");
        let none = HashSet::new();
        let vals = |ops: &[(EntryId, serde_json::Value)]| {
            ops.iter()
                .map(|(_, v)| v["v"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };

        // The OLD behaviour — base `None` — re-applies the whole saved history.
        assert_eq!(
            vals(&scene_ops_after(&saved, None, &me, &none)),
            vec![1, 2],
            "base=None double-applies history already baked into the files"
        );

        // The FIX: base = the head captured at load (what the manifest advertises).
        let head = saved
            .with_read(|j| j.merged_head())
            .expect("history is non-empty");
        assert!(
            scene_ops_after(&saved, Some(&head), &me, &none).is_empty(),
            "nothing to replay: the loaded files already reflect the whole journal"
        );

        // …and a client edit arriving AFTER that head still replays.
        let client = JournalResource::new(twin, AuthorId::new("peer-client"));
        for msg in full_journal_msgs(&saved, lunco_core_session::ReplicationScope::Application) {
            apply_inbound_entry(
                &client,
                &msg,
                None,
                lunco_core_session::ReplicationScope::Application,
            )
            .expect("valid application entry");
        }
        author_usd(&client, 3);
        for msg in full_journal_msgs(&client, lunco_core_session::ReplicationScope::Application) {
            apply_inbound_entry(
                &saved,
                &msg,
                None,
                lunco_core_session::ReplicationScope::Application,
            )
            .expect("valid application entry");
        }
        assert_eq!(
            vals(&scene_ops_after(&saved, Some(&head), &me, &none)),
            vec![3],
            "live client edits past the base must still project onto the host's scene"
        );
    }

    #[test]
    fn scene_ops_after_selects_remote_usd_ops_past_base() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, JournalEntry, LifecycleKind, TwinId};

        let me = AuthorId::new("peer-1");
        let journal = JournalResource::new(TwinId::new("t"), me.clone());
        let host = |lam: u64| EntryId {
            author: AuthorId::new("host"),
            lamport: lam,
        };
        let mk = |lam: u64, kind: EntryKind| JournalEntry {
            id: host(lam),
            parents: if lam <= 1 {
                vec![]
            } else {
                vec![host(lam - 1)]
            },
            author: AuthorTag {
                user: "host".into(),
                tool: "t".into(),
            },
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
        let lam = |v: &[(EntryId, serde_json::Value)]| {
            v.iter().map(|(id, _)| id.lamport).collect::<Vec<_>>()
        };

        // base = e1 (downloaded snapshot) → apply the newer USD ops (2, 4); the
        // lifecycle entry (3) and the baked base (1) are excluded.
        assert_eq!(
            lam(&scene_ops_after(&journal, Some(&host(1)), &me, &none)),
            vec![2, 4]
        );
        // base = None → every remote USD op is new.
        assert_eq!(
            lam(&scene_ops_after(&journal, None, &me, &none)),
            vec![1, 2, 4]
        );
        // base present but not yet received → defer (don't double-apply history).
        assert!(scene_ops_after(&journal, Some(&host(99)), &me, &none).is_empty());
        // Already-applied ids are skipped.
        let done: HashSet<_> = [host(2)].into_iter().collect();
        assert_eq!(
            lam(&scene_ops_after(&journal, Some(&host(1)), &me, &done)),
            vec![4]
        );
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use lunco_core_session::ReplicationScope;
    #[test]
    fn journal_entry_rejects_old_mount_and_application_without_mutating() {
        let journal = JournalResource::new(
            lunco_twin_journal::TwinId::new("generic"),
            AuthorId::local(),
        );
        let old = ReplicationScope::Twin(lunco_workspace::TwinId::new(1));
        let next = ReplicationScope::Twin(lunco_workspace::TwinId::new(2));
        let msg = JournalEntryMsg {
            scope: crate::scope::wire_scope(old),
            json: "invalid-json".into(),
        };
        assert_eq!(apply_inbound_entry(&journal, &msg, None, next), None);
        assert_eq!(
            apply_inbound_entry(&journal, &msg, None, ReplicationScope::Application),
            None
        );
        assert!(journal.with_read(|journal| journal.is_empty()));
    }
    fn source(twin: &str, value: i64) -> JournalResource {
        let journal =
            JournalResource::new(lunco_twin_journal::TwinId::new(twin), AuthorId::new("host"));
        journal.with_write(|journal| {
            journal.append_local(
                lunco_twin_journal::AuthorTag {
                    user: "host".into(),
                    tool: "generic".into(),
                },
                lunco_doc::DocumentId::new(1),
                EntryKind::Op {
                    domain: DomainKind::Usd,
                    op: serde_json::json!({"value":value}),
                    inverse: serde_json::json!({}),
                },
                None,
            );
        });
        journal
    }
    #[test]
    fn mirror_isolates_same_entry_id_payloads_and_exact_connection() {
        let mut world = World::new();
        let connection = world.spawn_empty().id();
        let replacement = world.spawn_empty().id();
        let a = ReplicationScope::Twin(lunco_workspace::TwinId::new(1));
        let b = ReplicationScope::Twin(lunco_workspace::TwinId::new(2));
        let first = source("first", 1);
        let next = source("next", 2);
        let first_msg = full_journal_msgs(&first, a).pop().unwrap();
        let next_msg = full_journal_msgs(&next, b).pop().unwrap();
        let mut mirror = ReplicatedJournal::default();
        let first_id = mirror
            .apply(&first_msg, a, connection, AuthorId::new("client"))
            .unwrap();
        mirror.retire(a);
        let next_id = mirror
            .apply(&next_msg, b, connection, AuthorId::new("client"))
            .unwrap();
        assert_eq!(first_id, next_id);
        assert!(mirror.for_owner(a, Some(connection)).is_none());
        assert!(mirror.for_owner(b, Some(replacement)).is_none());
        let journal = mirror.for_owner(b, Some(connection)).unwrap();
        assert_eq!(
            journal.with_read(|journal| match &journal.get(&next_id).unwrap().kind {
                EntryKind::Op { op, .. } => op["value"].as_i64().unwrap(),
                _ => panic!("op"),
            }),
            2
        );
        assert_eq!(first.with_read(|journal| journal.len()), 1);
    }
    #[test]
    fn local_tail_is_pinned_before_scene_commands_and_mirrors_only_new_entries() {
        let mut world = World::new();
        let connection = world.spawn_empty().id();
        let scope = ReplicationScope::Twin(lunco_workspace::TwinId::new(3));
        let local = source("local", 1);
        let mut mirror = ReplicatedJournal::default();
        mirror.admit_local_tail(scope, connection, &local);
        assert_eq!(
            mirror.local_tail_for(scope, Some(connection), &local),
            Some(1)
        );
        local.with_write(|journal| {
            journal.append_local(
                lunco_twin_journal::AuthorTag {
                    user: "host".into(),
                    tool: "generic".into(),
                },
                lunco_doc::DocumentId::new(1),
                EntryKind::Op {
                    domain: DomainKind::Usd,
                    op: serde_json::json!({"value":2}),
                    inverse: serde_json::json!({}),
                },
                None,
            );
        });
        let cursor = mirror
            .local_tail_for(scope, Some(connection), &local)
            .unwrap();
        let tail =
            local.with_read(|journal| journal.entries().skip(cursor).cloned().collect::<Vec<_>>());
        assert_eq!(tail.len(), 1);
        let id = tail[0].id.clone();
        mirror.merge(tail[0].clone(), scope, connection, local.local_author());
        let current = mirror.for_owner(scope, Some(connection)).unwrap();
        assert_eq!(current.with_read(|journal| journal.len()), 1);
        assert_eq!(
            current.with_read(|journal| match &journal.get(&id).unwrap().kind {
                EntryKind::Op { op, .. } => op["value"].as_i64().unwrap(),
                _ => panic!("op"),
            }),
            2
        );
        mirror.retire(scope);
        assert!(
            mirror
                .local_tail_for(scope, Some(connection), &local)
                .is_none()
        );
    }
}
