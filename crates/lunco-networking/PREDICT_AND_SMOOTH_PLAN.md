# Predict-and-Smooth — detailed implementation plan (REVISED 2026-06-11)

Status: **SHIPPED & VERIFIED 2026-06-11 — client feel confirmed good by user.**
Steps 1, 4, and a re-architected 2 landed; step 3 (input hardening) remains future
work. Cross-checked against networked-physics best practice: Fiedler *Networked
Physics* (error smoothing, quantization, Hermite), Rocket League GDC 2018
(predict-all + held-input extrapolation), Overwatch GDC 2017 (redundant unreliable
inputs + de-jitter buffer).

---

## ★ OUTCOME — what actually fixed it (read this first)

The visible client jitter had **two independent causes**, found in order. The
second was the real one and took an architecture change, not a constant.

### The root cause: never write `Transform` from game code when interpolation is on

`crates/lunco-client/src/bin/sandbox.rs` enables
`PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all())`.
That makes avian's `bevy_transform_interpolation` the **sole owner of every body's
`Transform`** at render rate: it eases `Transform` between the 64 Hz physics poses,
and `reset_easing_states_on_transform_change` **treats ANY external `Transform`
write as a teleport and drops that body's easing for the frame**.

Every netcode path that wrote `Transform` therefore silently *disabled* render
interpolation for the corrected body:
- the reconcilers wrote `tf.translation/rotation` directly;
- the first Step-2 attempt (a decaying `RenderErrorOffset` applied to `Transform`
  in `PostUpdate` + stripped in `FixedFirst`) wrote `Transform` **every frame** an
  offset was live (~1 s after each correction).

Result: the owned rover rendered at raw 64 Hz steps against the render clock →
persistent jitter **while merely holding the throttle, contact or not**. The host
never reconciles, so it never entered this state — which is exactly why *only the
client* jittered, *even on one machine with zero latency*.

### The fix: physics-space error reduction (Rocket League model)

Game code now **never touches `Transform`**. Corrections are reduced in physics
space and rendered by the same interpolation as everything else:
1. Reconcilers (`reconcile_owned_prediction`, `reconcile_predicted_dynamic`) no
   longer pop the pose. A `Correct`-class divergence is **parked** as a
   `PendingCorrection { pos, rot }` residual component.
2. `drain_pending_corrections` (FixedUpdate, before the solve) bleeds that residual
   into avian `Position`/`Rotation` at a hard cap of **≤2.5 cm / ≤0.9° per tick**
   (exp time-const `CORRECTION_TAU = 0.12 s`). The nudge flows through solve →
   writeback → interpolation, so it renders as a smooth sub-perceptible slide and
   never disturbs a contact.
3. Only a **gross desync** (`> snap_pos = 6 m`) still seats the pose directly —
   there an easing reset (a real teleport) is correct.

### What else mattered (necessary, but not the root cause)

- **Velocity feed-forward, not deadbeat** (Step 1). `v = (target − pos)/h`
  commanded ~50 m/s spikes (cubic-Hermite overshoot × 1-tick deadbeat) → tunnelled
  contacts. Replaced with `v = curve_velocity + soft_correction/TAU`, capped at
  `PROXY_MAX_SPEED = 50` so a diverging cosim body can't fling its proxy.
- **Predict remote rovers** (Step 4). A kinematic proxy *pushes* but never
  *yields*; driving into it bounced your rover off a wall that authoritatively
  moved. Marking remote `RoverVessel` proxies `PredictedDynamic` (reusing Phase B)
  makes them yield locally → crisp mutual push. **Guard:** never vehicle-predict a
  rover *this* session owns (the Phase-A drive-grace lapse otherwise flapped it
  OwnedLocally↔PredictedDynamic and the state-reconciler yanked it — a tap-steer
  sawtooth).
- **Reconcile dead-zone widened** `eps_pos 0.25→0.40 m`, `eps_rot 1.7°→5.7°`.
  Measured free-driving divergence is ~13–27 cm / 2–6° per ack (host-vs-client
  input-timing skew, inherent without rollback); the old thresholds corrected on
  nearly every ack. Now ordinary skew reads InSync.
