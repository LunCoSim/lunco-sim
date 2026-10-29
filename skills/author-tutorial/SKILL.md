---
name: author-tutorial
description: >
  Author an interactive tutorial, guided lesson, onboarding flow, coach-mark
  tour, or objectives checklist in LunCoSim. Use for work under
  `assets/tutorials/` and for requests involving `mission`, `objective`,
  `coach_step`, `hint`, or `spotlight`.
---

# Author an authored lesson

A lesson is a file-backed Rhai scenario with optional standard USD scene
content. The application Rhai policy reads generic JSON asset snapshots,
selects the tutorial catalog, and builds the workbench menu. The shared Rust
asset and UI layers know only text assets, JSON values, and generic menu trees.
The lesson does not need a Rust type, registry, lifecycle owner, or custom USD
schema.

Read [`author-scenario`](../author-scenario/SKILL.md) first, then use
[`assets/tutorials/README.md`](../../assets/tutorials/README.md) and the
examples under `assets/tutorials/`.

## Add a lesson

1. Add `assets/tutorials/<track>/<name>.rhai`.
2. If it needs a world, reuse or add an authored scene under `assets/`.
3. Add an entry to `assets/tutorials/catalog.json`:

```json
{
  "track": "Sandbox",
  "title": "First Drive",
  "blurb": "Take control of a rover and drive it to a lunar flag.",
  "difficulty": "beginner",
  "lesson_type": "exercise",
  "duration_minutes": 5,
  "source_asset": "lunco://tutorials/sandbox/first_drive.rhai",
  "scene_asset": "lunco://tutorials/sandbox/first_drive.usda"
}
```

The `track` value determines the submenu containing the lesson. Reuse an
existing track when the lesson belongs to that learning path. The Rhai policy
at `assets/scripting/policy/application_asset_lifecycle.rhai` discovers the unique
marked catalog from generic asset-scope events and uses the shared `parse_json`
function; do not add tutorial-specific Rust loading or menu code. The reserved
experiment-results namespace is classified by the asset owner and excluded by
the menu policy before parsing. Keep catalogs in authored source directories.
The `application_json_catalogs` production gate covers catalog selection,
result exclusion, similar directory names and malformed authored entries.

For classroom onboarding, use the maintained
[Lunar Rover Lab curriculum](../../docs/tutorials/06-classroom-onboarding.md).
`lesson_type` distinguishes a guidance tour from an assessed exercise or an
independent challenge; `duration_minutes` is an estimate, not a measured result.
Both fields are optional for catalogs that do not publish that metadata.
An optional `next_source_asset` links to another entry in the same catalog.
The policy validates targets and cycles and passes a typed `lesson_path` to the
scenario; do not duplicate a curriculum registry in Rust or a second tool.

A project-authoring lesson may name `twin_asset` with an exact registered
`twin.toml` URI. Its manifest selects the scene and the scene embeds its guide.
The launcher submits `OpenTwinAsset`; the asset adapter resolves it off-thread
and Workspace owns admission. Do not also specify `scene_asset` for that entry.
Resolve a live prim's owning document once with
`assembly_edit::document_for_entity`, and retain that explicit id across panel
changes. A detached library demonstration stage has no editable document owner.

The menu submits the generic `RunScenarioAsset` command. It uses
`ScenarioReloadPolicy::Restart` for a predictable fresh start and lets the
command resolve an omitted host target to the active `WorldRoot`. Other apps
can reuse the command without importing tutorial code.

## Rhai contract

Use the shared prelude:

- `hint(...)`, `spotlight(anchor, caption)`, and `notify_kind(...)` for
  presentation;
- `coach_step(steps, index)` with `tutorials::tour_event` for a guided tour;
- `mission(me, ctx)` and `objective(...)` for an objective-driven exercise;
- `input_binding(...)`/`input_hint(...)` for the controller-owned semantic
  labels.

LunCoSim defaults complete HUD cards to View. A lesson that guides work across
perspectives declares `hud_perspectives([])` in its initialization; a lesson
limited to particular layouts declares their registered ids instead. The
declaration applies only to that script's card, including objectives and input.
Use ordinary `hint(...)` and mission helpers afterward. Spotlights and coach
steps use their existing layout anchors. Do not inspect lesson names or special
case tutorial assets in Rust to choose visibility.

Use `panel.center` for the viewport or central work area, a generic
`panel.side_browser`, `panel.right_inspector`, or `panel.bottom` anchor for a
whole dock, and `panel.<id>` for a specific panel. Choose an anchor published
by the active layout; `coach_step` focuses a named panel before drawing its
spotlight.

