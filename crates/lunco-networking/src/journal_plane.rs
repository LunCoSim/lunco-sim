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
use lunco_twin_journal::{AuthorId, DomainKind, EntryId, EntryKind, JournalEntry, MergeStrategy};

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

/// This peer's **stable journal author** — durable across reconnects *and* across
/// the single-player→networked transition, so a peer's own offline edits keep the
/// same author and therefore upload + merge correctly when it (re)connects. This
/// is the fix for the old session-scoped author, under which a reconnect minted a
/// fresh `peer-<session>` id → the peer's prior offline entries (authored under
/// the old id) were filtered out of `broadcast_journal_entries` and silently
/// stranded. Precedence:
///
/// 1. `LUNCO_PEER_ID` env override — distinct ids for multiple instances on ONE
///    machine (tests, `net_smoke`, `run_host_client`), which otherwise share the
///    persisted install id and would collide.
/// 2. A per-install id persisted in the user config dir (`identity/peer_id`),
///    minted once from fresh entropy and reused forever — the real-product path.
/// 3. A fresh random id if the config dir can't be read/written (never collides
///    within a run; just not durable — logged).
pub fn local_author_id() -> AuthorId {
    if let Ok(id) = std::env::var("LUNCO_PEER_ID") {
        let id = id.trim();
        if !id.is_empty() {
            return AuthorId::new(id);
        }
    }
    AuthorId::new(persisted_install_id())
}

