# Routes and route points in USD

Route geometry and route execution are program-level concerns. A route is an
ordinary USD scope containing reusable route-point prims and a child Rhai
program. The scope may be authored in the scene or in a separate route-plan
USD asset that the scene composes at its canonical path. New and edited route
points are opinions in the document's `@runtime@` layer, saved to the owning
Twin's `.lunco/runtime/<scene-path>` sidecar when
`usd.runtime_persistence` is enabled. The source scene and route-plan files
remain untouched. The subject is an authored relationship on the program, so a
vehicle does not own a waypoint list and the Rust core does not know about
vehicles or autopilot programs.

## Ownership

| Concern | Owner |
|---|---|
| Route identity, point placement, trigger radius, look, and subject relationship | Composed USD |
| Route sequencing, enable/disable policy, mission reactions | Rhai program |
| Avian sensor transitions (`SENSOR_ENTER` / `SENSOR_EXIT`) | Generic physics/sensor runtime |
| Optional named geofence events (`enter:<zone>` / `exit:<zone>`) | Generic physics/sensor runtime from `TriggerZone` metadata |
| Steering math and named-port writes | Generic navigation/port mechanisms |
| Equations, actuator dynamics, and contact response | Modelica / Avian |
| Route ribbon presentation | Reusable `waypoint_editor` Rhai tool + standard USD BasisCurves asset in the disposable `@view@` layer |

The standard reusable marker is
[`assets/markers/route_point.usda`](../../assets/markers/route_point.usda).
It contains a visual-only dome and a separate invisible overlap sensor. The
asset uses standard USD geometry/material properties plus the registered
project schemas needed for a trigger zone and billboard presentation. Its
sensor emits generic transitions whether or not it has a zone label. A
non-empty `lunco:triggerZone` adds named geofence events for route policy. It is
a route annotation, not a vessel component.

## Scene shape

```usda
def Scope "Route"
{
    def Scope "Program" (prepend apiSchemas = ["LunCoProgramAPI"])
    {
        uniform token info:implementationSource = "sourceAsset"
        uniform asset info:sourceAsset = @lunco://scenarios/route_follow.rhai@
        rel inputs:subject = </Traverse/Rover>
    }

    def Xform "P0" (
        prepend references = @lunco://markers/route_point.usda@</RoutePoint>
    )
    {
        over "Trigger" { token lunco:triggerZone = "route_p0" }
    }
}
```

The program reads its own composed USD parent, enumerates point children in
authored order, resolves their poses, and reads `inputs:subject`. It does not
copy coordinates into Rust or into the subject. Multiple paths are ordinary
siblings, for example `/Traverse/RouteNorth/Program` and
`/Traverse/RouteSouth/Program`, or distinct scopes composed from separate
route-plan files. The subject binds every executable with the plain USD
`programs` relationship:

```usda
rel programs = [
    </Traverse/RouteNorth/Program>,
    </Traverse/RouteSouth/Program>
]
```

The program browser selects one canonical program path at a time. That
selection is transient session state, not a scene fact and not a `/Route`
convention; Alt-click, F, and route presentation resolve against it. With one
bound program the tool can use the relationship directly. With several, it
refuses to guess and requires explicit selection. A program emits the generic
typed `program.ready` event after its `on_start` hook has completed. Runtime
controls that send a one-shot gesture or semantic edge after a source switch
wait for that event instead of using a fixed delay or racing the hot-reload
boundary.

Route authoring does not require possession of the route subject. When the
selected prim is a route point, the editor resolves its enclosing route scope
and uses that program's `inputs:subject` binding only to identify the rover;
the selected waypoint is never mistaken for the rover. If several routes are
available, select the intended route program or subject before editing.
Delete accepts a selected route point when the command has no pointer target;
when pointer paths are present, they remain authoritative.

The live scene can finish attaching its USD document after the route program's
visualization hook. In that case the program binds its document and route paths
on the matching `usd.document.projected` event. `on_visualization` prepares the
disposable ribbon after the scene and terrain inputs settle, without waiting
for Modelica admission. The later `on_start` performs occupancy and control
setup, then emits `program.ready`; the route task stays idle until its document
binding is available instead of retrying an unbound identity each pass.

