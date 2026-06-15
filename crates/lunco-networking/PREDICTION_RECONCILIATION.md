# PREDICTION_RECONCILIATION.md

**Client-prediction + server-reconciliation (input-replay) for the owned avian rover.**

Status: DESIGN, ready to implement. Supersedes decision **D2** for the owned rover only.
Author: networking architect. Date: 2026-05-30.
Audience: engineers implementing this. Every claim is anchored `file:line` to the real codebase.

This doc replaces *continuous smooth-correction* (`correct_owned_prediction`,
`crates/lunco-sandbox-edit/src/commands.rs:402-500`) on the **locally-owned** rover with
**input-replay reconciliation**: snapshot acks an input seq → client compares its
predicted-state-at-that-seq to authoritative → snaps the avian body to authoritative →
re-steps avian over the still-unacked inputs → render-smooths the residual.

---

## 1. DECISION REOPEN

### 1.1 What changes

**D2** (`crates/lunco-networking/DECISIONS.md:35-43`) currently reads:

> **D2 — Reconciliation: smooth error-correction, never full avian rollback**
> Predict the rover kinematically; error-correct toward server state (projective
> velocity blending). Full rollback is ruled out *by construction*: avian's solver is
> global (contact islands couple bodies) and non-deterministic across platforms — a
> wasm client can't reproduce it bit-exact.

**New D2 (this doc):**

> **D2 — Reconciliation: input-replay reconciliation with on-demand avian re-stepping
> for the OWNED rover; interpolation for all remote bodies.**
> Predict the owned rover by running real avian dynamics locally. On each snapshot
> that acks an input sequence number, snap the owned body's 4 integrator components to
> authoritative state and re-step avian over the unacked-input buffer (~3–6 fixed
> ticks). State replication + re-anchoring (NOT deterministic lockstep) bounds drift to
> the unacked window; a render-only smoothing pass absorbs the residual.

### 1.2 Why the "ruled out by construction" argument no longer holds

D2's three premises were each individually true but jointly do **not** rule out *this*
design — they ruled out *deterministic lockstep rollback*, which is a different thing:

1. *"avian's solver is global (contact islands couple bodies)"* — true in general, but on
   a **client** the owned chassis is the **only `Dynamic` body**. Every other replicated
   rover is pinned `RigidBody::Kinematic` by `force_kinematic_proxies`
   (`crates/lunco-sandbox-edit/src/commands.rs:149-188`, the `Without<OwnedLocally>`
   filter at `:164`), and every wheel system early-outs on `Kinematic`
   (`crates/lunco-mobility/src/lib.rs:144,220,295-299`). A replay tick therefore solves a
   **1-body island** — there is no coupling to replay.
2. *"non-deterministic across platforms — wasm can't reproduce it bit-exact"* — true, and
   **we do not need bit-exactness.** This is *state replication*, not *deterministic
   replication*. Each snapshot re-anchors the client to the server's authoritative
   `{Position, Rotation, LinearVelocity, AngularVelocity}`; replay only spans the ~3–6
   unacked ticks since that anchor, so non-determinism produces a *bounded residual*, not
   unbounded divergence. `lightyear_avian3d-0.26.4` (already in cargo cache) ships exactly
   this state-replication path and keeps the `deterministic` cargo feature OFF. (See §4.5.)
3. *"mainstream FPS reconcile only the local movement component"* — that is precisely what
   we now do: the *local movement component* here **is** the single owned rigid body.

### 1.3 The unfixable rubber-band (latency argument — why we must reopen, not retune)

`correct_owned_prediction` is a continuous exponential blend toward a velocity-extrapolated
authoritative target (`commands.rs:438-490`, `CORRECTION_RATE=3.5`,
`CORRECTION_EXTRAP_MAX=0.06`). Its error is structural, not a tuning problem:

- The authoritative target the client blends toward is **already old** by one full link
  latency + the 20 Hz snapshot quantum (`NetworkConfig::replication_hz=20`,
  `sync.rs:129`). Snapshots are also `only_if_changed`-gated (`sync.rs:439-446`), so a
  rover that just stopped emits *nothing* and the blend extrapolates against the
  reconstructed finite-difference velocity (`commands.rs:447`).
- Because the local body keeps integrating live avian forces while the blend pulls it
  toward a stale target, the two fight every tick: raise `CORRECTION_RATE` and you get a
  visible *snap/jitter*; lower it and you get *lag/overshoot* on every direction change.
  There is no value that is both responsive **and** smooth, because the target is
  intrinsically a latency behind the player's own inputs. This is the classic continuous-
  correction rubber-band, and it cannot be retuned away.
- Input-replay fixes it at the root: the player's **own** inputs are applied locally with
  zero latency (full prediction), and the server's authoritative state is folded in by
  *comparing at the same input seq* and replaying *only* the inputs the server hasn't seen
  yet — so the corrected state already accounts for everything the player has done since.

### 1.4 Scope of the reversal (narrow)

