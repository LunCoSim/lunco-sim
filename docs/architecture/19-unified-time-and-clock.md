# 19 — Unified Time, Clocks & Animation

> Status: Active · Audience: contributors working on time, simulation clocks, and animation

`lunco-time` owns the mission-time spine: the fixed simulation tick, transport,
calendar anchor, causal `WorldTime`, the ordinary render interpolation sample,
and one `CelestialTime` sample. `CelestialTime` is an affine child of
`WorldTime`; celestial state and the environment inputs derived from it all use
that same sample.

## 1. Master time and transport

`SimTick` is the deterministic master for causal simulation. `TimeTransport` is
the single authority for play/pause and rate; UI, API, and input surfaces use
`SetTimeTransport` rather than keeping another pause or rate state.

The live transport exposes one bounded ladder: `0.1x, 0.25x, 0.5x, 1x, 2x,
4x, 8x, 16x, 32x, 64x`. Every accepted positive rate advances the fixed-step
world. The fixed timestep does not change, so a higher rate performs more
completed physics ticks per rendered frame.

`TimeTransport` is projected onto Bevy's virtual clock before the fixed loop.
After that loop drains its admitted ticks, `WorldTime` is published from the
latest completed `SimTick`. Its mission seconds, elapsed seconds, and epoch are
derived from `MissionClock`; no consumer accumulates its own calendar time.
`SetMissionEpoch` re-anchors the calendar at the current tick without creating
another running clock.

`CelestialTime` resolves from the published `WorldTime` in `PreUpdate`. Its
single angular-error gate commits the exact celestial epoch consumed by body
placement and rotation, the semantic sun, light and shadow projection, and
celestial geometry queries. It must not commit an epoch newer than the sample
those systems read.

Fixed-step producers use the shared `SimTickSet` ordering anchor. The scripting
set runs after that anchor, so lifecycle hooks and event delivery read the tick
for the step they are changing. Telemetry records the tick and the `MissionClock`
seconds derived from it, so collection does not use a render or wall-clock
accumulator.

Scene time selection is one application policy: `scene.time.select`, installed
by `assets/scripting/policy/startup.rhai` from `policy/index.toml`. Startup
installs the policy; it does not choose an epoch before a scene exists. The USD
scene lifecycle invokes it once at `SceneTransitionCompleted`, after the
composed stage dependencies and queued structural projection have settled.
CPU-generated render meshes may continue streaming because they do not affect
the selected epoch. A completion notification that did not enter the loading phase is
ignored, so an idempotent load of the active scene does not select or apply time
again. Its typed facts include the selected root path, composed epoch API and
value status, celestial-source presence, and a fresh computer UTC→TDB candidate.
Rhai selects a valid non-zero root `lunco:time:epochJd` when present; otherwise
it selects current computer time. Rust verifies that the returned epoch matches
one of those candidates and passes the result to the time owner. The hook call
uses `Twin/Lifecycle/Preparation`, keyed by that exact `SceneTransitionId`, with
no elapsed clock. The deferred `SceneTimeSelection` carries the same id; the
time owner ignores an apply, failure, or clear edge when it belongs to an older
transition. One-shot policy inspection uses its separate
`Application/Repl/Evaluation` context.

Until the selection is installed, `SceneTimeState` holds the physical fixed
loop and gates USD time-sample animation, celestial placement/presentation, and
USD DEM terrain construction. Applying the selection resets `SimTick`, the
mission calendar, and clock-domain samples. In the following `PreUpdate`, the
terrain bridge resolves authored DEM prims and the simulation assembly records
their exact progress holds before `TimeSpineSet` can admit a fixed tick. This
includes Twin manifest scans and downloads that have not produced a terrain
request yet. Pending DEM data, collider construction, and the browser worker's
full result keep `SimTick`, Rhai simulation hooks, Modelica, and Avian held
while render and UI schedules continue. Physics readiness then covers
fixed-step body and joint admission.
`ResetTime` uses the retained selection and never invokes the policy. This
keeps a replacement scene from consuming the outgoing scene's epoch while its
assets and projections are still arriving. A missing epoch warns when celestial
sources are present; an invalid `LunCoEpochAPI` value always warns. Both select
current computer time, and the `epoch-api-missing-time` lint reports the issue
before runtime. A valid root epoch makes celestial scenes repeatable across
launches.

