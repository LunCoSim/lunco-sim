# 19 — Unified Time, Clocks & Animation

> Status: Active · Audience: contributors working on time, simulation clocks, and animation

`lunco-time` owns the mission-time spine: the fixed simulation tick, transport,
calendar anchor, causal `WorldTime`, and the physical-time sample used by
presentation.

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

Scene time selection is one application policy: `scene.time.select`, installed
by `assets/scripting/policy/startup.rhai` from `policy/index.toml`. Startup
installs the policy; it does not choose an epoch before a scene exists. The USD
scene lifecycle invokes it once at `SceneTransitionCompleted`, after the
composed stage dependencies and queued visual/mesh projection work have
settled. A completion notification that did not enter the loading phase is
ignored, so an idempotent load of the active scene does not select or apply time
again. Its typed facts include the selected root path, composed epoch API and
value status, celestial-source presence, and a fresh computer UTC→TDB candidate.
Rhai selects a valid non-zero root `lunco:time:epochJd` when present; otherwise
it selects current computer time. Rust verifies that the returned epoch matches
one of those candidates and passes the result to the time owner.

Until the selection is installed, `SceneTimeState` holds the physical fixed
loop and gates USD time-sample animation, celestial placement/presentation, and
USD DEM terrain construction. Applying the selection resets `SimTick`, the
mission calendar, and clock-domain samples before releasing those consumers.
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

## 2. Physical-time presentation sample

Render-only consumers read `SimulationPresentationTime`, published after
`WorldTime` and the fixed loop, before transform propagation. If the latest completed tick is
`n`, the running presentation sample is:

```text
sample tick = n - 1 + Time<Fixed>.overstep_fraction()
```

The fraction is bounded to `[0, 1]`, so the sample stays between two completed
physics states and never predicts beyond `SimTick`. On pause or a simulation
barrier it presents the upper endpoint, `n`, then remains fixed. The sample's
simulation seconds, MET, and Julian date all come from the same `MissionClock`
mapping as `WorldTime`.

Ordinary USD time-sampled transforms, visibility, and material channels use
this physical presentation sample. Celestial render frames and solar
presentation use the same sample. Causal physics and body state continue to
read `WorldTime` at integer ticks; render-only consumers may interpolate between
those completed states, but they do not advance on wall time.

Pausing the fixed simulation holds the sky, authored scene animation, and other
simulation-time presentation together. The sky-time UI reads
`SimulationPresentationTime`.

## 3. Clock tree

A `TimeDomain` is an affine child:

```text
local_t = offset + scale * parent_t
```

The only raw roots are:

- `Tick`: deterministic simulation time, frozen by the physical transport;
- `Wall`: `Time<Real>`, non-deterministic and never paused.

`Clocks` publishes the `real`, `sim`, and wall-rooted `interaction` handles.
The interaction domain drives camera, avatar, and UI easing that must remain
responsive while the simulation is paused. It does not drive scene animation,
celestial state, or physics.

A derived domain follows its parent. A driven domain adds `Playback` with its
own seekable head, range, rate, loop, and pause state. `TimeBinding` attaches an
entity to a domain. `ResolvedDomains` resolves these explicit bindings once per
frame; missing required bindings are reported by their owner.

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

Pausing the physical transport freezes `SimTick`, physics, and every render
sample derived from that tick. The interaction schedule remains available for
camera and UI response. It is a separate presentation cadence, not another
simulation clock. A terminal runtime fault also holds the shared fixed clock;
only physics would leave Rhai, celestial time, and co-simulation advancing from
an invalid state.

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

Physics determinism also requires the Avian compute order to be pinned. The
production headless/server builder publishes `PhysicsDeterminism`; the
`clock_snapshot()` query exposes that contract and reports a missing admission
resource as a runtime fault rather than treating the world as deterministic.

Never replicate a private floating-origin cell/local split as the time
contract. Coordinate projection and time authority are separate boundaries.

## 8. Invariants and owners

1. Store one causal master (`SimTick`); derive calendar and consumer views.
2. Presentation samples completed physical ticks and never advances beyond the
   latest one.
3. Scene animation and celestial presentation use the physical-time sample by
   default; independent playback requires an explicit `TimeBinding`.
4. Causal state advances only on the fixed simulation cadence.
5. The USD sampler is the shared authored-animation funnel.
6. Interaction cadence serves camera/avatar/UI response and remains separate
   from the physical simulation cadence.

| Layer | Owner |
|---|---|
| master tick and transport | `lunco-core` / `lunco-time` |
| mission/calendar anchor and `WorldTime` | `lunco-time` |
| physical presentation sample | `lunco-time` |
| explicit domains, playheads, and bindings | `lunco-time` |
| USD value evaluation and visual projection | `lunco-usd-bevy-animation` |
| celestial render projection | `lunco-celestial-spatial` |
| Modelica stepping and communication points | Modelica/cosim owners |
| physics stepping | Avian and the fixed simulation schedule |
| avatar/camera/UI presentation cadence | `InteractionSchedule` |

`SetTimeTransport` controls the physical simulation. `SetMissionEpoch` changes
the calendar anchor at the current tick. `ControlAnimation` controls explicitly
bound preview or driven domains. `SetSimulationExecutionMode` controls host
pacing only; it does not change `TimeTransport.rate`.
