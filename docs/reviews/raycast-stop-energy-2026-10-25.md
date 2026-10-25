# Shared raycast suspension stop: impact energy

Repeated Roadster lift commands exposed a shared suspension error: a late ray
hit admitted an already-overlapping XPBD stop. Its positional correction became
separating velocity, producing a higher unpowered rebound. A held lift case
rose to 64.98 m under thrust and later rebounded above 282 m with thrust off.

## Ownership and implementation

USD still owns suspension travel and geometry. Modelica still owns propulsion,
gas and vehicle controls. `lunco-mobility` owns raycast wheel contact integration;
Avian owns body integration, islands, effective inertia and the normal contact
law. The fix has no vehicle-name branch and applies to every `WheelRaycast`
carrier, including articulated rover carriers. Jointed collider wheels retain
their native contact path.

The application-owned adapter uses Avian's public `ContactNormalPart`, native
softness, speculative impulses, warm starting and unbiased velocity relaxation.
It joins the actual carrier and ray-hit support through the existing joint
graph. Both bodies receive the corresponding linear and angular impulse; native
dominance handles static and kinematic supports. Reported stop load is summed
impulse divided by the master step. No third-party source was changed.

Wheel queries include the hub's swept distance over the next master step,
including angular velocity and constant linear acceleration. Speculative hits
preserve their signed free-travel gap. Actual spring and tire contact still
requires a hit within authored rest length. This addresses late detection in
the reproduced fast landings without inventing a force, clamping telemetry,
changing suspension constants or increasing the standard eight substeps.

There is no new behavior hook: this is continuous contact dynamics and solver
integration. Vehicle-specific commands and acceptance thresholds remain in Rhai.

## Regression evidence

- The shared `stop_impacts_do_not_create_energy_or_pull_separating_bodies` test
  exercises 54 native integration cases: carrier masses of 1, 100 and 2000 kg;
  initial penetration of 0, 50 and 300 mm; central and off-centre contact; static,
  dynamic and moving kinematic supports. It checks energy, actual impact
  response, equal/opposite dynamic reaction, reported impulse and separation.
- The production `drivetrain_parity`, `rocker_bogie_drive`,
  `paused_wheel_effort` and `static_wheel_contact` scene gates passed. These
  exercise raycast/physical driving, articulated driving, coupled pause and
  static graph admission through the normal production runner.
- Headful Roadster regressions passed repeated lift and a ten-second hold.
  First powered apexes remain 6.127 m and 64.980 m. Maximum compression is
  65.001 mm and 65.004 mm against the unchanged 65 mm travel. Six native K
  presses with refills also settle without escalation.

The Rust impact test is part of the ordinary `lunco-mobility` unit suite. The
Roadster owns its additional `scenarios/tests/jump_rebound.rhai` acceptance and
`scripts/check_jump_rebound.py` production runner. This makes the generic energy
invariant independent of whether a particular Twin test is run.

This does not certify every moving-platform collision, terrain geometry or
packaged platform. In particular, the wheel query horizon is based on carrier
motion; arbitrary rapidly approaching supports require separate collision
qualification. The late-contact energy invariant is tested even when initial
penetration is deliberately large.
