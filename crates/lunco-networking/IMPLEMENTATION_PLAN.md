# Implementation plan — build order follows the architecture

Phased plan for networked rovers. **Order follows the mechanism dependency graph**
from `SYNC_ARCHITECTURE.md`: the substrate (M6 clock + M1 identity) first — because
everything rides on a shared tick and stable ids — then M3 (cheapest, reuses
existing code), then M2 (the core), then M4 (prediction), then cosim values.

Constraints honored: builds are `-j=2`, no broad cargo sweeps; each phase ends with
a **visible-in-app** verification the user runs, not automated curl loops.

Target MVP (from `STACK_COMPARISON.md §3`): native host + browser client, both see
each other's rovers, owner predicts its rover, cosim telemetry streamed read-only.

**The concrete user scenario** — *N people connect to one world, each creates + possesses
+ individually drives their own rover, everyone sees them move* — is **not a single phase**:
it spans **Ph2** (connect, identity, spawn, possess — reliable, no motion) → **Ph3** (rovers
move, everyone sees it — laggy) → **Ph4** (your own rover feels responsive, you can't drive
anyone else's). Stage-by-stage breakdown + the 5 gaps it surfaced (G1–G5):
[`MVP_MULTIPLAYER_GAPS.md`](MVP_MULTIPLAYER_GAPS.md).

---

## Phase 0 — Spike + backend decision  *(DONE 2026-05-29)*
**Goal:** de-risk and choose lightyear vs replicon+renet2.
- Native server opening WebTransport (+WebSocket fallback) + one **predicted** avian
  cube driven by `DriveRover`; one **browser** client joins and sees it move.
- Exercises M2-Predicted + M6 tick-sync + transport in one shot.

**Decision:** prediction quality + host-client robustness picks the backend.
**RESULT: lightyear committed and validated** (D1). Ran lightyear's `simple_box`
example (tag 0.26.4, Bevy 0.18) headless: native host-client + a joining client
replicate/predict with stable tick-sync and zero panics under the default latency
conditioner; and a **browser** wasm client connects over WebTransport and receives
replicated state. Everything after this is backend-agnostic at the domain layer.

**Verify:** ✅ server-side `New connection on netcode` + replicated player entity to
the browser. Subjective in-browser input-feel is a manual eyeball (non-gating).

**⚠️ Cert lesson for our own browser client** (full detail in `SPIKE_PH0.md`
§dev-cert-gotchas): WebTransport forces TLS even on localhost; **mkcert/CA trust does
NOT work** when the client sends `serverCertificateHashes` (forces Chrome's hash-only
path). The dev story is the **cert-hash** path: fresh ECDSA-P256 v3 cert, validity
< 14 days, digest the client sends must match the served cert. The example bakes the
digest at compile time and its URL-hash override is dead code — **wire the URL-hash
(or `window.CERT_DIGEST`) digest in our client** so cert rotation never forces a wasm
rebuild.

---

## Phase 1 — Substrate: M6 (clock) + M1 (identity)  *(APPLIED + BUILT GREEN 2026-05-29)*
Nothing can sync correctly without a shared tick and matching ids. Build both.
**Landed** in `lunco-core`: `identity.rs` (`Provenance` + deterministic `derive_id`,
exact port of the green proto-tests), `GlobalEntityId` locked (private field, no
`new()`/`Default`, `pub get`/`from_raw`, crate-internal `allocate_authoritative`),
provenance-aware `assign_global_entity_ids` (safe/incremental fallback), `SimTick`
resource + `advance_sim_tick` (FixedUpdate), `IsServer` resource, +5 Bevy-wiring
tests. **USD-loader provenance stamping DONE** (2026-05-29):
`lunco-usd-bevy::instantiate_usd_prim` — the one chokepoint every prim entity (root +
recursive children) passes through — stamps `Provenance::Content { namespace:"usd",
source:<stage asset path via AssetServer::get_path>, path:<prim path> }`, so USD prims
get deterministic ids instead of the warn-once fallback (verified: `lunco-usd-bevy`
compiles green via `cargo check -p lunco-sandbox-edit -j2`). Known follow-up: instancing
collision (same asset twice → same id; caught by D3a debug check) — see DESIGN_GAPS §B.1.
The concrete warp-in-MP wiring is the last Phase-1 item; it rides on later phases'
integration.

**M1 — identity (`lunco-core`):**
- Add `Provenance` enum; lock down `GlobalEntityId` (no public int constructor).
- Add the single `on_add(Networked)` hook: derive id for Content/Derived,
  server-allocate for Authoritative, panic on violations.
- `hash53` = a **fixed** cross-platform hash (blake3/xxhash), 53-bit, canonical inputs.
- USD loader stamps `Provenance::Content { namespace:"usd", source, path }`.

**M6 — clock:**
- Adopt the backend's tick-sync; replicate sim-tick + dt + `TimeWarpState`.
- Stamp inputs/snapshots/ops in **sim-ticks**, not wall-clock.
- **Decide warp-in-MP policy** (recommend: host-only, applied to all).

**Verify:** two peers load the same USD scene → identical `GlobalEntityId`s for the
same prims (log/inspect); both report the same sim-tick. No motion yet.

---

## Phase 2 — M3 op-log + connect/identity/spawn/possess (stages 1–4 of the MVP scenario)
**STATUS 2026-05-31: ✅ largely DONE + committed.** lightyear WebTransport host+client
wired in-app (`server.rs`/`client.rs`); `SessionId` allocation + `SessionRegistry`;
handshake (session+tick); `SpawnEntity` over the sync layer + replicate with **G2 fixed**
(`SkipContentStamp`→Authoritative id, no collision); over the sync layer `PossessVessel` with
**server ownership validation** (`authorize()`, G4) + `broadcast_ownership`; `ControllerLink`
ownership replicated; **G5 disconnect cleanup** (`release_session`). Verified headless by
`net_smoke` (possess→drive→snapshot + exclusivity). Remaining: **G3** server-provisioned
session avatars (avatars are still client-local), and the live two-browser eyeball test.

Lowest new code — the envelope and dispatch already exist. This phase delivers
**connect → per-user identity → create a rover → possess it**, reliably, with **no
motion yet** (that's Ph3). Full rationale + the code audit behind these items:
[`MVP_MULTIPLAYER_GAPS.md`](MVP_MULTIPLAYER_GAPS.md).

- `declare_channel` routes by `SyncChannel`: `CommandBus`→reliable-ordered
  channel, `ControlStream`→best-effort INPUT channel (later, Phase 4).
- Resolve `GlobalEntityId`↔`Entity` at the boundary via `ApiEntityRegistry`.
- `OpId` dedupe for idempotent apply; server validates + broadcasts.
- **`SessionId` allocation** — server assigns one per connection; fills today's bare-`u64`
  stub with a real per-client session table.
- **Minimal session→avatar handshake (G3, pulled forward from Ph6):** server provisions one
  `Provenance::Authoritative` avatar per `SessionId`; reply tells the client its own avatar's
  `GlobalEntityId`; client stores it as a `LocalAvatar` resource. (Other players' avatars need
  **not** replicate — you see rovers, not cameras.) Just `avatar-id + sim-tick + scene-id`;
  full snapshot/op-log-checkpoint baseline stays Ph6.
- **`SpawnEntity` over the sync layer — now CORE, not stretch (it is stage 3 of the scenario):**
  server spawns, allocates the `Authoritative` root id, and broadcasts it in the mutation so
  peers converge. Geometry loads locally from the shared USD asset (no streaming).
- **Runtime-spawn identity fix (G2 / DESIGN_GAPS §B.1 — REQUIRED here, no longer deferrable):**
  runtime-spawned rovers get `Provenance::Authoritative` (server-allocated unique root) +
  `Derived` children — **not** `Content`. The USD loader's unconditional `Content` stamp is
  correct for startup-scene prims but **wrong for runtime instances** (two `skid_rover.usda`
  spawns would derive the same id → collision). The spawn path must suppress the loader's
  `Content` stamp for runtime subtrees and stamp `Authoritative`+`Derived` instead.
- **Server-side ownership validation (G4):** `PossessVessel` rejects if the target is already
  possessed by another session, or the inbound `avatar` ≠ the sender's session-bound avatar.
  Accepted → apply + broadcast + `Ack`; rejected → `Reject` (add `NotAuthorized`).
- Replicate `ControllerLink` (minimal M2) so peers see who owns what.
- Flow `PossessVessel`, `SpawnEntity`, `ParameterChanged` server→clients.

**Verify (scenario stages 1–4):** two clients connect; each `SpawnEntity`s a rover (both
appear on **both** peers with **distinct** ids — the B.1/G2 guard); each possesses **its own**;
a possess targeting someone else's rover is rejected. Rovers do **not** move yet.

---

## Phase 3 — M2: state replication (the core; absorbs gaps A & C)
**STATUS 2026-05-31: 🟡 PARTIAL.** 20 Hz snapshot replication is built (`gather_snapshot`
host → `ingest_snapshots`/`interpolate_proxies` client), with velocity on the sync layer and a
client interpolation buffer (`INTERP_DELAY`, folded so a body at rest holds its pose).
**Gap A advanced (2026-05-31):** the snapshot now carries the **absolute f64 `pos`**
(avian `Position`) + the **`CellCoord`**, and the client interpolates `pos` in f64 + seats
avian `Position` precisely — so lunar/orbital-scale bodies don't collapse to f32. **TODO
still open:** the per-client **cell→origin rebase** (cells are `[0,0,0]` today under
`switching_threshold=1e10`; the apply assumes cell 0 — see DESIGN_GAPS §A) and gap C
(predict-eligibility by cosim-computability — today the owner predicts any owned dynamic
body; a cosim-driven owned body should interpolate).

- Replicate **`(CellCoord, Transform)`** + drive state — *not* bare Transform.
- **Per-client floating-origin rebasing** at render (gap A): map server cell+offset
  into each client's own origin. Quantize the bounded within-cell offset.
- `SyncClass` registration; **role by computability** (gap C): owner's avian-driven
  rover = candidate Predicted; cosim-driven body (balloon) = Interpolated always.
- Interpolation buffer for simulated proxies; **fold into the existing
  `TranslationInterpolation`/`RotationInterpolation`** (don't double-interpolate).

**Verify:** host + browser client both see each other's rovers move smoothly
(interpolated); the cosim-driven balloon moves (interpolated) without desync as the
camera rebases origin. **Scenario milestone:** at end of Ph3 the full "create + possess +
drive, everyone sees it" loop *works* but feels laggy (input still server-authoritative,
no prediction) — Ph4 makes your own rover feel responsive.

---

## Phase 4 — M4: input + prediction + reconciliation (stage 5 of the MVP scenario)
**STATUS 2026-05-31: ✅ CORE DONE.** Per-tick seq+tick-stamped input
(`compute_vessel_input` in Update for edge-safe latches → `emit_vessel_input` in
FixedUpdate), buffered in `OwnedInputLog`; host records last-applied seq
(`AppliedInputSeq`) and echoes it in every snapshot; **predict-own** (`OwnedLocally` +
`RigidBody::Dynamic`, others kinematic-pinned); **input-replay reconciliation**
(`reconcile_owned_prediction` + the pure `lunco_core::reconcile_decision`, 5 unit tests) —
compare-at-acked-seq so the latency lead cancels (no rubber-band), correct only genuine
divergence + seat velocity, smoothing blend on the rare correction. **Drive-side ownership
validation (G4) enforced** (`authorize()` — net_smoke confirms cross-rover drive is
rejected). **Remaining for "feels right under loss/latency":** client-ahead **tick-sync** +
**server jitter buffer** + redundant input on a dedicated unreliable channel (inputs ride
the reliable command bus today — correct, not yet loss-optimized); **G1** input-isolation
gating to `LocalAvatar` (moot while a client has one avatar, needed for 2-avatar-in-one-world);
**G5** is done (`release_session`); and end-to-end GUI verification under RAM headroom.

- **Input isolation (G1) — load-bearing for "drive only MY rover":**
  `translate_intents_to_commands` reads **process-global** `ButtonInput` today and fans
  WASD to **every** `(VesselIntentState, ControllerLink)` controller — two possessing
  avatars in one world would drive both rovers. Gate raw-input→intent→command to
  `With<LocalAvatar>` only; the **server runs no input mapping for remote clients** (their
  input arrives as `ControlStream`/`DriveRover` messages). Remote-avatar proxies never map input.
- Sample local intent → **redundant, tick-stamped** send (M4) **and** apply to the
  owned rover locally (M2-Predicted) — moves now.
- Server **jitter buffer**: consume one input per tick.
- **Drive-side ownership validation (G4 extended):** server applies a client's `DriveRover`
  only if that session possesses the target rover.
- **Reconciliation = smooth error-correction** toward server state (projective
  velocity blend), **not** full avian rollback (global solver — gap F).
- **Disconnect cleanup (G5):** on session drop, release its `ControllerLink` (free the
  rover) and despawn/retire its avatar.

**Verify (scenario stage 5, the full target):** you drive *your* rover with no perceived
input latency; others see it interpolated; you **cannot** drive anyone else's; corrections
are smooth, no teleport snaps under normal LAN latency.

---

## Phase 5 — Cosim values (completes the MVP: "slice + cosim values")
- Modelica worker stays **server-only**.
- Replicate `AvianSim.outputs` / `SimComponent.outputs` at **low rate** (M2 slow).
- Client telemetry panel reads the replicated values.

**Verify:** browser client shows the rover's live thermal/force telemetry matching
the host, while driving — the full "rover + its sim" synced.

---

## Phase 6+ — post-MVP (each independently, as needed)
- **M5** Modelica source collab (yrs/Yjs) + collaborative cursors (awareness).
- **Late-join baseline polish** (scene handshake + snapshot + op-log checkpoint).
- **Interest management** (only when entity counts grow — README has the design).
- **Auth / roles / ACL** (when leaving trusted LAN).
- **Compression stack** (quantization is partly free via bounded offsets; add LZ4 later).
- **Desync hash beacon** (M6-stamped state-hash compare) + **network-condition sim**
  (latency/jitter/loss injection) for testing Phases 3–4.

---

## Dependency graph (why this order)

```
M6 clock ─┐
M1 ident ─┴─▶ M3 op-log ─▶ M2 replication ─▶ M4 prediction ─▶ cosim values (M2 slow)
                                  │
                                  └─ gap A (big_space) + gap C (predict-eligibility) live here
M5 / interest / auth / compression : independent, bolt on after MVP
M7 (local) : already exists, unchanged
```

Each phase is shippable and verifiable on its own; a stall at any phase still
leaves a working, better-synced app than the phase before.

---

## What stays constant across all phases (the invariants)
- Domain crates call only `app.sync::<T>()` / `register_command` / stamp `Provenance`.
  They never import a backend (PREP.md rule).
- **Networking is an opt-in Cargo feature** (`networking`); see DECISIONS D7.
  `lightyear` is `optional = true`. The **substrate** (Provenance / GlobalEntityId /
  SimTick / IsServer — Ph1) is **always compiled**, never gated. The
  `app.sync::<T>()` / `register_command` facade is always present and is a **no-op**
  when the feature is off, so domain crates never `#[cfg]`-fork. Only the sync layer
  (replication/transport/prediction, lightyear-importing code) lives behind the gate.
- **Local is the default** — no accidental sync traffic.
- Everything stamped in M6 sim-ticks.
- Identity is provenance-derived; ordering is `OpId`. Separate, always.
- Backend choice is contained; swapping it touches only `lunco-networking` glue.
