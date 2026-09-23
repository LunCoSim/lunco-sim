# 19 — Unified Time, Clocks & Animation

> Status: Active · Audience: contributors working on time, simulation clocks, and animation

`lunco-time` owns the mission-time spine. It provides one transport authority,
the deterministic simulation tick, calendar projections, a rooted clock tree, and
the animation transport used by USD and editor surfaces.

## 1. Master time and transport

`SimTick` is the deterministic master for the causal simulation. `TimeTransport`
is the single internal authority for play/pause and rate; UI, API, and input
surfaces dispatch its typed command rather than maintaining another pause or rate
state.

The live transport exposes one bounded causal ladder: `0.1x, 0.25x, 0.5x, 1x,
2x, 4x, 8x, 16x, 32x, 64x`. Every accepted positive rate advances the
deterministic fixed-step world; pause is a separate transport mode and higher or
lower rates are rejected. The fixed timestep remains unchanged, so 32x and 64x
perform more solver iterations per render frame rather than switching to a
second time meaning. A presentation-only celestial clock can use its own
`SetClock` scale when a larger rate is useful.

`WorldTime` is a derived view. Calendar and Julian-date values are computed from
the transport anchor and tick; they are not independently accumulated by a
consumer. A re-anchor is an explicit transport event and must remain
host-authoritative in a networked run.

Fixed-step producers use the shared `SimTickSet` ordering anchor. Telemetry
records the tick and the `MissionClock` seconds derived from it, so collection
does not use a render or wall-clock accumulator.

`CelestialTime` is a separate presentation projection of the celestial clock.
It normally equals `WorldTime.epoch_jd`, but a `SetClock` re-parent onto `Real`
may advance it while the causal simulation is paused. Only explicitly
presentation-owned celestial consumers may read it (for example detached globe
imagery and the rendered sky). The physical solar provider, active surface
frame, body state, and other causal consumers read `WorldTime` and must never be
driven by `CelestialTime`. When the two epochs diverge, a typed render-only Sun
direction from the celestial presentation drives the scene key light. Its
finalized `SunRenderState` continues through the existing directional-shadow,
terrain-material, and horizon-cache consumers. The detached clock therefore
changes visible illumination and shadows without changing `SunState`, physics,
or co-simulation inputs. Static scenes continue to render from `SunState`.
The sky clock UI displays this same `CelestialTime` epoch, so its date readout
tracks the globe and sky while the independent rate is active. Its per-frame
delta comes from the resolved celestial clock sample, not from subtracting
Julian-date projections; a step longer than one day is valid at presentation
rates such as 100,000×.

Rendered globe imagery is also presentation-owned. Earth and Moon globe tiles
are placed under dedicated visual celestial frames that consume
`CelestialTime`; the physical body, picking collider, surface terrain, and
active BigSpace/Avian frame remain in the `WorldTime` hierarchy. This is the
required boundary for a high-rate sky time-lapse: it moves the visible globe
without teleporting the simulated body or invalidating the surface scene. When
the active camera remains on a physical surface, the render-only hierarchy is
rigidly mapped so the CelestialTime body-fixed observer coincides with that
WorldTime camera; relative celestial positions and body rotations still follow
CelestialTime. This keeps Earth and the Sun in the right local sky as the two
epochs diverge, without moving terrain, stations, links, or physics.
The procedural Sun disc direction is published in the active camera's view
coordinates, matching the starfield's view-space rays; camera motion also
refreshes that direction while the celestial clock is paused.

An active celestial presentation requests the shared bounded realtime frame
cadence while its clock advances. Focused rendering remains vsync-paced, and
unfocused rendering uses the same fixed 60 Hz cadence as other realtime work;
celestial animation does not force an unbounded max-speed render loop.

The rate ceiling and fixed-step catch-up budget live in `lunco-time`; consumers
must not add another rate path or silently drain an unbounded fixed-step burst.
The canonical labels are supplied by `lunco-time` so fractional slow-motion
rates are rendered consistently by every UI.

## 2. Clock tree

A `TimeDomain` is an affine child:

```text
local_t = offset + scale * parent_t
```

Root clocks are the only places raw time enters the tree:

- `Tick` reads the deterministic simulation time and freezes with the sim;
- `Wall` reads `Time<Real>` and is non-deterministic but never pauses;
- `Epoch` exposes the mission/calendar projection used by celestial consumers.

