# Networking — resolved decisions (canonical log)

Decisions locked. This is the source of truth; the "open questions"
sections in the other docs point here. Phase-local implementation/tuning choices
are intentionally **not** here — they're decided in-phase with real code (listed
at the bottom).

---

## D1 — Backend: **lightyear** (committed)
`lightyear 0.26.4` (Bevy 0.18). It ships exactly what's hardest to hand-roll:
M2 Predicted/Interpolated roles, M6 tick-sync (`lightyear_sync`), proven f64-avian,
wasm WebTransport, host-client. The `Mutation`/`OpId` envelope remains the M3
payload regardless of backend, so fallback cost is bounded.
- The Ph0 spike is **not** an open A/B — it's narrowed to verifying lightyear's one
  real risk: **host-client robustness under latency**. If that fails → fall back to
  replicon+renet2 (envelope work carries over). Otherwise lightyear stands.
- **Ph0 RESULT: host-client risk RETIRED on the native path.** Built
  clean on Bevy 0.18; host-client boots, remote client completes the
  netcode/WebTransport handshake, replication + prediction engage, tick-sync stable
  30 s under the default latency conditioner, zero panics across 3 runs. The only
  anomaly (a single capped 252-tick rollback) was a late-join transient that did not
  recur on a normal-timing join. (Full Ph0 spike log was in `SPIKE_PH0.md` — git history.)
- **Browser/wasm leg also PASSED:** the wasm client builds, boots
  (WebGL/ANGLE), and **connects over WebTransport + receives replicated server state**
  (verified twice, plus clean reconnect). The only remaining item is the subjective
  in-browser input-feel (non-gating; CDP-driving backgrounds the tab and Chrome's
  throttle drops the keepalive before movement can be captured — a tab-lifecycle
  artifact, not a lightyear issue). The cert pain along the way was a stale
  baked-in digest in the example (dev-cert gotchas now captured in `DEPLOY.md` →
  *Appendix — local / self-signed dev cert*), not a backend problem. **Net: D1 (lightyear) fully validated for our topology — native host-client
  AND browser WebTransport.**
- Supersedes STACK_COMPARISON §2.4 "open" status and DESIGN_GAPS Q4.

## D2 — Reconciliation: **input-replay reconciliation, re-stepping our own avian for the OWNED rover** (REOPENED 2026-05-30)
Predict the owned rover by running real **f64 avian** dynamics locally. On each
snapshot that acks an input sequence number, snap the owned body's 4 integrator
components (`Position`/`Rotation`/`LinearVelocity`/`AngularVelocity`) to authoritative
state and re-step avian over the unacked-input buffer (~3–6 fixed ticks). This is
**state replication + re-anchoring** (NOT deterministic lockstep), so f64 non-determinism
only matters across the unacked window before the next snapshot snaps back to truth. All
**remote** bodies stay `Kinematic`-pinned + interpolated, unchanged. As-built summary:
README → *Client-Side Prediction* (the original design + phased plan
`PREDICTION_RECONCILIATION.md` is in git history).

**Why the old D2 was reopened:** continuous smooth-correction toward the latest snapshot is
an *unfixable rubber-band* — the authoritative echo is always one link-latency behind a
forward-moving client, so blending toward it is a permanent backward tug (no `CORRECTION_RATE`
is both responsive and smooth). The old "full rollback ruled out by construction" premises
(global solver, cross-platform non-determinism) do **not** apply here: on a *client* the owned
chassis is the **only `Dynamic` body** (every other replicated rover is `Kinematic`-pinned,
wheel systems early-out on `Kinematic`), so a replay tick solves a **1-body island** — no
coupling to replay — and re-anchoring bounds drift to the unacked window.

**Why NOT lightyear-native prediction** (the natural "do it via lightyear" instinct): hard-blocked,
verified firsthand — `lightyear_avian3d`/`lightyear_replication` 0.26.4 require **avian `^0.5`**
(we're on 0.6.1) and are **f32-only** (`default = [..,"avian3d/parry-f32"]`, no `f64`/`parry-f64`
feature; `.f32()` hardcoded in the correction path). Going native would force downgrading avian
**and** de-precisioning the whole physics stack to f32 — reversing the f64 double-precision that
big-space/lunar-orbital coordinates depend on. lightyear release notes (through 0.26.0, 2026-01)
have **never mentioned f64**. (Full analysis was in `LIGHTYEAR_NATIVE_REVIEW.md` — git history.) **lightyear stays the
transport/netcode/sync substrate (D1); prediction is hand-rolled over our own f64 avian** — the
exact Source/Overwatch predict+reconcile algorithm, minus the f64/version wall. Re-open native
only if a future lightyear targets avian 0.6 **and** ships a `parry-f64` path.

- Rover-rover contact corrections: accept the snap (or disable inter-player rover
  collision) — deferred, see DESIGN_GAPS.
- Supersedes DESIGN_GAPS Q1. Reverses original D2 (smooth-correction) on our own terms;
  keeps D3/D4/D5/D6/D7 intact (the hand-rolled path is D7-clean — no lightyear types in
  always-on substrate, no avian schedule surgery).

