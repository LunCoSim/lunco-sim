# Prediction membership — computability, not ownership (A + B + C)

Status: **A SHIPPED · B SHIPPED · C SUPERSEDED · §6 guard SHIPPED** (2026-06-11).
Extends [`PREDICTION_RECONCILIATION.md`](./PREDICTION_RECONCILIATION.md)
(D2, the *how* of correcting a predicted body) with the missing *which bodies do we
predict at all* decision. Closes [`DESIGN_GAPS.md`](./DESIGN_GAPS.md) **Gap C**.

> **UPDATE 2026-06-11 (user decision: full mutual push):** Phase C (§5, contact-island
> promotion) is **superseded by predict-all-vehicles** — every non-opaque remote
> `RoverVessel` stays locally Dynamic at all times, driven by **held-input** replicated
> in snapshots, state-reconciled via the Phase B path, with corrections smoothed by a
> decaying render offset. Islands concentrated their seams (promotion timing,
> time-base jump, hysteresis) exactly at the approach/contact instant; at 2–4 rovers
> the CPU they save doesn't justify that. Islands remain the documented **CPU
> fallback** — if ever built, trigger by *proximity* (not first contact) and seed by
> *extrapolate-to-now*. Full plan + best-practice rationale:
> [`PREDICT_AND_SMOOTH_PLAN.md`](./PREDICT_AND_SMOOTH_PLAN.md). The computability
> principle (§0) and the §6 opaque guard are unchanged and load-bearing.

> **Decision 2026-06-10 (user):** Phase A (correctness) and Phase B (crisp single
> bumped prop) are enough for MVP. **Phase C (contact-island auto-membership) is
> PARKED** as a documented follow-up — it is the only phase that enters the
> explicitly-deferred two-predicted-bodies-in-contact territory
> (`DECISIONS.md:65-66`), costs a per-tick contact-graph BFS + promote/demote
> churn, and needs a 2-client stress test to validate, for a convenience (transitive
> auto-promotion) rather than a correctness gain. The §6 opaque guard — the one
> genuinely-useful piece underneath C — **was implemented** independently as a
> cheap, always-safe backstop.

---

## 0. The one-line principle

> **A client predicts a body iff that body's motion is dominated by forces the
> client can reproduce locally.** Everything else is server-interpolated.

This is **computability**, not **ownership**. The current code decides prediction
membership purely from `SessionRegistry::owns()` (`maintain_owned_locally`,
`commands.rs:484`). That is the bug, not a tuning issue. Gap C already says so:

> "First-class input: *Is this entity's motion locally computable?* must inform the
> predict/interpolate decision, **not just ownership**." — `DESIGN_GAPS.md` Gap C

The three fixes are three rungs of the same ladder — each widens "locally
computable" by one step:

| Fix | "Locally computable" means | Membership set |
|-----|----------------------------|----------------|
| **A** | I am *actively supplying the dominant input* this body responds to | `owned ∧ actively-driving` |
| **B** | (correction method) a predicted body with *no input* — driven by local contact — reconciled by **state**, not input-replay | adds the *how* for non-input members |
| **C** | I predict a body because it shares a **contact island** with something I already predict | `island(my predicted set) \ opaque` |

---

## 1. The observed bug, precisely

