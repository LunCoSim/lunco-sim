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
`prelude/input.rhai` helpers enqueue typed Bevy window events and exercise the
application event path after event creation. They bypass the OS device,
compositor, and winit event-delivery path, so they cannot verify that a physical
mouse reaches the app. egui, HUI, picking, focus, input bindings, and authored
Rhai tools process the injected Bevy events. Injected events do not mutate the
native window cursor. Bevy Picking's `PointerLocation` is the shared application
cursor state; cursor-driven consumers such as the transform gizmo read it
through `PrimaryMousePointer` in `lunco-interaction-core`.

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
scene selection. Selecting a point is separate from the `Move route point`
action: select enables the generic gizmo, while Move selects its explicit point
and arms click-to-place with a disposable ghost. That selected point remains the
move target until placement or cancellation. Hover alone must not arm movement.
The context gesture itself must not enable the transform gizmo. Ordinary clicks
carry `active_pointer_move { interaction_id, tool, hook, context }` only for
an interaction registered in their document. Placement consumes that context;
an idle click must not discover armed tools or routes by traversing USD. The hover
dispatcher scopes movement from the hit prim's document and carries the
same-document selected/control paths as route context. The hit prim's registered
`LunCoPointerInteractionAPI` must authorize that button as `context`; the generic
viewport adapter applies its per-button blocking behavior before the ordered-hit
pass, and route policy uses canonical hit paths rather than screen proximity.
Picking can target a child collider or terrain LOD entity without `UsdPrimPath`;
resolve the nearest ancestor prim before determining its document, as click
routing does.
The popup host registers the foreground menu rectangle with
`ScenePickGate` as chrome, even when that rectangle lies inside the 3D viewport;
menu clicks must not also start a gizmo drag. A semantic chord alone does not
create a menu. Unarmed route-edit
and selection clicks go through one `scene_interaction` Rhai policy; simulation
possession accepts only an exclusive `selection.replace` intent. Spawn, terrain,
attachment, and camera consumers are not yet under one captured gesture
manager. The editor gizmo has a local captured lifecycle: a same-frame handle
hit owns primary input through release or cancellation and suppresses preview
pan for that gesture. A route fixture passing does not prove global viewport
arbitration.

An authored pass-through hit can still emit its own Bevy pointer event. The
shared scene dispatcher must stop that hit's ancestor propagation before
de-duplicating the gesture, so a lower blocking or context target can receive
it. Disposable scene previews that overlap interactive geometry must author
pass-through behavior for the buttons they should not consume.

The retained Bevy UI backend shares the window target with scene picking. The
Workbench's UI camera sorts above the scene, so unmarked Bevy UI nodes would
block scene hits even when they are decorative. `RuntimeUiPlugin` requires UI
pick markers and marks only visible authored press controls and draggable
surface content as `Pickable`; keep that explicit target policy when adding UI
nodes. Native pointer routing and tool dispatch run in the application input
schedule before fixed simulation, so route context handling must not wait for a
physics tick. Scene-pointer observers use the picked hit and viewport-aware
chrome capture as the ownership decision. `EguiFocus` is published after
picking and can still describe the old cursor location during the first event
that leaves a menu; do not use that snapshot to reject a valid scene hit.

`RunRhaiTool` and `RunRhaiToolHook` callbacks use a bounded UI queue drained
after picking in `PreUpdate`, before fixed simulation. Keep authored pointer
and menu policy in those tool hooks rather than sending it through the general
REPL queue or a fixed-tick scenario. Heavy synchronous work in an input hook
would still occupy the application thread, so keep the hook bounded and move
preparation or I/O to its owning asynchronous boundary.
When a UI hook reads a collection of USD prims, request the member list once
and use `QueryUsdPrims` for their needed attributes, schemas, or relationships;
declare that provider in the caller's `query_reads`. Avoid per-member
`QueryUsdPrim` loops in input policy because every read occupies the same
application thread. Resolve a live USD entity with its owning document identity
when several documents can mount the same authored path. Generic `MoveEntity`
persistence resolves the write document from the moved entity's stage, not the
active editor tab.