Progression must observe semantic commands or authoritative state. Never gate a
lesson on a physical key name or a timer. A lesson must not open a USD layer
directly; if it needs a world, the catalog's `scene_asset` is the request.
Never branch on the Rust build profile (`is_debug()` or `debug_assertions`). The
authored `lint.rhai` policy rejects that coupling; use `is_unattended()` when
attended and automated execution need different behavior.

Tours may advance on Next because they review guidance. Their completion must
not claim that unobserved editing or engineering actions occurred. Use the
shared cursor and navigation policy:

```rhai
fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
```

Initialize `this.i = 0` and show the opening coach in `on_start`.
The helper makes Skip terminal and emits `TUTORIAL_SKIPPED`, without emitting
`MISSION_COMPLETE`. An exercise instead calls `tutorials::finish(ctx, message)`
after its own verified result, and handles `tutorials::navigation` in its event
hook. A standalone launch without catalog context has no next-lesson action;
its copy should direct the learner to Tutorials rather than promise a dialog.
`tutorials::start_path` relaunches a guide with its exact calling Twin owner;
Restart uses this same path and preserves scene edits. Do not use `RestartScene`
as a guide restart: a scene-free tour has no scene to reload. An explicit
document-reset action must still state that it discards edits.

For a saved report, resolve the current application lesson with
`tutorials::application_path(source_asset)` in the interactive presentation
path. It reads enabled typed actions from `InspectScriptedMenus`, whose reader
and renderer share the installed immutable trees. Use the returned titles and
path for continuation instead of duplicating catalog destinations. Unknown or
ambiguous lessons reject visibly. Keep headless report assessment independent
of windowed menu presentation. The focused acceptance is
`scripts/api/test_tutorial_navigation.py`.

Use existing guided action buttons for direct control/help and the generic
`RecoverVessel` command for explicit righting. Never treat command admission,
a zero trace, a file picker, or a save request as a successful result. For a
saved project, verify the owning document's saved state and Twin admission.
`QueryUsdPrim` includes disposable live-stage state. For persistent authoring
assessments, inspect document-layer ownership too and verify save/reopen;
an ECS move alone must not count as a saved edit.
Store measurements as typed authored values with their assumptions; do not
silently substitute zero when a measurement is unavailable.

For physical waypoint arrival, author an Avian sensor in USD and map its generic
`SENSOR_ENTER` event to a lesson event in Rhai. Match `evt.source` to the exact
sensor with `sensor_entered(evt, sensor_id)`, then validate that `evt.value`
identifies the intended subject or one of its colliders before emitting the
lesson event. Use `SensorOccupants` in `on_start` to handle an existing contact.
Objectives and task waits consume the lesson event; distance thresholds do not
define arrival. Do not add a tutorial-specific `TriggerZone` label to a shared
sensor asset.

Example objective:

```rhai
fn mission(me, ctx) {
    [
        objective("take_control", #{
            text: "Take control of the training rover",
            done: |m| {
                let role = controller(find_path("/Training/Rover"));
                role != () && role != "AiAgent"
            },
        }),
        objective("reach_flag", #{
            text: "Drive to the glowing flag",
            requires: ["take_control"],
            requires_event: "flag_reached",
        }),
    ]
}
```

## USD scene contract

Scene ownership stays in the USD scene command layer. `RunScenarioAsset`
submits a `SceneTransitionIntent`; the USD owner resolves and composes it, and
the generic scenario driver waits for the completion/readiness edge.

Use standard USD composition and schemas: `subLayers`, `references`,
`payloads`, `UsdPhysics`, and `UsdLux`. For a repeatable celestial lesson,
author a non-zero epoch on the scene root with the celestial payload. Otherwise
the startup-installed scene-time policy selects current computer UTC converted
to TDB after the scene and its queued projections settle. Time-dependent
consumers wait for that result. Missing time with celestial sources is reported
by runtime warning and lint. For a basic or UI lesson, author a fixed
`DistantLight` or omit the world. Do not add tutorial-specific API schemas.

## Test without a Rust rebuild

Put behavior assertions in `assets/scenarios/tests/<name>.rhai` and use the
production scene-test binary. Keep Rust tests limited to generic scripting,
asset, USD, and lifecycle seams.

For a navigation-only lesson, omit the world payload and use
`// @timing presentation`. Launch it with the exact `owner_twin_id`; Twin
ownership does not require scene ownership. Check `InspectGuidedHud.tour` for
the owner, step and anchor, advance Next/Back while no scene is admitted, then
replace the Twin and verify the tour cannot return. The production acceptance
is `python3 scripts/api/test_workbench_perspectives.py --scene-free --port PORT`;
it launches its own windowed session and requires the authored verdict.

```bash
"$LUNCOSIM_BIN" test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

`--validate` proves preflight only. Inspect the authored verdict and process
exit code for runtime evidence. Rhai, catalog, and authored USD edits should
be replayed through the already-built production binary whenever possible.