The well-known `Clocks` resource publishes handles for `real`, `sim`,
`interaction`, and `celestial`. `interaction` is wall-rooted for avatar and
camera presentation. Physics and celestial time are separate concerns; a physics
readiness hold must not accidentally become a solar-system clock dependency.

A derived domain follows its parent. A driven domain adds `Playback` with its
own playhead, range, rate, loop, seek, and user pause state. `TimeBinding` attaches
an entity to a domain. Per-object, per-project, and preview playback therefore use
one mechanism rather than separate clock types.

`ResolvedDomains` contains the resolved `{ t, dt }` sample for each domain once
per frame. Clock arithmetic is pure and unit-testable; Bevy systems only advance
the roots, resolve the tree, and publish the sample.

## 3. Animation funnel

All authored animation follows one path:

```text
TimeDomain / Playback
        -> authored value source
           (USD timeSamples or a live time-domain driver)
        -> projection / write
```

`lunco-usd-bevy-animation::UsdAnimationPlugin` binds ordinary USD animation to the
`AnimationPreview` driven domain and writes the supported visual channels. The
preview domain can play, pause, seek, rate-scale, and loop without touching the
physics transport.

Camera paths are a separate live driver over the same domain machinery. A
`UsdGeomBasisCurves` path owns a driven domain and a `Playback`; its explicit
`CameraPathTransport` command controls that path. See
[`51-cinematic-camera.md`](51-cinematic-camera.md).

Pure tweens and state machines may be authored as behavior over a domain. They
must not introduce a second playback clock. If a result must be recorded or
scrubbed as authored animation, bake it through the USD operation path.

## 4. Coupling and rates

`DomainRegime` distinguishes the reason a domain exists:

| Regime | Meaning |
|---|---|
| `Kinematic` | pure function of time; may seek and rate-scale freely |
| `Causal` | integrates state; rate is bounded by solver stability and communication points |

The classification is informational for current animation consumers but is the
boundary for future Modelica/co-simulation domains. A causal participant must
advance at its declared communication point; it must not be made a kinematic
playback shortcut merely to simplify UI controls.

## 5. Pause, re-parenting, and bodies

### 5.1 Pause propagates structurally

There is no second `paused` flag to propagate through the tree. If a parent stops,
its resolved time stops and every child follows. Running one clock while another
is paused is a `SetClock` re-parenting operation, not a special-case branch.

### 5.2 A body is not a clock

Per-body physics suspension uses the authored/runtime body mechanism such as
`RigidBodyDisabled` and `ColliderDisabled`. It does not create a clock per body:
contact islands and one solver step require a coherent physics cadence.

The required `physics.body_escape` Rhai policy uses this same boundary. Its
application default disables the affected dynamic joint island and its
colliders for both finite world exits and non-finite position or velocity,
leaving other bodies and the simulation clock running. A Twin may author a
world-wide physics hold when that is its intended response; missing or invalid
policy results remain visible safety faults.

### 5.3 Cadence is not clock

The schedule answers how often a system runs; the domain answers which time it
reads. Causal simulation runs on the fixed schedule. Embodiment, camera, and UI
presentation use `InteractionSchedule` and `InteractionEased`, so their stable
presentation cadence does not become a second simulation clock.

## 6. Networking and determinism

Only deterministic simulation state and its authoritative transport decisions are
network state. A local animation-preview seek is a presentation decision, not a
physics tick. A camera path's clock is local unless its authored shot is part of
the shared scene contract; the path still uses the same explicit domain and
transport rules.

Physics determinism has a second admission requirement beyond the fixed clock:
the Avian compute order must be pinned. The production headless/server builder
uses one compute thread and publishes `PhysicsDeterminism { deterministic: true }`;
`clock_snapshot()` exposes `physics_contract_ok`, the admission error (if any),
and the selected `physics_compute_threads` to Rhai. A multi-threaded pool is still
available only as an explicit diagnostic/divergence configuration and reports
`physics_deterministic: false`. Fixed `dt` alone is not evidence of a
reproducible contact solve. If the admission resource is absent,
`clock_snapshot()` records a `physics-determinism-missing` runtime fault rather
than silently treating the world as deterministic.