- **Reversed for:** the single `OwnedLocally` rover on a client
  (`crates/lunco-core/src/session.rs:226-227`). It alone runs full local avian prediction
  and on-demand replay.
- **Unchanged for:** all remote rovers + balloons + cosim targets. They stay
  `Kinematic`-pinned (`force_kinematic_proxies`) and rendered via `interpolate_proxies`
  with `INTERP_DELAY=0.12s` (`commands.rs:251-310`, `:209`). The interpolation path is
  untouched.
- **Unchanged decisions:** D1 (lightyear), D3 (identity), D4 (spawn authority), D5
  (time-warp host-only), D6 (Tick seam — we re-step *avian directly*, we do **not** need
  lightyear's `Tick`; D6 stays Ph3/4, `MVP_MULTIPLAYER_GAPS.md:33`), D7 (opt-in feature).
- **One client-only plugin divergence to flag** (not yet decided, see §8): if we later
  adopt `lightyear_avian3d` wholesale, its plugin doc mandates disabling avian's
  `PhysicsInterpolationPlugin`/`PhysicsTransformPlugin` on the client. For the hand-rolled
  MVP in this doc we do **not** adopt that crate, so `interpolate_all()`
  (`crates/lunco-client/src/bin/sandbox.rs:220`) stays as-is.

---

## 2. ARCHITECTURE — the five pieces and where each lives

Data flow for one owned rover (client = predictor, host = authority):

```
 CLIENT (Update/observer)            SYNC                  HOST (Update + FixedUpdate)
 ┌───────────────────────┐                                ┌───────────────────────────┐
 │ (1) input seq+buffer  │  DriveRover{..,seq,tick} ───▶  │ apply_sync_command:        │
 │ translate_intents_..  │   (ControlStream, OrderedRel)  │   authorize → record       │
 │  stamp seq+SimTick,    │                                │   last_applied_seq[gid]    │
 │  push InputFrame ring │                                │   → on_drive_rover ports   │
 └──────────┬────────────┘                                │ FixedUpdate: wheel forces  │
            │                                              │   → avian solve → writeback│
 ┌──────────▼────────────┐                                │ gather_snapshot (post-fixed)│
 │ (2) local prediction  │   SnapshotMsg{tick, entries:   │   reads Pos/Rot/Lin/Ang +  │
 │  owned body = Dynamic,│ ◀─ [{gid,t,r,lv,av,last_seq}]} │   last_applied_seq per gid │
 │  full avian + wheels  │                                └───────────────────────────┘
 │  every FixedUpdate    │
 └──────────┬────────────┘
            │  + push predicted-state into history ring keyed by SimTick/seq
 ┌──────────▼────────────────────────────────────────────────┐
 │ (3)+(4) server ack + reconcile  (FixedPostUpdate, replaces  │
 │         correct_owned_prediction)                           │
 │  on snapshot with NEW last_seq for owned gid:               │
 │   a. compare authoritative(pos,rot,lv,av) vs history[last_seq]│
 │   b. if error > eps: snap body to authoritative             │
 │   c. drop InputFrames with seq <= last_seq                  │
 │   d. replay remaining InputFrames: re-step avian N times     │
 │   e. store post-replay state as new predicted "present"      │
 └──────────┬────────────────────────────────────────────────┘
            │
 ┌──────────▼────────────┐
 │ (5) render-smooth      │   residual = (pre-snap render pose) - (post-replay pose)
 │  visual offset decays  │   applied to Transform/visual only, never to Position
 │  to 0 over a few frames│
 └───────────────────────┘
```

### Piece-by-piece, concrete homes

**(1) Input seq + unacked buffer.** Born in `translate_intents_to_commands`
(`crates/lunco-controller/src/lib.rs:57`, edge-gate `:115-130`). At the existing
`if prev != current` block (`:117`): increment a per-vessel `u32` seq, read `Res<SimTick>`,
stamp `seq`+`tick` onto `DriveRover`/`BrakeRover` (both get the **same** seq — one input
frame = forward+steer+brake, fired together at `:118`/`:124`), and push an `InputFrame`
into a new client-side ring buffer (a `Component` on the owned vessel, or a `Resource`
keyed by `GlobalEntityId`). **Note the edge-trigger gotcha:** today inputs are *sparse
level setpoints with implicit hold*, not per-tick samples (`on_drive_rover` writes a
latched `DigitalPort.raw_value`, `crates/lunco-mobility/src/lib.rs:467-503`). See §4.2 and
§6.1 for how replay carries the latched value forward — and the §7 P2 decision to move
emission to `FixedUpdate` so each fixed tick has exactly one owned input.

**(2) Local prediction.** Already exists and is KEPT. The owned body is set
`RigidBody::Dynamic` by `maintain_owned_locally` (`commands.rs:323-358`, insert at `:350`),
and the mobility wheel chain runs on the client (the `!Client` gate was dropped,
`crates/lunco-mobility/src/lib.rs:43-58`). So the owned rover already integrates real avian
forces every `FixedUpdate`. We ADD a **predicted-state history ring**: after avian writeback
each fixed tick, record `{seq_last_applied, SimTick, Position, Rotation, LinearVelocity,
AngularVelocity}` so reconcile can look up "what I predicted at the seq the server just
acked."

**(3) Server ack.** Host records, per owned gid, the highest applied input seq, in
`apply_sync_command` right after `authorize` succeeds (`crates/lunco-api/src/sync.rs:291-300`
— gid already in hand at `:292`) into a new always-on `AppliedInputSeq(HashMap<u64,u32>)`
resource. `gather_snapshot` (`sync.rs:414-459`) and the connect baseline
(`crates/lunco-networking/src/server.rs:162-181`) read it and stamp `last_input_seq` per
`SnapshotEntry`. See §3 for sync shape and §6.4 for the apply/gather phase issue.

**(4) Reconcile = compare-at-seq + snap + replay.** New system replacing
`correct_owned_prediction`, same slot: `FixedPostUpdate`, after `PhysicsSystems::Writeback`
(`commands.rs:720-723`). Reads a new velocity+seq-bearing inbound queue (NOT the pose-only
`InterpBuffers`), gated on `OwnedLocally`. Logic in §4.

**(5) Render-smooth.** A small visual-offset decay applied to the rover's rendered
`Transform` (or a child visual), **never** to avian `Position`. Repurposes the existing
dead-band/ramp machinery (`commands.rs:472-490`) but on the *post-replay residual* only.
See §4.6.

---

## 3. SYNC CHANGES (exact)

The sync layer is **unversioned JSON** (`serde_json::to_vec`/`from_slice` on `SyncEnvelope`,
`crates/lunco-networking/src/shared.rs:16-22`; no `deny_unknown_fields` anywhere). Therefore
**all new fields are `#[serde(default)]`** → free forward/backward compatibility (old peer
ignores unknown fields; new peer fills defaults). No `SyncEnvelope` variant bump. Optionally
bump `PROTOCOL_ID` (`shared.rs:13`) so mismatched builds *refuse to connect* rather than
silently drop frames (`deserialize_env` swallows parse errors via `.ok()`).

### 3.1 `SnapshotEntry` — add velocity + per-entity ack

`crates/lunco-api/src/sync.rs:50-55`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub gid: u64,
    pub t: [f32; 3],                      // translation  (existing)
    pub r: [f32; 4],                      // rotation quat (existing)
    #[serde(default)] pub lv: [f32; 3],   // NEW: linear velocity  (avian LinearVelocity, f64→f32)
    #[serde(default)] pub av: [f32; 3],   // NEW: angular velocity (avian AngularVelocity, f64→f32)
    #[serde(default)] pub last_input_seq: u32, // NEW: server's last-applied input seq for THIS gid (0 = none)
}
```

**`last_input_seq` is PER-ENTITY, never global on `SnapshotMsg`.** Two hard reasons:
(a) one client can own multiple rovers (`SessionRegistry` maps many gids→one session,
`release_session` frees a `Vec<u64>`, `session.rs:174`) — each needs its own ack;
(b) snapshots are serialized once and broadcast `NetworkTarget::All`
(`crates/lunco-networking/src/server.rs:226`) — a global per-client ack would force
per-client serialization. Per-entity keeps the one-serialize-broadcast-all model; each
client reads acks only for gids it owns (`reg.owns(local.0, gid)`, `commands.rs:337`) and
ignores the rest. For non-input bodies (balloons) `last_input_seq` stays `0`, harmless.

### 3.2 `SnapshotMsg` — unchanged shape

`sync.rs:58-62`. Keeps `tick: u64` (server `SimTick` at gather time) + `entries`. The
per-entity ack lives in the entries (§3.1). `tick` is the *secondary* coordinate (server
tick the state holds at); the ack match itself is **seq-based** (§4.1).

### 3.3 `SnapshotSample` (in-process mirror) — mirror the new fields

`crates/lunco-core/src/session.rs:258-263` must gain `lv`, `av`, `last_input_seq` so
`drain_sync_inbox` (`sync.rs:365-376`) can copy them through. `ingest_snapshots`
(`commands.rs:216-238`) and `InterpSample` (`commands.rs:192-197`) only need the velocity/seq
for the **owned** path; remote interpolation ignores them.

### 3.4 Outgoing `DriveRover` / `BrakeRover` — add seq + tick

`crates/lunco-mobility/src/lib.rs:430-442`:

```rust
#[Command]
pub struct DriveRover {
    pub target: Entity,
    pub forward: f64,
    pub steer: f64,
    #[serde(default)] pub seq: u32,   // NEW: monotonic per-vessel input seq
    #[serde(default)] pub tick: u64,  // NEW: client SimTick at emit (debug / coarse ordering)
}
// BrakeRover { target, intensity, seq, tick }  — SAME seq as the paired DriveRover
```

These fields ride through `capture_command` **for free**: it reflect-serializes by field
name (`sync.rs:243-253`) and `extract_target_gid` only reads `params["target"]`
(`sync.rs:272-273`). No codec change. The seq stamped on the *payload* reaches the host
inside `SyncCommand.data`; the host reads it in `apply_sync_command` (§6.4). We do **not**
need to add seq to the `Mutation` envelope (`commands.rs:150`) — payload-level is simpler
and `capture_command` stays untouched.

**Why not reuse `OpId` as the seq?** `OpId` (`commands.rs:41-50`) is a time-sorted 53-bit
*hash*, not a dense `++1` counter — you can compare "newer than" but you **cannot** do
`last_seq + 1` arithmetic or index a ring with it, and it is global, not per-rover. Use it
(it already travels + dedupes, `SyncDedup` at `sync.rs:141-172`) as the idempotency token;
use a **dense per-vessel `u32` `seq`** for ack/replay math.

---

## 4. THE REPLAY MECHANISM

### 4.1 Compare key = seq, not tick

Inputs are **not** 1:1 with ticks: `capture_command`/`translate_intents_to_commands` fire on
the observer/`Update` timeline, `advance_sim_tick` runs in `FixedUpdate`
(`crates/lunco-core/src/lib.rs:341,350-355`) — many inputs can land in one tick or zero
across several. And on the client today `SimTick` is **non-monotonic**: it is hard-clobbered
to the server's tick on every snapshot (`sync.rs:375`, `tick.0 = snap.tick`). So:

- **Ack match is by `seq`** (dense, monotonic per-vessel). `SnapshotEntry.last_input_seq`
  tells the client "I (server) have integrated everything up to seq N for this rover."
- **`tick` is the local replay clock / history ring index**, derived from the client's own
  fixed-step counter — see §6.5 (we add a never-clobbered `ClientPredictedTick`, or stop
  letting Snapshot overwrite `SimTick` for the owned path).

### 4.2 State to save/restore (the integrator state)

Per the avian map, the **complete** XPBD integrator state for one `Dynamic` body is exactly
4 components:

| Component | f64 type | Role |
|---|---|---|
| `Position` | `DVec3` | restore target |
| `Rotation` | `DQuat` | restore target |
| `LinearVelocity` | `DVec3` | restore target |
| `AngularVelocity` | `DVec3` | restore target |

**Snap-set = these 4 on the chassis only** (wheels are children with raycasts, not bodies).
Do **NOT** save/restore:
- **Forces** — rebuilt from scratch each tick by the wheel systems
  (`crates/lunco-mobility/src/lib.rs:43-48`); avian zeroes external forces after each step.
- **`WheelRaycast.last_normal_force`** (`mobility/lib.rs:93`) — written by suspension and
  read by drive *within the same FixedUpdate* (`:172`→`:225`, chained `:43-48`), recomputed
  before consumed every tick. No cross-tick memory.
- **`RayHits`** — recomputed by avian's spatial-query step each schedule run from restored
  collider positions.
- **FSW / port state** (`FlightSoftware.brake_active`, `DigitalPort.raw_value`,
  `PhysicalPort.value`) — this is the **INPUT**, not state. It must be **re-driven** from the
  `InputFrame` buffer each replayed tick (§4.4), because the
  `DigitalPort → PhysicalPort → wheel-force` chain lives entirely in `FixedUpdate`
  (`crates/lunco-hardware/src/lib.rs:20-24`, `mobility/lib.rs:43`) and is reconstructed, not
  saved.

### 4.3 How to re-step avian on demand

**Re-run the fixed-main-loop, NOT the bare `PhysicsSchedule`.** Avian's step lives in the
`PhysicsSchedule` sub-schedule, *driven from* `FixedPostUpdate` by `run_physics_schedule`
(avian `src/schedule/mod.rs:240`; installed via `PhysicsPlugins::default()` →
`FixedPostUpdate`, `sandbox.rs:220`). `run_physics_schedule` advances `Time<Physics>` by the
generic clock delta and runs the schedule **only if delta ≠ 0** (the `is_zero()` guard,
`mod.rs:266-269`).

We must re-run the **whole `FixedUpdate` + `FixedPostUpdate`** for each replayed tick, NOT
just `PhysicsSchedule`, because the **force pipeline** (`DigitalPort → PhysicalPort → wheel
suspension/drive/steer`) lives in `FixedUpdate`, *upstream* of the physics step. Bare
`PhysicsSchedule` (the manual-step API documented in `crates/lunco-cosim/src/avian.rs:38-49`)
would skip the wheel forces entirely — wrong for a rover.

Per replayed tick K (for `seq > last_input_seq`):
1. Re-drive ports from `InputFrame[K]` (re-apply the latched setpoint in effect at K — see
   §4.4 for the held-key carry-forward).
2. Run `FixedUpdate` (wheel forces) then `FixedPostUpdate` (avian solve + writeback). Set the
   physics delta to the fixed `1/60 s` so `run_physics_schedule` steps exactly once.
3. Repeat for the next seq.

The cosim "do NOT step manually" comment (`lunco-cosim/src/lib.rs:80-83`,
`step_avian.rs:5-6`) is a **double-step warning**, not a feasibility blocker: it only mandates
that replay re-stepping be **mutually exclusive with the normal once-per-frame step** (replay
runs off the normal frame cadence, inside the reconcile system, exactly once per snapshot
with a new ack). The Modelica/balloon cosim force path is already gated
`run_if(NetworkRole != Client)` (`lunco-cosim/src/lib.rs:104-106`) so it does **not** re-run
during client replay — correct. The rover wheel path is un-gated (`mobility/lib.rs:50-58`)
so it **does** re-run — also correct.

### 4.4 Held-key carry-forward (the load-bearing replay detail)

Inputs are *latched level setpoints*, not per-tick impulse samples
(`translate_intents_to_commands` edge-gate `:115-130`; `on_drive_rover` writes a persistent
`DigitalPort.raw_value`). So the `InputFrame` buffer is **a sequence of setpoint *changes*,
each tagged with the tick it took effect**, NOT one entry per tick. Replay must, for each
re-stepped tick, re-apply the **last setpoint in effect at-or-before that tick** (carry the
latched value forward across ticks with no new edge). Concretely the reconcile system holds a
cursor over the `InputFrame` ring and, per replayed tick, advances the cursor to the newest
frame whose `tick ≤ replay_tick`, writes its `(forward, steer, brake)` to the ports, then
steps.

The clean alternative (recommended in §7 P2): **emit one `InputFrame` per fixed tick** by
moving emission to `FixedUpdate` and dropping the `if prev != current` gate — then every avian
fixed-step has exactly one owned input and replay is a trivial 1:1 loop (Gambetta/Source
style). That is a behavioral change to `translate_intents_to_commands`; it is the load-bearing
fork for clean replay and is scoped as its own phase.

### 4.5 How many ticks, cost, and why imperfect f64 determinism is fine

**N = unacked ticks.** Snapshots arrive at 20 Hz = every 50 ms = 3 fixed ticks at 60 Hz; with
16–100 ms RTT, **N ≈ 3–6**. Cost = N extra `FixedUpdate`+`FixedPostUpdate` iterations per
snapshot, each solving a **1-body island** (owned chassis is the only `Dynamic` body; all
proxies `Kinematic`, wheel systems early-out on `Kinematic`). A 1-body avian tick with ~4–6
raycasts is tens of µs; N=6 ⇒ low-hundreds of µs, ×20/s ⇒ **single-digit ms per wall-clock
second, well under 1% of a 60 fps frame budget.**

**Imperfect f64 determinism is acceptable because this is state replication, not lockstep.**
The `deterministic` cargo feature stays OFF; `parallel` stays ON
(workspace `Cargo.toml:110`). Two determinism-breakers are present and tolerated: rayon
solver order (`parallel`) and f64 raycast/`sqrt` friction. They don't matter because **every
snapshot re-anchors** the client to the server's authoritative 4-component state for the
acked seq; replay only spans the ~3–6 unacked ticks *since that anchor*. Drift can only
accumulate over that window before the next snapshot snaps it back to truth → **bounded
residual**, which §4.6 render-smooths. `lightyear_avian3d-0.26.4` uses exactly this default
state-replication path (replicated `Position` arrives as `Confirmed<Position>`, triggers
rollback that inserts the correct `Position`; `plugin.rs:5-6`), keeping `deterministic` OFF.

### 4.6 Render-smoothing the residual

After snap+replay, the avian `Position`/`Rotation` are authoritative-correct but may *visibly
jump* from where the rover was rendered last frame. To hide it:

1. Before snapping, capture the **current rendered pose** `P_render`.
2. After replay, the new authoritative-predicted pose is `P_post`.
3. Store a **visual offset** `Δ = P_render − P_post` (position + rotation).
4. Each render frame, decay `Δ → 0` over a few frames (the existing
   `CORRECTION_RATE`-style exponential / ramp at `commands.rs:472-490`, repurposed), and add
   the *current* `Δ` to the **rendered `Transform` only** — never to avian `Position`.

This is a pure cosmetic offset: physics stays authoritative, the camera sees a smooth slide
into the corrected pose. It replaces the *full prediction error* the old corrector smoothed
with the much-smaller *post-replay residual* (which is zero whenever determinism happened to
agree).

---

## 5. WHAT TO RIP OUT / KEEP / ADD

All in `crates/lunco-sandbox-edit/src/commands.rs` unless noted.

### KEEP (predict-own substrate the new design also needs)
- `OwnedLocally` marker — `crates/lunco-core/src/session.rs:226-227`. The single classifier.
- `maintain_owned_locally` — `commands.rs:323-358`. Still flips `OwnedLocally` +
  `RigidBody::Dynamic` from `SessionRegistry` (`Dynamic` insert `:350`).
- Owned-rover-runs-local-physics — the dropped `!Client` mobility gate
  (`mobility/lib.rs:43-58`) and `Kinematic` skip-guards (`:144,220,295-299`). Replay
  re-steps avian on this exact body; it must stay `Dynamic` with live wheel forces.
- `force_kinematic_proxies` (`commands.rs:149-188`, `Without<OwnedLocally>` `:164`) and the
  `interpolate_proxies` owned-skip (`commands.rs:271-272`). Both still correct.
- `ingest_snapshots` + remote interpolation (`commands.rs:216-310`) — unchanged for *other*
  players' rovers (they need the extra velocity/seq fields plumbed through but don't read
  them).

### REPLACE
- `correct_owned_prediction` (`commands.rs:402-500`) and its constants (`:366-387`) → new
  `reconcile_owned_rover` system (same slot: `FixedPostUpdate` after
  `PhysicsSystems::Writeback`, `commands.rs:720-723`). Snapshot-triggered snap+replay (§4)
  instead of per-tick continuous blend.
- The owned rover's dependence on the pose-only `InterpBuffers` (`commands.rs:421-453`) →
  reconcile reads a new velocity+seq-bearing inbound queue for the owned gid directly.

### ADD (none exist today — grep-confirmed)
- `seq`+`tick` on `DriveRover`/`BrakeRover` (`mobility/lib.rs:430-442`) — §3.4.
- Per-vessel **input seq counter** + **`InputFrame` unacked ring buffer** (client) — new
  component/resource; populated in `translate_intents_to_commands`
  (`controller/lib.rs:115-130`).
- **Predicted-state history ring** (client) — `{seq, tick, pos, rot, lv, av}` recorded after
  avian writeback each fixed tick; indexed for compare-at-seq.
- `lv`/`av`/`last_input_seq` on `SnapshotEntry`/`SnapshotSample`/`InterpSample` — §3.
- Velocity read in `gather_snapshot` (query `+ &LinearVelocity, &AngularVelocity`,
  `sync.rs:421`) and the connect baseline (`server.rs:121`).
- `AppliedInputSeq(HashMap<u64,u32>)` resource (host) written in `apply_sync_command`
  post-authorize (`sync.rs:291-300`), read in gather + baseline.
- `reconcile_owned_rover` system + render-smoothing offset.
- `ClientPredictedTick` (or stop clobbering `SimTick` for the owned path) — §6.5.

---

## 6. EDGE CASES & RISKS

### 6.1 Held-key continuous vs edge inputs
Covered in §4.4. The risk if ignored: with edge-only `InputFrame`s and a naive "one input per
buffer entry replayed once" loop, a held `W` (one press-edge frame, one release-edge frame)
would replay the throttle on exactly *one* tick instead of every tick it was held → the rover
under-shoots on replay and rubber-bands *worse* than today. **Mitigation:** carry-forward
cursor (§4.4) for MVP; the P2 per-tick-emission change removes the hazard entirely.

### 6.2 Ownership change mid-flight
`maintain_owned_locally` (`commands.rs:323-358`) flips `OwnedLocally` from `SessionRegistry`
each `Update`. On **gain**: start a *fresh* seq counter at 0 and an empty `InputFrame` ring +
empty history ring; until the first snapshot acks a seq for this gid, run pure prediction with
no reconcile (snap directly to first authoritative snapshot). On **loss**: drop the ring +
history, remove `OwnedLocally` (`:353`), and the body reverts to `Kinematic`-pinned + interpolated
on the next `force_kinematic_proxies`/`interpolate_proxies` pass. Risk: a stale ack for the
*previous* owner arriving after a fast hand-off — guard by clearing both rings on the
`OwnedLocally` transition and ignoring acks `< current_seq_floor`.

### 6.3 Cosim / FSW port state during replay
The reconcile re-drives `DigitalPort.raw_value` from `InputFrame`s each replayed tick (§4.2).
**Risk:** after replay finishes, the ports must be left holding the **newest** (live) setpoint
so the *next live* `FixedUpdate` continues correctly — i.e. replay must end with the cursor at
the latest `InputFrame`, re-applying the current latched value. The Modelica/balloon cosim path
is gated off on clients (`lunco-cosim/src/lib.rs:104-106`) so it neither replays nor corrupts —
but confirm no client-side cosim system writes the rover's ports outside the gate.

### 6.4 SimTick sync / apply-vs-gather phase offset
**Real risk flagged by the server map.** Commands apply in `Update`
(`apply_sync_command`/`on_drive_rover`), but the body only *moves* on the **next**
`FixedUpdate`; `gather_snapshot` runs in `Update` on a wall-clock 20 Hz accumulator
(`sync.rs:419,427-432`), reading `Transform` (post-PhysicsSet writeback). So a seq stamped
`last_applied_seq` at apply-time does **not** yet correspond to integrated state when a
snapshot is gathered in the same `Update` — it's off by the Update↔FixedUpdate phase + the
HZ-accumulator jitter. **Mitigation:** record `last_applied_seq` at apply-time but only let
`gather_snapshot` emit it as "applied as of this snapshot" **after** the `FixedUpdate` that
consumed those ports. Cleanest fix = move `gather_snapshot` (and the seq read + velocity read)
to run **in/after `FixedPostUpdate`** so `tick`, velocity, and ack-seq are sampled coherently
from the same post-step world. This is a scheduling change (P3/P4), not a sync change.

### 6.5 Client SimTick non-monotonicity
Today `SimTick` on the client is hard-set to `snap.tick` every snapshot (`sync.rs:375`) and
seeded once at handshake (`sync.rs:384-388`). That makes it **non-monotonic** → unusable as a
local replay clock. **Mitigation:** the ack/compare key is `seq` (already monotonic, §4.1).
For the history-ring index and replay loop, add a **never-clobbered `ClientPredictedTick`**
(advanced in `FixedUpdate` alongside `advance_sim_tick`) OR stop letting the Snapshot arm
overwrite `SimTick` for the owned path and store `server_tick` separately. `SimTick.wrapping_diff`
(`lunco-core/src/lib.rs:283-289`) is available for lead-distance math if we later want
client-ahead-by-N tracking. There is **no** continuous tick sync/offset estimation today —
none is required for seq-based reconcile.

### 6.6 Physical-vs-raycast (raycast-only MVP)
The rover is a raycast-suspension vehicle: one `Dynamic` chassis + child `WheelRaycast`s
(`mobility/lib.rs:93`), no wheel rigid bodies. This is the **easy** case — the restore-set is
just the 4 chassis components, and `RayHits` recompute from restored chassis pose. We do **not**
support physical (collider) wheels in replay for MVP. If physical wheels are ever added, their
bodies join the restore-set and the 1-body-island cost assumption breaks — out of scope.

### 6.7 Determinism drift bound
Residual per snapshot ≤ drift accumulated over N≈3–6 ticks of non-deterministic 1-body avian.
Empirically this is small (single body, no contacts), but **unbounded in pathological cases**
(e.g. a wheel grazing a terrain seam where raycast hit/miss flips). The render-smoother caps
*visible* jump rate; if residual exceeds a hard `SNAP_POS` threshold (reuse `CORRECTION_SNAP_POS=6.0`,
`commands.rs:387`) skip smoothing and hard-snap (a teleport is better than a long rubber slide).

### 6.8 Cost at high tick-debt
If a client stalls (GC pause, tab-backgrounded — see `SPIKE_PH0.md` throttle note) the unacked
buffer grows and N spikes. **Cap N** (e.g. `MAX_REPLAY_TICKS = 12`): if more inputs are unacked
than the cap, **discard the oldest, snap to authoritative, and replay only the newest cap
ticks** (accept a one-time correction rather than an unbounded replay that blocks the frame).
This mirrors lightyear's capped-rollback behavior (the lone 252-tick cap seen in Ph0 was a
late-join transient, `DECISIONS.md:22`).

---

## 7. PHASED PLAN

Each phase is independently testable and leaves the build green.

### P1 — Velocity on the sync layer + dead-reckon (stepping stone, no seq yet)
**Goal:** owned rover predicts forward using *real* authoritative velocity instead of a
finite-difference reconstruction — kills the worst of the rubber-band before any seq/replay
machinery exists.
- Add `lv`/`av` `#[serde(default)]` to `SnapshotEntry` (`sync.rs:50-55`), `SnapshotSample`
  (`session.rs:258-263`), `InterpSample` (`commands.rs:192-197`).