## D3 — Identity: **deterministic from provenance** (confirmed)
Network id = pure function of provenance. Content/Derived → deterministic hash
(local spawn, not replicated); Authoritative → server-allocated + spawn replicated;
Local → never networked. Unreal net-stable-names / content-GUID model. The logic
lives in `lunco-core` (see README → *Entity Identity Mapping*).

### D3a — Collision policy: **53-bit on the sync layer + debug-time collision check**
Keep ids in the JS-safe 53-bit space (hard browser constraint). Add a debug-time
collision check at content load. Do **not** build 128-bit-internal/narrow-on-the-sync-layer
machinery now — per-scene populations are tiny (5000-prim sample: zero collisions);
birthday-bound at 53 bits is comfortable for thousands of entities. Revisit only if
a real scene approaches ~10⁶ entities.
- Supersedes IDENTITY Q1.

### D3b — `SourceId`: **logical scene name for identity, content-hash for cache**
Identity keys on the stable logical scene name + canonical prim path, so a USD edit
does **not** reassign ids (path-based id is exactly why live edits keep identity
stable — IDENTITY Q3 confirmed *yes*). The content-hash is a separate concern, used
only for asset fetch/dedupe.
- Supersedes IDENTITY Q2 and Q3.

## D4 — Spawn authority: **content = local spawn + deterministic id; runtime = server spawns + replicates**
The Unreal level-actor vs dynamic-actor split. Content-instanced entities are
spawned locally on each peer (only their *state* replicates); runtime-born entities
are spawned by the server, which allocates the id and replicates the spawn.
- Supersedes DESIGN_GAPS Q3.

## D5 — Time-warp in multiplayer: **host-only, applied to all; forbidden when ROS owns a vessel**
- MVP: only the host may warp/pause; it applies to the whole shared world (server
  owns the clock). Per-client independent warp is **never** allowed (desyncs by
  definition).
- When a ROS controller holds authority over a vessel: warp is **forbidden** (a nav
  stack can't be fast-forwarded). Hard rule.
- Matches KSP (forbids MP warp) / Factorio (speed tied to lockstep, voted).
- Supersedes DESIGN_GAPS Q2 and ROS2_BRIDGE Q5.

## D6 — Clock seam: **drive lightyear's `Tick` from our `SimTick`** (one clock)
The sim owns time; the netcode tick is *derived* from it (same idea as ROS
`use_sim_time`). Do not run two independent clocks. This is a Ph3/Ph4 integration
detail — finalize the exact wiring once lightyear's `Tick` API is in hand, but the
*direction of authority* is decided: SimTick is authoritative.

## D7 — Networking is an **opt-in Cargo feature** (`networking`), gating the sync layer only
Networking must not be linked into a default/local build. `lightyear` is an
**`optional` dependency** behind `feature = "networking"`; the sync layer
(replication/transport/prediction systems, `lunco-networking` guts that import
lightyear) compiles only when the feature is on.

**The substrate stays always-on** (NOT gated): `Provenance`, the locked
`GlobalEntityId`, `SimTick`, `IsServer` (the Ph1 `PH1_CORE_CHANGES` patch). They are
plain data + cheap systems, valuable standalone (deterministic ids, `Local` opt-out,
discrete tick), and — critically — keep domain crates **byte-identical** whether or
not the feature is set. The `app.sync::<T>()` / `register_command` facade is also
always present, compiling to a **no-op registration** when the feature is off, so
domain crates call it unconditionally and never `#[cfg]`-fork.

Rationale: if the substrate were also gated, domain code would split into two
compile paths and every networking toggle would force a full `lunco-core` rebuild.
Gating only the sync layer means flipping `networking` rebuilds just the sync layer. This
is the facade pattern the architecture already assumes ("domain crates never import a
backend" — IMPLEMENTATION_PLAN invariants).
- Supersedes the implicit "always-linked" assumption in earlier docs.

---

## ROS2 — staged (not blocking; confirm at the ROS phase)
- Coupling: **free-run + realtime cap** first; controller-paced lockstep (true HIL)
  only if a real controller needs it. (ROS2_BRIDGE Q1)
- Integration: **rclrs in-process** (direct ECS, no extra hop) unless ROS distro
  lifecycle coupling becomes painful. (ROS2_BRIDGE Q2)
- Human+ROS authority arbitration: **deferred** — possession model already handles
  single-owner; co-control is a post-MVP feature. (ROS2_BRIDGE Q3)
- Frames/units: REP-103/105 + metric, `map` frame anchored at a chosen cell —
  confirm concrete anchor at the ROS phase. (ROS2_BRIDGE Q4)

## Deferred (acknowledged, standard practice)
Rover-rover collision under prediction, lag compensation, input validation/anti-cheat,
interest management, compression stack. Source/Overwatch don't predict pawn-vs-pawn
either — these are correctly out of MVP scope.

---

## NOT decided here — phase-local (decided in-phase, with code)
These are implementation/tuning, wrong to fix on paper:
- **Ph3:** how `CellCoord` rides alongside lightyear `Transform` replication; offset
  quantization scheme.
- **Ph3 (gap H):** fold interpolation into existing
  `TranslationInterpolation`/`RotationInterpolation` vs use lightyear's `Interpolated`.
- **Ph4:** input redundancy depth N; server jitter-buffer size (measured under real
  latency).
- **Ph6:** late-join handshake payload details.

These do not block; the architecture is complete without them.