The hook owner is `lunco-time`; its typed input is the settled scene event, root
path, authored epoch status/value, celestial-source presence, and sampled
computer time. Its output is a source, the selected candidate, and an optional
warning. A missing or invalid policy result faults the application and leaves
time-dependent consumers held. The production Rhai hook test checks both
selections; `artemis_motion` and `lint_selftest` cover authored and
computer-time scene behavior through the production scene-test runner.

The rate ceiling and fixed-step catch-up budget live in `lunco-time`; consumers
must not add another rate path or silently drain an unbounded fixed-step burst.
The canonical labels are supplied by `lunco-time` so fractional slow-motion
rates are rendered consistently by every UI.

## 2. Physical-time presentation sample

Render-only consumers read `SimulationPresentationTime`, published after
`WorldTime` and the fixed loop, before transform propagation. If the latest
completed tick is `n`, the running presentation sample is:

```text
sample tick = n - 1 + Time<Fixed>.overstep_fraction()
```

The fraction is bounded to `[0, 1]`, so the sample stays between two completed
physics states and never predicts beyond `SimTick`. On pause or a simulation
barrier it presents the upper endpoint, `n`, then remains fixed. The sample's
simulation seconds, MET, and Julian date all come from the same `MissionClock`
mapping as `WorldTime`.

Ordinary USD time-sampled transforms, visibility, and material channels use
this physical presentation sample. Avian and other causal simulation state
advance from `WorldTime` at integer fixed ticks; celestial body state reads
`CelestialTime`. Ordinary render-only consumers may interpolate between
completed physical states, but they do not advance on wall time.

Celestial frames and the rendered solar direction use `CelestialTime`. By
default it has identity rate and zero offset over `WorldTime`; `SetCelestialClock`
may change its rate or seek its epoch, but it cannot change the parent. At
100,000×, ephemerides, body rotation, the semantic SunState, shadows, and
celestial queries advance from this one sample. Physics and Modelica continue
at their ordinary fixed-step cadence; Modelica receives the latest
CelestialTime-derived environment inputs at its usual communication points.
The celestial solve gate applies its certified angular error budget to this
sample. The Time menu and optional sky-clock HUD expose the same rate and seek
controls; `SetTimeTransport` still controls the separate 0.1×–64× physical
transport.

## 3. Clock tree

A `TimeDomain` is an affine child:

```text
local_t = offset + scale * parent_t
```

The only raw roots are:

- `Tick`: deterministic simulation time, frozen by the physical transport;
- `Wall`: `Time<Real>`, non-deterministic and never paused.

`Clocks` publishes the `real`, `sim`, wall-rooted `interaction`, and
`WorldTime`-child `celestial` handles. The interaction domain drives camera,
avatar, and UI easing that must remain responsive while the simulation is
paused. `CelestialTime` is the resolved calendar view of the celestial child;
it is never re-parented to `Wall`.

A derived domain follows its parent. A driven domain adds `Playback` with its
own seekable head, range, rate, loop, and pause state. `TimeBinding` attaches an
entity to a domain. `ResolvedDomains` resolves these explicit bindings once per
frame. An unbound entity uses its documented owner clock; an explicit binding
that is absent from `ResolvedDomains` is unavailable. Its owner reports the
missing domain and holds that operation instead of silently switching clocks.

## 4. Animation funnel

Authored animation has one projection path:

```text
physical presentation time or an explicit TimeBinding
        -> USD timeSamples
        -> visual projection
```

Unbound USD animation follows the interpolated physical timeline. A deliberate
editor or cinematic preview can bind its entities to `AnimationPreview` or a
camera-track domain and use `ControlAnimation`; this binding is explicit and
does not replace the scene's physical-time default.

The USD animation adapter samples authored xform, visibility, and supported
material channels in `PostUpdate`, after the physical presentation sample is
published and before transform propagation. Camera paths remain an explicit
driven-domain feature; see [`51-cinematic-camera.md`](51-cinematic-camera.md).

Pure tweens and state machines may use a domain, but must not add another
independent clock resource. Authored time samples remain USD data and are
evaluated by the shared adapter.

## 5. Coupling and rates

`DomainRegime` distinguishes the reason a domain exists:

| Regime | Meaning |
|---|---|
| `Kinematic` | Pure function of time; may seek and rate-scale when explicitly bound. |
| `Causal` | Integrates state; rate is bounded by solver stability and communication points. |

