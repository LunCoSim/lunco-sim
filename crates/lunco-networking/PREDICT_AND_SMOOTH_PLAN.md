# Predict-and-Smooth — detailed implementation plan (REVISED 2026-06-11)

Status: **PLAN, agreed with user 2026-06-11.** Supersedes the fix sketch in
[`CONTACT_JITTER_HANDOVER.md`](./CONTACT_JITTER_HANDOVER.md) §5 (two corrections to it,
see §0) and **supersedes `PREDICTION_MEMBERSHIP.md` §5 Phase C** (contact-island →
predict-all-vehicles, see §6). Cross-checked against networked-physics best practice:
Fiedler *Networked Physics* (error smoothing, quantization, Hermite), Rocket League
GDC 2018 (predict-all + held-input extrapolation), Overwatch GDC 2017 (redundant
unreliable inputs + de-jitter buffer).

---

## 0. What changed vs the handover (critical review outcomes)

1. **Part 1 goes further: proxies are *velocity-driven*, not teleport+velocity-hint.**
   A teleported kinematic body can still create penetration-from-nowhere regardless of
   the velocity we claim. The standard moving-platform technique: each fixed step set
   `v = (target_pose − current_pose)/h` and let avian integrate the kinematic body.
   Position teleports become the *exception* (first sample / big gap / snap), not the
   mechanism. Bonus: friction works (things resting on a proxy get dragged correctly).
2. **Part 2 as written in the handover was wrong.** "Never seat the physics body" lets
   prediction error compound forever (offset decays back to a wrong body → drift until
   the 6 m Snap). The correct model (Fiedler's error smoothing, what
   `PREDICTION_RECONCILIATION.md` step 5 meant): **corrections DO seat the body,
   immediately**; the *visual discontinuity* of each correction is pushed into a
   decaying render offset, so the eye sees a ~150 ms ease instead of a pop.
3. **Phase C is no longer contact-islands; it is predict-all-vehicles** (Rocket League
   model). Islands concentrate their seams (promotion timing, time-base jump,
   hysteresis) exactly at the moment of approach/contact — the most fragile instant —
   to save CPU we probably don't need to save at 2–4 rovers. Islands remain the
   documented fallback if the CPU gate fails (§6.5).