Reproduced live 2026-06-09 (host `Skid_Raycast_1` driven by session 0; client
`Skid_Raycast_2` possessed by the client session, pushed by the host's rover):

- Client possesses green → `maintain_owned_locally` sets `OwnedLocally` from
  `reg.owns()` alone → body is `Dynamic`, **excluded from**
  `force_kinematic_proxies` *and* `interpolate_proxies`.
- The client supplies **no** input to green (the user drives on the host).
  `emit_vessel_input` still fires `DriveRover{forward:0,…}` every tick
  (`controller/lib.rs:265`), so the host acks zero-inputs and
  `reconcile_owned_prediction` runs — but the *prediction* is "sits still / local
  physics," while authority is "being pushed." Error grows; reconcile only
  `Correct`s by `blend=0.3` at the 20 Hz ack rate, or `Snap`s past 6 m.
- Net: green free-runs locally and lurches toward authority in steps, while the
  *pusher* (red) renders as a normal proxy `INTERP_DELAY=0.18 s` in the past.
  → **"pushed without contact," converging at rest** ("then it syncs").

The proxies are fine (measured Δpos host↔client = 0.00 for every interpolated
body; only the locally-owned green diverged, Δ = 0.12).

### Why the naive "Fix A" (gate the input seq) is WRONG

Zeroing the seq in `emit_vessel_input` keeps `OwnedLocally` on, so the body stays
`Dynamic`, stays *un*-interpolated, and the ack freezes → reconcile no-ops → the
body free-runs with **zero** correction. Strictly worse. **Fix A must flip
`OwnedLocally` off**, returning the body to the proxy path.

---

## 2. How this matches the goal

luncosim networking is a **server-authoritative, client-predictive simulation**
(the Unreal model — `DESIGN_GAPS.md` §0), *not* a deterministic-lockstep game.
The product is a digital twin with sim-time, time-warp, and Modelica cosim. Two
consequences pin the design:

1. **Correctness > twitch latency.** Conservative re-anchoring (snap + bounded
   blend) is the right default; aggressive rollback is neither needed nor sound.
2. **Cosim coupling is the defining constraint (Gap C).** A balloon driven by
   Modelica forces is computable *only* on the server → must never be predicted.
   An idle rover shoved by another rover is the **same class** — externally
   driven, not locally computable. The computability rule unifies them: it is the
   one rule that keeps cosim bodies and pushed rovers both correct.

So the Rocket-League question is a **stress test of the interaction model**, not a
new product requirement. The MVP answer (Phase A) makes everything correct;
crisp shared-ball play (Phases B/C) is a *nice-to-have* the architecture can grow
into, not a pivot.

---

## 3. How this matches avian (3D 0.6.1)

Facts that shape the plan (verified against the avian source):

- **Kinematic → Dynamic pushes; Kinematic ↔ Kinematic does not collide**
  (`solver/plugin.rs`: contacts where neither body is dynamic are skipped). This
  is the enabler for Phase B: keep a shared **ball `Dynamic`** on the client and
  the `Kinematic` rover proxies *will* push it locally → crisp local response,
  with no input to replay → reconcile by **state**. It is also why two *proxy*
  rovers pass through each other (the already-deferred rover-rover caveat).
- **Contact graph is exposed** — `Collisions` system-param / `ContactGraph`
  resource (`collision/contact_types/contact_graph.rs`). avian does *not* expose
  solver **islands** directly, so Phase C computes connected components by BFS over
  the touching contact pairs of our predicted set (small: a rover + a ball + maybe
  one neighbour). No need to reimplement contact detection.
- **State sync order** — write avian `Position`/`Rotation` (f64 truth) *and*
  `Transform`; the `Position→Transform` writeback (`PhysicsTransformSystems`,
  Writeback) re-derives Transform from `Position` otherwise and clobbers a
  Transform-only edit. The existing reconcile already does this
  (`commands.rs:666-686`); B/C must follow the same discipline.
- **Not cross-platform deterministic** (`enhanced-determinism` feature is off) →
  never assume bit-exact replay; every member is re-anchored to authority each
  snapshot. Confirms D2.
- **SubstepCount = 12** on our rovers (suspension stability), not avian's default
  1. Any local re-stepping/prediction runs the *same* FixedUpdate physics, so it
  inherits 12 automatically — do not hand-roll a separate integrator.

---

## 4. How this matches big_space

- **Single-cell today** (`world.rs`: `cell_edge_length=2000`,
  `switching_threshold=1e10`) → recentering never fires; absolute position ≡
  `Transform`; `cell` is always `[0,0,0]`. All current seat-points are valid.
- The snapshot **already** carries absolute f64 `pos` + `cell` (`session.rs`
  `SnapshotSample`), and interpolation already lerps in f64 absolute space — the
  precision-correct ("gap A") choice for lunar/orbital scale.
- **Invariant for A/B/C:** the truth for a predicted/reconciled body is the f64
  absolute `Position`. Every place that seats a render pose must, *once recentering
  is enabled*, split absolute→`(cell, local)` via `grid.translation_to_grid()` and
  update `CellCoord` — exactly the TODO already flagged at `commands.rs:424`. The
  plan adds **no new** single-cell assumptions; it routes B/C poses through the
  same f64-Position seat-point so the future multi-cell fix is one shared change.

---

## 5. The plan

### Phase A — membership = ownership ∧ active input  *(fixes the bug; MVP)*

**Goal:** an owned-but-not-actively-driven body falls back to the interpolated
proxy path (the correct class for externally-driven motion).

1. Track **last active-input tick** per gid. `InputFrame` already carries `tick`
   (`session.rs:320`). Add `last_active_tick: u64` to `VesselInputLog` and set it
   in the `record_drive_input` observer (`controller/lib.rs:42`) whenever
   `|forward|+|steer|+|brake| > ε`. (Frames-present is *not* a usable signal —
   zero-input frames are emitted every tick.)
2. In `maintain_owned_locally` (`commands.rs:470`), compute
   `mine = reg.owns(local.0, gid) ∧ (SimTick − last_active_tick ≤ GRACE)`.
   `GRACE ≈ 30 ticks (0.5 s)` gives hysteresis so it doesn't flap between key
   taps. Owned-but-idle ⇒ `mine=false` ⇒ marker removed ⇒
   `force_kinematic_proxies` re-pins Kinematic + `interpolate_proxies` drives it.
3. The Dynamic↔Kinematic flip on start/stop of driving is the only cost; the
   grace window hides it. (Optional later: a 2-3 frame blend across the flip.)

**Scope/limits:** does not make a *pushed* body crisp — it makes it *correct*
(smoothly interpolated like every other remote body). That is the right MVP
behaviour and matches Gap C.

**Touch:** `lunco-core/session.rs` (`VesselInputLog`), `lunco-controller/lib.rs`
(`record_drive_input`), `lunco-sandbox-edit/commands.rs` (`maintain_owned_locally`).
Always-on substrate; no `networking`-feature change.

### Phase B — state-based reconciliation for input-less predicted bodies  *(enables the ball)*

**Goal:** a predicted body with **no input** (a ball you're pushing) is corrected
by *state* (snap-if-large / bounded blend / seat velocity), not input-replay.

1. New marker `PredictedDynamic` (always-on substrate) = "predict this body's
   physics locally even though I send it no input." It excludes the body from
   `force_kinematic_proxies` and `interpolate_proxies` (keep it `Dynamic`), same as
   `OwnedLocally`.
2. New system `reconcile_predicted_dynamic` (FixedPostUpdate, after Writeback):
   for each `PredictedDynamic` body, on each fresh snapshot run a *state* decision
   — reuse `reconcile_decision` but compare **authority-now vs body-now** (there is
   no input seq to align on), `Snap` past `snap_pos`, else `Correct` by `blend`,
   then seat `LinearVelocity`/`AngularVelocity` to authority so it stops
   re-diverging. Render-smooth the residual.
3. Designate the demo ball `PredictedDynamic` on the client. Because Kinematic
   proxies push Dynamic bodies, your rover proxy (or your owned rover) shoves the
   ball locally → crisp; the snapshot pulls the small error out.

**Scope/limits:** state reconciliation rubber-bands *more* than input-replay (no
seq alignment), so reserve `PredictedDynamic` for bodies you actively interact
with; everything else stays interpolated. Cosim-opaque bodies are **never**
`PredictedDynamic` (see §6).

**Touch:** `lunco-core/session.rs` (marker + reuse `reconcile.rs`),
`lunco-sandbox-edit/commands.rs` (new system + the two exclusion queries).

### Phase C — contact-island membership  *(promotes/demotes automatically)*  — **PARKED 2026-06-10**

> **Not built.** Documented follow-up only. A+B cover MVP; C is convenience +
> deferred-territory (see decision note at top). The §6 guard below — its only
> load-bearing prerequisite — *was* shipped, so picking C up later is unblocked on
> that front. Before building, **re-verify the avian 0.6.1 contact API** (the
> `Collisions` system-param / contact-pair iteration); the signature is not yet
> confirmed in code.

**Goal:** the predicted set grows to whatever your predicted bodies are *touching*
and shrinks when they separate — so car↔ball and your-car↔pushed-object feel
right without hand-tagging.

1. New system `tag_predicted_island` (FixedPostUpdate, after Writeback): seed from
   `OwnedLocally`; BFS over `Collisions`/`ContactGraph` touching pairs; insert
   `IslandMember` on each reached body that is **not** opaque (§6); remove it from
   bodies that left the island. Cap BFS depth/size as a backstop.
2. `force_kinematic_proxies`, `interpolate_proxies`, and the reconcile systems add
   `Without<IslandMember>` / `Or<(OwnedLocally, IslandMember, PredictedDynamic)>`
   to their filters so an island member is predicted (Dynamic) + state-reconciled
   (Phase B machinery) while in contact, and snaps back to interpolation when it
   leaves.

**Scope/limits:** this enters the **explicitly-deferred** "two predicted bodies in
contact" territory (`DECISIONS.md:65-66`, `DESIGN_GAPS.md` DEFER). MVP stance
stands: two *remote* rovers (both proxies) pass through each other (Kinematic↔
Kinematic); only *your* predicted body vs a proxy/ball interacts. Full
symmetric rover-rover prediction is out of scope.

**Touch:** `lunco-sandbox-edit/commands.rs` only (new system + filter widening).

---

## 6. The hard guard — never predict an opaque body  — **SHIPPED 2026-06-10**

> **Implemented.** `lunco_core::NotPredictable` marker (always-on substrate,
> `session.rs`), stamped at the cosim takeover site by `tag_cosim_opaque` in
> `lunco-usd-sim`'s `cosim::install` (any body with a `SimComponent` + `RigidBody`
> that is not a `RoverVessel`), and respected by Phase B's
> `maintain_predicted_dynamic` (`Without<NotPredictable>`). When C is unparked, its
> island BFS must also refuse to cross/promote a `NotPredictable` body.

Independent of A/B/C: a body whose motion is **not locally computable** must be
excluded from every predicted set, even if owned or in contact. These are Gap C's
"Opaque" bodies — primarily **cosim-driven** ones (balloons, anything moved by
Modelica forces the client doesn't run). Add a `NotPredictable` (Opaque) marker,
stamped where cosim takes over a body, and make island BFS (C) and manual
designation (B) refuse to cross/promote it. Without this guard, Phase C would
"helpfully" predict a balloon the instant a rover bumps it — the exact failure
Gap C warns about.

---

## 7. Sequencing & verification

- **A** is independent and ships the correctness fix. Verify with the exact repro:
  host drives one rover into a client-owned idle rover → on the client the pushed
  rover must move *only* in contact with the (interpolated) pusher and match host
  spacing. Add a headless unit test on `maintain_owned_locally`'s membership
  predicate (pure: ownership × input-recency × tick).
- **B** depends on the `reconcile.rs` decision (exists) + the new marker; verify
  with one owned rover + one `PredictedDynamic` ball, single client.
- **C** depends on B; verify two clients + ball (the Rocket-League stress test),
  accepting the deferred rover-rover caveat.
- Each phase keeps the **f64 `Position` seat-point** as the single truth so the
  big_space multi-cell split (§4) remains one future change, not three.

---

## 8. Decisions needed from the user

1. **Ship Phase A now?** (small, fixes the live bug, no scope risk.)
2. **Pursue B+C** for a real shared-ball demo, or **park** them as documented
   follow-ups and keep MVP at "ball interpolated, contact reconciled by snap"?
3. Confirm the **deferred rover-rover** stance (proxies pass through each other)
   stays as-is for MVP.
