# Tutorial, Task Programs, and Port Contracts

> Status: Active · Audience: contributors authoring tutorials, task programs, and USD/Modelica connections

This page records the runtime contracts that make a tutorial both teachable by
a person and executable by an automated scenario. It also records the
environment-provider failure mode that is easy to miss when a USD API schema
declares ports without authored properties.

## 1. Human and authored-program control are one path

An authored task program is a different **policy**, not a second actuation
mechanism. Human and unattended paths use the same control sequence:

```text
AcquireControl
    -> ControlBinding / intent mapping
    -> SetPorts (or the same live ControlStream surface)
    -> PortRegistry
    -> Avian or Modelica input
```

The scenario may select unattended execution with `is_unattended()`, but it
must not write `LinearVelocity`, `Position`, `ModelicaModel.inputs`, or a
private autopilot component to move the vehicle. Those paths bypass the
authority and input plumbing that a person exercises.

For a route-following program, the continuous controller is an authored
Modelica program or a generic named-port consumer. Rhai reads composed route
points and sensor events, then publishes the program's named inputs through the
same port surface used by human control. It does not calculate dynamics or
write transforms every tick. Authority remains an engine-level input gate, so
possession and authored programs cannot create competing actuation paths.

For a tutorial acceptance test, observe both command events and the resulting
state:

- `cmd:AcquireControl` proves that the controller acquired authority through the
  normal session path.
- `cmd:SetPorts` proves that it used the same actuation surface as a human.
- A live position/port predicate proves that the command had an effect.
- The final waypoint or mission predicate proves the behaviour, rather than
  merely proving that the script emitted commands.

This is why `tutorial_lander_mission.rhai` counts possession and port writes,
then checks arrival at the final waypoint. A `MISSION_COMPLETE` event by itself
is not sufficient evidence when the test can complete without exercising the
controls.

## 2. Scenario-test shape

Keep the test scene and the lesson separate when the production scene already
owns deployment or mission sequencing:

```text
assets/scenes/tests/<lesson>.usda       # references the authored scene
assets/scenarios/tests/<lesson>.rhai    # emits a real PASS/FAIL verdict
assets/tutorials/.../<lesson>.rhai      # teaching HUD and user-facing policy
```

Use separate `TutorialHost` and `TestHost` program prims when both scripts need
to observe the same world. The test must not replace the production supervisor
just to make the verdict easier.

The production scene-test boundary is the built binary, not parsing or a
queued command:

```bash
RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim \
  --no-default-features --features lunco-api,transport-http -j 4
target/debug/luncosim test \
  --scene assets/scenes/tests/tutorial_lander_mission.usda \
  --threads 1 --jitter 0 --max-ticks 30000
```

Capture the process exit code and the final authored verdict. A parse-only
`--validate` run proves asset syntax; it does not prove that composition,
projection, wiring, physics, and the scenario all ran. Likewise,
`{"data":{"accepted":true}}` means that an API command completed without a
result payload. A result-returning command puts its command-specific data in
the same `data` field; deferred commands return it when their owner finishes.

## 3. Declared topology is separate from live samples

Every cosimulation source has two different facts:

1. **Declared output topology** — the names that may be connected.
2. **Current samples** — values that are available after a provider has
   produced data.

They must not be represented by the same map or by fabricated zeroes. The
runtime component `DeclaredOutputPorts` carries the first fact. The ordinary
`SimComponent.outputs` map carries the second. A connection may bind to a
declared output before its first sample, while a value read before that sample
remains unavailable.

This distinction is required for the environment probe. Its normal
`probe.usda` asset is intentionally empty: `LunCoEnvironmentProbeAPI` supplies
the fixed gravity interface, while direction outputs are materialized from
composed wire demand. An authored-property enumeration can therefore see no
output attributes even though the composed schema declares gravity outputs.
The USD projection recognizes the API schema and publishes the authoritative
base output set:

```text
gravity_accel, gravity_x, gravity_y, gravity_z
```

The gravity fields are USD `double`, matching their f64 simulation values.
Direction outputs use `<id>_mount_x/y/z`, where `id` is the stable identity of
any finite target or explicitly framed ray. The source id comes from the
connection; the consuming Modelica component names its local vector
`target_mount_x/y/z` and can be rewired to another object without changing its
equations. Each probe resolves its own vector through the shared BigSpace f64
conversion and normalizes once at the provider boundary. If a target or
coordinate frame is absent, ambiguous, or coincident with the probe, the
direction sample is removed and a structured diagnostic is published; no zero
or stale sample is supplied.

The ownership boundary is therefore:

```text
USD API schema / projection  -> declared port names
environment domain           -> current f64 gravity and requested direction samples
cosim wiring                 -> topology resolution
solver / consumer            -> reads only available samples
```

Adding authored dummy properties to `probe.usda`, teaching the wire resolver a
special EarthTracker alias, or returning zero for an absent sample would hide
the same bug in another form and is not an acceptable compatibility path.

## 4. Runtime proof and readiness

For a live production check, use one explicitly-port-bound
`target/debug/luncosim` process and verify:

```bash
curl -fsS http://127.0.0.1:4101/api/ready
```

Trust the session only when the response reports `ready:true`,
`world_hold:false`, and `pending_count:0`. Use the API `Exit` command before
replacing a session and verify that its process and port are gone. Do not
overlap GUI/API sessions or hide a rebuild behind `cargo run`.

The environment-port regression is closed when a fresh production log shows
the EarthTracker programs bound and compiled without `target_mount_*` missing
port diagnostics. That closure is independent of the lander's physical
acceptance: a mission can have correct EarthTracker wiring and still fail later
because a body, solver, joint, or autopilot state diverges.

## 5. Current follow-up boundary

The `DeclaredOutputPorts` change fixes the missing environment-source contract.
Physical lander acceptance remains a separate force/solver/joint/finite-state
boundary; investigate it through the powered-lander and BigSpace contracts,
not by changing environment port declarations.

See also:

- [`22-domain-cosim.md`](22-domain-cosim.md) — the master loop and port surface
- [`23-domain-environment.md`](23-domain-environment.md) — environmental facts
- [`28-modelica-realtime-physics.md`](28-modelica-realtime-physics.md) — the
  unresolved fixed-step deterministic solver contract
- [`46-bigspace-deep-analysis.md`](46-bigspace-deep-analysis.md) — current
  coordinate-frame and physics-bridge maintenance contract
- [`lander-actuation-modelica.md`](lander-actuation-modelica.md) — current
  powered-lander force, torque, and Modelica boundary
