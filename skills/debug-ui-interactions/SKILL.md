---
name: debug-ui-interactions
description: >
  Reproduce and verify LunCoSim desktop UI workflows as a user would perform
  them, including native key chords, pointer picking, scene selection, HUI
  actions, route editing, and camera controls. Use this for headful UI bugs;
  use test-via-api for headless or command-only verification.
---

# Debug native UI interactions

## Read first

Read [`skills/test-via-api/SKILL.md`](../test-via-api/SKILL.md) for the
production-runtime lifecycle and
[`docs/architecture/rhai-integration.md`](../../docs/architecture/rhai-integration.md)
for the typed input boundary. Use
[`skills/coordinate-frames/SKILL.md`](../coordinate-frames/SKILL.md) when a
screen hit, world position, camera frame, terrain point, or BigSpace grid is
part of the failure.

## Use the real application input path

Build or resolve the production binary as `LUNCOSIM_BIN`, launch exactly one
windowed `luncosim` process with an explicit free API port, and wait for
`/api/ready` before injecting input. The `InjectWindowInput` command and
`prelude/input.rhai` helpers enqueue typed Bevy window events. They do not call
a scene tool directly, so egui, HUI, picking, focus, input bindings, and
authored Rhai tools see the same application path as hardware input.

For a modifier gesture, use separate event phases and allow a frame between
them when the result matters:

```rhai
input_key_press("AltLeft");
input_pointer_move(x, y);
input_pointer_press("primary", x, y);
input_pointer_release("primary", x, y);
input_key_release("AltLeft");
```

Use `KeyF` for the default action binding only after checking the active
`input_bindings` setting; authored tests should use the semantic binding or
`input_binding(...)` rather than assuming a physical key. Use a secondary
pointer event for context menus. Do not combine press and release events in
one same-frame script step when testing focus, modifier state, or picking.

A route-point secondary click opens its authored context menu without changing
scene selection. Selecting the point is a separate menu action; the context
gesture itself must not enable the transform gizmo. The hit prim's registered
`LunCoPointerInteractionAPI` must authorize that button as `context`; the
generic viewport adapter applies its per-button blocking behavior before the
ordered-hit pass, and route policy uses canonical hit paths rather than screen
proximity. A semantic chord alone does not create a menu. Unarmed route-edit
and selection clicks go through one `scene_interaction` Rhai policy; simulation
possession accepts only an exclusive `selection.replace` intent. Spawn, terrain,
attachment, camera, and gizmo consumers are not yet under one captured gesture
manager, so a route fixture passing does not prove global viewport arbitration.
The repeatable production gate is `assets/scenes/tests/route_interaction.usda`,
run by `scripts/run_editor_scene_tests.sh`; it sends typed native-window input
through picking and verifies the mounted fixture, waypoint hit, semantic
context intent, unchanged pre-menu selection, and explicit menu selection
action. The runner waits for `/api/ready` and requires
the API `Exit` command and port release after every verdict.

Runtime-authored route edits belong in Twin `@runtime@`. Run
`scripts/run_scene_tests.sh --exact route_runtime_persistence` to exercise a
manifest-backed Twin through two production API sessions: add a point, verify
the `.lunco/runtime` sidecar write, then reopen and require that the point is
present before the scene's initial projection. This test is separate from the
isolated editor fixture gate because the latter intentionally disables
runtime-overlay I/O.

Coordinates are logical primary-window pixels. Obtain them from a current
screenshot and record the window geometry used for the run. A coordinate is
test input, not domain state: never use it to infer a USD position or replace
the canonical BigSpace/frame conversion.

## Verify each observable boundary

After a gesture, check the owning public surface instead of relying on the
absence of a notification:

- `InspectSelection` proves selection; `QueryUsdPrim` proves composed USD
  topology and authored relationships.
- `ScriptInspect` or a focused Rhai query proves program state and event
  delivery; `port(...)`, `owner_of(...)`, and `is_controlled(...)` prove the
  generic control boundary.
- `CaptureScreenshot` or an X11 window capture proves the visual result.
- For route workflows, assert the authored point count/revision, marker or
  ribbon projection, `program_active`, and a nonzero guidance output after
  selecting the rover and pressing the action binding. Add, move, context-menu
  delete, and undo/redo are separate assertions over the same canonical USD
  document; do not treat a spawned ECS entity as persistence proof.

For repeatable acceptance, put the assertions in
`assets/scenarios/tests/*.rhai` and drive the native events from the scenario
with `RunScenarioAsset` in the already-running production session. Keep each
phase event-driven or bounded by a timeout, and emit one terminal authored
verdict. This is a headful interaction test, not a Rust test and not a direct
call to `waypoint_editor`.

## HUI boundary

Runtime HUI is the native HTML-like surface documented by
[`skills/runtime-ui/SKILL.md`](../runtime-ui/SKILL.md), not a browser DOM. Use
the generic typed action surface and Rhai-authored labels/options. Do not add
JavaScript, browser automation, or a Rust branch for a domain-specific button.

## Display and workspace handling

When several agents run graphical sessions, use the same `DISPLAY` and
Wayland/X11 environment as the agent shell. On X11, inspect
`xprop -root _NET_CURRENT_DESKTOP` when diagnosing workspace placement. If the
compositor does not expose that EWMH atom, there is no reliable workspace id
the application can target; starting on the current display is the only
portable best effort. Do not add application state or a fake workspace
selector to compensate for a compositor capability that is not exposed.

## Stop cleanly

Stop the session with the API `Exit`, then verify both the process and API port
are gone before replacing your own session. Agents may use separate ports; never
control another agent's session, use `pkill`, or
reuse a port owned by another agent. A screenshot, command acknowledgement, or
open TCP socket alone is not a completed UI test; hand off the exact runtime,
input sequence, queries, screenshots, verdict, and any remaining compositor
limit.