- **Velocity half-blend, not full re-seat** in `reconcile_owned_prediction` — a
  full seat to a snapshot-stale velocity hiccuped the speed each correction.
- **Drive-grace `PREDICT_GRACE_TICKS` 30→240** so a rover released at speed isn't
  handed to the proxy path mid-coast (a ~0.3 m render warp on every key release).

### Red herrings ruled out by measurement (not theory)

A throwaway census (`[DRIVE]`/`[RECON-*]`/`[JIT]` `eprintln`s) was decisive each
time — guessing was wrong every time:
- "teleporting every tick" — **no**, census showed 100% velocity branch.
- "reconcile correcting constantly" — after the dead-zone widen, **zero**
  corrections, yet the user still saw jitter → proved it was *render-layer*, which
  pointed the audit at the interpolation plugin.
- The `[JIT]` detector (per-frame backward-step on the *post-propagation*
  `GlobalTransform`) is what localised the stutter to the render layer for certain.

**Diagnostics are still in the tree** (search `DIAG`) — strip them before the final
commit; they cost a couple of branches per tick and some log spam.

### Still open (future, not blocking)

Step 3 (tick-stamped inputs on an unreliable channel + host de-jitter buffer) is the
*root* cure for the corrections themselves: it removes the host-vs-client input
phase skew, so divergence — hence correction size — shrinks. With real latency the
corrections grow; physics-space drain keeps them smooth, but Step 3 keeps them
*small*. See §4.

---

## 0. Design rationale (the four review outcomes this plan was built on)

> These were the corrections to the original fix sketch that shaped the plan.
> Outcome **1, 3, 4 held**; **outcome 2 was itself later superseded** — see ★OUTCOME.

1. **Proxies are *velocity-driven*, not teleport+velocity-hint.** ✅ held. A
   teleported kinematic body creates penetration-from-nowhere regardless of the
   velocity we claim. Each fixed step set `v = (target − current)/h` and let avian
   integrate; teleports become the *exception* (first sample / big gap / snap).
   (Shipped with a refinement: feed-forward `v`, not deadbeat — see ★OUTCOME.)
2. ~~Corrections seat the body immediately; the visual discontinuity goes into a
   decaying **render** offset.~~ **SUPERSEDED.** Seating the body was right;
   smoothing on a *render `Transform` offset* was wrong — it fights
   `bevy_transform_interpolation`. The shipped model reduces the error in **physics
   space** (`PendingCorrection` drain), never touching `Transform`. See ★OUTCOME.
3. **Predict-all-vehicles, not contact-islands.** ✅ held (shipped by reusing
   `PredictedDynamic`). Islands concentrate their seams at the fragile
   approach/contact instant to save CPU we don't need at 2–4 rovers; they remain the
   documented fallback if the CPU gate (§5.5) ever fails.
4. **Input hardening promoted from "optional" (Gap G) into the plan.** ✅ still
   agreed — but **not yet built** (§4). Reliable-ordered inputs head-of-line-block
   under loss → ack stalls → larger corrections; the cure is an unreliable channel
   with redundant last-~10 frames + a host de-jitter buffer + applied-input in the
   snapshot (held-input extrapolation).

---

## 1. Build order (each step independently verifiable)

| Step | What | Status (2026-06-11) |
|---|---|---|
| 0 ✅ | Pop stash, strip diagnostic | done |
| 1 ✅ | Velocity-driven kinematic proxies (lin+ang) + Hermite + **feed-forward** | shipped; deadbeat→feed-forward fix applied (see OUTCOME) |
| 4 ✅ | Predict-all-vehicles (remote rovers `PredictedDynamic`, reuse Phase B) | shipped; own-rover guard added |
| 2 ✅ | Correction smoothing — **re-architected to physics-space** (`PendingCorrection` drain), NOT a render-Transform offset | shipped; this was the root-cause fix |
| 3 ⬜ | Input hardening (redundant unreliable + jitter buffer + input-in-snapshot) | **not started** — future; shrinks corrections at the source under real latency |

