# Design: authored guided scenarios

**Status**: Implemented

## Ownership

Tutorials are application content, not a Modelica or simulation domain. The
repository keeps authored lesson files under `assets/tutorials/`; the
application menu is the only Rust surface that names the tutorial catalog.
The following layers remain generic:

| Concern | Owner |
|---|---|
| Lesson copy, objectives, sequencing, and policy | Rhai scenario assets |
| Catalog presentation and menu dispatch | Application UI (`lunco-luncosim`) |
| Script asset identity and loading | `lunco-assets` / `lunco-scripting` |
| Scene resolution, USD composition, and scene lifecycle | USD scene command layer |
| Scenario start/stop, event delivery, and reload policy | `lunco-scripting` |
| Coach cards, hints, objectives, and widget anchors | generic `lunco-workbench` guided overlay |
| Modelica equations and continuous state | `lunco-modelica-core` |

There is no tutorial crate. Modelica crates expose Modelica editor/runtime
capabilities only; the luncosim application may project authored lessons into a
menu. The same generic scripting and scene commands are available to other
applications without importing that menu.

## Authored content

`assets/tutorials/catalog.json` contains presentation and launch data:

```json
{
  "track": "Sandbox",
  "title": "First Drive",
  "blurb": "Take control of a rover and drive it to a lunar flag.",
  "difficulty": "beginner",
  "source_asset": "lunco://tutorials/sandbox/first_drive.rhai",
  "scene_asset": "lunco://tutorials/sandbox/first_drive.usda"
}
```

The catalog is not a runtime state store. Progress, if a future application
needs it, belongs to that application's settings. A lesson is not a special
USD prim and does not require a tutorial-specific schema.

## Generic launch and reload contract

The application menu dispatches `RunScenarioAsset`:

- `target` selects the scenario host; `Entity::PLACEHOLDER` resolves to the
  stable `WorldRoot` for a convenient application default;
- `source_asset` names a Rhai asset through the normal asset graph;
- `params` carries optional scenario parameters;
- `scene_asset` optionally submits a `SceneTransitionIntent` to the USD scene
  owner;
- `reload_policy` is `retain` by default or `restart` when the caller wants
  `on_start` to run again after a scene replacement.

The scripting command never opens a USD layer directly. The scene owner
resolves and composes `scene_asset`, publishes the normal transition lifecycle,
and the generic scenario driver starts after readiness is released. This makes
the same mechanism useful for lessons, demos, onboarding, and automated
scenario launches.

`restart` is the menu's user-experience choice: choosing a lesson produces a
fresh, deterministic start. `retain` remains the generic API default for a
caller attaching policy to an already-running world.

## Rhai behavior

The shared prelude exposes `hint`, `spotlight`, `coach_step`, `mission`, and
`objective`. Lesson progression observes semantic command events or
authoritative state. It does not read physical key names, poll a bespoke Rust
state machine, or use timers as completion evidence.

The guided overlay is presentation only. It does not own lesson identity,
progress, scene loading, or lifecycle. `input_hint(...)` resolves the current
controller-owned binding for copy, so changing user settings does not make a
lesson's control instructions stale.

## USD contract

Lesson worlds are ordinary USD assets. Use `subLayers`, `references`,
`payloads`, `UsdPhysics`, and `UsdLux` according to the scene's purpose.
Celestial scenes author an explicit epoch when opting into celestial time;
fixed-light scenes author their `DistantLight`; UI-only scenarios omit a scene.
The scene's authored facts are authoritative. No tutorial runtime fallback
repairs missing lighting, time, relationships, or physics metadata.

`LunCoProgramAPI` remains the generic scene-embedded program binding for any
domain. It is not a tutorial schema and is not needed by a menu-launched
`RunScenarioAsset` lesson.

## Verification

Authored runtime tests live under `assets/scenes/tests/` and
`assets/scenarios/tests/`. They run through the production `luncosim` binary,
assert public commands/events and real state changes, and report a verdict.
Rust tests cover only generic asset parsing, USD composition, scripting
lifecycle seams, and pure engine behavior that authored scenarios cannot
observe.

```bash
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

`--validate` is preflight evidence only. Editing a Rhai script, catalog, or
authored scene should be replayable without rebuilding the Rust core whenever
the production asset loader supports the target platform.