When a route plan is stored separately, keep the route scope and its points in
that file and reference or payload the scope into the scene; do not duplicate
points in the scene and the plan. The scene owns subject placement and its
`programs` binding, while the route-plan file owns route topology. A
document-backed live edit still uses the composed canonical path and the
existing USD journal boundary.

The route tool derives a ribbon from the same point children after the canonical
USD projection has settled. Route-point topology and transforms are durable
`@runtime@` edits; the ribbon and visited-marker colors are disposable `@view@`
presentation. The view layer projects into the live scene while open, but does
not enter Save, runtime-sidecar persistence, or the journal. It references the reusable
[`assets/markers/route_ribbon.usda`](../../assets/markers/route_ribbon.usda)
asset as a child of the route scope and writes only generated `BasisCurves`
opinions to the document's `@view@` layer. Keeping the view
under the route is a frame invariant: the ribbon anchor and every route point
are expressed in the same USD parent space, so a transformed scene scope
cannot put the overlay in a different frame. The Twin therefore contains no
persisted ribbon prim: removing the view layer leaves the authored route
unchanged, and another Twin can use the same tool without importing a
Twin-specific presentation object.

Route points remain ordinary selectable USD prims after authoring. The standard
scene gizmo persists translation/rotation through the generic runtime-layer
authoring commands, including local overrides for referenced children. The
standard Delete command removes a runtime-only point and deactivates a
base-authored or referenced point in the runtime layer; it never tries to
remove a spec from a layer that does not own it. Durable changes are journaled
and feed the same route revision/ribbon refresh. Runtime-layer snapshots are
serialized and written asynchronously, with newer revisions coalesced while a
write is in flight; no whole-scene serialization or file I/O runs in the
pointer handler. Ribbon point readback comparison respects USD `point3f`
precision, so a stable f32 value does not trigger repeated geometry writes.
For a point below a reference, payload, or selected variant, the canonical
composed path is the edit identity: the stronger local layer authors an `over`
and the transform opinion there, and undo/redo removes or restores only that
local opinion.
The document layer is authoritative during the short interval before the
canonical projection contains a newly authored point. Move and delete resolve
that local target immediately, while a point-name or composed-stage read that
is not synchronized reports a retryable authoring error rather than guessing
or silently reusing an existing path. Once mounted, `QueryUsdPrim` reads the
current canonical stage even at generation zero, so it observes inactive
referenced points and live `@view@` previews instead of the loader's initial
snapshot. A document-scoped query can observe an exact generation after it has
reached the mounted stage, even while the ECS projection cursor is catching up;
operations still pending at the stage boundary remain retryable. Synchronous
document preflight composes base, runtime, and view at their actual USD layer
strengths against the loaded dependency closure. It sees children from the
selected variant when validating a move or route `primOrder` edit, including for
document-scoped queries before the canonical live stage mounts by using the
loaded stage recipe. Once the live entity exists, the same canonical path is
used for selection and menu dispatch.

## Progression

Every Avian sensor transition emits `SENSOR_ENTER` or `SENSOR_EXIT`, with the
sensor GID in `TelemetryEvent.source` and the moving body's GID in the event
value. This identity-based event does not require a `TriggerZone` label. A
non-empty label additionally emits `enter:<zone>` or `exit:<zone>` for authored
geofence policies that use names. The route program consumes the named form,
accepting an enter event when its payload is the current subject or a registered
descendant collider in that subject's generic parent chain, and the zone is the
next unvisited point in authored route order. An out-of-order enter does not
change visit state or advance the route; the point must be entered again after
its predecessors. At startup, only the first unvisited point can be marked from
`SensorOccupants`, because a simultaneous occupancy snapshot does not establish
arrival order. An accepted visit changes that point's reusable marker to its
visited colour through the generic transient USD view tool, emits the authored
route-level `route_point_reached` event for mission policy, and advances the
local cursor. Physics reports sensor events; Rhai maps those facts to route
progression.

