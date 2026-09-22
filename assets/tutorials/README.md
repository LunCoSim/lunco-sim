# Authored lessons

Lessons are ordinary file-backed Rhai scenarios with optional USD scene
content. The generic text-asset layer publishes asynchronous JSON-scope events;
the application Rhai policy parses the catalog, builds the menu contribution,
and launches a selected entry through the generic `RunScenarioAsset` command.
Rust contains no tutorial catalog or menu implementation, lifecycle crate, or
tutorial-specific USD schema.

```
assets/tutorials/
  catalog.json                 # app-menu metadata and launch references
  basic/                       # rover-driving scenarios and their worlds
  luncosim/                    # LunCoSim UI/control scenarios
  lunica/                      # Modelica-workbench scenarios
  perspectives/                # workbench-navigation scenarios
  sandbox/                     # scene-building and co-simulation scenarios
```

The catalog is presentation data. Each entry names a `source_asset`, may name a
`scene_asset`, and belongs to a `track`; the application presents each track as
a submenu and keeps the entries in authored order. Both assets are resolved
through the asset system. The menu uses `reload_policy: "restart"` so selecting
a lesson always gets a fresh scenario start after its requested scene is
composed. Other applications can reuse the same generic command with their own
catalog or no menu at all.

An opened Twin may contribute the same catalog shape in any JSON asset under
its root, provided it carries `kind = "lunco.tutorial-catalog.v1"`. The shared
asset layer asynchronously loads indexed JSON assets and sends a complete
scope snapshot and canonical URI root to the application policy. Rhai uses
`parse_json`, validates the unique catalog, resolves relative references under
that scope root, and contributes the tracks under the Twin's name. Closing the
Twin removes its menu contribution. A Twin lesson that needs a world must name
its scene explicitly, so selecting it cannot accidentally keep an unrelated
scene from the previous lesson.

## Authoring a lesson

1. Add a `.rhai` file under a track directory. Use the shared scripting prelude
   for `hint`, `spotlight`, `coach_step`, `mission`, and semantic command/event
   observation. A script does not open USD layers directly.
2. Add one catalog entry in the application catalog JSON. Use a standard
   authored scene under `assets/` when the lesson needs a world; omit
   `scene_asset` for a UI-only lesson. Twin-owned lessons use the same fields
   in a Twin-owned catalog JSON, with Twin-relative source and scene paths.

Scene ownership remains with the USD scene command layer. `RunScenarioAsset`
submits a `SceneTransitionIntent`; USD resolves and composes the scene, then
the generic scenario driver waits for the scene/readiness lifecycle before
starting the script.

Use USD's existing composition vocabulary in scene assets: `subLayers`,
`references`, `payloads`, `UsdPhysics`, and `UsdLux`. A scene that depends on
celestial time must author its epoch and opt into its celestial payload; a
fixed-light scene should author its `DistantLight` explicitly. Do not add a
lesson-specific schema or hide a missing environment with a runtime fallback.

Tutorial controls use semantic input bindings (`input_hint(...)`) and
progression uses semantic commands or authoritative state. Do not hardcode
physical keys or advance from a timer.

## Test without rebuilding Rust

Put runtime assertions in `assets/scenarios/tests/` and execute them through
the production scene-test binary. Keep Rust coverage limited to generic
scripting, asset, USD, and lifecycle seams.

```bash
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

`--validate` checks preflight only; it does not prove runtime behavior. Editing
Rhai, catalog metadata, or authored USD should use the already-built binary
where possible.
