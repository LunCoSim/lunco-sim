# Scenes

- `base/` contains reusable world foundations.
- `luncosim/` contains interactive application and demonstration scenes:
  `sandbox_scene.usda` is the general driving yard, `lander_ops.usda` is the
  interactive powered-descent mission, `lander_cinematic.usda` demonstrates
  event-led camera direction, and `solar_rover_demo.usda` demonstrates solar
  power and route following. `selection_intents.usda` is an editor review
  fixture for replacing and extending selected USD targets.
- `tests/` contains focused executable regression scenes.
- `celestial/` contains celestial-specific examples.

A scene composes components, vessels, structures, lighting, and behavior. Do not
inline a reusable physical part merely because one scene currently uses it.

Regression scenes should have one observable purpose, deterministic assertions,
and a `LunCoProgramAPI` test/scenario script where behavior must be assessed.
Cross-domain acceptance belongs in one deliberately integrated scenario; small
regressions remain focused so failures are diagnosable.

The [`vessels` guide](../vessels/README.md) maps the built-in rover and lander
assets. The [`scenarios` guide](../scenarios/README.md) explains the behavior
programs composed by the interactive examples.
