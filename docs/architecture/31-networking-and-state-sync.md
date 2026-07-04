# 31 — Networking & State Sync

**Status:** As-built (2026-07-04, merged from the `networking` branch).
**Crate:** [`lunco-networking`](../../crates/lunco-networking) · **Feature-gated** on `networking`.

LunCoSim multiplayer is **not** one replication stream. State that differs in
lifetime, authority, and merge semantics travels on **separate planes**, each with
its own encoding, channel, and conflict rule. This doc is the map of those planes,
the wire, area-of-interest routing, and how authored edits (USD ops, journals,
policies) ride the network as first-class history rather than bespoke broadcasts.

> Prerequisite reading: [`10-document-system.md`](10-document-system.md) (Documents,
> DocumentOps), [`21-domain-usd.md`](21-domain-usd.md) (USD as canonical scene +
> op-driven projection), [`18-unified-journal-and-history.md`](18-unified-journal-and-history.md)
> (the Lamport/DAG journal).

## 1. Why planes, not one stream

A physics pose and an authored scene edit have nothing in common operationally:

- a pose is **continuous, ephemeral, latest-wins** — you interpolate it and drop stale frames;
- an edit is **discrete, permanent, mergeable** — you must never lose it and must converge on order.

Forcing both through one reliable-ordered channel means either the pose head-of-line
blocks the edit, or the edit rides an unreliable lane and is lost. So the transport
is a tagged union (`SyncEnvelope`, `sync.rs:518`) ferried by `SyncOutbox`/`SyncInbox`,
and the *planes* below decide encoding and merge. The taxonomy lives in code at
[`journal_plane.rs`](../../crates/lunco-networking/src/journal_plane.rs) and is
mirrored here.

## 2. The five planes

| Plane | Lifetime | Merge rule | Encoding | Code |
|-------|----------|-----------|----------|------|
| **Command** | ephemeral, replay-once | apply in receipt order | `SyncCommand` | `sync.rs:59` |
| **State** | continuous, overwrite | latest-wins + interpolate | `SnapshotMsg`/`SnapshotEntry`, quantized | `sync.rs:78,175` |
| **Content** | immutable | content-addressed (CID) | scenario manifest + asset chunks | `scenario.rs`, `scenario_sync.rs` |
| **Journal** | permanent | convergent DAG merge (Lamport) | `JournalEntryMsg { json }` | `journal_plane.rs:38` |
| **Presence** | ephemeral | last-writer-wins | `RunStatusMsg`, cursors, tutor/student | `sync.rs:416` |

### Command plane
Structural / control **intent** (drive a rover, spawn, teleport). Replayed once in
receipt order; no persistence. Host authoritative.

### State plane
Continuous physics: position, orientation, velocity. Quantized to cut bandwidth
(`quantize_pos` `sync.rs:106`, `encode_quat` `:127`) and streamed on an unreliable,
unordered lane — a dropped snapshot is simply superseded by the next. The receiver
interpolates. This is the only plane where loss is *correct* behaviour.

### Content plane
Immutable file bytes (scenario assets, meshes) addressed by **CID**. A peer that
already holds the CID skips the transfer; the manifest lists CIDs, chunks stream on
the bulk lane. See [`scenario_sync.rs`](../../crates/lunco-networking/src/scenario_sync.rs).

### Journal plane
The authored **history** of every domain document — the heart of collaborative
editing. A local edit records a typed op into the one Lamport-ordered journal
([`lunco-twin-journal`](../../crates/lunco-twin-journal)); `broadcast_journal_entries`
(`journal_plane.rs:160`) ships new entries host→client as `JournalEntryMsg { json }`,
and `apply_inbound_entry` (`:136`) feeds them to `Journal::append_remote` for a
**deterministic branch merge** — every peer converges on the same DAG regardless of
arrival order. Late joiners get a full replay via `full_journal_msgs` (`:123`).
Authorship is stamped from a minted install id (`local_author_id`, `:60`).

**This is why there is no bespoke sync code per domain.** A USD edit, a Modelica
experiment op, an obstacle-field spec change, a script/shader edit — all serialize
into `EntryKind::Op` and travel this one plane. USD ops implement both
`DocumentOp` and `lunco_twin_journal::OpPayload` (`document.rs:394,404`), so authoring
onto a document *is* journaling *is* syncing.

