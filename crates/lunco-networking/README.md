# lunco-networking

Status: Active (shipped — lightyear WebTransport) · Audience: anyone touching networking/replication

Networking layer for LunCoSim — the transparent bridge between simulation state and
sync protocols. **Domain crates never import this crate.** They declare what crosses
the network (replicated components, `#[Command]`s) and the networking layer handles
sync format, authority, and protocol translation silently.

> **Multiplayer Status:** Multiplayer is fully implemented using a **lightyear 0.26.4** backend
> over **WebTransport** (native host + dedicated server + wasm client), provenance-derived
> entity identity, server-authoritative state replication, client prediction +
> input-replay reconciliation + physics-space smoothing, RBAC command/relay gating, and
> a headless server (`sandbox --no-ui --host`). The original design called for
> `renet2 + bevy_replicon` with CCSDS/YAMCS/DDS bridges over an 11-phase roadmap; much of
> the prose below still describes that **aspirational** plan. The
> [Implementation status](#implementation-status) table is the source of truth for
> shipped-vs-planned, and [Known gaps](#known-gaps-open) lists what is not built.

---

## What's here / where to go

This README is the **overview + as-built record**. Five sibling docs hold the deep
detail; this file links into them rather than duplicating them:

- **[DECISIONS.md](./DECISIONS.md)** — the canonical, dated log of *resolved* decisions
  and their rationale (backend = lightyear, reconciliation model, identity = provenance,
  spawn authority, clock seam, the `networking` Cargo feature, deferred items).
- **[SYNC_ARCHITECTURE.md](./SYNC_ARCHITECTURE.md)** — *how everything stays in sync*: the
  seven mechanisms (M1–M7), the case matrix, the tick pipeline, the convergence argument,
  and the procedure for choosing a mechanism for a new feature. As-built prediction lives
  in §4.1 (which points back here for the canonical summary).
- **[USD_REPLICATION_POLICY.md](./USD_REPLICATION_POLICY.md)** — the entity/state
  **replication contract**: what bodies replicate, how a USD scene declares it (derived by
  default; `lunco:net:*` overrides), and what the internal markers mean.
- **[DEPLOY.md](./DEPLOY.md)** — deploying `sandbox.lunco.space`: headless server build,
  systemd unit, nginx, TLS cert + auto-renew, and the local self-signed dev-cert path.
- **[ROS2_BRIDGE.md](./ROS2_BRIDGE.md)** — ROS2/DDS integration as a *bridge* (not a new
  sync mechanism), coordinate-frame and time translation, and the Copper (cu29) alternative.

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Transport Abstraction (as-built)](#transport-abstraction-as-built)
- [Authentication & Authorization](#authentication--authorization)
- [ECS Replication Model](#ecs-replication-model)
- [Authority & Possession](#authority--possession)
- [Client-Side Prediction (as-built)](#client-side-prediction-as-built)
- [Entity Identity Mapping (as-built)](#entity-identity-mapping-as-built)
- [What Domain Code Sees](#what-domain-code-sees)
- [Planned subsystems (not built)](#planned-subsystems-not-built)
- [Existing Solutions Evaluated](#existing-solutions-evaluated)
- [Implementation status](#implementation-status)
- [Known gaps (open)](#known-gaps-open)
- [Cargo feature](#cargo-feature)
- [References](#references)

---

## Architecture Overview

Networking is a **Layer 2b** domain plugin — self-contained, headless-compatible,
removable without affecting simulation correctness:

```
Layer 4: UIPlugins            — lunco-workbench, lunco-ui, domain ui/panels
Layer 3: SimulationPlugins    — Rendering, Cameras, Lighting, 3D viewport, Gizmos
Layer 2: DomainPlugins        — Celestial, Avatar, Mobility, Robotics, OBC, FSW
Layer 2b: NetworkingPlugin    — lunco-networking (transport, replication, auth, bridges)
Layer 1: SimCore              — MinimalPlugins, ScheduleRunner, big_space, Avian3D
```

Domain code speaks only the semantic API (replicated components, typed commands,
`DigitalPort`/`PhysicalPort`, `DVec3`) — **no networking types anywhere**. Below the
`Peer { SessionId }` boundary, the networking layer translates to/from the internal game
protocol and (planned) external bridges:

```
┌─── Domain Code (lunco-mobility, lunco-celestial, lunco-obc) ──────┐
│  DigitalPort(i16), PhysicalPort(f32), DVec3, Typed Commands       │
└──────────────────────┬────────────────────────────────────────────┘
                       │  lunco-networking (transparent shim)
    ┌──────────────────┼──────────────────┐
    ▼                  ▼                  ▼
 Internal game     CCSDS / YAMCS     DDS / ROS2
 protocol          bridge (planned)  bridge (planned)
 (lightyear/WT)    → YAMCS mission   → ROS2 nav/perception
 → LunCo clients     control
```

### Layered auth principle

```
Domain systems react to:  Command (local)  |  Command (remote, verified via policy)
                                       ▲
        Provenance verification ── Auth layer ── Transport layer
```

**Key principle:** Commands stay pure — they never carry origin. Local systems trigger them directly. Remote commands arrive as serialized payloads (`SyncCommand`), get auth-verified via `CommandPolicyRegistry` at the boundary, then execute locally. See [Authentication & Authorization](#authentication--authorization).

---

## Transport Abstraction (as-built)

**SHIPPED.** We picked **one backend — lightyear 0.26.4 — and committed to it** (no
runtime backend-swap abstraction; see [DECISIONS.md D1](./DECISIONS.md)). Domain crates
still never import it: they speak only the semantic API, and everything below
`Peer { SessionId }` is transport-erased.

```
  browser client ──WebTransport (hostname URL + CA cert) ──┐
  native client  ──WebTransport (SocketAddr + digest)   ───┤──▶ [accept] ──▶ Peer{SessionId}
  host's own client (listen-server)  ───────────────────────┘    ──▶ replication + commands
                                                                  (transport tag = diagnostics only)
```

The wire itself is **transport-agnostic** (`sync.rs`: codec, command capture/apply, state
snapshots — no lightyear dep). The lightyear adapter ferries pre-serialized
`sync::SyncEnvelope`s between `SyncOutbox`/`SyncInbox` and two lightyear messages: a
**reliable `CmdChannel`** (commands) and a **best-effort `SnapChannel`** (snapshot deltas).
Above the `Peer` boundary nothing branches on which transport a client used.

As-built transport is **WebTransport only** (QUIC/TLS, browsers *and* native):

| Profile | Build | Transport role |
|---|---|---|
| Native host (listen-server) | `networking` (+ `ui`) | full client+server WebTransport |
| Dedicated server | `networking`, no `ui` (`sandbox --no-ui --host`) | server WebTransport, headless |
| Browser (wasm) | `networking`, client-only | client WebTransport — `wt_client` dials a **hostname URL** so a real CA cert validates with no digest (lightyear's built-in IO is IP-only) |

---

## Authentication & Authorization

The transport layer knows **which connection** sent a message (an opaque handle). Domain systems need to know **who** sent it and **what they're allowed to do**, cryptographically verifiable — not a forgeable client-provided field. Three stages bridge the gap:

```
Transport      "message came from connection #47"  → opaque handle, no identity
   ▼
Auth layer     "#47 = session abc123, role Operator" → maps handle → verified Session;
               validates can-this-session-send-this-command; rejects unauthorized/expired
   ▼
Provenance     attaches verified authorship to command execution context (UserId metadata)
   ▼
Domain systems listen to target commands, and the CommandPolicyRegistry checks authorization.
```

**Single Command Path — commands stay clean, never carry origin:**

- Commands are triggered locally via `commands.trigger(MyCommand { ... })`.
- Networked commands arrive as serialized payloads (`SyncCommand`), where the server resolves the connection to a `Session` (establishing the `UserId` author metadata) and performs an RBAC check against the command policy registry.
- Provenance is verified *at the boundary* between the network and the ECS world, so domain observers can attribute edits to a verified, unforgeable session.

**As-built RBAC:** Command and relay gating is implemented via the `CommandPolicyRegistry` (open-by-default, command + relay gates unified, RBAC-ready). The richer aspirational design — `Session`/`Identity`/`Role` enums, per-role command ACLs, `AuthRegistry` with HMAC session secrets, `Certificate`/`PublicKey` identities — is in git history and not all built.

---

## ECS Replication Model

Domain code declares what crosses the network with **zero networking awareness**; the
networking layer reads it at boundary crossings. Single-player adds no replication plugins,
so there is zero networking footprint.

**Dependency direction (no reverse deps, no aggregator crate):**

```
lunco-mobility / lunco-fsw → lunco-networking (optional, feature: networking)
lunco-networking           → lunco-core (for GlobalEntityId / Provenance types only)
```

**As-built, replication policy is derived from the USD scene, not from a central
`app.replicate::<T>()` registry.** Every non-static rigid body replicates by default
(host-authoritative; clients see a smoothly interpolated proxy); articulated rovers
replicate per-link; cosim-driven bodies are marked opaque automatically. Scene authors
hand-author only *exceptions*, via `lunco:net:*` attributes. The complete contract —
default derivation table, override attributes, internal markers (`NetReplicate`,
`NetExcluded`, `ArticulatedVehicle`/`Link`, `NotPredictable`), and the load/membership
pipeline — lives in **[USD_REPLICATION_POLICY.md](./USD_REPLICATION_POLICY.md)**. The
broader command/op vs state-replication split is in
**[SYNC_ARCHITECTURE.md](./SYNC_ARCHITECTURE.md)** (M1–M7).

> **PLANNED (replicon-era):** the original model registered replication per component in
> domain `replication.rs` submodules (`app.replicate::<RoverMobilityState>()` with custom
> quantizing serializers) and split replicated *state* from locally-reconstructed *topology*
> (`Wire.source/target`, `FlightSoftware.port_map`, etc., stay `Entity` and are rebuilt per
> process, never serialized). That state-not-topology principle still holds; the
> per-component declaration API does not — replication is USD-derived today.

---

## Authority & Possession

Possession negotiation runs through the server so only one session controls a vessel at a
time. A `NetworkAuthority { owner_session, pending_request }` component tracks control;
`RequestAuthority` → server grants/denies → `AuthorityGranted` → local control begins, and
the authority change replicates to all clients. The command itself flows as any other:
client raycast → possess command (`PossessVessel`) → serialize (`SyncCommand`) → server auth+ACL check → execute on server → update `NetworkAuthority` status.

Ownership/authority is mechanism **M3** (totally-ordered-from-authority) in
[SYNC_ARCHITECTURE.md](./SYNC_ARCHITECTURE.md). Note: *ownership ≠ predictability* — owning
a cosim-driven entity still makes it interpolated (opaque), not predicted.

---

## Client-Side Prediction (as-built)

**Client-Side Prediction Status:** Client-side prediction is implemented. The detailed design lives in git history
(`PREDICTION_RECONCILIATION.md`, `PREDICT_AND_SMOOTH_PLAN.md`, `PREDICTION_MEMBERSHIP.md`);
the mechanism context is [SYNC_ARCHITECTURE.md §4.1](./SYNC_ARCHITECTURE.md) (which points
back here for the canonical summary). The as-built shape:

- **Predict-all-vehicles membership** — three disjoint, client-only sets:
  - *Owned, actively driven* (`OwnedLocally`): **input-replay** predicted. The body records
    its post-step pose each tick keyed by input `seq`; on a snapshot that acks a `seq` it
    compares *prediction-at-seq* vs *authority-at-seq* (apples-to-apples, so the latency lead
    cancels) and corrects **only on genuine divergence**. The pure decision is
    `lunco_core::reconcile_decision` (unit-tested, no sync layer).
  - *Predicted props + all remote rovers* (`PredictedDynamic`): run local avian `Dynamic`,
    **state**-reconciled per snapshot. Remote rovers predict so they **yield** to a local
    push (mutual push), not just push.
  - *Everything else* (interpolated proxies): kinematic, **velocity-driven** toward the
    snapshot curve each tick (not teleported), so motion enters contact resolution.
    Cosim-opaque bodies (`NotPredictable`) are never predicted.
- **Correction is physics-space, never render-space.** A diverging reconciler does not touch
  `Transform`; it parks the delta in `PendingCorrection` and `drain_pending_corrections`
  (FixedUpdate, pre-solve) bleeds it into avian `Position`/`Rotation` at a hard cap
  (≤2.5 cm / ≤0.9° per tick, τ≈0.12 s) → smooth, contact-safe slide. Only a gross desync
  (>6 m) seats directly.
- **★ Load-bearing invariant — never write `Transform` from game/netcode.** The client
  enables avian `PhysicsInterpolationPlugin::interpolate_all()`, so
  `bevy_transform_interpolation` owns every `Transform`; any external `Transform` write resets
  that body's easing → client-only jitter. All correction goes through
  `Position`/`Rotation`/velocity. (This is the single most important client-side sync
  invariant — it cost a multi-hour debug.)
- Full rollback of the whole avian world is **ruled out** (global solver, non-determinism);
  we predict-and-correct a 1-body island on the client instead.

> **Still open:** M4 **input hardening** (tick-stamped redundant inputs + host de-jitter
> buffer) is specced but unbuilt — it would shrink corrections at the source under real
> latency (today they stay *smooth*, not *small*). See [Known gaps](#known-gaps-open).

---

## Entity Identity Mapping (as-built)

> **The law:** an entity's network identity is a pure function of its **provenance**.
> Deterministic derivation is the default; server allocation is the rare exception for
> entities genuinely born at runtime. If two peers load the same content, they
> independently arrive at the **same ids** with zero coordination.

The problem this solves: Bevy `Entity` ids are process-local (an index into one World's
storage — meaningless across processes, like a file descriptor). `GlobalEntityId` is the
stable cross-process identity, derived as follows:

- **`Provenance`** (in `lunco-core`) is a required component of any networked entity, a
  small closed set:
  - `Content { namespace, source, path }` — instantiated from shared content (USD today;
    glTF/procedural future). `id = hash53(namespace:source:path)`. Spawned **locally** on
    each peer; spawn is **not** replicated, only state is.
  - `Derived { parent, role }` — deterministic sub-part (rover→wheels, runtime-instance
    descendants). `id = hash53(parent_id/role)`; follows the parent.
  - `Authoritative` — born at runtime, not derivable. Id is **server-allocated**; spawn
    **is** replicated to clients.
  - `Local` — camera/gizmo/selection/preview. No global id, never networked.
- **Enforced by design, not convention.** `GlobalEntityId` has no public integer
  constructor — it is minted only by the identity layer from a `Provenance`, or received
  from the authority. A single `on_add` hook is the only assignment point; contradictions
  (Authoritative spawned on a client, Local marked Networked, missing Provenance)
  **debug-panic**. Adding a new content format only registers a new `ContentLoader`
  namespace — the identity machinery is untouched.
- **`hash53`** is a *fixed, specified* hash (never `DefaultHasher`), truncated to **53 bits**
  (JS-safe), over canonicalized bytes so it is byte-identical across desktop/wasm. 53-bit
  collision handling is per [DECISIONS.md D3a](./DECISIONS.md) (debug-time check at load;
  revisit only near ~10⁶ entities).
- USD as-built: `lunco-usd-bevy::instantiate_usd_prim` stamps
  `Provenance::Content { namespace:"usd", source:<stage asset path>, path:<prim path> }`.
  Runtime-spawned instances get an authoritative root id (replicated) and `Derived`
  descendants (per-peer reconstructible) — the USD-standard hierarchical-identity model that
  fixed the instance-collision bug (B.1; see
  [USD_REPLICATION_POLICY.md](./USD_REPLICATION_POLICY.md) and [DECISIONS.md D3/D4](./DECISIONS.md)).

`OpId` (operation ordering) stays separate from `GlobalEntityId` (entity identity) — don't
conflate them.

**Design rule that still holds: `GlobalEntityId` is a component, never a field type.**
Domain code uses `Entity` everywhere (queries, `Wire.source`, `ControllerLink.vessel_entity`,
`ChildOf`); the networking layer reads `GlobalEntityId` only when crossing boundaries
(serialize, command resolution, edit logging). Putting `GlobalEntityId` in component fields
would force a HashMap lookup into every system iteration — Bevy needs `Entity` for component
access regardless.

> **PLANNED (replicon-era, superseded):** the original scheme minted random/time-based
> **ULID-derived `u64`** ids via an `On<Add<Replicated>>` observer and tracked them in a
> bidirectional `EntityRegistry`. Provenance derivation replaces it; a registry-style local↔global
> map still exists as an implementation detail, but ids are derived, not random.

---

## What Domain Code Sees

```rust
// lunco-mobility/src/lib.rs — ZERO networking awareness
#[derive(Component, Clone, Copy, Reflect)]
#[reflect(Component)]
struct DriveCommand { digital: DigitalPort, physical: PhysicalPort }

fn apply_drive_commands(mut query: Query<(&DriveCommand, &mut GlobalTransform)>) {
    for (drive, mut transform) in query.iter_mut() {
        transform.translation += DVec3::Z * drive.physical.value as f64 * dt;
    }
}
```

That's it. Replication, prediction, auth, identity, and (planned) CCSDS/YAMCS export — all
handled by the `lunco-networking` plugin registered at startup.

---

## Planned subsystems (not built)

These were part of the original 11-phase roadmap. They are **designed but not implemented**
(see [Implementation status](#implementation-status)); the full prose is in git history.
Summaries:

- **Collaborative editing (event sourcing).** Every sandbox edit recorded as a structured
  `EditEvent` (Spawn/Delete/TransformChanged/ParameterChanged/WireConnected/Undo/
  CatalogEntryAdded) in an append-only `EditLog`, ordered by a `LamportClock`, replayable for
  late-join and reversible for networked undo. Conflicts resolve by server-assigned `op_id`
  total order (last-writer-wins per field). An op-log substrate exists today (`Mutation<P>` /
  `OpId`), but `EditLog`/checkpoint/undo are not built. Mechanism = **M3** in
  [SYNC_ARCHITECTURE.md](./SYNC_ARCHITECTURE.md).
- **Yjs for Modelica code collaboration.** Concurrent `.mo` text edits need a CRDT for
  deterministic merge; the plan uses `yrs` (Yjs-Rust) docs synced over a dedicated channel
  plus the awareness protocol for collaborative cursors. No `yrs` dependency yet. Mechanism =
  **M5**.
- **Dynamic USD support.** A file watcher broadcasts `RELOAD_USD_FILE`; the server records
  deletes+spawns as `EditEvent`s and clients converge to the reloaded scene. Runtime catalog
  edits broadcast as `CatalogEntryAdded`. Not built.
- **Compression stack.** Three layers — semantic (position quantization `DVec3`→`u16×3`,
  smallest-three quaternions, delta encoding, dead reckoning, bit-packed bools, varint ids,
  command dictionary; ~5–10x), protocol-level (~2–3x), and LZ4/Zstd with per-channel threshold
  policy (~1.5x). Today snapshots carry absolute f64 pos + `CellCoord` with **no** quantization
  or LZ4.
- **Interest management.** Distance/possession-tiered subscription (HIGH ±500 m full @60 Hz,
  MEDIUM state-only @10 Hz, LOW aggregates only) to avoid the 1000-entity state explosion.
  Targeted ~33x bandwidth reduction (≈1.5 KB/s per client, ≈15 KB/s server egress for 10
  clients) is a *design estimate*, not measured. Today all clients get all entities.
- **Space-standards compatibility (CCSDS / PUS / XTCE / YAMCS).** A three-layer model: the
  internal game protocol (opaque to YAMCS), an XTCE schema auto-generated from Bevy `Reflect`
  types, and a CCSDS packet stream pushed to YAMCS over WebSocket. The structural mapping is the
  durable design artifact:

  | LunCoSim type | XTCE concept | CCSDS field |
  |---|---|---|
  | `DigitalPort` (i16) | `IntegerParameter` | 16-bit raw value |
  | `PhysicalPort` (f32) | `FloatParameter` | 32-bit engineering value |
  | `Wire` (scale + source) | `PolynomialCalibrator` | calibration coefficients |
  | Typed Command / type / fields | `MetaCommand` / name / `ArgumentList` | TC packet / APID / data field |
  | `Session` / `AuthRegistry` | PUS User Management | service type 1 |

  `DigitalPort` being `i16` is deliberate — it is exactly a typical spacecraft telemetry
  register. No code in `src/` yet.

---

## Existing Solutions Evaluated

**Decision: don't replace ROS2/DDS — bridge to it.** LunCoSim stays Bevy ECS internally
(deterministic system order, single `cargo run` binary, headless `MinimalPlugins`,
WASM/browser support, `f64`/`big_space`, `App::new()`-testable) and communicates with ROS2
nodes / DDS publishers over a transparent bridge. This mirrors NASA VIPER's **cFS (flight) +
ROS2 (autonomy)** hybrid: `lunco-obc` + typed commands ≈ cFS Software Bus,
`lunco-mobility`/`lunco-robotics` ≈ ROS2 nodes, networking layer ≈ the bridge. Bridge design
is in **[ROS2_BRIDGE.md](./ROS2_BRIDGE.md)**; the backend choice (lightyear over
renet2/replicon) and its rationale are in **[DECISIONS.md D1](./DECISIONS.md)**.

Standards landscape (all bridge-side, **not built**): CCSDS Space Packets, XTCE
(CCSDS 660.0), PUS (ECSS), DDS (OMG), cFS Software Bus UDP, F Prime serialization, CCSDS
Time, CFDP file delivery.

---

## Implementation status

Source of truth for shipped-vs-planned. The original 11-phase roadmap assumed
`renet2 + bevy_replicon`; what shipped used **lightyear** instead, so the historical per-task
checklists (kept in git history) are moot at the task level even where the *capability* is
delivered.

| Phase | Status |
|---|---|
| **1. Foundation** (transport, replication, auth, identity) | ✅ **SHIPPED** — lightyear WebTransport + provenance identity + RBAC (not replicon/renet2) |
| **2. Collaborative Editing** (EditLog, Lamport, replay) | ❌ **PLANNED** — op-log substrate exists (`Mutation<P>`/`OpId`); EditLog/checkpoint not built |
| **3. Networked Undo** | ❌ **PLANNED** — not built |
| **4. Client-Side Prediction** | ✅ **SHIPPED** — predict-all-vehicles + input-replay reconciliation + physics-space smoothing (see [Client-Side Prediction](#client-side-prediction-as-built)) |
| **5. Compression** (quantization, delta, dead-reckoning, LZ4) | ❌ **PLANNED** — snapshot carries absolute f64 pos + CellCoord; no quantization/LZ4/dead-reckoning |
| **6. Interest Management** | ❌ **PLANNED** — not built (all clients get all entities) |
| **7. Yjs for Modelica Collaboration** | ❌ **PLANNED** — no `yrs` dependency |
| **8. Dynamic USD Support** | ❌ **PLANNED** — not built |
| **9. Space Standards Bridge** (CCSDS/PUS/XTCE/YAMCS) | ❌ **PLANNED** — no code in `src/` |
| **10. ROS/DDS Bridge** | ❌ **PLANNED** — no ros2/dds code in `src/` (design in [ROS2_BRIDGE.md](./ROS2_BRIDGE.md)) |
| **11. UI Plugin** | ⚠️ **PARTIAL** — in-sim Connect panel + presence cursors shipped (`mod ui`); authority/peer-list/interest-debug panels not built |

---

## Known gaps (open)

Distilled from the now-deleted `DESIGN_GAPS.md` (full A–I analysis + the DONE/RESOLVED items
are in git history). The model itself — **state replication + client prediction**
(Source/Overwatch/Unreal/lightyear), *not* lockstep (avian is not cross-platform
deterministic) and *not* full physics rollback (global solver) — is settled. Still open:

- **Gap A — per-client `big_space` cell→origin rebase.** PARTIALLY DONE: snapshots carry
  absolute **f64 `pos`** + `CellCoord`, interpolated in f64 and seated into avian `Position`.
  The live app runs a single huge cell so `CellCoord` is always `[0,0,0]` — the cell is
  *carried but not consumed*. TODO: once recentering is enabled, the apply must decompose `pos`
  into the client's own `(CellCoord, Transform)` via
  `lunco_core::coords::world_to_grid_local` (rebase math already proven by the `proto-tests`
  `rebase_*`/`world_roundtrip_*` suite).
- **Gap G — M4 input hardening (redundancy + server-side jitter buffer).** UNBUILT. Inputs
  ride an unreliable channel; a dropped input is a hitch. Need: each packet carries the last N
  unacked inputs; the host keeps a small per-client input buffer. This shrinks prediction
  corrections at the source under real latency.
- **Deferred (acknowledged, not built):** rover↔rover collision under prediction (accept the
  snap for now), server-rewind lag compensation (no shooting → low priority), real input
  validation / anti-cheat (LAN co-op; clamp inputs server-side).

---

## Cargo feature

Networking is an **opt-in Cargo feature** (`networking`) that gates the sync layer only —
single-player compiles it out entirely (see [DECISIONS.md D7](./DECISIONS.md)). As-built the
single transport is WebTransport; prediction runs client-side only (the server is
authoritative and never predicts). The richer per-transport feature matrix
(`transport-udp/-ws/-wt/-server`) was part of the aspirational multi-transport plan and is not
how the crate is gated today.

---

## References

- [lightyear](https://github.com/cBournhonesque/lightyear) — **the shipped backend** (0.26.4): WebTransport transport, replication, prediction/interpolation, tick-sync
- [renet2](https://github.com/UkoeHB/renet2) — *evaluated, not used* — transport abstraction (UDP, WS, WT, Steam)
- [bevy_replicon](https://github.com/simgine/bevy_replicon) — *evaluated, not used* — ECS replication for Bevy
- [bevy_replicon_renet2](https://github.com/simgine/bevy_replicon_renet) — *evaluated, not used* — Renet2 backend for replicon
- [yrs (Yjs Rust)](https://github.com/y-crdt/y-crdt) — CRDT-based collaborative editing (planned)
- [CCSDS 133.0-B-2 Space Packet Protocol](https://ccsds.org/Pubs/133x0b2e2.pdf) — 6-byte primary header standard
- [CCSDS 660.0-B-2 XTCE](https://ccsds.org/Pubs/660x0b2.pdf) — XML Telemetric and Command Exchange
- [YAMCS](https://docs.yamcs.org/) — Mission control system with WebSocket API
- [NASA cFS](https://github.com/nasa/cFS) — core Flight System framework
- [F Prime (JPL)](https://github.com/nasa/fprime) — Flight software framework (Ingenuity helicopter)
- [SpaceROS](https://github.com/space-ros) — Hardened ROS2 for space robotics
- [VIPER Rover Architecture](https://ntrs.nasa.gov/api/citations/20250004148/downloads/viper-2025-04-24.pdf) — cFS + ROS2 hybrid pattern
