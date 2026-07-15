# 19 — Unified Time, Clocks & Animation

> Status: Active · Audience: contributors working on time/simulation clocks.
>
> Provides a unified time spine, time-domain hierarchy, and animation transport system for LunCoSim.
>
> All time-scale and Julian Date nuance lives in the `lunco-time` crate (which handles `MissionClock`, `TimeTransport`, and `WorldTime`). USD animation channels (transformation, rotation, matrices, visibility, materials) compose via `lunco-usd-bevy` (`UsdAnimated` and `sample_usd_animation`) and drive the runtime under a preview transport.

This architecture uses **one stored master (the tick), a tree of derived clocks, and a single animation funnel** to prevent synchronization drift.

---

## 0. TL;DR

- We don't lack clocks — we have **five floating ones** *and* **four integrators that
  each answer "speed up" differently**. The fix is consolidation + a conversion layer,
  not a greenfield clock.
- **Store one master: `SimTick`.** Everything calendar/astronomical is **derived**, never
  accumulated. The old epoch accumulation in the core clock was the bug.
- **Many clocks are fine — *floating* clocks are the debt.** Model clocks as a **tree of
  affine transforms over the master** (`child_t = offset + scale·parent_t`, i.e. USD
  `LayerOffset`). "Speed only the factory" = scale one node. Coherent + seekable + many.
- **One animation funnel:** `TimeDomain (clock) → value source (sampled timeSamples | live
  tween/state-machine) → write`. Per-object / per-selection / per-project playback are just
  different domain bindings.
- **Tweens/state machines are welcome** as the *behavior + authoring* layer — but they ride
  a `TimeDomain` and **bake to `timeSamples`**; they are never a second playback clock.
- **Transport (pause/rate/warp) is host-authoritative** in multiplayer; only pause-gated
  *visual* preview-scrub is client-local. Only `SimTick` ever crosses the wire.

---

## 1. Time, Clock & Animation Substrates

### 1a. The five floating substrates (Original/Historical state before consolidation)

| # | Substrate | Where | Origin | Reads which clock |
|---|---|---|---|---|
| A | `CelestialClock` (Historical) | `lunco-core` | J2000 `2451545.0`, or wall-seeded | advanced from **`Res<Time>` (Virtual)**, *accumulated* |
| B | `TimeWarpState` (Historical) | `lunco-core` | mirror of A | written by A's tick system |
| C | `Time<Fixed>` 60 Hz + `SimTick(u64)` | `lunco-core` | tick 0 | `Time<Fixed>` ← Virtual; tick gated by `is_running()` |
| D | Modelica `current_time` | `lunco-modelica/src/worker.rs` | `0.0` | `Time<Fixed>` Δ, capped — but only ONE dispatch per **render frame** (`A3`), so the model's rate depended on GPU load |
| E | `RunBounds` | `lunco-experiments` | relative `0.0` | its own offline loop |

They did **not** share an origin (three time-zeros: J2000, `0.0`, `0.0`) and had **no
conversion layer**.

### 1b. The sharper problem the audit found — four integrators, four answers to "speed up" (Historical)

> **This subsection is the *problem statement*, not the current behaviour.** §3's
> `RealtimePhysics` regime is the answer to it: one `rate`, uniformly scaling the
> fixed-step cadence, so epoch, avian **and Modelica** move together. Read this
> table as what a change here must not reintroduce.

The audit's deeper finding was that there was not even one coherent *realtime*
timebase:

| Subsystem | Timebase (then) | `speed_multiplier = 50×` | `Time<Virtual>` slow-mo 0.01× |
|---|---|---|---|
| `CelestialClock.epoch` | Virtual, **accumulated** | sun races 50× | slows |
| `SimTick` (netcode) | Fixed, gated by `is_running()` | runs (≤100×) else **freezes** | unaffected |
| avian physics | Fixed ← Virtual | **stays 1×** | slows |
| Modelica solver | Fixed Δ, capped, gated by per-model `paused` | **stays 1×** | unaffected |

At 50× warp the sun moved 50×, the netcode tick 1×, the rover physics 1×, the
Modelica model 1× — **the calendar was detached from every integrator**, and `epoch`
(Virtual-accumulated) was not even the same clock as `sim_secs` (Fixed-tick-derived) in plain
"play." The two speed knobs — `CelestialClock.speed_multiplier`/`TimeWarpState.speed`
(scales epoch + gates stepping) vs `Time<Virtual>.relative_speed` (scales avian + `Res<Time>`
consumers) — were **never cross-wired**, so warp was effectively cosmetic.

### 1c. The one thing already correct

The netcode already does it right in miniature: `gen_t = tick · SECS_PER_TICK`
(`lunco-sandbox-edit/src/commands.rs:467`) **derives** seconds from ticks (never accumulates),
and `lightyear tick_duration = SECS_PER_TICK` (`lunco-networking/src/shared.rs:99`). The
master we want already exists and already works; this design generalizes it.

### 1d. Lighting — disconnected from the ephemeris (the headline gap)

`update_sun_light_system` (`lunco-celestial/src/systems.rs:138`) hardcodes `dir = Vec3::NEG_Z`
and reads **no time at all**. We compute the Sun's ephemeris position and then ignore it for
lighting. No terminator / day-night / thermal / solar-power coupling to date+location — the
single highest-value gap for a *lunar* sim.

### 1e. avian / Bevy time wiring

- avian `PhysicsPlugins::default()` (with `PhysicsInterpolationPlugin::interpolate_all()`,
  `SubstepCount(12)`) steps in `FixedPostUpdate`; pause = pause `Time<Virtual>`.
- `Time<Real>` is **unused** anywhere in `crates/`.
- `Time<Virtual>` is capped (`set_max_delta(33ms)`); `relative_speed` is set in exactly one
  place (`luncosim/src/main.rs` slow-mo toggle) and **never** from warp.

### 1f. Ephemeris engine (the good part)

`lunco-celestial-ephemeris`: `celestial-time` + `celestial-ephemeris`, TDB, two-part
`JulianDate(day, frac)`, VSOP2013 Sun/Earth/Emb, ELP/MPP02 Moon, JPL Horizons CSV.
`EphemerisProvider::position(body, epoch_jd)`. Default install = `NoOpEphemerisProvider`.

**`celestial-time` (already a dep, 0.1.1-alpha.2) covers the scale stack** — verified: modules
`scales`/`sidereal`/`transforms`; types `TAI`/`TDB`/`UT1`/`UTC` (each over two-part `JulianDate`);
`to_utc`/`to_tai`; a leap-second table (`TAI_UTC_OFFSETS`); leap-aware `utc_from_calendar`; and a
**sidereal/GMST** module. So UTC↔TAI↔TT↔TDB↔UT1 + GMST are **present in a dependency, just unwired
in app code** — T3 is wiring, not new math, and **`hifitime` is not needed**.
Genuinely absent: MET counter (trivial), SGP4/TLE, SPICE.

### 1g. USD already has the per-clock primitives

