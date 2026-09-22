# LunCoSim tutorials

This directory contains authoring walkthroughs and links to the authored
lessons shipped under `assets/tutorials/`. A lesson is an ordinary Rhai
scenario with optional USD scene content. The application Rhai policy owns
catalog selection and menu construction; the shared asset lifecycle and
workbench menu host stay tutorial-agnostic.

For cross-host skill usage, task routing, format boundaries, dynamic Rhai
tools, and the full development cycle, start with
[`../../skills/START-HERE.md`](../../skills/START-HERE.md). The
[`author-tutorial`](../../skills/author-tutorial/SKILL.md) runbook is the
tutorial-specific route after the task shape is clear.

## In-app lessons

At startup, the application policy manifest installs
`application.asset.lifecycle` from
`assets/scripting/policy/tutorial_catalog_menu.rhai`. The shared text-asset
layer emits loading and changed events for the engine JSON scope and each
opened Twin. Rhai receives complete snapshots, parses them with the shared
`parse_json` function, selects and validates the unique catalog, groups entries
by track in authored order, and returns generic workbench menu data. The
workbench renders the contribution and sends a selected item through the
`tutorials` Rhai tool in `assets/scripting/tools/tutorials.rhai`. The tool
submits `RunScenarioAsset` with a script, optional scene, parameters, and
`ScenarioReloadPolicy::Restart`. The Rhai menu payload uses the reflected
`Restart` variant. The command does not open a layer itself:
it submits a `SceneTransitionIntent`, USD composes the requested scene, and the
generic scenario driver starts after the scene/readiness lifecycle completes.
An omitted host target resolves to the active `WorldRoot`.

The Rust menu host validates and renders generic menu trees and dispatches
actions through the Rhai tool registry. A contribution is replaced by provider
when its asset scope changes and removed when a Twin closes; the tutorial
catalog, track grouping, path qualification, and launch policy remain authored
in Rhai; the shared asset layer supplies the canonical root for scope-relative
asset references.

The shared `lunco-workbench-guided-ui` package and Rhai prelude provide hints,
spotlights, coach cards, and objectives. They are reusable presentation and
scenario mechanisms, not tutorial ownership. Native asset loading rereads
authored files where supported, so Rhai and catalog edits can be replayed
without rebuilding the Rust core; wasm uses the same delivered asset tree.

The catalog is ordinary JSON presentation data, not a USD curriculum. Lesson
worlds remain ordinary USD scenes using standard composition (`subLayers`,
`references`, `payloads`), `UsdPhysics`, and `UsdLux`. Generic `LunCoProgramAPI`
is used only where a scene embeds a program; no tutorial-specific USD schema is
required.

For the complete authoring recipe, see [`../../assets/tutorials/README.md`](../../assets/tutorials/README.md)
and the [`author-tutorial`](../../skills/author-tutorial/SKILL.md) skill.

### Runtime tutorial tests

The test is an authored Rhai observer attached to a production scene fixture:

| Asset | Responsibility |
|---|---|
| `assets/scenes/tests/<name>.usda` | Composed world and lesson program |
| `assets/scenarios/tests/<name>.rhai` | Public command/event observation and verdict |

The observer must check the behavior being taught — for example, command
events, live movement or ports, and the final objective — rather than merely
waiting for `MISSION_COMPLETE`. It must not provide a second control path.
Shared assertions and `report_verdict(...)` come from
`assets/scripting/prelude/auto_tests.rhai`.

Run a gate directly after editing Rhai; it uses the already-built production
binary and does not require a Rust rebuild:

```bash
export LUNCOSIM_BIN="${LUNCOSIM_BIN:-luncosim}"
"$LUNCOSIM_BIN" test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

`"$LUNCOSIM_BIN" --validate` is only parse/preflight evidence. Generic
Rust tests may protect the scripting/lifecycle seam, but lesson-specific steps,
required events, and command counts belong in Rhai runtime observers.

## Authoring walkthroughs

| Tutorial | What you build |
|---|---|
| [00 — Create your first Twin](00-create-a-twin.md) | Create a small Twin and see how `twin.toml`, USD, Modelica, and Rhai divide ownership. |
| [01 — Lander → Rover mission](01-lander-rover-mission.md) | A reusable lander *vehicle* that flies itself down on a glowing engine plume, a scene that drops it into a mission, and an autopilot that drives the released rover through a waypoint course until you take over — with model-driven warnings, on-screen narration, and possession as the one source of control authority. |
| [02 — Author your own controller](02-authoring-a-controller.md) | Build a self-flying vessel from scratch: the control law in Modelica, logic in rhai, sensors + wiring + the `piloted` authority signal in USD — and a pilot who can take over. The layering behind every LunCoSim GNC. |
| [03 — Cosim: when a Model flies physics](03-cosim.md) | How a Modelica program and the physics engine exchange typed values at declared communication points, how USD connections become `SimConnection`s, and how to verify the live chain over the API. |
| [04 — Attach a simulation program](04-attach-a-program.md) | Attach a Modelica or Python source from the Models palette, author an explicit USD port contract, and verify the projected cosimulation participant. |

Each walkthrough pairs with an in-app lesson and the reference **[skills](../../skills/README.md)**:

| Walkthrough | In-app lesson | Reference skills |
|---|---|---|
| 01 — Lander → Rover mission | *Lander & Rover Mission* (luncosim) | [build-usd-scene](../../skills/build-usd-scene/SKILL.md) · [author-scenario](../../skills/author-scenario/SKILL.md) · [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) |
| 02 — Author your own controller | *Script a Rover* (luncosim) | [authoring-vessel-controllers](../../skills/authoring-vessel-controllers/SKILL.md) |
| 03 — Cosim: when a Model flies physics | *Cosim — Model meets Physics* (luncosim) | [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) · [inspect-simulation](../../skills/inspect-simulation/SKILL.md) |
| 04 — Attach a simulation program | No paired in-app lesson | [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) · [use-asset-library](../../skills/use-asset-library/SKILL.md) · [test-via-api](../../skills/test-via-api/SKILL.md) |

Walkthrough 04 remains a data/API authoring reference.

Looking for a reference rather than a walkthrough? The full script verb list is
in [`../scripting-guide.md`](../scripting-guide.md), the design behind scenarios is
in [`../architecture/34-scenario-and-multidomain.md`](../architecture/34-scenario-and-multidomain.md),
and every task skill is indexed in [`../../skills/README.md`](../../skills/README.md).
