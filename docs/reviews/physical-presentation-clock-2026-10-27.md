# Physical presentation clock and Roadster camera jitter

## Ownership and contract

`lunco-time` publishes one `SimulationPresentationTime` in
`RunFixedMainLoop`, after fixed execution and before physical transform easing.
`lunco-luncosim-simulation` consumes its interpolation fraction, USD animation
consumes the same snapshot, and the follow camera reads the eased body pose.

The presentation cursor advances within completed master-tick endpoints. It
cannot extrapolate or rewind within a mission. Coupling readiness admits fixed
execution but does not choose presentation phase. Pause displays the completed
endpoint; resume retains that endpoint until a newer interval exists. Epoch
re-anchoring resets presentation history at the current completed tick.

This is an engine clock invariant, so it belongs in Rust rather than a Rhai
policy hook. There is no Roadster-specific camera adjustment, second pose
buffer, new clock, dependency patch, or internal JSON boundary. The existing
native interpolation plugin owns endpoint capture, invalidation and restoration.

The fixed master clock, Avian substeps, Modelica communication periods, retained
pacing demand and wall-clock interaction cadence retain their owners.
`CelestialClock::epoch_at_tick` supplies physical celestial inputs at their exact
tick boundary; celestial rendering consumes its independently rate-scaled
`CelestialTime` snapshot. Explicit animation `TimeBinding` domains retain their
own playback and seek semantics.

| Clock | Consumer | Effect of this change |
|---|---|---|
| Master fixed ticks | Shared causal simulation | None; ordered admission and retained demand remain authoritative |
| Native physics substeps | Avian integration | None; no changed quantum or skipped steps |
| Modelica communication intervals | Participant solvers and input/output exchange | None; completion receipts only admit eligible master ticks |
| Physical presentation | Body easing and unbound USD animation | One bounded, monotonic snapshot per frame |
| Celestial affine child | Exact-tick physical providers and celestial rendering snapshot | None; its independent scale and pause inheritance remain intact |
| Wall-rooted interaction | Camera input and presentation damping | None; remains responsive during simulation pause |
| Explicit animation domains | Authored bound playback | None; explicit rate, pause and seek semantics remain authoritative |

## Reproduction and verification

Execution workspace: `/home/rod/Documents/luncosim-workspace/usd`, branch `usd`,
base `8bad5b08b`. The baseline executable reports `5d955303`; the difference to
this base is shader input admission and documentation, not the timing/camera
owners. The fixed executable was built in this checkout with
`cargo build -p lunco-luncosim -j 4`.

The owned window used API 4519 and the unmodified `tesla-rr` Twin. X11 permitted
continuous visible updates; the initial Wayland run was throttled and is not
used for the high-frequency comparison. Existing unrelated Cargo workloads
remained running; these observations are not FPS benchmarks.

| Sample | Observations | Fractional samples | Final speed | Rendered car reversals | Camera reversals |
|---|---:|---:|---:|---:|---:|
| Baseline | 423 | 225 | 20.10 m/s | 196 | 196 |
| Fixed, comparable drive | 240 | 240 | 23.12 m/s | 0 | 0 |
| Fixed, higher-speed drive | 51 | 0 | 28.00 m/s | 0 | 0 |

Native chassis-collider translation never reversed in these runs. The baseline
presentation clock rewound 197 times. Its maximum backward rendered-body step
was 0.188 m and camera step 0.033 m. The fixed samples had no presentation-clock
rewinds. Each API observation reads a coherent world; the follow-camera value
belongs to the preceding completed presentation pass, so these measurements
compare successive motion, not a simultaneous camera/body offset.

Evidence is retained under
`.runtime-check/roadster-presentation-2026-10-27/`, including the numeric samples,
summary, moving-view screenshots and production logs. Screenshot inspection
confirms the intended Roadster follow view; the numeric series establishes the
temporal reversal result.

- `cargo test -p lunco-time --lib render_time_ -j 4`: the regression fails before
  the change; all three focused clock tests pass afterward.
- `python3 scripts/api/test_physical_presentation_time.py --port 4520`: production
  scene-test PASS, 13 authored Rhai checks, 240 Hz input pacing with 35% jitter.
  Covers negative rewind/extrapolation samples, fractional interpolation,
  pause/resume, one-step admission, 0.25x and 8x transport, unchanged physical
  quanta, mission epoch reset, and 100,000x celestial running/paused/single-step
  behavior. The test host exited successfully and released its port.
- `target/debug/luncosim test --api 4521 --scene assets/scenes/tests/multi_rover_stress_4.usda --threads 1 --jitter 0.5 --seed 6840157149251759617 --determinism-reference scripts/tests/fixtures/deterministic-physics-reference.json`:
  PASS, 24 checks, 780 ticks, exact reference comparison. This is one maintained
  deterministic profile, not the complete matrix or cross-platform proof.
- `cargo test -p lunco-luncosim-simulation --lib physics_presentation -j 4`: all
  four low-level tests pass, covering solved-pose isolation, endpoint restoration,
  published-sample consumption, backlog bounds and malformed rotation handling.
- Skill catalogue validation and `git diff --check` pass.

The repair changes the shared engine presentation boundary. The Twin source is
unchanged. Validation was performed in the `usd` checkout.