- `gather_snapshot` query `+ &LinearVelocity, &AngularVelocity` (`sync.rs:421`), populate
  (`sync.rs:447`); same for connect baseline (`server.rs:121,164-168`); copy through
  `drain_sync_inbox` (`sync.rs:368-373`).
- `correct_owned_prediction` (`commands.rs:438-453`): use real `lv`/`av` instead of the
  `(last.pos-oldest.pos)/dts` reconstruction. **Still a blend, no replay** — just a better
  target. Test: drive in a straight line under 100 ms simulated latency, confirm reduced
  overshoot on stop.
- Files: `sync.rs`, `session.rs`, `server.rs`, `commands.rs`.

### P2 — Input seq + per-tick emission + unacked buffer (client only)
**Goal:** every owned input has a dense monotonic seq and lands in a replayable buffer.
- Add `seq`+`tick` to `DriveRover`/`BrakeRover` (`mobility/lib.rs:430-442`).
- `translate_intents_to_commands` (`controller/lib.rs:115-130`): per-vessel seq counter,
  stamp seq+`SimTick`, push `InputFrame` ring. **Decision point (§8):** keep edge-emission +
  carry-forward, or move emission to `FixedUpdate` for 1-input-per-tick. Recommend the latter.
- New `InputFrame` ring (component/resource keyed by gid).
- Test: log seq stream while driving; confirm monotonic, gap-free, latched-hold correct.
- Files: `mobility/lib.rs`, `controller/lib.rs`, new client buffer module.