4. **Input hardening is promoted from "optional" (Gap G) into this plan.** Inputs on a
   reliable-ordered channel head-of-line-block under loss → ack stalls → the unacked
   window grows → the next correction is large. Best practice: unreliable channel,
   each packet redundantly carries the last ~10 input frames, host keeps a small
   de-jitter buffer. Plus: replicate each vessel's *current applied input* in the
   snapshot (held-input extrapolation — the cheap middle path between "ballistic
   extrapolation" and "full input fan-out").

**Current tree state (2026-06-11):** stash@{0} popped (Phase B `PredictedDynamic` +
§6 `NotPredictable` restored — they are load-bearing for this plan), diagnostic
`return;` stripped from `reconcile_owned_prediction`. Uncommitted, on `main`,
baseline `248d0de8`.

---

## 1. Build order (each step independently verifiable)

| Step | What | Verify gate |
|---|---|---|
| 0 ✅ | Pop stash, strip diagnostic | tree compiles (Phase B tests existed green) |
| 1 | Velocity-driven kinematic proxies (lin+ang) + Hermite | kick→clean bounce; settled rover doesn't blink; balloon doesn't drift |
| 2 | Error-offset render smoothing + penetration guard | drive into rover: no buzz; open ground: unchanged feel |
| 3 | Input hardening (redundant unreliable + jitter buffer + input-in-snapshot) | drive under simulated loss: no correction bursts |
| 4 | Predict-all-vehicles (held-input + state reconcile + smoothing) | mutual push works; CPU gate (§6.5) passes |

Steps 1–2 fix the live jitter bug. Step 3 hardens. Step 4 delivers mutual push.
**Do not reorder 2 and 4:** step 4's transitions and per-snapshot corrections rely on
step 2's smoothing to be invisible.

---

## 2. Step 1 — velocity-driven kinematic proxies

### Design
- Proxies stay `RigidBody::Kinematic` (pinning in `force_kinematic_proxies` stays).
- **Stop zeroing velocity** (`commands.rs:181-186` deleted).
- New system `drive_kinematic_proxies` in **`FixedUpdate`** (before avian's step in
  `FixedPostUpdate`): for each buffered proxy (not `OwnedLocally` / not
  `PredictedDynamic` / has `RigidBody`):
  - advance the playback clock in the **fixed timebase** (move the clock out of
    `interpolate_proxies`' `Local` into a `Resource` (`ProxyPlaybackClock`) so the
    Update-time render path and the fixed-time driver share one clock),
  - evaluate the interpolation curve at `t_end = render_t + h` (end of this step),
  - `LinearVelocity = (target_pos − Position)/h` (f64),
  - `AngularVelocity` from the quaternion delta: `q_err = target_rot · rot⁻¹` →
    axis·angle/h (shortest arc; guard sign by `q_err.w < 0 → negate`),
  - **no Position/Rotation/Transform writes** in the steady state.
- **Teleport exceptions** (write `Position`/`Rotation` + zero velocity, as today):
  first-ever sample for a body; positional error vs curve > `PROXY_SNAP_DIST`
  (~2 m); buffer gap/discontinuity (> `CLOCK_SNAP`). After a teleport, resume
  velocity-driving next step.
- **Hermite interpolation** (Fiedler): the curve evaluator upgrades from
  lerp(a,b) to cubic Hermite through `(a.pos, a.lv) → (b.pos, b.lv)` — velocities
  are already on the wire per sample. Rotation stays slerp. Starved-buffer
  extrapolation keeps the existing velocity-glide + `INTERP_MAX_EXTRAP_DIST` cap.
- `interpolate_proxies` (Update) **shrinks to render-only concerns**: bodies
  *without* a `RigidBody` keep the old per-frame Transform writes; bodies *with* one
  are now rendered from avian writeback (60 Hz fixed pose) — same as the owned rover
  today, so visual cadence is consistent. `ReplicatedChassisMotion` stamping moves
  to / stays with whichever system still touches the body (driver is fine).

### avian facts this relies on — **PROBED & CONFIRMED 2026-06-11** ✓
Headless real-solver probe (`commands.rs` test mod `avian_kinematic_probe`, avian
0.6.1, `SubstepCount(12)`, deterministic `TimeUpdateStrategy::ManualDuration`
stepping). Both load-bearing facts hold:
- **Kinematic integrates `Position += LinearVelocity·h` per tick** ✓ — measured
  steady-state per-tick delta == `v·h` to within ~8 nm/tick (substeps split `h` into
  integer-nanosecond slices: `15625000/12` truncates → loses ~4 ns/tick; negligible).
  So `drive_kinematic_proxies` can steer purely by setting `v = (target−pos)/h`.
- **Kinematic velocity enters contact resolution** ✓ — a kinematic body moving into
  a `Dynamic` body (started from a *clear gap*, ruling out penetration-recovery)
  drives it +x in both Position and LinearVelocity. The ram→prop push is real.

Two semantics learned (carry into impl):
- **One-tick spawn/prepare lag**: the first `update()` after spawn syncs
  Transform→Position *without* integrating (absolute Position == `v·h·(N−1)`).
  Continuous driving is unaffected — just no tick-0 motion.
- Headless solver harness needs `AssetPlugin` + `init_asset::<Mesh>()` +
  `DiagnosticsPlugin` + explicit `app.finish()`/`cleanup()` (avian inserts its
  diagnostics resources in `finish`, which bare `update()` skips). Reusable for
  Step 1.8 unit tests if they want the real solver. Probe is delete-on-Step-1-land.

### Why the two old regressions don't return
- *Balloon drift*: old bug = stale velocity open-loop with nothing re-targeting it.
  Now velocity is recomputed **every step toward the curve** (closed loop) — error
  cannot accumulate beyond one step.
- *Settled-rover blink*: old bug = hard 20 Hz snaps + leftover velocity. Now a
  resting body's curve is flat → `v≈0` → it sits still; no snaps at all in steady
  state.

### Touch
`crates/lunco-sandbox-edit/src/commands.rs` (force_kinematic_proxies,
interpolate_proxies, new drive_kinematic_proxies + clock resource, schedule wiring in
the plugin), nothing else.

---

## 3. Step 2 — error-offset render smoothing ("smooth the pop, not the truth")

### Design
- New always-on substrate component `RenderErrorOffset { pos: Vec3, rot: Quat }`
  (lunco-core, reflect; identity default) + `decay(dt)`: exponential ease to identity
  with time-constant ~50 ms (≈ gone in 150 ms); **snap to identity** if magnitude
  exceeds `MAX_VISUAL_OFFSET` (~3 m / ~30°) — never smooth a teleport.
- Reconcilers (`reconcile_owned_prediction`, `reconcile_predicted_dynamic`) change in
  ONE way: after computing `new_pos/new_rot` and seating the body (exactly as today —
  Position, Rotation, velocity), they **accumulate the visual delta into the offset**:
  `offset.pos += old_rendered_pos − new_pos; offset.rot = old_rot · new_rot⁻¹ · offset.rot`.
- A render-time system applies `physics pose ⊕ offset` to what the user sees and
  decays the offset per frame.

### The avian feedback gotcha — two wiring options, decided by a 30-min probe
avian re-reads `Transform → Position` at step start (user-edit sync), so a smoothed
pose written onto the **body's** `Transform` can feed back into physics.

- **Option A (preferred if safe): offset on the body's `Transform`, stripped before
  physics.** `apply_render_offset` runs `PostUpdate` after avian writeback
  (`Transform = Position ⊕ offset`); a paired `strip_render_offset` runs first thing
  in `FixedUpdate` (`Transform = Position`) so physics never sees the offset.
  Pros: zero hierarchy assumptions. Cons: two-system discipline; probe MUST confirm
  avian's Transform→Position sync doesn't run between our strip and the step.
  (Also check: avian's own `TranslationInterpolation`/transform-interpolation feature
  — if enabled it already rewrites Transform per render frame without feedback; if
  so, mirroring its mechanism/ordering is the answer, or disabling
  `transform_to_position` sync for marked bodies via avian's sync config.)
- **Option B (fallback): offset on a render-only child.** Insert the offset into the
  visual subtree's local transform (USD prim children of the chassis). Pros: solver
  never sees it by construction. Cons: must locate/own the visual root per body;
  USD-loaded hierarchies vary; colliders that are children must NOT be offset.

### No-snap-into-penetration guard
When a `Snap` (or large `Correct`) would seat the body overlapping another **local
dynamic/kinematic** body (cheap check: `Collisions`/spatial query at the target
pose against nearby rigid bodies), degrade `Snap → Correct` (blend) for that ack and
let the solver depenetrate softly over the next steps. Never teleport into overlap.

### Touch
`lunco-core/src/session.rs` (component) or new `lunco-core/src/smoothing.rs` (+unit
test for decay/snap math), `lunco-sandbox-edit/src/commands.rs` (reconcilers + the
two systems + schedule).

---

## 4. Step 3 — input hardening (Gap G, promoted)

1. **Unreliable redundant input channel.** New lightyear channel `InputChannel`
   (UnorderedUnreliable). Client sends, each fixed tick, a packet with the **last
   N=10 input frames** (seq, tick, forward/steer/brake) for each owned vessel —
   redundancy replaces retransmission; one lost packet costs nothing unless 10 in a
   row die (then the reliable path's worst case was no better).
2. **Host de-jitter buffer.** Per (session, gid): ingest packets, dedupe by seq,
   apply in seq order at the fixed tick; tolerate gaps (hold last input ≤ a few
   ticks, matching the client's held-input assumption); drop stale (< already
   applied seq).
3. **Input-in-snapshot.** Extend `SnapshotSample` with the vessel's currently-applied
   input (`in_fwd, in_steer, in_brake` — f32 or quantized i8). Host stamps it in
   `gather_snapshot` from the same source `AppliedInputSeq`/mobility reads. Consumers:
   starved-proxy extrapolation (steer along the arc) and step 4's held-input
   prediction. Wire cost ≈ 3 bytes/body.

Where the input currently flows must be mapped during implementation (today
`DriveRover` rides the reliable CommandBus; the capture seam is
`lunco-controller/src/lib.rs` `emit_vessel_input` → `lunco-networking/src/wire.rs`).
Keep the *command* path reliable for everything else; only the per-tick drive input
moves to the new channel.

### Touch
`lunco-networking/src/{protocol,wire}.rs`, `lunco-controller/src/lib.rs`,
`lunco-core/src/session.rs` (`SnapshotSample`), host apply path.

---

## 5. Step 4 — predict-all-vehicles (mutual push)

### Design (Rocket League model, adapted to no-rollback D2)
- Client: every replicated **vehicle** (`RoverVessel` + `NetReplicate`) that is not
  `OwnedLocally` and not `NotPredictable` gets marker `PredictedVehicle` (new,
  lunco-core) — **always**, no proximity/contact trigger, no promotion seams.
- New system `maintain_predicted_vehicles` (client-only), mirroring
  `maintain_owned_locally`'s hard-won lesson (`commands.rs:499-511`): on entry
  insert `RigidBody::Dynamic` **and seed pose+velocity** from the latest snapshot
  **extrapolated to local now** (samples carry lv/av; after step 3, also held
  input). On exit (possession transfer to me / NotPredictable appears) remove marker
  — `force_kinematic_proxies` re-pins.
- **Held-input drive:** feed the snapshot's replicated input (step 3.3) into the same
  per-vessel input component the local controller writes, every tick, for
  `PredictedVehicle` bodies. Their mobility/drivetrain then simulates normally —
  same FixedUpdate physics, inherits `SubstepCount(12)`.
- **Reconcile:** the Phase B state path (`reconcile_predicted_dynamic`) widens its
  query to `Or<(PredictedDynamic, PredictedVehicle)>` — once per fresh snapshot,
  authority-now vs body-now, blend/snap + velocity seat, **discontinuity into the
  step-2 offset**.
- Exclusions unchanged and load-bearing: `NotPredictable` (cosim/opaque — never
  predicted); non-vehicle scene props stay interpolated proxies (step 1 path);
  `PredictedDynamic` runtime props keep working as today.
- Mutual push falls out: both rovers are Dynamic in the local solver — you push it,
  it pushes back, server stays authority, divergence is corrected smoothly.

### What changes for spectating
A watched remote rover is now a 60 Hz local sim with 20 Hz smoothed corrections
instead of a 0.18 s-delayed interpolation. Slightly noisier in theory; with
held-input it should be near-identical in practice. Judge by eye at the verify gate.

### 5.5 CPU gate + fallback
Before accepting: measure host+client frame time with 3–4 rovers all simulated
client-side (the `scripts/perf/` subsystem exists). Budget: client FixedUpdate must
hold 60 Hz on this 14 GB machine with two windows. **If it fails:** fall back to
island promotion (the §5C design in `PREDICTION_MEMBERSHIP.md`) — but with the two
fixes from this review: **proximity**-triggered promotion (before first contact, not
contact-BFS-triggered) and **extrapolate-to-now seeding**, with both transitions'
pops absorbed by the step-2 offset.

### Touch
`lunco-core/src/session.rs` (marker), `lunco-sandbox-edit/src/commands.rs`
(maintain system, query widening), `lunco-controller` or mobility input seam
(held-input feed), perf scripts for the gate.

---

## 6. Verification matrix

| # | Scenario | Expected after |
|---|---|---|
| V1 | Ram a parked remote rover (step 1, reconcile ON) | one clean bounce, no buzz, no kick |
| V2 | Settled remote rover, nobody driving | pixel-still (no blink/glide) |
| V3 | Cosim balloon | follows snapshots, no drift/flyaway, never predicted |
| V4 | Open-ground driving, owned rover | unchanged crisp feel; InSync dominant |
| V5 | Drive while corrections forced (step 2) | corrections invisible (offset decays), body converges |
| V6 | Packet loss ~5% (step 3) | no correction bursts; input never head-of-line stalls |
| V7 | Two clients push each other's rovers (step 4) | mutual push, both feel contact, converge to host truth |
| V8 | CPU gate: 3–4 predicted rovers | client holds 60 Hz fixed step |

Run protocol: host+client from repo root per `CONTACT_JITTER_HANDOVER.md` §6
(`--host --api 4001` / `--connect 127.0.0.1:5888 --api 4002`, `--no-throttle`,
CWD = repo root, probe via ctx_execute fetch, ports 4001+).

## 7. Risks / empirical unknowns (checked at the marked probes)

1. avian kinematic velocity-integration semantics under substeps (step 1 probe).
2. avian Transform→Position sync feedback for the render offset (step 2 probe;
   decides Option A vs B).
3. Cosim/USD writers fighting proxy Transforms (watch during V3).
4. Held-input feed seam: mobility must accept injected input identically to local
   controller input (step 4; reuse the `DriveRover` observer path if possible).
5. CPU of N articulated rovers (V8 gate, fallback defined).

## 8. Deliberately NOT in scope

- Rollback/resimulation (D2 stands; smoothing covers our accuracy class).
- Full input fan-out of remote players' raw input streams (held-input via snapshot
  is the chosen middle path; revisit only if V7 feel is insufficient).
- State quantization (Fiedler) — noted for later, not needed at this fidelity.
- Per-wheel/articulation replication; interest management; late-join (Gap I).