Route progress is keyed by canonical USD point path and survives disabling and
re-enabling the program. Initial scenario admission waits for all scene
readiness holds because the route program references a separate subject through
`inputs:subject`. Avian's `SensorOccupants` query reads current touching moving
bodies by stable sensor id. At `on_start`, the route checks the next unvisited
point and marks it visited if occupied before autopilot is enabled. Later
`enter:<zone>` events mark only the next unvisited point while the program is
disabled and advance the active cursor when enabled. Each start resumes at the
first unvisited point from the ordered visits, including a contact established
before the script started.
The transient marker state follows those route visits; no visit is authored to
the USD asset.

The route task remains live while the scene is running. Editing point placement,
point order, or the subject relationship is observed through the normal USD
projection path. Those edits update route facts and presentation only; they do
not re-project or recreate the live route owner, subject, physics, Modelica
state, or possession. The program's `this.enabled`, cursor, and current point
are runtime state in the Rhai scenario instance. The authored
`inputs:enabled` value is only the initial scene policy and is never written
back when the operator presses F. If a point edit briefly leaves a target
unprojected, the task waits for the generic projection to settle; it does not
persist runtime state into USD or emit a false stop.
The program retains the last resolved subject entity while a structural USD
edit briefly reprojects its relationship, so a one-shot control edge or
safe-stop action cannot be lost to an unrelated point edit.

## Interaction

For unarmed route editing and ordinary selection, the viewport adapter builds
one typed context and dispatches it to the `scene_interaction` Rhai policy.
That policy gives route editing first refusal, then routes an eligible primary
gesture to the generic selection command. Physical pointer chords are resolved
by the persisted shared `input_bindings.pointer_bindings` settings, which
publish open-ended semantic names in `context.pointer_intents`; authored
policies never hardcode Alt, mouse buttons, or modifier combinations. Rust
resolves the canonical document, prim paths, screen position, raw diagnostic
metadata, semantic intents, and coordinates; it does not decide that a click
means “waypoint”. `world_position` is a typed `point3` map in the
`active_physics` frame; `render_position` is a separate floating-origin point
for presentation diagnostics only. `pointer_point(context)` is the standard
Rhai entry point and returns an explicit error when the active frame is
unavailable. The popup host renders authored menu items and dispatches their
typed tool hooks.

This router is not yet a global gesture manager. Spawn, terrain, attachment,
possession, camera, and gizmo paths still have engine-owned input consumers.
Their active modes must remain mutually exclusive and the native gate must
prove which owner received the physical gesture. The target contract is one
captured gesture with one owner, selected by typed Rhai policy and applied by
generic Rust mechanisms; a tool-specific observer must not race selection or
another armed tool.

The USD `LunCoPointerInteractionAPI` is enforced by the generic viewport
adapter per mouse button before Bevy computes ordered hits. This preserves a
primary pass-through marker while making the same visible marker the secondary
context target. Route editing resolves only canonical paths in those hit facts;
screen distance is never used to guess which waypoint was clicked. The adapter
translates authored hit behavior into Bevy's backend contract; it does not
choose a route action or tool owner.

Pass-through hits can still emit their own Bevy pointer event, while the
`Pickable` hit map also includes lower hits. The shared dispatcher stops the
pass-through event's ancestor propagation and returns before click and hover
de-duplication, so the independently emitted lower blocking or context event
can own the gesture or terrain position. The move preview and its Dome child author
pass-through behavior for both buttons, so their visual overlap cannot consume
waypoint selection, context clicks, or move-preview terrain samples.

The `Move route point` context-menu action selects the point and arms placement
for the next ordinary primary scene hit. The selected point path remains its
identity; the editor writes a `SetTranslate` opinion to `@runtime@` and leaves
the route program and live subject intact. Pointer movement is coalesced per
pointer and picking frame, retaining only its newest position from scene Move
or Enter hits before the typed UI hook enters the bounded script queue. Enter
supplies the first scene sample when a cursor leaves a menu and the chrome hit
from the prior picking frame is being retired. Scene pointer consumers use the
picked hit to distinguish scene input from chrome; they do not gate that hit
with the later-published `EguiFocus` snapshot. Rhai creates a translucent `Xform` and
Dome in the disposable `@view@` layer, with a view-layer target child that
identifies the armed point. The Dome copies the selected marker's authored
radius and local offset. Hover sends a typed `SetUsdViewPreviewTransform`
command that validates the projected target against its document and view-layer
authorship, converts the active-frame position through the target's real parent
hierarchy, and updates only its live render transform. It does not edit the USD
document or trigger projection. Preview creation and cleanup each use one
transient view-layer edit; only the placement click authors the waypoint's
canonical `@runtime@` position through the existing `MoveEntity` command, which
owns active-frame conversion and persistence. Selecting a point alone does not
arm movement.
The terrain-sampled ribbon is rebuilt after commit, not on each hover event, so
preview work does not batch-sample terrain or resample curve geometry.