#[cfg(not(target_arch = "wasm32"))]
fn persisted_install_id() -> String {
    let path = lunco_assets::user_config_subdir("identity").join("peer_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let id = existing.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    let fresh = format!("peer-{:016x}", lunco_core::ids::random_u64());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // TODO(multiplayer): deferred — singleplayer focus for now, RBAC disabled for
    // ease of debugging. Raw `std::fs::write` bypasses `lunco-storage`'s atomic
    // rename: a kill mid-write leaves a zero-byte file, so the next start mints a
    // new id and all journal entries authored under the old id become
    // unattributable. Related: the wasm branch below (TODO(web-identity)) mints a
    // fresh id every page reload — same identity-durability gap. Revisit before
    // multiplayer hardening (INDEPENDENT-REVIEW-2026-07-19_agy.md NET-1).
    if let Err(e) = std::fs::write(&path, &fresh) {
        warn!(
            "[journal-plane] could not persist install id to {}: {e}",
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
    format!("peer-{:016x}", lunco_core::ids::random_u64())
}

/// Stamp this peer's stable install id ([`local_author_id`]) as the journal's
/// local author, so authored edits are attributable and durable across
/// reconnects. Runs on **both** roles the frame the [`JournalResource`] appears
/// (idempotent — only writes when it differs). Was host-only + session-scoped
/// (stamped on handshake); now identity is set once at startup and never churns.
pub fn stamp_local_journal_author(journal: Option<Res<JournalResource>>) {
    if let Some(j) = journal {
        let me = local_author_id();
        if j.local_author() != me {
            j.set_local_author(me);
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
/// `lunco_usd::DocumentRegistry::<UsdDocument>::replay_op` and records the id.
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
/// `DocumentId` follow-up — see `lunco_sandbox::replay_scenario_journal`), it MUST
/// select via `domain_ops_after(.., DomainKind::Modelica)` and feed
/// [`lunco_modelica`]'s `replay_op`, NOT iterate raw `entries()` (insertion order),
/// or Modelica state would diverge under a scripted merge policy. Verified for both
/// domains by the `scripted_policy_reorders_*_replay` tests.
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

// ── Layer C: pluggable convergent merge policy (rhai) ─────────────────────────

/// Activate a **scripted convergent merge policy** on `journal`: compile+register
/// the rhai `source` (entry fn `entry(a, b) -> int`, C-`memcmp` convention over
/// the two `{ author, lamport, domain }` maps) under `hook_id` as a
/// **deterministic** hook, then switch the journal to [`MergeStrategy::Scripted`]
/// so *every* convergent read uses it: the client scene replay
/// ([`scene_ops_after`]), the `append_remote` `main` re-pointing, and
/// [`merged_head`](lunco_twin_journal::Journal::merged_head).
///
/// # Determinism contract
///
/// Every peer MUST call this with the **identical** `hook_id` and `source`, or
/// peers linearize the same history differently and their scenes diverge — the one
/// thing the merge plane exists to prevent. Distribute the policy script over the
/// content plane so all peers run byte-identical source. Returns the rhai compile
/// error (journal unchanged) on failure.
pub fn activate_scripted_merge_policy(
    journal: &JournalResource,
    hook_id: &str,
    entry: &str,
    source: &str,
) -> Result<(), String> {
    lunco_hooks_rhai::register_rhai_hook(hook_id, entry, source, true)?;
    journal.with_write(|j| j.set_merge_strategy(MergeStrategy::Scripted(hook_id.to_string())));
    Ok(())
}

/// Revert `journal` to the built-in `(lamport, author)` convergent order.
pub fn use_default_merge_policy(journal: &JournalResource) {
    journal.with_write(|j| j.set_merge_strategy(MergeStrategy::Default));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_author_id_respects_env_override() {
        // The env override is how multiple instances on one machine (tests,
        // net_smoke) get distinct stable authors. Single test touching this var.
        std::env::set_var("LUNCO_PEER_ID", "peer-override-xyz");
        assert_eq!(local_author_id(), AuthorId::new("peer-override-xyz"));
        std::env::remove_var("LUNCO_PEER_ID");
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
        let c2 = JournalResource::new(twin.clone(), AuthorId::new("peer-2"));

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
            for msg in full_journal_msgs(from) {
                apply_inbound_entry(to, &msg);
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
        for msg in full_journal_msgs(&saved) {
            apply_inbound_entry(&client, &msg);
        }
        author_usd(&client, 3);
        for msg in full_journal_msgs(&client) {
            apply_inbound_entry(&saved, &msg);
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

    /// A scripted merge policy activated on the journal changes the convergent
    /// replay order at the real networking call site ([`scene_ops_after`]): two
    /// concurrent edits are tie-broken by the rhai hook, not the built-in
    /// `(lamport, author)` key. Proves the wiring end-to-end.
    #[test]
    fn scripted_policy_reorders_scene_replay() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, TwinId};

        let viewer = AuthorId::new("viewer");
        let journal = JournalResource::new(TwinId::new("t"), viewer.clone());
        // Two CONCURRENT root USD ops from different authors at the same lamport —
        // neither is a causal ancestor, so only the tie-break orders them.
        let mk = |author: &str, v: i32| JournalEntry {
            id: EntryId {
                author: AuthorId::new(author),
                lamport: 1,
            },
            parents: vec![],
            author: AuthorTag {
                user: author.into(),
                tool: "t".into(),
            },
            at_ms: 0,
            twin: TwinId::new("t"),
            doc: DocumentId::new(1),
            kind: EntryKind::Op {
                domain: DomainKind::Usd,
                op: serde_json::json!({ "v": v }),
                inverse: serde_json::json!({}),
            },
            change_set: None,
        };
        journal.with_write(|j| {
            j.append_remote(mk("aaa", 1));
            j.append_remote(mk("bbb", 5));
        });
        let none = HashSet::new();
        let vals = |ops: &[(EntryId, serde_json::Value)]| {
            ops.iter()
                .map(|(_, v)| v["v"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };

        // Default: author ascending → aaa (1) before bbb (5).
        assert_eq!(
            vals(&scene_ops_after(&journal, None, &viewer, &none)),
            vec![1, 5]
        );

        // Activate a rhai policy that orders authors DESCENDING (a<b ⇒ a AFTER b).
        activate_scripted_merge_policy(
            &journal,
            "test.net.author_desc",
            "cmp",
            "fn cmp(a, b) { if a.author < b.author { 1 } else if a.author > b.author { -1 } else { 0 } }",
        )
        .unwrap();
        // Now bbb (5) sorts before aaa (1).
        assert_eq!(
            vals(&scene_ops_after(&journal, None, &viewer, &none)),
            vec![5, 1]
        );

        // Reverting restores the built-in order.
        use_default_merge_policy(&journal);
        assert_eq!(
            vals(&scene_ops_after(&journal, None, &viewer, &none)),
            vec![1, 5]
        );

        lunco_hooks::unregister("test.net.author_desc");
    }

    /// The Modelica replay leg (when wired) honors the scripted merge policy for
    /// free, because it routes through the SAME strategy-honoring selection as USD
    /// — only the domain filter differs. Concurrent Modelica ops reorder under the
    /// author-descending policy exactly like the USD ones, and USD ops are excluded
    /// from a Modelica selection.
    #[test]
    fn scripted_policy_reorders_modelica_replay() {
        use lunco_doc::DocumentId;
        use lunco_twin_journal::{AuthorTag, TwinId};

        let viewer = AuthorId::new("viewer");
        let journal = JournalResource::new(TwinId::new("t"), viewer.clone());
        let mk = |author: &str, domain: DomainKind, v: i32| JournalEntry {
            id: EntryId {
                author: AuthorId::new(author),
                lamport: 1,
            },
            parents: vec![],
            author: AuthorTag {
                user: author.into(),
                tool: "t".into(),
            },
            at_ms: 0,
            twin: TwinId::new("t"),
            doc: DocumentId::new(1),
            kind: EntryKind::Op {
                domain,
                op: serde_json::json!({ "v": v }),
                inverse: serde_json::json!({}),
            },
            change_set: None,
        };
        journal.with_write(|j| {
            j.append_remote(mk("aaa", DomainKind::Modelica, 1));
            j.append_remote(mk("bbb", DomainKind::Modelica, 5));
            j.append_remote(mk("ccc", DomainKind::Usd, 9)); // other domain — excluded
        });
        let none = HashSet::new();
        let vals = |ops: &[(EntryId, serde_json::Value)]| {
            ops.iter()
                .map(|(_, v)| v["v"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };
        let modelica = |j: &JournalResource| {
            vals(&domain_ops_after(
                j,
                None,
                &viewer,
                &none,
                DomainKind::Modelica,
            ))
        };

        // Default: author ascending, USD op filtered out → [1, 5].
        assert_eq!(modelica(&journal), vec![1, 5]);

        // Under the author-descending scripted policy the concurrent Modelica ops
        // flip → [5, 1] (same reordering the USD leg gets — one selection path).
        activate_scripted_merge_policy(
            &journal,
            "test.net.modelica_desc",
            "cmp",
            "fn cmp(a, b) { if a.author < b.author { 1 } else if a.author > b.author { -1 } else { 0 } }",
        )
        .unwrap();
        assert_eq!(modelica(&journal), vec![5, 1]);

        use_default_merge_policy(&journal);
        lunco_hooks::unregister("test.net.modelica_desc");
    }
}
