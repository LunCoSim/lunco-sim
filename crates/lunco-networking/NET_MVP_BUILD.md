# NET_MVP_BUILD — the one-shot networking MVP (lightyear + WebTransport)

Implementation record for the multiplayer MVP: **N people connect to one world, each
creates a rover, possesses it, drives only their own; everyone sees everyone's rovers
move.** Browser + native clients, one WebTransport server (D1, validated Ph0).

Builds on the corpus: `IMPLEMENTATION_PLAN.md`, `PH2_OP_LOG.md`, `MVP_MULTIPLAYER_GAPS.md`,
`DECISIONS.md` (D1–D7), `SYNC_ARCHITECTURE.md` (M1–M7), `SPIKE_PH0.md` (cert recipe).

## The de-risking decision: thin adapter, fat substrate

lightyear 0.26.4 is large (≈20 min cold build) and its 0.26 aeronet entity-component API
is intricate. The user's machine struggles with broad builds. So:

- **ALL of our logic lives in always-on crates with NO lightyear dependency**
  (`lunco-core`, `lunco-api`). Capture, apply, dedupe, authority, sessions, snapshot
  gather/apply — all compile in seconds and iterate cheaply.
- **`lunco-networking` is a THIN lightyear adapter** behind the `networking` feature
  (D7). Its entire job: configure WebTransport, manage connections↔sessions, and **ferry
  pre-serialized envelopes** between our `SyncOutbox`/`SyncInbox` and two tiny lightyear
  messages (reliable + unreliable) on two channels. It contains no game logic.

This isolates the slow/heavy/rarely-changing code from the fast/iterated code, and is the
literal shape of D7 (substrate always-on; sync layer optional; facade no-ops when off).

## Identity model: snapshots-as-messages, not lightyear replication

We do **not** use lightyear's component replication / prediction for the MVP. lightyear's
replication spawns its own mirror entities and manages its own entity mapping, which fights
our `GlobalEntityId`/`Provenance` identity (M1). Instead:

- The server is authoritative: it runs cosim/avian, simulates rovers.
- State replicates as **Snapshot messages**: `Vec<(GlobalEntityId, Transform[, CellCoord])>`
  for entities whose transform **changed**, at a **tunable HZ** — the user's "tunable HZ +
  only-if-changed" ask, implemented directly (`NetworkConfig { replication_hz,
  only_if_changed }`).
- Clients are thin renderers: they spawn rover geometry **locally** (M1 content-
  reconstruction, USD loaded on each peer) pinned to the server-sent `GlobalEntityId`, and
  move it by applying snapshots. No avian on clients for networked rovers.
- lightyear stays a pure transport+message bus. (lightyear replication/prediction is the
  Ph4 upgrade path; the seam is unchanged.)

## Pieces

### lunco-core (always-on substrate)
- `NetworkRole { Standalone, Host, Client }` (resource) — Standalone default (single-player).
- `LocalSession(SessionId)` — this peer's id; `SessionId::LOCAL` until a client handshake
  replaces it. Stamped as `origin` on outgoing mutations.
- `SyncApplyGuard(Option<SessionId>)` — `Some(origin)` while a sync command is being applied
  (so capture observers suppress the echo; possession skips camera-bind for remote origins).
  `None` = locally originated.
- `SessionRegistry` — `owners: HashMap<u64 /*rover gid*/, SessionId>`; `claim/owns/release_
  session`; the single `authorize(origin, type_name, target_gid) -> Result<(), Reject>` gate.
  Single-player: `IsServer=true` + everything ungated/owned-by-LOCAL → passes trivially.

### lunco-api (always-on capture/apply/codec/snapshots)
- `SyncCommand { type_name: String, data: serde_json::Value /*GlobalEntityIds*/ }`.
- `SyncEnvelope` = `enum { Command(Mutation<SyncCommand>), Snapshot(SnapshotMsg), Handshake(..), Ack(Ack) }`
  with a `SyncChannel` hint for reliable/unreliable routing.
- `SyncOutbox(Vec<(SyncChannel, SyncEnvelope)>)`, `SyncInbox(Vec<(SessionId, SyncEnvelope)>)`
  — the only contract `lunco-networking` touches. No-op when nothing drains (feature off).
- `declare_channel::<C>(SyncChannel)` ext → registers `capture::<C>` global observer:
  serialize `C` via `TypedReflectSerializer` (symmetric with the existing
  `TypedReflectDeserializer` apply path), `globalize_ids_in_json` (Entity.to_bits →
  `GlobalEntityId` via `ApiEntityRegistry::api_id_for`, inverse of `resolve_ids_in_json`),
  wrap `Mutation::local`, push to `SyncOutbox`. Skips when `SyncApplyGuard` is set (echo) or
  `NetworkRole::Standalone` (single-player no-ops).
- `SyncCommandEvent { type_name, params, op_id, origin }` + `apply_sync_command` observer:
  dedupe by `OpId`; `authorize`; `resolve_ids_in_json`; set `SyncApplyGuard(Some(origin))`;
  `ReflectEvent::trigger`; clear guard. (This is `api_command_dispatcher` + guard + authority.)
- `drain_sync_inbox` system: `SyncInbox` → `SyncCommandEvent` / apply snapshot.
- Snapshots: `gather_snapshot` (server, runs at `replication_hz`, `Changed<Transform>` when
  `only_if_changed`) → `SyncOutbox`; snapshot apply on clients by `GlobalEntityId`.

### Gameplay gates (the MVP_MULTIPLAYER_GAPS fixes)
- **G1 input isolation** — `LocalAvatar` marker (lunco-avatar); gate
  `translate_intents_to_commands` to `With<LocalAvatar>`; server maps no input for remotes.
- **G2 spawn identity** — runtime spawns stamp `Provenance::Authoritative` root +
  `SkipContentStamp`; USD loader suppresses its `Content` stamp under that marker; children
  `Provenance::Local` (addressed locally via `port_map`, never collide). Single-instance
  startup-scene prims unchanged.
- **G3 sessions** — client handshake → `LocalSession` + mark its avatar `LocalAvatar`.
- **G4 ownership** — `PossessVessel` claims ownership (server, from `SyncApplyGuard` origin);
  `DriveRover` authorized against ownership; `authorize` rejects cross-rover commands.
- `declare_channel`: `DriveRover`/`BrakeRover`→`ControlStream`; `PossessVessel`/`SpawnEntity`
  →`CommandBus`. Spawn broadcast carries server gid so peers converge (content-recon).

### lunco-networking (thin lightyear adapter, `networking` feature)
- `lightyear = { version = "0.26.4", optional = true, features = [...] }`.
- `ProtocolPlugin`: `register_message::<ReliableFrame>()` + `::<UnreliableFrame>()`
  `.add_direction(Bidirectional)`; `add_channel::<CmdChannel>(OrderedReliable)` +
  `add_channel::<SnapChannel>(UnorderedUnreliable)` Bidirectional.
- `NetworkingPlugin`: from CLI spawn `Server`/`Client`/host-client entities (ServerPlugins +
  ClientPlugins). WebTransport: server `Identity::self_signed` (ECDSA-P256), print digest +
  write `dist/cert_digest.txt`; native client digest `""`/pinned; wasm client reads URL-hash
  digest. `On<Add, LinkOf>`→ReplicationSender; `On<Add, Connected>`→allocate `SessionId`,
  `SessionRegistry`, send Handshake. `On<Add, Disconnected>`→`release_session`.
- Ferry: drain `SyncOutbox`→`MessageSender`/`ServerMultiMessageSender` (channel by
  `SyncChannel`); `MessageReceiver`→`SyncInbox` (stamp sender `SessionId` on server).

### App wiring
- `sandbox.rs`: parse `--host [port]` / `--connect <addr>`; set `NetworkRole`/`IsServer`;
  add `LunCoNetworkingPlugin`. `lunco-client` `networking` feature → `dep:lunco-networking`.
- Browser: `trunk` `index.html` + runtime cert-digest (URL `#hash`, avoids the spike's
  baked-digest staleness, SPIKE_PH0 §dev-cert-gotchas #4). Build notes for the wasm client.

## Critical path delivered
Ph2 (connect+identity+spawn+possess, reliable) + Ph3 (motion via snapshots, "laggy but
correct"). Ph4 (lightyear prediction for own-rover feel) is the documented next upgrade,
seam unchanged.

## Test localhost
Native host: `cargo run -p lunco-client --bin sandbox --features networking -- --host 5888`.
Native client: `... -- --connect 127.0.0.1:5888 --api 3001`.
Browser: serve the wasm client via trunk, open with `#<digest>` from `dist/cert_digest.txt`.