Confirmed in our openusd fork:
- `LayerOffset { offset, scale }`, `apply(t) = offset + scale·t` + inverse (`src/sdf/mod.rs:129`)
  — the **affine clock transform**, carried on `Reference.layer_offset` and sublayers, **and
  genuinely applied to `timeSamples` times** in value resolution (verified: `usd/stage.rs:459`,
  `usd/attribute.rs:649`, `usd/diff.rs:624`; PCP concatenates scales across the chain in
  `pcp/compose_site.rs`). *Caveat:* our sampler reads the **flattened** `sdf::Data`, not the live
  Stage — so T5 must confirm the flatten carries the composed retime, else the sampler applies the
  per-prim composed offset itself (a known, bounded task, not a blocker).
- `startTimeCode` / `endTimeCode` (clip range) + `timeCodesPerSecond` (tick rate)
  (`src/sdf/schema.rs`).
- `timeSamples` (`Value::TimeSamples = Vec<(f64, Value)>`) + `usd::evaluate` (held/linear).

So the data substrate for scaled, ranged, per-object animation is **native**, not invented.

---

## 2. Problems (what we are fixing)

1. **No single authority** — nothing answers "what instant is *now*, in every representation?"
2. **Fragmented playback** — pause in three places, speed in two disjoint knobs, warp cosmetic.
3. **Lighting ignores the ephemeris** (§1d).
4. **Sim is neither seekable nor epoch-anchored** — `current_time` starts at 0 with no calendar
   meaning; run outputs can't be timestamped in absolute time.
5. **Accumulation, not derivation** — `epoch += Δt` drifts, is frame-rate dependent, can't seek.
6. **No model for "many clocks"** — we want a factory at 100×, several projects on independent
   time, and per-object replay, with no clean way to express it.

---

## 3. The model — one master, a clock tree, pure projections

### 3a. The census of times (≈14 representations, 2 inputs)

**Only two true inputs (the only stored, mutable time state):**

| Time | Type | Role |
|---|---|---|
| **`SimTick`** | `u64`, discrete | The integrator count. Deterministic, replicable, the *only* thing on the wire. |
| **Wall clock** (`Time<Real>`) | continuous | Frame pacing, input, interp buffers, warp-preview. **Never** sim logic. Seeds `epoch0` once. |

**Everything else is a pure projection — computed on demand, never stored/accumulated:**

- *Sim domain (from tick):* `sim_secs = (tick − tick0)·SECS_PER_TICK (+ overstep)`; **MET** `= epoch − epoch0`.
- *Calendar/celestial (from `epoch`):* **TDB** (two-part JD, ephemeris input) · **TT** (≈TDB) ·
  **TAI** (TT − 32.184 s) · **UTC** (TAI − leap seconds) · **UT1** (UTC + DUT1) · **GMST/sidereal**.
- *Environment (from epoch + body + surface):* sun elevation/azimuth, local solar time,
  sub-solar point, lunar rotation phase, days-since-J2000.
- *Run domain:* `RunBounds.t_start/t_end/dt` and Modelica `current_time`, anchored via run `epoch0`.

### 3b. The anchor = the conversion layer

`anchor: { epoch0: TwoPartJd, tick0: u64 }` is the single bridge between the discrete sim
domain and the continuous calendar domain:

```
sim_secs = (tick − tick0) · SECS_PER_TICK
epoch    = epoch0 + sim_secs            (TDB days)      ← derived, never accumulated
met      = epoch − epoch0
```

`epoch0` is a **two-part** constant (precision lives here and at the ephemeris call boundary);
the live quantities are `u64` tick + `f64` sim_secs, both exact / sub-µs over a multi-year
mission, combined to two-part only when calling the ephemeris.

### 3c. Architecture — thin resources, not a god-object

`SimTick` is already the master; do **not** subsume it. Add:

- **`MissionClock { anchor: { epoch0, tick0 }, warp: WarpState }`** — the conversion layer + warp.
- **`TimeTransport { mode, rate }`** — replaces the three pause flags + two speed knobs.
- **Pure projection fns** `epoch(tick, mission)`, `utc(..)`, `met(..)`, `gmst(..)`, … — free
  functions, `#[cfg(test)]`-covered with fixtures. *Not* methods on a fat resource.

`CelestialClock.epoch` / `TimeWarpState` become **driven views** a shim-system writes each frame
from the projections, so existing readers (`ephemeris_update_system`, telemetry, UI) are untouched
during migration.

### 3d. The clock tree — many clocks, done right

A clock is **an affine child of a parent**, rooted at the tick master:
`child_t = offset + scale·parent_t` (USD `LayerOffset`). Floating clocks are debt; *rooted*
clocks are free — independently controllable yet always convertible back to the master.

A **`TimeDomain`** node carries:
- **parent** (ultimately the tick master),
- **`(offset, scale)`** relative to the parent,
- **regime** (`Kinematic` = rate-scale freely · `Causal` = integrates, needs communication points),
- and, if **driven**, a **transport** `{ mode, head, range:[start,end], loop, rate }`.

Two node kinds:
- **Derived domain** — `local_t = offset + scale·parent_t`. Rigidly follows the parent (the
  factory at 100×). No independent playhead.
- **Driven domain** — its own **playhead**; advances by the parent's *delta* when playing, but
  seek/pause/replay/loop independently. The per-object / per-selection animation player.

Bindings (all the same machinery, only the bound set differs):
- **Global** — the world domain (default).
- **Per-project / twin** — domain-per-document; two projects open run on independent clocks.
- **Per-selection** — "new domain from selection" binds N entities to a fresh driven domain.
- **Per-object** — one prim's animation, replayable on demand.

> **"Speed only the factory"** = a derived domain with `scale = 100` (Tier 1; see §5).
> **"Replay this object"** = a driven domain, `head = start; mode = Playing` (loop = wrap).
> **"Run several projects on separate time"** = N domains advanced independently (Tier 2).

---

## 4. The animation system — one funnel

There is exactly one animation pipeline; everything is a layer in it:

```
TimeDomain (clock: global | project | selection | object; derived or driven)
        │  resolve(entity.TimeBinding) → local_t
        ▼
value source ──► Transform / attr
   ├─ sampled:    evaluate(prim.timeSamples, local_t · timeCodesPerSecond)   ← canonical
   └─ procedural: tween / state-machine evaluated at local_t                  ← behavior
```

The **clock** layer (domains) and the **value-source** layer (sampled vs procedural) are
orthogonal — that orthogonality is what lets per-object replay, factory scaling, project-level
run, and a reactive door share one sampler.

### 4a. The sampler (one code path)

A per-frame system evaluates entities carrying `UsdAnimated` (tracks extracted from `timeSamples` at translate/asset-change time). 