### P3 — Server ack: record + echo last-applied seq
**Goal:** snapshots carry the authoritative ack.
- Add `last_input_seq` `#[serde(default)]` to `SnapshotEntry`/`SnapshotSample`.
- `AppliedInputSeq(HashMap<u64,u32>)` resource; write in `apply_sync_command` post-authorize
  (`sync.rs:291-300`); read in `gather_snapshot` (`sync.rs:435-447`) + baseline (`server.rs`).
- Read `seq` from `SyncCommand.data` (already arrives via reflect, §3.4) — plumb into
  `SyncCommandEvent` if not already accessible at apply.
- **Address the apply/gather phase offset (§6.4):** move `gather_snapshot` to in/after
  `FixedPostUpdate`.
- Test: client logs `(owned gid, last_input_seq)` from snapshots; confirm it tracks the seq
  it sent, lagged by RTT.
- Files: `sync.rs`, `server.rs`, optionally `commands.rs` (SyncCommandEvent).

### P4 — Compare-at-seq + snap + replay (the core)
**Goal:** replace continuous correction with snap+replay.
- Add predicted-state history ring (record after writeback each fixed tick).
- New `reconcile_owned_rover` in `FixedPostUpdate` after `Writeback` (replaces
  `correct_owned_prediction` at `commands.rs:720-723`): on new `last_input_seq`, compare
  vs `history[last_input_seq]`; if error > eps → snap 4 components to authoritative, drop
  acked `InputFrame`s, re-step avian over the remainder (§4.3) with carry-forward (§4.4),
  cap at `MAX_REPLAY_TICKS` (§6.8).
