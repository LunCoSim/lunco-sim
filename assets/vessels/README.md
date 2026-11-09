# Built-in vehicle examples

These assets are teaching and simulation examples. Their USD files are the
source of truth for geometry, composition, physics, and connections. A composed
asset describes a vehicle; it does not run its guidance or a mission by itself.

## Rovers

| Asset | What it demonstrates | Good place to start |
|---|---|---|
| [`rovers/rocker_bogie.usda`](rovers/rocker_bogie.usda) | Six-wheel articulated rocker-bogie suspension and differential body leveling. | [`tutorials/basic/rover_variants.usda`](../tutorials/basic/rover_variants.usda) |
| [`rovers/ackermann_rover.usda`](rovers/ackermann_rover.usda) | Four-wheel steering with a selectable raycast or physical drivetrain. | Steering and wheel-attachment experiments. |
| [`rovers/skid_rover.usda`](rovers/skid_rover.usda) | Four-wheel skid steering, with battery and solar power variants. | [`scenes/luncosim/solar_rover_demo.usda`](../scenes/luncosim/solar_rover_demo.usda) |
| [`rovers/six_wheel_rover.usda`](rovers/six_wheel_rover.usda) | Six-wheel skid steering with authored actuator wiring and raycast suspension. | Drive-mix and drivetrain wiring examples. |
| [`rovers/six_wheel_independent.usda`](rovers/six_wheel_independent.usda) | Individual motor channels and a USD-authored drive mix. | Independent wheel-control experiments. |
| [`rovers/rucheyok/rucheyok.usda`](rovers/rucheyok/rucheyok.usda) | Solar-powered four-wheel rover assembled from referenced parts. | Component-composition and solar-power studies. |

`rovers/variants/rover_easy.usda`, `rover_medium.usda`, and `rover_awful.usda`
share one rocker-bogie chassis, mass, center of mass, inertia, and suspension.
They vary tire friction and motor stall torque. Standard versus Hauler keeps
motor torque fixed and changes tire friction; Explorer changes both. The
Rover Variants lesson explains the comparison and does not claim a center-of-
mass difference.

| Profile | Tire | Friction coefficient | Motor stall torque |
|---|---|---:|---:|
| Explorer LT | Cleated | 1.8 | 0.9 N·m |
| Standard | Worn | 1.0 | 0.4 N·m |
| Hauler | Bald | 0.8 | 0.4 N·m |

The shared gearbox ratio and efficiency map those shaft stall values to about
153 N·m (Explorer) and 68 N·m (Standard and Hauler) at the axle. Tire friction
is authored on each wheel's tire variant; torque is authored on each motor.

`rover_solar.usda` is a power-system profile. `rover_stress.usda` is an internal
stress-matrix fixture, not a user vehicle. `differential_rig.usda` is a test rig,
not a rover asset.

## Lander

[`landers/descent_lander.usda`](landers/descent_lander.usda) is a powered-
descent vehicle with a Modelica propulsion/control interface. It has no
standalone descent guidance. For a live guided mission, use
[`scenes/luncosim/lander_ops.usda`](../scenes/luncosim/lander_ops.usda) and the
**Lander & Rover Mission** exercise. For a cut-scene example, use
[`scenes/luncosim/lander_cinematic.usda`](../scenes/luncosim/lander_cinematic.usda).

## Learning path

Start with **Scenarios → Tutorials → Start Here → Lunar Rover Lab** for hands-on
practice. Use **Moon Driving Basics** for a focused driving exercise and **How
Simulation Works** for the engine walkthrough. The [tutorial guide](../tutorials/README.md)
explains the tour and exercise tracks; the [scenario guide](../scenarios/README.md)
maps the reusable behavior assets.