To ensure O(1) frame evaluation and avoid expensive stage traversal or path parsing in the frame loop, a per-entity **`AnimationPlan`** component is derived (built on `Added<UsdAnimated>` or stage reload). The `AnimationPlan` caches:
*   Parsed `SdfPath` structures.
*   The `timeCodesPerSecond` scale factor.
*   An `XformDrive` enum (specifying whether the transformation uses an `xformOpOrder` array, a raw 4x4 matrix, translation/rotation/scale channels, or none).
*   A `visibility` flag.
*   An optional `MaterialPlan` (resolving the target shader's `SdfPath` and key channel attributes).

During the frame step, `sample_usd_animation` and `sample_usd_material_animation` consume the pre-calculated `AnimationPlan` to evaluate animation values directly at time `t`, eliminating per-frame `SdfPath` parsing, topology scans, and two-hop shader resolution.

```
for each animated entity:
    local_t = resolve(entity.TimeBinding)        // defaults to the world domain
    value   = evaluate(track, local_t · timeCodesPerSecond)   // openusd::usd::evaluate
    write Transform / material / attr
```

- Reads `attribute_value_at` (already built): `timeSamples` win over `default`; held past ends.
- `local_t` carries the fixed-step **`overstep_fraction`** so render-rate motion is smooth over
  the 60 Hz integrator — sharing **one** interpolation alpha with avian's
  `PhysicsInterpolationPlugin` (no double-smoothing) and the netcode `INTERP_DELAY` buffer.
- Kinematic-animated prims that also have bodies must be `RigidBody::Kinematic`, or writing
  `Transform` fights the avian solver. Runs **before** transform propagation (big_space).

### 4b. Tweens / state machines — behavior + authoring, never a second clock

"Tween vs keyframe" is the **same axis as pure vs causal**:

| | Tween / state machine | `timeSamples` |
|---|---|---|
| Nature | procedural, stateful, event-driven | data, pure function of time |
| Strength | reactive behavior (doors, arms, UI), terse authoring | authored/recorded timeline, scrub, replicate |
| Seekable | only within current transition | freely |
| Replicable | causal (host-auth or local-only) | pure (derives identically) |

A tween is a 2-keyframe eased curve; a state-machine-of-tweens is a *timeline generator*. The
unification:

1. **Adopt the tween + state-machine *pattern*** as the authoring/behavior layer.
2. **It advances on a `TimeDomain`, never on `Time` delta.** (A crate like `bevy_tweening` runs
   its own timer → wouldn't pause/scrub/replicate/bake → the floating-clock antipattern reborn.
   A thin in-house tween that evaluates against a domain playhead is cleaner than adapting one.)
3. **One playback path.** Procedural sources either run **live** (reactive, causal) or **bake to
   `timeSamples`** (record → scrub/replicate). The sampler always reads `timeSamples`. If a state
   machine's *transitions are timestamped on its domain*, the whole machine becomes seekable and
   bakeable.

Which to reach for:
- Authored/recorded/scrubbable timeline (ConOps, baked run, cinematic, replay) → `timeSamples` + domain.
- Reactive/event-driven behavior (player opens a door, arm responds to a command) → state machine +
  tween **on a domain**, recordable.
- Terse authoring ("A→B over 2 s, ease") → a tween that **bakes to two keyframes**.

---

## 5. Regimes & coupling — three tiers of "own clock / own rate"

The clock tree gives many clocks; whether a subsystem may run at an *independent rate* depends on
whether it **integrates** and whether it **interacts**:

- **Tier 1 — Kinematic / baked.** State is a pure function of time (`timeSamples`, baked curves,
  schedules). Rate-scaling is exact resampling: `LayerOffset{scale}`. The vast majority of
  "speed only X" wants. No coupling risk, fully scrubbable.
- **Tier 2 — Decoupled live integrator.** Its own ODE/discrete-event solver that exchanges **no**
  state with the 1× world during the speedup. Give it a driven domain / per-model dt scale; bounded
  by solver stability (a stiff ODE can't go 100×). Independent projects live here.
- **Tier 3 — Coupled subsystems at different rates = co-simulation.** When the fast factory must
  exchange state with the slow rover, free-running clocks break causality. The correct structure is
  **communication points** (FMI master-algorithm semantics): between barriers each subsystem advances
  independently; at a barrier they sync at a common wall-anchored time. We already orchestrate models
  in `lunco-cosim` — Tier 3 is "add communication points + per-FMU time," not greenfield.

**Global regimes** of the *live world clock* (distinct from offline run execution):
- **`RealtimePhysics`** — tick advances; epoch slaved to tick; `rate` scales the fixed-step cadence
  *uniformly* (drives both `Time<Virtual>.relative_speed` **and** the tick), so physics + Modelica +
  epoch move together. Bounded by solver stability.

  > **How Modelica actually keeps up** (finding `A3`, fixed 2026-07-12). A `rate` burst produces
  > **more fixed ticks**, never longer ones, and the Modelica solver runs off-thread — so "moving
  > together" cannot mean "one solver step per dispatch." It means the model's clock is driven to the
  > fixed-step clock: `ModelicaModel.target_time` advances by exactly one `Time<Fixed>` delta per
  > **unpaused fixed tick**, and each tick the master requests
  > `dt = target_time − current_time` (clamped to `MAX_MACRO_STEP_DT`, then integrated as an integer
  > ladder of `SECS_PER_TICK / 3` micro-steps). A model that misses ticks — worker busy, long compile,
  > `rate = 10` — **catches the time up** instead of losing it.
  >
  > **Model time is therefore a pure function of the fixed-step clock. It does not depend on the
  > render frame rate, on GPU load, or on window focus.** It used to: the dispatcher skipped any tick
  > with a step in flight and always sent `Time<Fixed>::delta`, so at most one macro step ran per
  > RENDER FRAME — at 30 FPS the model ran at half speed, at `rate = 10` it ran 10× too slow, and the
  > skipped time was gone for good.
  >
  > The residual coupling delay (the model state is one in-flight macro step old) is **measured**, not
  > assumed: `lunco_modelica::worker::CosimLag` records `|model_time − world_time|` every fixed tick
  > and warns past 0.25 s.
- **`KinematicWarp`** — tick **freezes** (physics and Modelica pause); only **pure** consumers
  (ephemeris, spin, lighting, sidereal) advance, as pure functions of epoch. **A `rate` above
  `MAX_REALTIME_RATE` falls into this regime** (`lunco-time/src/lib.rs`).

  > **Why `MAX_REALTIME_RATE` is `8.0`, not 100.** A rate is realised as *more fixed ticks per
  > frame*. At rate 100, one hitched frame demands ~198 fixed steps (≈2376 avian substeps) in a
  > single frame — which makes that frame slow, which demands the same burst again next frame. It
  > is a guaranteed death spiral: the engine cannot integrate 100× realtime physics and trying is
  > worse than declining. Above the cap the tick is frozen and only the pure, closed-form consumers
  > advance, which they can do at any rate. **A scenario that used to ask for physics at 20× now
  > warps instead.**

**Live world vs offline run** are *different axes*. The batch/interactive runner
(`experiments_runner.rs:1184`) owns its own loop, decoupled from Bevy. It is **not** a transport
regime — it produces a **baked artifact** (dense timestamped outputs → `timeSamples` via the anchor)
that the live clock then scrubs as a Tier-1 pure consumer. "How do you seek a Modelica run?" You
don't seek the live solver — you run it, bake, and scrub the bake.

---

## 6. Transport, networking & determinism

- **Only `SimTick` crosses the wire.** Absolute time is derived (`epoch0 + tick·SECS_PER_TICK`);
  no `server_time`. `epoch0`/`tick0` handed over at connect (`server.rs` already sends the tick).
- **Transport is host-authoritative in multiplayer.** A client cannot unilaterally pause/warp/
  fast-forward — it would freeze/desync the shared tick (`gen_t` stalls). Pause/rate/warp/re-anchor
  are **replicated host commands**.
- **The anchor is piecewise-constant host state**, not a pure constant — it changes only on explicit
  **re-anchor (fast-forward)** events, which replicate.
- **Preview-scrub is the only client-local time op** — and only for **pure visual** consumers, and
  only **while paused** (else moving celestial body positions, which can feed gravity/landing, would
  move the ground under a networked rover; on resume everything re-derives from the tick).
- **Projections need *not* be cross-peer bit-deterministic** — they're never compared across the wire
  (only the tick is). Platform-divergent transcendentals in GMST/ephemeris are fine for
  display/environment.

### Seek / scrub / replay semantics

- **Pure consumers** (ephemeris, lighting, sidereal, sampled `timeSamples`) — set the master/domain,
  they recompute instantly (already change-gated on `last_jd`).
- **Causal consumers** (live physics/Modelica) — cannot seek; they replay a **bake** or re-integrate
  from `t_start` (short Interactive runs).
- **Preview-scrub** (non-destructive): drag the sun to check shadows; local; pause-gated; returns on
  release; anchor unchanged.
- **Fast-forward** (destructive): skip cruise; physics extrapolated/skipped; **re-anchor**;
  host-authoritative; replicated.

---

## 7. Invariants

1. **Derive, never accumulate.** Zero per-frame `+=` on any time. Warp-preview derives from
   `Time<Real>` (`epoch_at_warp_start + wall_elapsed · rate`), so even warp doesn't accumulate.
2. **One stored master: the tick.** Calendar/celestial is always derived.
3. **Sim logic keys on continuous `tick`/`sim_secs`/`MET` only.** UTC/calendar/GMST are display +
   environment, never sim inputs (leap-second safety).
4. **Only the tick crosses the wire.** Projections are local; they need not match bit-for-bit across peers.
5. **Transport + anchor are host-authoritative.** Only pause-gated visual preview-scrub is client-local.
6. **`sim_secs` carries the fixed-step overstep**, sharing one interpolation alpha with avian — no
   double-smoothing.
7. **Many clocks, but every clock declares parent + `(offset, scale)` + regime.** No floating clocks.
8. **One animation funnel.** Procedural sources (tween/state-machine) ride a `TimeDomain` and bake to
   `timeSamples`; the sampler is the single playback path.
9. **Precision:** two-part `JulianDate` for `epoch0` and at the ephemeris boundary; never collapse to
   single-`f64` JD on that path.

---

## 8. System layers (T1–T7)

The clock system is built in layers, each independently headless-testable. T1–T3,
T5, and T7 are built; T4, T4.5, and T6 are planned (marked below).

### T1 — Master spine + derivation swap
- **New crate `lunco-time`** (not `lunco-core/src/time.rs` — a dedicated crate, `lunco-time →
  lunco-core`): `MissionClock` (fixed mission origin + re-anchorable calendar anchor + warp),
  `TimeTransport` (mode/rate), the derived `WorldTime` view, and the pure `advance_clock` step.
  8 headless fixtures green (`cargo test -p lunco-time --lib`): tick→epoch derivation, sim_secs
  round-trip, paused-freezes, rate-unifies-knob, high-warp→KinematicWarp, warp epoch from wall,
  warp-exit re-anchor continuity, paused-doesn't-warp.
- **Swapped accumulation → derivation:** `lunco-celestial/src/clock.rs` no longer does
  `epoch += Δt`; the spine derives `epoch = epoch0 + (tick−tick0)/86400`.
- **Unified the speed knobs:** `advance_world_clock` sets `Time<Virtual>.relative_speed` from
  `rate`, so one knob drives epoch+physics together. The `luncosim` slow-mo toggle writes
  `TimeTransport.rate` directly. The `paused → physics_enabled=true` inconsistency is gone (folded
  into the regime: paused ⇒ not running ⇒ tick+physics frozen).
- **`CelestialClock` removed.** The T1 compat shim was retired once the migration was
  complete: the struct (`lunco-core`) and the three bridge systems
  (`sync_transport_from_celestial`/`sync_celestial_from_world`/`get_default_celestial_clock`) are
  deleted. Every `.epoch` reader now takes `Res<WorldTime>` (`.epoch_jd`); every
  `.speed_multiplier`/`.paused` writer now takes `ResMut<TimeTransport>` (`.rate`/`.mode`). The
  mission origin is seeded directly from the wall clock by `seed_mission_clock_from_wall` (Startup).
  `WorldTime`/`TimeTransport` are the only time authorities — no driven middleman.
- **Single pause authority.** Pause was split: the workbench toolbar button and the
  obstacle-field physics-hold toggled `Time<Virtual>.pause()` directly, parallel to
  `TimeTransport.mode`, so the ⏸/▶ glyph couldn't see a pause issued by the avatar hotkey /
  mission-control / celestial panel. Unified on `TimeTransport` as the sole authority (the *opposite*
  of bidirectional mirroring, which would re-introduce dual-master drift): `TimePlugin` is now added
  (guarded) by `WorkbenchPlugin`, so the transport is present wherever a pause control lives —
  including modelica-only `lunica`. Both the button and the physics-hold now read/write
  `TimeTransport.mode`. Pause has a **single representation**: the direct clock state
  `Time<Virtual>.relative_speed`. `relative_speed = 0` freezes `Time<Fixed>` accumulation (→ tick +
  avian), and `relative_speed > 0` *is* the "is running" gate that every physics consumer reads
  directly. The spine deliberately does **not** also toggle `Time<Virtual>::pause()`: that boolean
  would be a second encoding of "paused ⇔ speed 0" that nothing reads and can only drift.
- **`TimeWarpState` removed + `ClockSample` dropped.** "Is physics advancing" had three
  redundant encodings — `TimeWarpState.physics_enabled` ≡ `TimeWarpState.is_running()` ≡
  `relative_speed > 0`. Two were forced by boundaries (`relative_speed` by avian/`Time<Virtual>`;
  `TimeWarpState` by the `lunco-core` ↔ `lunco-time` layering, since `advance_sim_tick` can't import
  the spine). Collapsed to one: `advance_sim_tick` and the physics gates
  (hardware/mobility/usd-sim) now read `Res<Time<Virtual>>` directly (`relative_speed_f64() > 0`),
  and `TimeWarpState` is deleted (struct + the manual sandbox/example inserts — Bevy's default
  `Time<Virtual>` is already *running*). `advance_clock` now **returns `f64`** (the one control
  output) and mutates the clock; the caller reads `epoch`/`regime` back from it — the
  `ClockSample` struct (which duplicated clock state) is gone. The control write is also
  **change-driven**: `advance_world_clock` writes `relative_speed` only when it differs (self-healing,
  no redundant per-frame store), while the epoch is still sampled per-frame (the clock is moving).
- **Two clocks kept distinct** (a correctness refinement over the first cut): `sim_secs`/MET use a
  *fixed* mission origin (`mission_tick0`), while the calendar `anchor` re-anchors on warp exit —
  so a warp can never corrupt the integrator clock.
- *Residual:* single-`f64` JD ⇒ MET via `(epoch−epoch0)·86400` cancels to ~4e-5 s precision; sub-ms
  MET needs the two-part `JulianDate` (T3).

### T2 — Sun from ephemeris
- `update_sun_light_system` (`lunco-celestial/src/systems.rs`) now points the `DirectionalLight` along
  the **ephemeris** Moon→Sun direction at `clock.epoch` (`-ecliptic_to_bevy(global_position(Moon))`,
  the Sun being the heliocentre), replacing the hardcoded `Vec3::NEG_Z`. Direction math is the pure,
  unit-tested `sun_emit_direction(p_sun, p_moon)`.
- **Single authoritative writer per context** (resolves the old web-build conflict where two systems
  fought the sun every frame): it targets the **brightest** `DirectionalLight` (canonical `pick_sun`
  rule — Earthshine fill ~12 lx ≪ ~128 000 lx, and it dodges the `single_mut()`-with-two-lights trap),
  and **returns early under `NoOpEphemerisProvider`** (every position ZERO ⇒ degenerate), so sandbox /
  no-ephemeris contexts keep dynamic manual `SetEnvironmentLight` (yaw/pitch) control. The ephemeris is
  authoritative only when a real provider (`lunco-celestial-ephemeris`) is installed.
- The terrain `sun_dir` shader uniform follows for free: `pick_sun`/`wire_terrain_materials`
  (`environment/src/horizon.rs`) derive it from the light's world transform, and `compute_local_solar`
  (co-sim solar) reads the same light — so one write propagates everywhere.
- Tests: `sun_dir_tests` (degenerate→None, unit-length & points-away-from-sun, tracks-Moon-position)
  green; the `celestial_integration` test exercises the system live via the stub provider.
- *Residual:* uses the Moon (301) as observer — Earth/EMB differ by ≲0.15° (negligible for a distant
  directional light); a per-camera-body choice is a later refinement. The light is assumed to live in
  an inertial (non-rotating) frame, so local rotation ≈ world (true for the solar/big_space root).

### T3 — Time scales + sidereal
- **New `lunco-time/src/scales.rs`** wraps `celestial-time` (zero-dep pure-math crate) behind plain
  `f64`/radian projection fns — the rest of the workspace never imports `celestial-time` and never
  re-derives JD↔UTC. The master epoch is **TDB**; `TimeScales::from_tdb_jd(tdb)` derives
  `{tt,tai,utc,ut1}_jd` + `gmst_rad` via `TDB→TT→TAI→UTC` (leap table) + `GMST::from_ut1_and_tt`.
  `WorldTime::scales()` / `WorldTime::utc_string()` expose them on the derived view.
- **Seed conflation fixed:** `get_default_celestial_clock` (`clock.rs`) now seeds via
  `scales::utc_now_tdb_jd()` (`Utc::now()` → JD(UTC) → TAI → TT → TDB) instead of treating
  `Utc::now()` as a JD directly (was ~69 s = TT−UTC early, with no leap seconds).
- **Consolidated the 3 drifted `jd_to_utc_string` copies** into the one `scales::tdb_jd_to_utc_string`:
  `clock.rs`, `lunco-celestial/src/ui/mod.rs`, and `lunco-ui/src/mission_control.rs` now all delegate
  (the last two had treated the master epoch as UTC anchored at J2000 — one even truncated to whole
  days). `lunco-ui` gained a direct `lunco-time` dep so it reuses the spine rather than reaching
  through celestial.
- *Tests (green, `cargo test -p lunco-time --lib`):* TDB−UTC ≈ 64.184 s at J2000-era; UTC→TDB→UTC
  round-trip < 1 ms; the TT−TAI=32.184 s / TAI−UTC=37 s ladder; GMST valid + advancing at the
  sidereal rate (+1.0027379 h per solar hour).
- *Residual:* `UT1` uses `DUT1 = 0` (no Earth-orientation data wired) — UT1 ≈ UTC to < 0.9 s, GMST good
  to ~15″; wiring real EOP/DUT1 is a follow-up. The default mission epoch is `lunco-time`'s
  `J2000_JD` constant, used until `seed_mission_clock_from_wall` re-anchors at startup.

### T4 — Epoch-anchored runs + MET + bake *(planned)*
- `RunBounds` gains optional `epoch0`; `lunco-experiments` writes `anchor` at run start and **bakes**
  run outputs to USD `timeSamples` with absolute timestamps. Sim outputs + telemetry timestamp in
  absolute time, coherent with the environment.
- *Test:* short Interactive sim anchored at a date → output timestamps and sun angle co-move.

### T4.5 — Replicate the transport (multiplayer correctness gate) *(planned)*
- Host-authoritative pause/rate/warp + anchor re-anchor events on the wire (commands + a replicated
  `TimeTransport`/anchor). Required before any networked time control. Small but a correctness gate.

### T5 — TimeDomain tree + animation system
- **The sampler** (`lunco-usd-bevy`): `UsdAnimated` marker stamped at instantiation when any
  channel is animated (`prim_is_animated`: xform op, `visibility`, geom `primvars:displayColor`, or a
  bound shader's `inputs:diffuseColor`/`inputs:opacity`); `sample_usd_animation` (in `Update`,
  `.after(DomainResolveSet)`, before `PostUpdate` propagation) resolves each entity's clock and
  evaluates the animated channels via `read_vec3_f64_at` (`openusd::usd::evaluate`, held/linear),
  writing `Transform` + `Visibility` (token, held). The whole local transform is decoded by one
  shared stack, `local_transform_at`: authored **`xformOpOrder`** is honored exactly
  (`compose_xform_order_at` — op order + `!invert!`, matching openusd's row-vector `S·R·T`), else a
  full `xformOp:transform` matrix (`read_matrix_transform_at`), else the implicit piecewise translate
  + rotation + scale. Rotation covers **every** USD channel (`local_rotation_at`: all six Euler orders
  `rotateXYZ`…`rotateZYX`, the slerped quaternion `xformOp:orient` incl. half-precision `quath`, and
  single-axis `rotateX/Y/Z`). The same helpers back the static load decoder (`read_transform_from_usd`
  + the instantiate path), so static and animated transforms agree. `sample_usd_material_animation`
  (sibling system) writes animated base-color / opacity into the entity's **`PbrLook`** — the
  render-free appearance intent ([`render-decoupling.md`](render-decoupling.md)); `lunco-render-bevy`
  is the only crate that turns that into a `StandardMaterial`. Per-channel gated — static channels
  keep their instantiated pose.

  > **Why the animated prim opts out of material sharing.** `PbrLook` materials are cached by
  > *content*, so identical-looking prims share one handle. A prim whose `displayColor` is animated
  > re-keys **every frame** — which would mint a fresh material per frame and free none, an unbounded
  > leak that presents as a slow memory climb rather than a crash. Animated prims therefore carry the
  > explicit `unshared` opt-out and own their material outright.
- **Composed `timeSamples` reach the runtime** (`lunco-usd-bevy/src/compose.rs`):
  `flatten_stage` now copies each attribute's composed `timeSamples` (via `Attribute::time_samples()`)
  alongside its `default`, and stamps `timeCodesPerSecond` on the pseudo-root. Previously flatten kept
  only the default-time value, so animation worked **only** for single-layer `usda::parse` stages —
  composed/referenced assets (the asset-loader path) silently lost their samples. PCP retimes the
  samples through any sublayer/reference `LayerOffset` inside `time_samples()`, so the **`LayerOffset`
  chain is composed for free**.
- **`timeCodesPerSecond`** (`stage_time_codes_per_second`, default 24 per USD spec): the
  samplers map resolved seconds → time codes (`code = seconds * tcps`) instead of assuming `tcps=1`.
- **`RigidBody::Kinematic` on animated bodies** (`lunco-usd-avian`):
  `enforce_kinematic_on_animated` demotes a `Dynamic` body to `Kinematic` when its prim is also
  `UsdAnimated`, so the sampler's `Transform` writes don't fight Avian's integrator.
- **The clock tree** (`lunco-time/src/domain.rs`): `TimeDomain` (parent + `(offset, scale)` +
  regime) + `Playback` (driven playhead: head/mode/rate/range/loop) components; `TimeBinding` on
  entities (absent → world domain). `advance_and_resolve_domains` (in `DomainResolveSet`, `Update`)
  advances driven heads by the world delta and resolves every domain's `local_t` into
  `ResolvedDomains`; the sampler reads it via `domain_time`. **Derived** domain `scale=100` = the
  factory at 100×; **driven** domain = per-object replay (seek/play/loop). "New domain from
  selection" = `spawn_driven_domain` + add `TimeBinding` to the set. Pure resolution math
  (`derived_local_t`/`step_playhead`/`resolve_snapshot`) is headless-tested (16 tests green
  `cargo test -p lunco-time --lib`; 3 sampler read-path tests green in `lunco-usd-bevy`).
- **Planned refinements:** thread the fixed-step `overstep_fraction` for render-rate smoothing;
  driven-under-driven head advance (driven heads currently advance on the *world* delta, not a driven
  parent's delta); animate emissive / metallic / roughness (only base-color + opacity wired so far).
- **Animation transport (T7 for the preview domain):** a singleton `AnimationPreview` driven
  domain (`lunco-time`, spawned by `TimePlugin`) that USD-animated entities auto-bind to
  (`bind_animated_to_preview` in `lunco-usd-bevy`, `Added<UsdAnimated>` + `Without<TimeBinding>`). It
  advances with the sim while playing, but its `Playback` is paused / seeked / rate-scaled by the
  `ControlAnimation` command (headless, API + MCP: `{"command":"ControlAnimation","params":{...}}`)
  and the Inspector **Animation** section — without touching the physics clock (`TimeTransport`).
- **Planned authoring UX:** domain-per-project wiring; selection→domain command (bind an arbitrary
  selection to its own scrubbable domain — the preview domain is the global default).

### T6 — Tween / state-machine behavior layer *(planned)*
- Thin in-house tween (eased 2-key) + object state machine, **evaluated on a `TimeDomain` playhead**,
  with **bake-to-`timeSamples`** for the recordable/scrubbable path. No parallel clock.
- *Test:* a tween on a domain produces the same samples as the equivalent baked `timeSamples`;
  pausing the domain pauses the tween.

### T7 — Transport UI + preview-scrub (feeds ConOps)
- **Animation preview transport:** the `ControlAnimation` command (headless,
  API + MCP — `{"command":"ControlAnimation","params":{"playing"|"seek_secs"|"rate"}}`) and the
  Inspector **Animation** section drive the singleton `AnimationPreview` `Playback`
  (play/pause/scrub/rate), scrubbing authored USD animation independently of the physics clock. Scrub
  range tracks the bound clips' authored span (`animated_time_range`).
- **Planned:** a global transport widget with UTC/MET/epoch readout reading `TimeTransport`;
  per-domain controls; step. Entry point for the timeline/ConOps doc.

### Later (not scheduled)
- SGP4/TLE constellations; SPICE-kernel frames; moon-phase/eclipse/comms-pass lanes (consume GMST +
  ephemeris); Tier-3 co-sim communication points when subsystems actually couple at different rates.

---

## 9. Migration notes / invariants recap

- **Netcode determinism preserved:** replication stays on integer `SimTick`; absolute time is derived,
  never sent; `epoch0`/`tick0` are host-authoritative session state (constant except on re-anchor).
- **No new wall-clock dependency on the sim path:** wall time seeds `epoch0` once and drives only the
  non-deterministic warp-preview view — never per-frame sim logic.
- **One funnel, one master, many rooted clocks.** If a new clock can't name its parent + `(offset,
  scale)` + regime, it doesn't get to exist.

---

## 10. Reuse & Bevy Mapping

### 10a. Reuse — most of the substrate already exists

| Need | Already present | Verdict |
|---|---|---|
| Master tick + `SECS_PER_TICK` | `lunco-core` `SimTick`, `FIXED_HZ` | reuse; add `MissionClock`/`TimeTransport`/projections in `lunco-core/time.rs` |
| Calendar + **all time scales** + GMST + leap table | **`celestial-time`** (dep): `TAI`/`TDB`/`UT1`/`UTC`, `to_utc`/`to_tai`, `TAI_UTC_OFFSETS`, `sidereal` | reuse; **no `hifitime`** |
| Two-part precision | `celestial-time::julian::JulianDate(day, frac)` | reuse |
| Sun/body from epoch | `celestial-ephemeris` + `ephemeris_update_system` (change-gated) | reuse; the pure-consumer template |
| Curve eval + keyframe ops | `openusd::usd::evaluate`, `SetTimeSample`/`RemoveTimeSample`, `attribute_value_at` | reuse |
| Per-clock affine retime | `openusd LayerOffset` (applied to `timeSamples`, verified) + `start/endTimeCode` + `timeCodesPerSecond` | reuse; the clock-tree edges |
| Bake substrate | `lunco-modelica` `run_stepping_loop` + dense-output decimation; `RunBounds` | reuse; T4 routes output → `SetTimeSample` |
| Recording / replay | `lunco-twin-journal` | reuse |
| Time-unit display | `lunco-axes-and-units` | reuse |

**Do not adopt:** `bevy_animation` (not in build; immutable clip assets + glTF-shaped targets fight
live-authored USD), `bevy_tweening`/`bevy_easings`/`seldom_state` (each ships its own clock → the
floating-clock antipattern). Build the thin tween/FSM **on a `TimeDomain`**; reuse only easing math
(`bevy_math::curve` `Animatable`, available without `bevy_animation`).

### 10b. How it falls into Bevy

**Resources** (`lunco-core`): `SimTick` (exists) + `MissionClock` + `TimeTransport` (new, small,
reflected). Projections = pure free fns.

**The clock tree — `Time<T>` for the *few*, ECS data for the *many*:** `Time<T>` needs a
**compile-time marker**, so it models only a *fixed, small* set of standing contexts (World, maybe a
couple). It **cannot** model dynamic per-object/per-selection/per-project domains — you can't mint
marker types at runtime. So **driven domains are ECS data**: a `TimeDomain` component (parent +
`(offset, scale)` + regime + playhead) on a domain entity, with a `TimeBinding` relation from animated
entities. Derived domains store nothing (pure read). ("New domain from selection" = spawn a component.)

**Schedules:**
- `First`/`PreUpdate` — advance `MissionClock`/transport (derive, don't accumulate).
- `FixedUpdate`/`FixedPostUpdate` — causal layer (`SimTick`, avian, Modelica, cosim),
  `run_if(regime == RealtimePhysics)`.
- `Update` — advance driven-domain playheads + the domain-aware sampler (with `overstep_fraction`).
- `PostUpdate` — transform propagation (incl. big_space), then pure consumers (ephemeris, lighting,
  sidereal); seek = projection changes → recompute.

**avian:** physics on `Time<Fixed>`; `regime` gate = pause `Time<Virtual>` / run condition;
`rate` = `relative_speed`; animated prims `RigidBody::Kinematic`; one shared `overstep` alpha with
`PhysicsInterpolationPlugin` (no double-smoothing).

**Networking:** only `SimTick` on the wire; `TimeTransport`/anchor are host-authoritative replicated
state via the existing lightyear command path.

---

## 11. Amendment (2026-07-14) — rooted clock tree, celestial as a clock, USD-authored celestial

§3d specified the clock tree correctly. What was *built* (`lunco-time/src/domain.rs`) implements the
affine node and the driven playhead, but roots every chain on `WorldTime.sim_secs` — the tick-derived
clock, which stops dead on a pause. Consequences, all observed:

- **Nothing can outlive a pause.** Pausing the sim froze the planets (the epoch is tick-derived) and
  would have frozen the avatar too — which is why `lunco-avatar` reaches around the clock system
  entirely and reads raw `Res<Time<Real>>` (`apply_fly`), plus a bespoke `spring_arm_paused_system`
  duplicated for the paused case. Every one of those is a symptom of a missing wall-rooted clock.
- **The celestial epoch is not a clock at all.** `WorldTime.epoch_jd` is computed straight from
  `MissionClock.anchor` + tick, bypassing the tree, so it cannot be re-parented, rate-scaled or seeked.
- **`ResolvedDomains` stores only `t`, no `dt`** — so no movement system can be driven by a domain
  (they all need a delta), which is the other half of why they reach for `Time<Real>`.

### 11a. Pause propagation is free — do not add a flag

The tree already gives the requested semantics with **no `paused` bit and no propagation pass**:

```
child_t = offset + scale · parent_t
```

If a parent stops advancing, `parent_t` is constant, so `child_t` is constant. **A frozen ancestor
freezes its whole subtree, structurally.** This is how USD `LayerOffset` and every DCC time-warp
works, and it is why the tree must be the *only* mechanism — a second "is paused" flag propagated
down the tree would be a redundant, desynchronisable copy of information the map already carries.

It also answers *"unpause only the celestial clock while physics stays paused"*: that is not a flag,
it is **a re-parent**. A clock is frozen because of where it hangs; move it somewhere that is
running. One operation, `SetClock { clock, parent }`, covers pause-subtree, unpause-one, and
time-dilate-one.

### 11b. The shape: two roots, siblings — NOT physics → celestial

```
  real ─ wall clock (Time<Real>). Never pauses. Non-deterministic by construction.
   └── interaction ......... avatar fly, camera smoothing, UI easing
  sim ── tick clock (SimTick ← TimeTransport). Deterministic, replicated, seekable.
   ├── physics ............. avian Time<Physics>
   ├── celestial ........... the epoch (epoch_jd = epoch0 + celestial_t/86400)
   └── <animation domains> . per-object / per-selection driven playheads
```

Celestial hangs off **`sim`, as a sibling of `physics`** — not downstream of it. Chaining
`physics → celestial` (the intuitive reading of "physics pauses celestial down the line") would be
wrong: it says the planets' motion is *caused by* the rigid-body solver, so a physics readiness hold
(a heightfield still baking!) would stop the solar system. They are siblings that happen to share a
parent; pausing `sim` freezes both because they are both its children, which is the behaviour wanted,
for the right reason.

Detaching celestial = `SetClock { clock: celestial, parent: real }`. It now runs on wall time while
the sim is paused. Re-parent back to `sim` to re-couple. Same operation seeks it:
`SetClock { clock: celestial, epoch_jd: … }`.

**Default is unchanged behaviour**: `celestial = derived(parent: sim, offset: 0, scale: 1)`, so
`epoch_jd` remains exactly the tick-derived value it is today — deterministic, network-safe,
"derive, never accumulate" intact. Independence is opt-in and explicit.

### 11c. Per-body physics pause is NOT a clock — it is `RigidBodyDisabled`

The request "pause physics for some avian bodies but not others" **must not** be built as per-body
clocks. avian has exactly one `Time<Physics>`, and that is not an implementation limit — it is
physics: two bodies in contact share an island, a solver iteration and a contact manifold. Stepping
body A at 1× and body B at 0× *inside one solver* has no meaning; the constraint between them would
be integrated with two different `dt`s.

The correct per-entity primitive already exists and is already used here
(`lunco-usd-sim/src/cosim.rs:1349`): **`avian3d::RigidBodyDisabled`** (+ `ColliderDisabled`). Add it
to freeze one body, remove it to resume — no clock involved.

> **The rule:** *clocks scale and gate TIME (global, tree-structured). `RigidBodyDisabled` gates one
> BODY.* Anything that wants "this object stops but the world runs" is a body/animation concern, not
> a clock concern. Keeping these separate is what stops the clock tree from growing into a
> general-purpose "disable stuff" mechanism.

### 11d. Where it goes in the update cycle

One resolve per frame, in `PreUpdate`; everything downstream *reads* the resolved sample and nothing
recomputes a clock:

```
PreUpdate
  TimeSpineSet          advance_world_clock        SimTick+TimeTransport → WorldTime, Time<Virtual>
                        resolve_clocks             walk the tree once → ResolvedClocks{ t, dt }   ← after the spine
  ClockApplySet         apply_physics_clock        celestial/physics nodes → Time<Physics> (pause/scale)
                        write_epoch_from_celestial WorldTime.epoch_jd ← celestial clock
  CelestialEpochSet     ephemeris → body rotation → site anchor        .after(TimeSpineSet)
FixedUpdate             SimTick advance; avian solver                   (frozen ⇒ zero delta, never runs)
Update                  gameplay; animation sampler                     .after(DomainResolveSet)
PostUpdate              avatar/camera on the INTERACTION clock's dt; then transform propagation
```

`resolve_clocks` must run **after** `advance_world_clock` (the `sim` root's `t` is written there) and
**before** any consumer. The wall root reads `Time<Real>` in the same system. Cycles/missing parents
are depth-capped and fall back to the root, as today.

`WorldTime.epoch_jd` is retained as the **derived read-only view**, now written *from* the celestial
clock instead of directly from the tick anchor. That keeps the ~15 existing `WorldTime.epoch_jd`
readers in `lunco-celestial` untouched — the clock becomes the source, the view stays the interface.

### 11e. Celestial is authored in USD, not enabled by a code flag

`CelestialConfig.spawn_hierarchy` is a hidden boolean that decides whether a solar system exists, and
`enable_celestial_on_site_anchor` flips it as a side effect of a scene authoring a site anchor. That
is a code-side switch for what is really *scene content*, and it has already produced one bug: the
trajectory layer ignored the flag and spawned Earth/Moon orbit views into every scene, including a
sandbox that had asked for no celestial content at all.

**Bodies become USD prims.** Nothing celestial exists unless the scene says so:

```usda
def Xform "Sun"   (prepend apiSchemas = ["LuncoCelestialBodyAPI"]) { int lunco:body = 10  }
def Xform "Earth" (prepend apiSchemas = ["LuncoCelestialBodyAPI"]) { int lunco:body = 399 }
def Xform "Moon"  (prepend apiSchemas = ["LuncoCelestialBodyAPI"]) { int lunco:body = 301 }
```

with a reusable `assets/celestial/solar_system.usda` that a scene pulls in by `references` when it
wants the standard set. The default sandbox references nothing ⇒ no Sun, no Earth, no Moon, no orbit
views, no ephemeris — which is the correct reading of "the sandbox is a flat test arena".

The `big_space` precision scaffolding (grids, SOI, `GravityProvider`, `GlobeLod`, reference frames)
stays in code — it is derived structure, not authored content — but it is **built from the declared
bodies** rather than from a boolean: no body prims, no hierarchy. `spawn_hierarchy` and
`enable_celestial_on_site_anchor` are deleted.

### 11e-bis. CADENCE ≠ CLOCK — the rule for choosing a schedule

Two orthogonal axes, and the code kept using one as a proxy for the other:

- **Cadence** — *which schedule* a system runs in. `FixedUpdate` (deterministic, one run per
  tick, stops on pause) vs `Update`/`PostUpdate` (once per rendered frame, never stops).
- **Clock** — *which time it integrates against* (`sim`, `celestial`, `interaction`, a driven
  domain). §11a–b.

The symptom: `spring_arm_system` was placed in `FixedPostUpdate` **to obtain a constant `dt`** —
choosing a *schedule* to get a *clock property*. And because `FixedUpdate` stops when the sim
pauses, a second render-rate copy (`spring_arm_paused_system`) had to exist for the paused case.
One behaviour, two systems, gated on the transport: that duplication is what a cadence/clock
conflation always produces.

**Two cadences, split by what they are FOR:**

| System mutates… | Cadence | `dt` | Pauses? | Rate-scales? |
|---|---|---|---|---|
| replicated **sim state** (physics, tick, cosim, netcode) | `FixedUpdate` — the fixed step **is** the tick | `SECS_PER_TICK` | yes | yes |
| **presentation** (avatar, cameras, UI easing, HUD) | `InteractionSchedule` — wall-rooted | constant (120 Hz) | **never** | **never** |

`InteractionSchedule` (`lunco-time/src/interaction.rs`) is a second stepped cadence, drained from
`Time<Real>`. Inside it the generic `Time` **is** the interaction clock — the same contract `Time`
is `Time<Fixed>` inside `FixedUpdate` — so systems just read `Res<Time>` and get the constant step.
There is **no path from `TimeTransport` into it**, which is what makes it unpausable *by
construction* rather than by a guard someone has to remember to write.

**Why the sim keeps Bevy's cadence** (all three verified in the dependency sources, not assumed):

1. **Rate is sub-stepping, and must be.** `sim_secs = tick × SECS_PER_TICK` — a tick is a *fixed
   size*, so 8× means eight ticks per wall-frame, never one tick of 8× the `dt`. avian's
   `Time<Physics>::relative_speed` does the latter: `run_physics_schedule` computes
   `timestep = driving_schedule.delta × relative_speed` (`avian3d/schedule/mod.rs:242`) — a 133 ms
   solver step at 8×, and it breaks the tick↔seconds invariant. "More fixed runs per frame" IS the
   mechanism, and that is exactly what `Time<Virtual>`'s `relative_speed` buys.
2. **Netcode is keyed to it.** `SimTick` is *defined* as one tick per fixed step (`lunco-core`), and
   prediction/rollback/input-recording are built on that 1:1 — as is lightyear's own tick manager.
3. **avian's smoothing is keyed to it.** `bevy_transform_interpolation` captures start/end in
   `FixedFirst`/`FixedLast` and eases on `Time<Fixed>::overstep_fraction()`. Move `PhysicsSchedule`
   off the Bevy fixed loop and every body's smoothing silently breaks.

**What IS driven from the time system:** avian's *pause*. `run_physics_schedule` gates on
`Time<Physics>::is_paused()` (`:237`), which is precisely what `lunco_physics::PhysicsHolds` writes.
That is the half of "control avian from time, not from the schedule" that genuinely belongs there.

Corollary — the camera stopped needing avian's `TranslationInterpolation`/`RotationInterpolation`.
Those eased a camera *written at the fixed rate* between fixed samples; a camera written on the
120 Hz interaction step is already ahead of the display, and keeping them would be a lerp of a lerp
— a lag source, not a smoothness source. The runner sits in `PostUpdate` **after** avian's writeback
and after `bevy_transform_interpolation` has eased every body into its render pose, so a camera
following a rover reads the body's *smoothed* pose.

> **Do clocks need their own step?** A clock is *continuous* (sampled per frame; `dt` varies) by
> default. Two are stepped, and only two: the `sim` clock (step = the tick — determinism and
> rollback are built on it) and the `interaction` cadence (step = constant, so presentation
> integrators have a stable `dt`). A cosim participant with its own communication step (Modelica,
> §11f) is `DomainRegime::Causal` and would be a third, bounded by co-sim communication points.
> Do **not** give every clock a step "for symmetry": each stepped cadence is an accumulator, a
> phase relationship, and an interpolation policy that someone has to reason about.

### 11f. A Modelica experiment is a driven domain (not a new concept)

An experiment's internal time (`t_start … t_stop`, its own `dt`) needs no separate
machinery: it is a **driven domain** — a clock node with a `Playback` head over a bounded
range. That gives, for free:

- **run it coupled** — parent `sim`, scale 1: the experiment advances with the world;
- **run it fast** — `scale = 100` ("speed only the factory", §3d);
- **replay a finished run while the world keeps moving** — `Playback { head, start, end }`
  scrubbed independently, parent still `sim`;
- **run it while the sim is paused** — re-parent to `real`.

"Binding sim time to experiment time" is then just a `TimeBinding` from the experiment's
entities to that domain — the same relation the USD sampler and per-selection scrub already
use. The thing to resist is a second, parallel "experiment clock" type: it would need its own
pause/rate/seek semantics, and they would drift from these.

Caveat (§5): an experiment *integrates*, so it is `DomainRegime::Causal` — its rate is bounded
by solver stability and co-sim communication points, unlike a `Kinematic` (baked `timeSamples`)
domain, which can be rate-scaled freely.

### 11g. Phasing

1. **Clock tree** (`lunco-time`): wall root + tick root as real clock entities; `ResolvedClocks{t,dt}`;
   well-known handles (`real`, `sim`, `interaction`, `celestial`); `SetClock` command (journaled +
   replicated — it is world state, per §6).
2. **Celestial as a clock**: `epoch_jd` written from the celestial node. Behaviour-identical default.
3. **Interaction clock**: `lunco-avatar` binds to it; deletes the raw `Time<Real>` reads and the
   duplicated `spring_arm_paused_system`.
4. **USD-authored celestial**: `LuncoCelestialBodyAPI`, `solar_system.usda`, delete the flag.