- Add `ClientPredictedTick` / stop clobbering owned `SimTick` (§6.5).
- Delete `correct_owned_prediction` + constants.
- Test: drive under high latency + packet loss; confirm the rover tracks server with no
  steady-state offset and no continuous rubber-band; confirm ownership hand-off (§6.2).
- Files: `commands.rs` (primary), `session.rs`/`lib.rs` (tick).

### P5 — Render-smooth the residual
**Goal:** hide the post-replay visual jump.
- Capture pre-snap render pose, compute post-replay residual offset, decay to 0 over a few
  frames applied to rendered `Transform` only (§4.6), reuse `CORRECTION_RATE`/ramp; hard-snap
  past `CORRECTION_SNAP_POS`.
- Test: visual — drive in circles under jitter, confirm no perceptible snapping at 20 Hz
  snapshot cadence.
- Files: `commands.rs`.

---

## 8. OPEN QUESTIONS FOR THE HUMAN

1. **Seq derivation / per-tick emission (load-bearing).** Move emission to `FixedUpdate` with
   one `InputFrame` per fixed tick (clean Gambetta-style replay, but a behavioral change to
   `translate_intents_to_commands`, `controller/lib.rs:57`), OR keep edge-emission + the
   carry-forward cursor (§4.4, smaller diff, slightly more replay logic)? Recommend per-tick
   emission; needs your sign-off because it changes input semantics.
