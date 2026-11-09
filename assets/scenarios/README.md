# Built-in scenario examples

Scenario behavior is authored in Rhai. The owning USD scene or vehicle decides
which source runs and supplies its identities; a Rhai file does not load USD
layers on its own. Modelica owns continuous equations, physics owns rigid-body
motion and contact, and scenarios sequence commands and react to events.

## Scene and vehicle behavior

| Source | Role | Example owner |
|---|---|---|
| [`lander_subsystems.rhai`](lander_subsystems.rhai) | Lander supervisor: translates authored propulsion, contact, and fuel events into typed GNC commands. | `DescentLander/Subsystems`; attached to the vehicle asset. |
| [`lander_cinematic.rhai`](lander_cinematic.rhai) | Directs camera cuts and joins the authored landing, deployment, and route events. | [`scenes/luncosim/lander_cinematic.usda`](../scenes/luncosim/lander_cinematic.usda) |
| [`rover_surface_ops.rhai`](rover_surface_ops.rhai) | Sequences lander-to-rover deployment and mission milestones. | The integrated lander operations scene. |
| [`route_follow.rhai`](route_follow.rhai) | Reusable route policy that publishes one target when route-point events advance. | A scene-authored Route scope with a Modelica controller on its subject. |
| [`solar_rover_cinematic.rhai`](solar_rover_cinematic.rhai) | Presents route milestones for the solar rover example. | [`scenes/luncosim/solar_rover_demo.usda`](../scenes/luncosim/solar_rover_demo.usda) |
| [`landing_target_presentation.rhai`](landing_target_presentation.rhai) | Maintains the rendered touchdown footprint from the navigation body's authored source. | The lander's landing-target prim. |

[`sensor_fsw_logic.rhai`](sensor_fsw_logic.rhai) is a pure decision-function
example, not a standalone scenario: it accepts a telemetry map and returns
load-shed, heater, navigation-mode, and descent-engine decisions. It currently
has no authored vehicle binding, so use it as a code example rather than
expecting it to appear as a runnable mission.

## Runnable examples

- **Lander & Rover Mission** is an assessed exercise with an authored lander
  scene, event-driven touchdown/deployment, rover control, and route milestones.
- **Solar Rover Demo** shows a rover following a route while battery demand,
  solar generation, tracking, and presentation events are visible.
- **Lander Cinematic** shows how presentation can react to simulation events
  without owning flight control.

For tutorial entry points and what each lesson asks the learner to do, see
[`tutorials/README.md`](../tutorials/README.md).