A causal participant advances on its declared communication point. It must not
be turned into a kinematic playback shortcut to simplify UI controls.

## 6. Pause and cadence

Pausing the physical transport freezes `SimTick`, physics, and render samples
derived from that tick. The interaction schedule remains available for camera
and UI response. The celestial child follows `WorldTime`, so it freezes with
the simulation even when its rate is scaled. A terminal runtime fault holds the
shared fixed clock, so Rhai and co-simulation cannot advance from invalid
causal state.

The fixed-step catch-up budget lives in `lunco-time`; consumers must not add a
second rate path or drain an unbounded burst. Celestial recomputation may use
its shared geometric error budget, but that cadence selects when derived
presentation is refreshed, not a new time source.

Per-body physics suspension uses the body mechanism such as
`RigidBodyDisabled` and `ColliderDisabled`. It does not create a clock per body:
contact islands and one solver step require a coherent physics cadence.

## 7. Networking and determinism

Only deterministic simulation state and authoritative transport decisions are
network state. Local animation-preview seeks are presentation decisions, not
physics ticks. A camera path's playback is local unless its authored shot is
part of the shared scene contract.

The production composition publishes `PhysicsComputeProfile`, which records
the observed Bevy Compute pool width. `clock_snapshot()` exposes
`physics_profile_known` and the optional `physics_compute_threads` value; an
absent profile or unavailable pool raises a runtime fault. A one-thread Compute
profile controls one source of scheduling variation. It is not a verdict that
physics or the whole simulation is deterministic, and it does not constrain IO
or AsyncCompute.

`RuntimeCycleSet` supplies ordering vocabulary, not an active clock sample or
an independent cadence driver. Every system and callback must use the clock
owned by its cycle. Nested functions and hooks inherit that execution context;
events preserve their producer clock stamp and are consumed at the subscriber's
declared cycle boundary. See
[`62-deterministic-runtime-and-async-boundaries.md`](62-deterministic-runtime-and-async-boundaries.md)
for the cross-domain invocation and async-commit contract.

Telemetry samples are read on their simulation-domain clock, then delivered
through the plugin-owned bounded `Telemetry` cycle after the fixed schedule.
The delivery cycle preserves each sample's source tick and never acquires a
simulation progress hold.

Rhai scenario preparation and hooks receive the scenario owner's typed cycle,
phase, clock sample, sequence, and event producer stamp. Paused lifecycle/event
callbacks receive no elapsed-time clock. One-shot REPL and tool calls use the
application cadence. `sim_tick()`, `dt()`, and `elapsed_seconds()` reject calls
outside the simulation cycle as a Rhai invocation error; missing mandatory
simulation clock resources remain a runtime fault.

Never replicate a private floating-origin cell/local split as the time
contract. Coordinate projection and time authority are separate boundaries.

## 8. Invariants and owners

1. Store one causal master (`SimTick`); derive calendar and consumer views.
2. Presentation samples completed physical ticks and never advances beyond the
   latest one.
3. Ordinary scene animation uses the physical-time sample. Celestial state and
   its model inputs use the one `CelestialTime` child of `WorldTime`.
4. Causal state advances only on the fixed simulation cadence.
5. The USD sampler is the shared authored-animation funnel.
6. Interaction cadence serves camera/avatar/UI response and remains separate
   from the physical simulation cadence.

| Layer | Owner |
|---|---|
| master tick and transport | `lunco-core` / `lunco-time` |
| mission/calendar anchor and `WorldTime` | `lunco-time` |
| physical presentation sample | `lunco-time` |
| `CelestialTime` child and resolved sample | `lunco-time` |
| explicit domains, playheads, and bindings | `lunco-time` |
| USD value evaluation and visual projection | `lunco-usd-bevy-animation` |
| celestial render projection | `lunco-celestial-spatial` |
| Modelica stepping and communication points | Modelica/cosim owners |
| physics stepping | Avian and the fixed simulation schedule |
| avatar/camera/UI presentation cadence | `InteractionSchedule` |

`SetTimeTransport` controls the physical simulation. `SetMissionEpoch` changes
the calendar anchor at the current tick. `SetCelestialClock` rate-scales or
seeks the `WorldTime` child without changing its parent or the physical cadence.
`ControlAnimation` controls explicitly bound preview or driven domains.
`SetSimulationExecutionMode` controls host pacing only; it does not change
`TimeTransport.rate`.