2. **Replay step budget.** `MAX_REPLAY_TICKS` cap value (proposed 12) and the
   over-budget policy (snap + replay-newest-N). Acceptable, or prefer a different ceiling?
3. **Keep dead-reckon as a fallback?** After P4, is the P1 velocity-blend worth keeping as a
   *no-replay fallback* for clients that can't afford replay (heavily throttled wasm tabs,
   `SPIKE_PH0.md`), or rip it out entirely once replay lands?
4. **`gather_snapshot` reschedule (§6.4).** Moving it from `Update`→`FixedPostUpdate` fixes the
   apply/gather phase offset but changes snapshot cadence from wall-clock-20 Hz to tied-to-fixed-
   step. Acceptable, or keep wall-clock gather and accept a 1-tick seq fuzz?
5. **Adopt `lightyear_avian3d` vs hand-roll?** This doc hand-rolls (re-run fixed loop). The crate
   does it for us but mandates client-only disabling of avian's interpolation/transform plugins
   (`sandbox.rs:220`) and a divergent client plugin config. Hand-roll for MVP, evaluate the crate
   for v2 — confirm?
6. **`PROTOCOL_ID` bump?** Additive `#[serde(default)]` fields are sync-compatible without it,
   but a bump makes mismatched host/client builds *refuse to connect* instead of silently
   dropping frames (`deserialize_env` `.ok()`). Bump on this schema change, or rely on
   build-together discipline?
7. **D2 amendment.** This reverses D2 for the owned rover. Confirm I should edit
   `DECISIONS.md:35-43` to the §1.1 New-D2 wording and cross-link this doc.
