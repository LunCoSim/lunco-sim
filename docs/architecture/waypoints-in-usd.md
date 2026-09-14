# Routes and route points in USD

Route geometry and route execution are scene-level concerns. A route is an
ordinary USD scope containing reusable route-point prims and a child Rhai
program. The subject is an authored relationship on the program, so a vehicle
does not own a waypoint list and the Rust core does not know about vehicles or
autopilot programs.

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
copy coordinates into Rust or into the subject. Multiple routes can coexist by
using distinct scopes and subject relationships; enablement is a property of
each program instance. A program emits the generic typed `program.ready` event
after its `on_start` hook has completed. Runtime controls that send a one-shot
gesture or semantic edge after a source switch wait for that event instead of
using a fixed delay or racing the hot-reload boundary.

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

## Progression

The generic sensor emits `enter:<zone>` and `exit:<zone>` events. The route
program accepts an enter event when its payload is the current subject or a
registered descendant collider in that subject's generic parent chain, and the
zone is the current point. It then emits the authored route-level
`route_point_reached` event for mission policy and advances its local route
cursor. Physics reports the sensor event; it does not publish a route-specific
“target reached” fact and it does not decide mission progression.

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
tool that declares `on_pointer(context)`. The tool owns modifier and button
semantics: `waypoint_editor` treats Alt+primary-click as append and secondary
click on a direct route point as its context-menu request. Rust resolves the
canonical document, prim paths, screen position, modifiers, and world position;
it does not decide that a click means “waypoint”. The popup host only renders
authored menu items and dispatches their typed tool hooks, so adding another
route action does not require an editor-specific Rust branch.

## Presentation

The marker's dome is emissive, translucent, and shadowless. Its trigger is
invisible and has its own authored radius. Billboard text and placement are
read by the generic billboard renderer. The ribbon is a separate, lightweight
world-space annotation: the route tool densifies long legs with the shared
`TerrainHeight` query, authors the sampled support normals, and standard
`normals` make its authored 0.12 m width a narrow readable flat strip rather
than a tube. Each sampled vertex is offset 0.03 m along its support normal, so
the annotation stays above slopes without applying a global vertical offset.
Long legs are sampled at 3 m intervals during ribbon rebuilds to avoid cutting
through streamed terrain relief while keeping the transient Rhai/USD payload
bounded; this work is not performed in the per-frame route-control task. It does
not participate in physics or route control. Route execution does not recolor
or rebuild marker geometry; a scenario may react to `route_point_reached` to
update mission state or the HUD through its own policy.

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
- Keep route and mission policy in `.rhai`; keep continuous control laws in
  Modelica or the generic navigation mechanism.
- Add a production Rhai scene test for an observable route/policy contract. Do
  not add a Rust fixture that recreates a route, a vehicle, or a mission tree.
- Keep nested-collider event coverage in the production route path: the
  `route_nested_collider` fixture proves that a generic sensor may report a
  child collider while the route subject remains the owning rigid body.
- Keep the marker asset reusable. A new route type needs a composition or
  generic schema extension only when existing USD facts cannot express it.
