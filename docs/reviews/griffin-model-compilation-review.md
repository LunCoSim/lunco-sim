# Griffin and shared Modelica model review — 2026-10-19

The review covered the canonical Griffin/FLIP Twin and the complete shared
`assets/models` source inventory. Repository and source/provenance verification
passed. All 113 initial Modelica files parsed; the final inventory has 115 files
including the physical and mechanical regression fixtures, and all parse successfully.
A production workbench sweep inspected 107 non-negative-test source files and
89 compile targets. Required-input failures and open physical connectors were
kept distinct from compiler errors. Closed production networks and explicit
physical experiments establish compilation/initialization for those components.
The intentionally failing files under `models/tests` are not expected to run
successfully as standalone examples.

## Findings and repairs

- **Motor energy balance:** the torque and back-EMF constants were independently
  derived from incompatible nameplate assumptions. At 28 V and half no-load
  speed, input minus winding heat and shaft power was -415 W; the regeneration
  case was off by 1245 W. The motor now uses equal SI constants and derives
  resistance from stall torque and no-load speed. The nominal torque-speed
  curve remains 0.9 N.m at stall and zero at 2400 rad/s. The continuous power
  rating remains nameplate metadata, rather than an assumed stall-power law.
  This follows the [MIT motor model](https://www.mit.edu/course/2/2.010/www_f00/psets/hw2_dir/tutor2_dir/tut2_h.html).
- **PID integral limit:** output telemetry was clipped while the command used
  an unbounded state. With zero proportional/derivative gains and a 0.25
  integral limit, the command reached 3.0 after three seconds. The command now
  consumes the bounded integral and outward integration stops at the limit;
  inward integration and output-saturation back-calculation remain active.
  Initialization is explicit. The limiter uses `noEvent` to retain Griffin's
  fixed-rk4 continuous-model contract. Reversal reaches the opposite bound
  without accumulated hidden windup.
- **Electrical sensor:** a single unconstrained sensing pin could not measure
  branch current or close the circuit. The sensor now has an ideal series
  `p`/`n` path with equal voltage and conserved current. No power is absorbed.
  Unused gain parameters were removed; authored ADC ranges remain authoritative.
- **Thermal sensor:** its heat-flow variable was unconstrained. The instrument
  is now explicitly non-loading (`port.Q=0`); its response lag remains a
  measurement state, rather than physical heat storage.
- **Phase-change tank:** its thermal potential had no closure. The reduced
  isothermal model now requires an authored positive `saturation_temperature_k`.
  It predicts heat-driven mass loss at that operating point, not pressure or
  saturation temperature. The unused relief-pressure parameter was removed.
- **Heater conservation:** an unpowered terminal could still supply 14.7 W of
  heat. Heat is now derived from actual terminal electrical power and the
  authored efficiency, including during brownout.
- **Allocator fixture compilation:** 18 equations duplicated bound input
  defaults. Instance modifiers now supply those inputs, leaving a balanced
  model. Orthogonal, coupled and zero-authority solutions are checked.
- **Cold document solve:** `RunModelicaSolve` dispatched before admitting its
  referenced library roots. A fresh process failed on LunCo classes while a
  warmed workbench experiment succeeded. It now uses Rumoca's strict-AST root
  discovery and the existing `admit_compile_roots` owner before dispatch,
  plus the shared read-only-library compile overlay contract.
- **Trajectory projection:** endpoint retention compared values. A constant or
  periodic signal could lose its final row while the time vector retained it.
  Endpoint identity now uses indices, and stride calculation reserves the
  endpoint within the point cap. Physics assertions read aligned finite rows.

The empty-battery gate exposed a separate test-fixture assumption: a falling
rover can back-drive motors and exchange regenerated energy between branches.
The zero-source rig now holds its chassis kinematic, excluding gravitational
work. Its original 0.1 mW tolerance is unchanged. The new motor experiment
separately validates regeneration and zero-duty braking.

## Ownership and validation

Equations remain in Modelica; numerical verdicts are authored in Rhai.
The API owns package admission and trajectory projection through existing
shared owners. No dependency source, Rust physics rule, flight gain, fabricated
input default, or solver tolerance was added to make a gate pass. Hooks are
not appropriate for continuous equations, source-root admission or row identity.
The two additional API dependencies reuse the maintained source-root owner and
Rumoca AST facts; no alternate package scanner or compiler was introduced.

Run the maintained regression with:

```
LUNCOSIM_BIN=<production-binary> python3 scripts/api/test_model_physics.py --output <directory>
LUNCOSIM_BIN=<production-binary> python3 scripts/api/test_model_physics.py --headless --output <directory>
```

Its 44 checks cover four-quadrant motor power, torque-speed symmetry, PID bounds
and reversal, non-loading sensors, tank mass/heat balance, a closed
pump/chamber feed, unpowered heating, bounded allocation and aligned capped
trajectories. The headless mode creates scratch documents and starts a cold
`RunModelicaSolve`; windowed mode also exercises workbench `RunExperiment`.

Existing production gates passed: drivetrain 6, electrical/thermal 6,
empty battery 4, plume conservation 48, RCS feed starvation 16, Griffin GNC 12,
engine-out 20, RCS commands 19, fuel exhaustion 11 and oxidizer exhaustion 11
(153 assertions). The solar rover energy cycle also passes 51 assertions,
and FLIP route/thermal verification passes 19 assertions (223 scenario
assertions in total). The route observer now preserves the route editor's typed `retryable`
status while a point edit projects, bounded by its existing ten-second deadline;
terminal errors still fail immediately with their diagnostic. The GNC gate uses the unchanged fixed-rk4 solver. All source
and requirement checks pass (214 typed targets/evidence records, 55 sources).
The normal surface-operations scene reaches readiness with zero pending holds
and no fault; all ten Modelica participants advance beyond 0.1 s.
The endpoint helper's exact production function and unit test were compiled
and run directly with rustc; a broad Cargo unit build was stopped when it
started building another dependency feature set with low disk space. The
production binary build and end-to-end API regression both pass.

Evidence is retained locally under `/tmp/griffin-model-review-20261019`.
Validation used optimization's production binary with the task checkout's
explicit asset root. Each API session uses owned port 4193, API Exit and
port-release checks. User port 3743 was not controlled. Full MSL compatibility,
flight qualification and calibrated spacecraft performance are outside this
review's acceptance scope.

## Passive mechanical losses

`GearRatio` keeps the rigid speed relation and applies the authored mesh
efficiency according to incoming power direction: `tau_b=-ratio*eta*tau_a`
when side a drives, and `tau_b=-ratio*tau_a/eta` when side b drives. Its signed
terminal powers sum to the exposed `heat` output. The constant-efficiency model
has no stored energy, backlash, slip or static-friction state; at rest it selects
the forward torque relation with zero power. The authored output torque is a
rating: exceeding it stops the solve with an owner diagnostic. A torque-limiting
clutch requires an explicit slip model. `BearingFriction` likewise exposes
nonnegative `d*w^2` as heat and rejects negative damping.

The [Modelica Association loss model](https://doc.modelica.org/Modelica%204.0.0/Resources/helpWSM/Modelica/Modelica.Mechanics.Rotational.Components.LossyGear.html)
uses direction-dependent mesh efficiency and distinguishes bearing friction.
This library uses the reduced constant-efficiency, viscous-only subset and does
not claim the full sticking/sliding model. FLIP's four gearbox heat outputs feed
four `HeatLoad` facets in its enabled left/right thermal banks. The
bank coefficients and lumping remain study assumptions, not FLIP calibration.

Native wheel bearing drag, brakes, tire slip and suspension damping retain their
existing native force/torque owners. No additional efficiency or bearing element
is applied to them. Their dissipated energy leaves the modeled mechanical
subsystem; its distribution into hub, brake, tire and terrain temperatures is
outside the motor/gear thermal banks. Native numerical dissipation is not
reported as physical heat. This is not a whole-vehicle thermal qualification.

The maintained `scripts/api/test_mechanical_losses.py` gate passes 41 authored
checks in both windowed and headless production sessions: four quadrants,
negative reduction, zero load/speed, unity efficiency, reversal, signed bearing
drag, motor-to-shaft power and stored-heat balance, invalid parameters, and
initial/transient overload. The transient rating violation is diagnosed after
startup. No Rust or third-party source change is needed for these equations.
Fresh affected gates pass drive 6, thermal 6, empty battery 4, solar energy 51,
and FLIP route/thermal 19. The route observes actual gear loss at the thermal
participant and a rising thermal-bank temperature while completing all three
waypoints. Surface startup advances all ten Modelica participants fault-free.
Evidence: `/tmp/griffin-mechanical-losses-20261019`.

## Remaining non-compilation findings

These source-level defects remain open; successful compilation is not evidence
that the entire component catalogue is physically qualified:

- `LunCo.Comms.DataBuffer` and `LunCo.Storage.MassMemory` limit negative storage
  rates but not the upper capacity. Constant net input of one storage unit per
  second fills a one-unit device to 200% at two seconds. Capacity enforcement
  needs explicit accepted/rejected or dropped data rates so upstream/downstream
  flows do not silently disappear.
- `LunCo.Comms.Transmitter` commands RF independently of actual terminal power.
  With zero bus voltage and transmit enabled it still reports its nominal 5 W
  RF output. Its regularized current law supplies zero electrical power in that
  state. Payload/data-rate models likewise lack a qualified brownout lifecycle.

This patch repairs the demonstrated compile, sensor/phase-change closure,
controller, motor and heater defects and establishes their production regression.
The storage/RF findings above remain separate plant-model work;
no claim of full system energy or capacity qualification is made.