### Presence plane
Ephemeral per-peer status: run state (`RunStatusMsg`), view cursors, tutor/student
banners. Last-writer-wins; never persisted.

## 3. The wire

Three bidirectional channels (`SyncChannel`, `protocol.rs`), chosen so the
join-critical path never blocks behind bulk transfer:

| Channel | Reliability | Carries |
|---------|-------------|---------|
| `CmdChannel` | Ordered-Reliable | handshake, commands, spawn, journal entries |
| `StateChannel` | Unordered-Unreliable | physics snapshots (latest-wins) |
| `BulkChannel` | Ordered-Reliable | scenario manifest + asset chunks |

Bulk is a **separate reliable channel** specifically so a large asset transfer
cannot head-of-line-block the `CmdChannel` a joining client needs.

Transport: **lightyear 0.27** (`replication` only, default features off) over
`aeronet_webtransport 0.20` — WebTransport works in the browser, so the same wire
serves native and web clients.

### Modes
`NetworkMode` (`lib.rs:84`):
- `Host { port }` — listen-server (native only);
- `Connect { server: String, client_id }` — the server is kept as a **string** (not a
  resolved `SocketAddr`) so a DNS name survives for browser WebTransport.

`NetworkRole` is Host / Client / Standalone (`shared.rs:101`). With no mode the app
idles as **Standalone single-player** until a `JoinServer` command dials — networking
is a no-op facade when the `networking` feature is off.

## 4. Area of Interest (per-peer routing)

Naïvely, broadcasting N bodies to P peers is O(N×P). AOI flips it to O(Σ interest):

- each peer reports a `ViewCenterMsg` (`sync.rs:322`) on the lossy control lane at
  `interest_hz`;
- `compute_interest_sets` (`server.rs`) builds a `PeerInterest` set per peer
  (**fail-open**: a peer with no center sees All);
- `assemble_and_send_snapshots` (`server.rs:640`) diffs a **per-peer digest**:
  soft-enter sends a baseline, soft-exit drops a body from the stream + digest but
  keeps a spawn proxy so a body spawns **at most once per peer**.

Interest recompute rate is `NetworkConfig.interest_hz` (`sync.rs:586`).

## 5. Policy & RBAC ride the journal too

There is **no dedicated policy-broadcast plane**. A `LuncoPolicy` USD prim
(`lunco:policy:{seam,entry,source,deterministic}`) is authored like any doc op, so it
persists, is RBAC-gated, and converges — see
[`scripted_policy.rs`](../../crates/lunco-networking/src/scripted_policy.rs). Policies
activate reactively (`project_policies`) into a non-authoritative
`ScriptedPolicyRegistry` cache. Hooks ([`lunco-hooks`](../../crates/lunco-hooks)) bind
the merge order (`MERGE_SEAM`), authorization (`lunco_core::session::AUTHORIZE_HOOK`),
and drive kernels (`lunco:driveKernel`).

**Determinism contract:** every peer must run the *identical compiled* policy — a
policy is a rhai/kernel program shipped by source, not a result broadcast.

## 6. Design invariants

1. **One journal, many domains.** Never add a per-domain broadcast; add an
   `OpPayload` impl and a `DomainKind` variant. The obstacle-field migration
   (`obstacle-field/journal.rs`, replacing the old `shared.rs:178` broadcast) is the
   reference example.
2. **The op is the delta.** Never re-derive an edit by reading state back; ship the
   typed op (author-once coherence, see [`21-domain-usd.md`](21-domain-usd.md)).
3. **Match the plane to the data's lifetime.** Continuous → State (lossy). Authored →
   Journal (convergent). Immutable bytes → Content (CID). Don't cross them.
4. **Fail open on interest, fail loud on certs.** A centerless peer sees everything;
   a bad cert aborts rather than silently running insecure (headless `--no-ui` server).

## 7. Open gaps

- **No dedicated design spec for the plane taxonomy existed before this doc** — it
  lived only in code comments. Keep this doc in step with `journal_plane.rs`.
- Client-side **predicted-Dynamic body** divergence/desync is a known open issue
  (predict-and-smooth reconciliation is partial).
- AOI interest is spatial only; **relevance by role/ownership** (e.g. always replicate
  a possessed vessel regardless of distance) is not yet modelled.