**Actual landing order was 1 → 4 → 2** (not 1 → 2), discovered empirically: step 4
made the *physics* correct (mutual push), which exposed that the remaining jitter was
purely a *render-layer* artifact (the interpolation-reset bug), fixed by the
re-architected step 2. The original "don't reorder 2 and 4" caution was about
smoothing hiding step-4 transitions — still true, and they did land together-ish.

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

> ⚠️ **SUPERSEDED — this whole section is the design that FAILED.** It was built,
> shipped, and caused the hold-the-key jitter (it writes the render `Transform`,
> which resets `bevy_transform_interpolation`'s easing). Kept for the record only.
> **What actually shipped = physics-space `PendingCorrection` drain — see ★OUTCOME
> at the top.** Do not implement what's below.

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

### Run protocol + operational gotchas (save the next person hours)

Two terminals from the **repo root** (CWD matters — see asset gotcha):
```bash
cargo run -p lunco-client --bin sandbox --features networking -j2 -- --host --api 4001 --no-throttle
cargo run -p lunco-client --bin sandbox --features networking -j2 -- --connect 127.0.0.1:5888 --api 4002 --no-throttle
```
- Networking is a **cargo feature** on `lunco-client`; flags `--host [port]` /
  `--connect addr[:port]` (default transport port 5888/udp). Native client uses an
  empty cert digest + `dangerous-configuration` → no mkcert/CA hassle on localhost.
- `--no-throttle` keeps rendering when unfocused (else background tab → ~1 FPS).
- API: `POST http://127.0.0.1:<port>/api/commands`, body `{"command":"Ping"}`. **curl
  is hook-blocked here** → probe via a JS `fetch` (`ctx_execute`). Stop a run with
  `{"type":"Exit"}`. `CaptureScreenshot{}` returns **raw PNG bytes**.
- **Asset-CWD trap (a full empty-scene red herring):** `sandbox` roots assets at
  `current_dir()/assets`. Launch from the wrong CWD (e.g. a shell that `cd`'d into
  `target/...`) → `sandbox_scene.usda` not found → **empty black viewport** → the 2 s
  fallback avatar spawns a *second* `FloatingOrigin` → `"multiple floating origins"`
  flood. Always launch from repo root.
- **Build/disk/RAM:** always `-j2` (this machine struggles; the networking build is
  the repo's heaviest). `/home` runs near-full — the build hit `No space left` twice;
  clear `target/debug/incremental` (26 GB once) to recover, never the sibling
  `../*/target`. Two GUI windows + a linker OOM'd the machine; don't build while both
  run. **If the client renders <1 FPS the "jitter" may be frame starvation — check
  client `Ping` ≈ frame time FIRST.**
- **avian 0.6.1 facts:** Kinematic↔Kinematic does NOT collide (two proxy rovers pass
  through — the deferred caveat); Kinematic→Dynamic DOES push (enables proxy↔owned
  interaction). `Collisions`/`entities_colliding_with` returns *collider* entities
  (may differ from the rigid-body entity for child colliders). `SubstepCount = 12` on
  rovers — any local re-stepping inherits it; don't hand-roll an integrator.
- **Diagnostics (turn on when jitter returns):** build with the **`net-diag` cargo
  feature** (`--features networking,net-diag`) — compiled out of normal builds.
  `lunco-networking/src/diagnostics.rs` then reports (a) **render jitter**
  (backward-steps on the *rendered* `GlobalTransform` — the keystone that found the
  interpolation bug), (b) **velocity spikes** (the 50/200 m/s signatures), (c)
  **correction pressure** (`PendingCorrection` residuals). Active once compiled;
  mute a run with `LUNCO_NET_DIAG=0`. Method lesson: when the sim is right but it
  *looks* wrong, measure the render layer, not the sim.

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
