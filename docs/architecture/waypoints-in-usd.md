# Routes and route points in USD

Route geometry and route execution are scene-level concerns. A route is an
ordinary USD scope containing reusable route-point prims and a sibling Rhai
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
each program instance.

## Progression

The generic sensor emits `enter:<zone>` and `exit:<zone>` events. The route
program accepts an enter event only when its source is the current subject and
the zone is the current point. It then emits the authored route-level
`route_point_reached` event for mission policy and advances its local route
cursor. Physics reports the sensor event; it does not publish a route-specific
“target reached” fact and it does not decide mission progression.

The route task remains live while the scene is running. Editing point placement,
point order, the subject relationship, or the program's enabled input is seen
through the normal USD/program reload path. An unresolved point or subject
causes a visible safe stop and diagnostic; it is not treated as the origin and
does not fall back to a vessel-owned route.

## Presentation

The marker's dome is emissive, translucent, and shadowless. Its trigger is
invisible and has its own authored radius. Billboard text and placement are
read by the generic billboard renderer. Route execution does not recolor or
rebuild marker geometry; a scenario may react to `route_point_reached` to update
mission state or the HUD through its own policy.

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
- Keep the marker asset reusable. A new route type needs a composition or
  generic schema extension only when existing USD facts cannot express it.
