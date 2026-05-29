# Implementation plan — build order follows the architecture

Phased plan for networked rovers. **Order follows the mechanism dependency graph**
from `SYNC_ARCHITECTURE.md`: the substrate (M6 clock + M1 identity) first — because
everything rides on a shared tick and stable ids — then M3 (cheapest, reuses
existing code), then M2 (the core), then M4 (prediction), then cosim values.

Constraints honored: builds are `-j=2`, no broad cargo sweeps; each phase ends with
a **visible-in-app** verification the user runs, not automated curl loops.

Target MVP (from `STACK_COMPARISON.md §3`): native host + browser client, both see
each other's rovers, owner predicts its rover, cosim telemetry streamed read-only.

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
tests. USD-loader provenance stamping + the concrete warp-in-MP wiring are the
remaining Phase-1 items (see below); they ride on later phases' integration.

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

## Phase 2 — M3: op-log over the wire (reuse `#[Command]`/`Mutation<P>`)
Lowest new code — the envelope and dispatch already exist.
- `register_command` routes by `Replication`: `Authoritative`→COMMANDS (reliable
  ordered), `Ephemeral`→INPUT (later, Phase 4).
- Resolve `GlobalEntityId`↔`Entity` at the boundary via `ApiEntityRegistry`.
- `OpId` dedupe for idempotent apply; server validates + broadcasts.
- Flow `PossessVessel`, runtime spawn, `ParameterChanged` server→clients.

**Verify:** host possesses/spawns a rover → browser client sees the entity appear
and ownership change. Reliable, still no smooth motion.

---

## Phase 3 — M2: state replication (the core; absorbs gaps A & C)
- Replicate **`(CellCoord, Transform)`** + drive state — *not* bare Transform.
- **Per-client floating-origin rebasing** at render (gap A): map server cell+offset
  into each client's own origin. Quantize the bounded within-cell offset.
- `SyncClass` registration; **role by computability** (gap C): owner's avian-driven
  rover = candidate Predicted; cosim-driven body (balloon) = Interpolated always.
- Interpolation buffer for simulated proxies; **fold into the existing
  `TranslationInterpolation`/`RotationInterpolation`** (don't double-interpolate).

**Verify:** host + browser client both see each other's rovers move smoothly
(interpolated); the cosim-driven balloon moves (interpolated) without desync as the
camera rebases origin.

---

## Phase 4 — M4: input + prediction + reconciliation
- Sample local intent → **redundant, tick-stamped** send (M4) **and** apply to the
  owned rover locally (M2-Predicted) — moves now.
- Server **jitter buffer**: consume one input per tick.
- **Reconciliation = smooth error-correction** toward server state (projective
  velocity blend), **not** full avian rollback (global solver — gap F).

**Verify:** you drive *your* rover with no perceived input latency; others see it
interpolated; corrections are smooth, no teleport snaps under normal LAN latency.

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
  when the feature is off, so domain crates never `#[cfg]`-fork. Only the wire layer
  (replication/transport/prediction, lightyear-importing code) lives behind the gate.
- **Local is the default** — no accidental wire traffic.
- Everything stamped in M6 sim-ticks.
- Identity is provenance-derived; ordering is `OpId`. Separate, always.
- Backend choice is contained; swapping it touches only `lunco-networking` glue.