The point context menu labels insertion by its placement: interior actions name
the neighboring points and midpoint, while endpoint actions name the point and
the fixed five-meter extension. Each inserts a new ordinary marker and authors
USD `primOrder` from the same settled list of active composed route points in
the runtime operation group, excluding inactive referenced points; new point
names also reserve authored-but-inactive children, so insertion cannot reuse a
path whose inactive opinion would keep the new point out of the composed route.
Existing point paths and route-progress identities do not change. Interior
insertions use the adjacent-point midpoint. Endpoint insertions extend five
meters outward along the neighboring segment's direction. A single-point route
has no direction to infer, so the menu does not offer these actions yet.
Right-clicking the ribbon offers `Add point at cursor on route`; its generated
`BasisCurves` prim receives the standard pointer-interaction API in `@view@`,
and the action uses the ribbon hit's world position, snapping its height to
terrain when the terrain query has a support point.

Rhai owns the gesture's meaning and returns typed semantic actions. Rust owns
pointer sampling, ordered hit testing, capture, continuous gizmo handle math,
and generic action application. In particular, a gizmo drag can be exposed to
Rhai as a typed lifecycle/policy decision, but its high-frequency ray tests,
transform math, and journaled commit remain engine mechanisms. The current
gizmo path is not yet routed through the global owner/capture policy, so the
route-specific gate proves only the unarmed route/selection path.

The `route.context` semantic intent opens the authored waypoint menu only when
the hit prim's registered `LunCoPointerInteractionAPI` marks that button as
`context`. The shared Rhai router gives route editing first refusal and then
dispatches generic selection for eligible primary gestures. Only the explicit
“Select route point” action selects the point and enables its transform gizmo;
“Move route point” also selects it and arms ghost placement. The popup host
registers its foreground egui bounds as chrome in
`ScenePickGate`; this lets menu rows own clicks that overlap the selected
point's transform handles. The windowed `route_interaction` production gate
requires the fixture root in the live editor, verifies possessed right-click
context on the waypoint, verifies selection alone leaves movement unarmed,
then chooses Move, previews and commits a native click-to-move, and uses the
production menu input path to delete the moved point. The menu carries
the document, enclosing route, and canonical point as its action context, so
its delete callback does not depend on viewport query scope or rover
possession. When a controlled rover remains selected, a route-bearing pointer
target takes precedence over that stale selection; this lets right-click target
the route point actually under the pointer. The production `route_lifecycle`
gate covers route selection precedence, explicit move arming, hover ghost
placement, insertion order, and ribbon context policy.

Pointer and menu tool hooks use the bounded UI queue, which runs after Bevy
picking in `PreUpdate` and before fixed simulation. General `RunRhai` requests
stay in the separate `Repl` queue, so unrelated script work cannot delay route
input policy. The `route_interaction` gate sends a burst of cursor samples and
requires the live ghost to follow within half a second without advancing the
USD document generation during hover.

In the editor, the runtime edit panel identifies `@runtime@` as the target for
route points, runtime spawns, and gizmo edits. Its Twin setting tells the user
whether those authored edits persist across sessions or remain session-only.
The `route_runtime_persistence` production Rhai scene gate uses the existing
Rhai task tree (`seq`, named `once`/`wait_until` callbacks, and `check`/`sel`
branches), with a reactive state guard and a bounded fixed-step watchdog. It
checks runtime-waypoint creation and restoration; `route_authoring_history`
separately exercises public undo/redo for route-point movement and deletion.
The API persistence gate opens a manifest-backed test Twin twice: the first
run adds the route point and waits for the sidecar write, and the second asserts
the point and trigger override were restored before the initial scene
projection. Both persistence runs verify the source scene file is unchanged.
While route autopilot is enabled, releasing rover possession ends only the human
control link: the route remains active and continues publishing guidance. The
rover status view follows the avatar's `ControlLink`, so it reports free flight
after release and driving again after possession.