Scene `Pointer<Move>` and `Pointer<Enter>` observers may collect raw hits, but
the viewport must dispatch movement only for a document with an active typed
`SetScenePointerMoveHook` subscription. Supply a caller-owned `interaction_id`
to both Set and Clear so stale cleanup cannot end a newer interaction.
Coalesce the newest raw hit per pointer and picking frame. With no active
subscriptions, drop samples before resolving scene identity; with subscriptions
on other documents, perform the lightweight document lookup and drop the hit
before coordinate conversion or terrain work. For an active interaction, a
fallback terrain raycast runs at most once per pointer per picking frame;
direct analytic surface hits do not need that fallback. Queue one typed hook
after resolution. Clear the subscription when the interaction ends; document
close, Twin close, and scene teardown own cleanup.
Enter provides the first sample after a scene hit replaces a popup's
previous-frame capture hit. Bevy can emit move events for both the pass-through
preview and lower hits. Before deduplication, skip the preview event and stop
its ancestor propagation; otherwise the preview can consume the cursor sample
intended for terrain beneath it. Keep the hook presentation-only: update the
live transform of the projected `@view@` preview through the generic typed
preview-transform command. It validates document and view-layer ownership and
uses the canonical active-frame/parent-local conversion; hover must not edit the
USD document, trigger projection, or sample a full terrain path. Route add and
delete hooks send the accepted route-point snapshot to `UpdateUsdCurveView`;
the presentation owner coalesces it and prepares sparse terrain-local strokes.
The terrain shader paints them on its own fragments without changing USD generation.
For a DEM route, require `InspectUsdCurveView.projection = "terrain_surface"`,
a positive `surface_binding_count`, and exactly one segment per authored leg;
the separate mesh stays hidden. Right-click identity comes from a foreground
terrain hit against the published stroke. Use `route_surface_annotation.rhai`
through `RunScenarioAsset` for publication and missing-coverage evidence; pass
its explicit `view_owner` scenario so the gate can isolate and restore that writer. The
next primary click commits a moved route point through the canonical
`@runtime@` USD edit path, whose projected change updates the ribbon once. The
`route_interaction` production gate verifies that Move selects and retains the
target, the live ghost follows a coalesced injected terrain cursor trace, and
document generation stays unchanged during preview. It sequences typed Bevy
press/release events on separate task ticks and waits for observable state with
the Rhai behavior tree. It does not verify OS, compositor, or winit input
delivery; actual headful input remains a separate check. Do not use
simulation-time sleeps or encode progression as numeric phase state.
The repeatable production gate is `assets/scenes/tests/editor/route_interaction/route_interaction.usda`,
run by `scripts/run_editor_scene_tests.sh`; it uses `InjectWindowInput` to send
typed Bevy window events through picking and verifies the mounted fixture,
waypoint hit, semantic context intent, unchanged pre-menu selection, live scene
selection in the focused editor owner, explicit Move menu action, ghost
placement, and deletion. This gate does not exercise physical OS mouse input.
Bevy emits one click observer per entity in its previous hover map, whose
iteration order is unspecified. The runtime scene router resolves from that
same event-source map by hit depth and the authored policy for the actual button
before it dispatches one scene event. The runner waits for
`/api/ready` and requires the API `Exit` command and port release after every
verdict.

Runtime-authored route edits belong in Twin `@runtime@`. Run
`scripts/run_scene_tests.sh --exact route_runtime_persistence` to exercise a
manifest-backed Twin through two production API sessions: add a point, verify
the `.lunco/runtime` sidecar write, then reopen and require that the point is
present before the scene's initial projection. This test is separate from the
isolated editor fixture gate because the latter intentionally disables
runtime-overlay I/O.

Coordinates are logical primary-window pixels. Obtain them from a current
screenshot and record the window geometry used for the run. For an Editor USD
preview, read `InspectUsdViewport`'s measured `image_rect` and `scale_factor`
to derive coordinates from the exact image area; keep authored fixture geometry
and the screenshot in the test review. A coordinate is test input, not domain
state: never use it to infer a USD position or replace the canonical
BigSpace/frame conversion.

## Verify each observable boundary

After a gesture, check the owning public surface instead of relying on the
absence of a notification:

- `InspectSelection` proves selection; `QueryUsdPrim` proves composed USD
  topology and authored relationships.
- `ScriptInspect` or a focused Rhai query proves program state and event
  delivery; `port(...)`, `owner_of(...)`, and `is_controlled(...)` prove the
  generic control boundary.
- `CaptureScreenshot` or an X11 window capture proves the visual result.
- For route workflows, assert the authored point count/revision, marker or live
  ribbon publication, `program_active`, and a nonzero guidance output after
  selecting the rover and pressing the action binding. Add, move, context-menu
  delete, and undo/redo are separate assertions over the same canonical USD
  document; do not treat a spawned ECS entity as persistence proof.

For repeatable acceptance, put the assertions in
`assets/scenarios/tests/*.rhai` and drive typed Bevy events from the scenario
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

## Telemetry catalog updates

Trace `SignalDescriptorsChanged` from `lunco-signal` to the persistent index in
`lunco-viz/src/telemetry_browser.rs`. Samples, API owner association, and selection do not
prepare descriptors. Channel admission, metadata, activity, removal, and changed owner
label/path/parent facts enqueue affected identities. Rewriting unchanged facts must queue no
work; comparison includes facts already captured by an in-flight worker. Each async batch captures and commits
at most 64 descriptors; incoming changes must not restart unrelated work or hide the tree.
Selection only updates focus membership and the visible-row index; cache inputs include
selected entity identities and their current USD paths. Ancestor facts are retained only while
indexed channels depend on them. Scene teardown cancels
workers and clears the outgoing index with a newer presentation key.

Use `InspectTelemetryCatalog` through `ExecuteCommand` to compare `initial_scans`,
`prepared_channels`, pending work, and capture/worker/commit costs before and after changes.
The optional exact `signal` path returns descriptors for all owners. Run
`scripts/api/test_telemetry_catalog.py` with an existing scene, measured entity id, real numeric
source port, and free API port. It owns its windowed production session and invokes the authored
Rhai verdict, including the missing-channel negative case; inspect its admission and settled
screenshots and confirm its session closes.
