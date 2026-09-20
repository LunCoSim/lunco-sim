# Routes and route points in USD

Route geometry and route execution are program-level concerns. A route is an
ordinary USD scope containing reusable route-point prims and a child Rhai
program. The scope may be authored in the scene or in a separate route-plan
USD asset that the scene composes at its canonical path. The subject is an
authored relationship on the program, so a vehicle does not own a waypoint
list and the Rust core does not know about vehicles or autopilot programs.

## Ownership

| Concern | Owner |
|---|---|
| Route identity, point placement, trigger radius, look, and subject relationship | Composed USD |
| Route sequencing, enable/disable policy, mission reactions | Rhai program |
| Sensor overlap and `enter:<zone>` event production | Generic physics/sensor runtime |
| Steering math and named-port writes | Generic navigation/port mechanisms |
| Equations, actuator dynamics, and contact response | Modelica / Avian |
| Route ribbon presentation | Reusable `waypoint_editor` Rhai tool + standard USD BasisCurves asset in the runtime layer |

The standard reusable marker is
[`assets/markers/route_point.usda`](../../assets/markers/route_point.usda).
It contains a visual-only dome and a separate invisible overlap sensor. The
asset uses standard USD geometry/material properties plus the registered
project schemas needed for a trigger zone and billboard presentation. It is a
route annotation, not a vessel component.

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

When a route plan is stored separately, keep the route scope and its points in
that file and reference or payload the scope into the scene; do not duplicate
points in the scene and the plan. The scene owns subject placement and its
`programs` binding, while the route-plan file owns route topology. A
document-backed live edit still uses the composed canonical path and the
existing USD journal boundary.

The editor's route tool derives a ribbon from the same point children after the
canonical USD projection has settled. It references the reusable
[`assets/markers/route_ribbon.usda`](../../assets/markers/route_ribbon.usda)
asset as a runtime child of the route scope and writes only the generated
`BasisCurves` opinions to the document's `@runtime@` layer. Keeping the view
under the route is a frame invariant: the ribbon anchor and every route point
are expressed in the same USD parent space, so a transformed scene scope
cannot put the overlay in a different frame. The Twin therefore contains no
editor ribbon prim: removing the runtime view leaves the authored route
unchanged, and another Twin can use the same tool without importing a
Twin-specific presentation object.

Route points remain ordinary selectable USD prims after authoring. The standard
scene gizmo persists translation/rotation through the generic runtime-layer
authoring commands, including local overrides for referenced children. The
standard Delete command removes a runtime-only point and deactivates a
base-authored or referenced point in the runtime layer; it never tries to
remove a spec from a layer that does not own it. Both paths are journaled and
feed the same route revision/ribbon refresh.
For a point below a reference, payload, or selected variant, the canonical
composed path is the edit identity: the stronger local layer authors an `over`
and the transform opinion there, and undo/redo removes or restores only that
local opinion.
The document layer is authoritative during the short interval before the
canonical projection contains a newly authored point. Move and delete resolve
that local target immediately, while a point-name or composed-stage read that
is not synchronized reports a retryable authoring error rather than guessing
or silently reusing an existing path. Once the live entity exists, the same
canonical path is used for selection and menu dispatch.

## Progression

The generic sensor emits `enter:<zone>` and `exit:<zone>` events. The route
program accepts an enter event when its payload is the current subject or a
registered descendant collider in that subject's generic parent chain, and the
zone is the current point. It then changes the point's reusable marker to its
visited colour through the generic transient USD view tool, emits the authored
route-level `route_point_reached` event for mission policy, and advances its
local route cursor. Physics reports the sensor event; it does not publish a route-specific
“target reached” fact and it does not decide mission progression.

Route progress is keyed by canonical USD point path and survives disabling and
re-enabling the program. Each start resumes at the first unvisited point. The
start transaction also marks a consecutive visited prefix from sensor enters
observed while disabled and a one-time occupancy read for a rover already
inside a route sensor; those paths receive the same visited marker state. After
that snapshot, Avian sensor events remain the arrival authority.

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

The editor exposes a generic typed scene-pointer context to any registered Rhai
tool that declares `on_pointer(context)`. Physical pointer chords are resolved
by the persisted shared `input_bindings.pointer_bindings` settings, which
publish open-ended semantic names in `context.pointer_intents`; authored tools
never hardcode Alt, mouse buttons, or modifier combinations. The bundled route
policy consumes `route.add_point` and `route.context`, so a user can remap
those names without a Rust rebuild or a waypoint-specific input branch. Rust
resolves the canonical document, prim paths, screen position, raw diagnostic
metadata, semantic intents, and coordinates; it does not decide that a click
means “waypoint”. `world_position` is a typed `point3` map in the
`active_physics` frame; `render_position` is a separate floating-origin point
for presentation diagnostics only. `pointer_point(context)` is the standard
Rhai entry point and returns an explicit error when the active frame is
unavailable. The popup host only renders authored menu items and dispatches
their typed tool hooks, so adding another route action does not require an
editor-specific Rust branch.

The `route.context` intent opens the authored waypoint menu without changing
scene selection. Only its explicit “Select route point” action selects the
point and enables its transform gizmo; delete and move resolve the point from
the original pointer context. Selection and context-menu listeners therefore
own separate gestures. While route autopilot is enabled, a generic
`cmd:ReleaseControl` safe-stop transfers control to the active route when no
session owns the rover, then invalidates the cached setpoint so the route
publishes its current waypoint again after manual possession ends.

## Presentation

The marker's dome is translucent, unlit, and shadowless. Its authored unvisited colour
is green; the route program changes the dome's standard
`primvars:displayColor` to gray when the generic sensor event reaches it. The unlit
surface bypasses light, normal, and shadow processing, and emits no light. Its
trigger is invisible and has its own authored radius. Billboard text and placement are
read by the generic billboard renderer. The ribbon is a separate, lightweight
world-space annotation: the route tool densifies long legs with the shared
`TerrainHeight` query, authors the sampled support normals, and standard
`normals` make its authored 0.12 m width a narrow readable flat strip rather
than a tube. Each sampled vertex is offset 0.03 m along its support normal, so
the annotation stays above slopes without applying a global vertical offset.
The curve is authored with standard `wrap = "nonperiodic"` topology, so only
adjacent ordered points are connected; the last point never connects back to
the first.
Long legs use a 3 m base sampling interval during ribbon rebuilds to avoid
cutting through streamed terrain relief. The route tool caps the transient
payload at 768 samples, quantizes only the transient text representation to
millimetre positions and 0.1 mm normals, and increases spacing only for
unusually long routes. Every authored waypoint remains an endpoint without
allowing the Rhai/USD string transport to overflow. This work is not performed
in the per-frame route-control task. It does not participate in physics or
route control. The marker and route tool own this shared presentation contract;
individual Twins do not duplicate it.

The visual contract is covered by
[`assets/scenes/tests/waypoint_visual.usda`](../../assets/scenes/tests/waypoint_visual.usda)
and its Rhai observer. The route/task behavior contract is covered by authored
scene scenarios, including
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