A pressed or pulsed non-`Action` `intent.edge` from the currently possessed
route subject is a manual override: `route_follow` disables guidance, clears any
pending start, and safe-stops its guidance ports. Input from the free avatar or
another unpossessed control surface leaves the route alone. The shared
controller continues applying the operator's bound port frame on subsequent
input ticks. `Action` remains the explicit route toggle; these rules use
semantic intents and apply to any authored subject profile.

## Presentation

The marker's dome is translucent, unlit, and shadowless. Its authored unvisited colour
is green; the route program changes the dome's standard
`primvars:displayColor` to gray when the generic sensor event reaches it. The unlit
surface bypasses light, normal, and shadow processing, and emits no light. Its
trigger is invisible and has its own authored radius. Billboard text and placement are
read by the generic billboard renderer. The ribbon is a separate, lightweight
world-space annotation: the route tool densifies long legs, sends all sample
coordinates through one bounded `TerrainHeights` query, authors the sampled
support normals, and standard
`normals` make its authored 0.12 m width a narrow readable flat strip rather
than a tube. Each sampled vertex is offset 0.03 m along its support normal, so
the annotation stays above slopes without applying a global vertical offset.
The curve is authored with standard `wrap = "nonperiodic"` topology, so only
adjacent ordered points are connected; the last point never connects back to
the first.
Long legs use a 3 m base sampling interval during ribbon rebuilds to avoid
cutting through streamed terrain relief. The route tool caps the transient
payload at 256 samples, quantizes only the transient text representation to
millimetre positions and 0.1 mm normals, and increases spacing only for
unusually long routes. Every authored waypoint remains an endpoint without
allowing the Rhai/USD string transport to overflow. This work is not performed
in the per-frame route-control task. It does not participate in physics or
route control. The marker and route tool own this shared presentation contract;
individual Twins do not duplicate it.

The visual contract is covered by
[`assets/scenes/tests/waypoint_visual.usda`](../../assets/scenes/tests/waypoint_visual.usda)
and its Rhai observer. Real Avian trigger arrival and route resume are covered
by [`route_progress.usda`](../../assets/scenes/tests/route_progress.usda); the
windowed pointer/menu path is covered by
[`route_interaction.usda`](../../assets/scenes/tests/editor/route_interaction/route_interaction.usda).
The manifest-backed runtime-layer write and restore path is covered by
[`route_runtime_persistence.usda`](../../assets/scenes/tests/route_runtime_persistence.usda)
and `scripts/api/test_route_runtime_persistence.py`.
The remaining route/task behavior contract is covered by authored scene
scenarios, including
[`scripting_task_contract.rhai`](../../assets/scenarios/tests/scripting_task_contract.rhai).
Rust tests retain only generic USD projection, sensor, and task-shape
mechanisms that the production scene surface cannot isolate more directly.

## Authoring rules

- Use composed USD queries for runtime scene facts and authored-layer reads only
  for document/edit questions.
- Use exact composed `SdfPath` identity and an authored relationship for the
  subject; do not resolve by display name or query order.
- Bind every executable to its subject through `rel programs`; do not invent a
  root-level `/Route`, vehicle-name lookup, or Rust route registry.
- For more than one program, select the program prim in the generic scene
  selection/program-browser surface before editing or starting it. Selection
  is transient; program and point changes are the journaled authored facts.
- Prefer a separate route-plan USD asset when the route is reusable, imported,
  or edited independently from the physical scene. Compose it into the Twin
  rather than copying its points into every scene.
- Keep route and mission policy in `.rhai`; keep continuous control laws in
  Modelica or the generic navigation mechanism.
- Add a production Rhai scene test for an observable route/policy contract. Do
  not add a Rust fixture that recreates a route, a vehicle, or a mission tree.
- Keep nested-collider event coverage in the production route path: the
  `route_nested_collider` fixture proves that a generic sensor may report a
  child collider while the route subject remains the owning rigid body.
- Keep the marker asset reusable. A new route type needs a composition or
  generic schema extension only when existing USD facts cannot express it.