Never replicate a private floating-origin cell/local split as the time contract.
Coordinate projection and time authority are separate boundaries.

## 7. Invariants

1. Store one causal master (`SimTick`); derive calendar and consumer views.
2. Every derived or driven domain names its parent and its `(offset, scale)`.
3. Every independent playhead is a `Playback` on a `TimeDomain`, not a parallel
   resource or a boolean flag.
4. Causal state advances only on the causal cadence; kinematic preview can seek.
5. The USD sampler is the one generic authored-animation funnel.
6. A new clock must state its root, coupling regime, pause behavior, and owner.
7. A consumer reports its resolved time/delta rather than reading a raw clock to
   bypass the spine.

## 8. System mapping

The production split is:

| Layer | Owner |
|---|---|
| master tick, transport, anchor, `WorldTime` | `lunco-time` / `lunco-core` |
| domain tree, playheads, bindings, resolved samples | `lunco-time` |
| USD value evaluation and visual projection | `lunco-usd-bevy` and `lunco-usd-bevy-camera` |
| Modelica stepping and communication points | Modelica/cosim owners |
| physics stepping | Avian and the fixed simulation schedule |
| avatar/camera/UI presentation cadence | `InteractionSchedule` |
| calendar scales, sidereal and ephemeris projections | celestial/time consumers |

The API surface is `SetTimeTransport` for the bounded live world and
`ControlAnimation` for the preview or an explicitly addressed driven domain.
Consumers do not invent aliases for either command. `SetClock` is the separate
API for a pure celestial presentation clock, including scales above the live
transport ceiling; it publishes `CelestialTime` and does not overwrite
`WorldTime`.

`SetSimulationExecutionMode` is the separate host-pacing command. `Realtime`
means that the host admits the fixed lattice on a wall-clock cadence;
`MaxSpeed` means that a headless, recording, or test host may run without an
intentional wait. It never changes `TimeTransport.rate`: a run can therefore
be realtime at `1x`, realtime at `4x`, or max-speed with an explicitly authored
transport rate. The host adapter owns the wait (`ScheduleRunner` or Winit),
while a deterministic recorder owns its `TimeUpdateStrategy`; no subsystem may
write both knobs as a shortcut.

## 11. Current clock-tree boundaries

These boundaries are part of the contract and are retained here because they
prevent the most common regressions.

### 11a. Pause propagation is free

`child_t = offset + scale * parent_t`. A frozen ancestor freezes its subtree.
To detach celestial or interaction behavior from a pause, re-parent its clock;
do not add a propagated pause bit or a duplicate paused system.

### 11b. Root placement carries meaning

`real` is for non-deterministic interaction. `sim` is for causal, replicated
world state. `interaction` is the wall-rooted presentation clock. Celestial
placement is explicit and must not be downstream of a physics readiness hold.

### 11e. Celestial content is authored

Whether a scene contains celestial bodies is a USD composition decision. Runtime
code projects declared body prims and their authored references; it does not turn
the solar system on as an incidental side effect of a generic scene or site
anchor. The precision scaffolding and physical constants remain engine-owned
derived state.

### 11e-bis. Cadence is independent of clock

Presentation systems use the interaction cadence and its `InteractionEased`
history. Causal systems use the fixed simulation cadence. A system must not be
duplicated merely because it needs a different pause behavior; put it on the
correct cadence and bind it to the correct domain.

### 5.4 Pause is an admission barrier

`SetTimeTransport { playing: false }` projects to `Time<Virtual>` synchronously
at the command boundary. If the command was raised from inside `FixedUpdate`,
the time spine also discards only the unspent `Time<Fixed>` overstep, preserving
the completed tick without admitting another fixed iteration from the same render
frame. The physics owner mirrors the paused virtual clock immediately before
Avian's solver schedule and zeroes `Time<Physics>`'s delta. This is the pause
contract; a one-tick delay, wall-clock guess, or UI-only flag is not equivalent.

Scene teardown resets the fixed-clock admission state, physics holds and step
debt, simulation tick, and prediction/control buffers before the replacement
scene integrates. A pause explicitly issued while a scene transition is
admitted is carried through that reset and consumed once, so a queued
`restart_scene(); pause();` pauses the replacement rather than being overwritten
by the reset's normal playing default.
