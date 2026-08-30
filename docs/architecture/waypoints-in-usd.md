# Missions: BT.CPP XML + USD waypoint prims

> Status: Active · Audience: contributors on waypoints, routes, and mission authoring

## The route contract

Three statements settle the design:

1. **The behaviour tree is the model.** A waypoint is a spatial leaf of a tree.
2. **Waypoints are a visualization of the tree.** Editing a pin is editing the tree.
3. **Visuals are the USD scene.** A pin is a real prim, not a gizmo.

## The split: topology vs geometry

XML and USD are not competing — they answer different questions, so each stores what
it is actually good at.

| | Format | Why |
|---|---|---|
| **Tree topology** — sequences, decorators, which tool fires where | BehaviorTree.CPP v4 XML | Portable: **Groot2 edits it, ROS/Nav2 runs it**. The codec (`btcpp_xml`) already existed. |
| **Mission geometry** — where the waypoints *are* | USD prims | Selectable, gizmo-draggable, journaled, undoable, persisted, replicated — by machinery that already serves every prim. |

The XML's spatial leaves **reference** the prims by path rather than baking
coordinates — which is how BT.CPP is meant to be used anyway (leaves read ports, not
constants):

Mission target paths are absolute, composed USD prim paths. They are identity
references, not names: resolution uses the exact composed `SdfPath` string together
with the stage and instance scope. Relative paths, property paths, variant-selection
paths, malformed paths, and paths that are not present on the composed stage are
invalid mission data and keep the route in an explicit unresolved state. No suffix,
relative-name, or query-order matching is permitted.

```xml
<!-- behaviors/rover_patrol.btxml — canonical LunCoSim name; Groot2 opens this -->
<root BTCPP_format="4" main_tree_to_execute="MainTree">
  <BehaviorTree ID="MainTree">
    <Repeat><Sequence>
      <Action ID="drive_to" target="/World/Route/W0"/>
      <Action ID="run_tool" tool="science::take_photo"/>
      <Action ID="drive_to" target="/World/Route/W1"/>
    </Sequence></Repeat>
  </BehaviorTree>
</root>
```

```usda
def Xform "Rover" {
    def Scope "Patrol" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @behaviors/rover_patrol.btxml@
    }
}

def Scope "Route" {
    def "W0" (prepend references = @vessels/markers/waypoint.usda@) {
        double3 xformOp:translate = (10, 0, 3)      # ← drag this; the rover re-routes
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
```

A behaviour tree is a program like any other: a `LunCoProgramAPI` child prim selecting
the XML through `info:implementationSource = "sourceAsset"` (or selecting inline
`info:sourceCode`).
The engine that runs it comes from the source's extension, so nothing about the tree
needs a binding of its own — and deleting the prim deletes the mission, which is exactly
what a patrol should be.

The marker has one authored USD identity and one runtime arrival path:

- **The marker has separate visual and event geometry** —
  `assets/vessels/markers/waypoint.usda` authors a visible `UsdGeomSphere` named
  `Dome` and an invisible ground-anchored `UsdGeomSphere` named `Trigger`.
  Both use explicit standard USD `radius` values: the dome radius controls the
  visual annotation and the trigger radius controls the interaction volume.
  Only `Trigger` has `PhysicsCollisionAPI` and the waypoint trigger tag. They
  are separate authored geometry contracts because the dome is lifted for
  presentation while the trigger is anchored to the terrain.
  The dome is opaque and emissive, so its authored green display remains solid
  and independent of scene lighting. Its standard
  `primvars:doNotCastShadows` flag excludes the annotation from shadow maps
  without changing the existing material binding or adding a marker renderer.
  This keeps the visible dome lifted above terrain while the overlap volume
  remains useful on slopes.
- **Arrival is one runtime fact** — `CollisionStart` on that Sensor updates the
  vessel's live `ReachedWaypoints` set and emits `waypoint.reached` with the marker
  path. The route UI uses that set for visited appearance; an autopilot cursor may
  identify the active leg but cannot mark a waypoint visited.

This keeps USD as the source of truth for identity, geometry, placement, and sensor
size. Rhai consumes the event and sequences mission policy; it does not scale marker
meshes or poll a duplicate distance tolerance.

### Labels follow the waypoint

A waypoint that needs a screen label authors `lunco:billboard = true` and its
`lunco:billboard:*` presentation fields on the waypoint prim. The generic USD
billboard renderer is the only label reader: it consumes the waypoint's propagated
`GlobalTransform`, which BigSpace derives from the terrain grid and its ancestors,
and projects that render pose with the active camera. Route projection does not
draw a second label, subtract active-frame positions from camera positions, or
reimplement distance/coordinate conversion. This keeps a label attached to its
waypoint when celestial parents rotate and leaves the route snapshot responsible
only for terrain-grid ribbon geometry and visited-marker state. The editor's
waypoint authoring helper writes this contract for every new authored marker;
runtime-only markers attach the same data-only `UsdBillboard` contract plus the
generic `BillboardIndex` fact to their shared USD marker root. Their `Name`
identity remains owned by the spawn/catalog path; the route key is used only by
arrival state. Both paths therefore feed the same renderer without a
route-specific label path or alternate identity path.

The change-gated surface projection uses the shared BigSpace conversion
`position_in_grid_to_parent_local`: it samples in the active terrain frame and
stores the result in the marker's actual parent, splitting a `CellCoord` only
when that parent is a `Grid`. This keeps plain USD scene-root markers and
terrain-grid markers on the same canonical hierarchy.

`BehaviorSpec`'s own doc already declares JSON its wire format and names "USD
metadata" as an intended channel.

### One authored marker, explicit visual and interaction geometry

The reusable marker follows the standard-schema boundary:

```usda
def Sphere "Dome"
{
    double radius = 2.5
    double3 xformOp:translate = (0, 2.5, 0)
}
def Sphere "Trigger" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
{
    double radius = 2.5
    custom string lunco:triggerZone = "waypoint"
}
```

`lunco:triggerZone` is the mission meaning that USD does not define; `radius`,
transform, visibility, material, and collision are standard USD/UsdPhysics data.
`lunco-usd-bevy::read_shape_dims` projects both spheres from their authored
radius, while only `Trigger` is projected into the Avian overlap sensor. The
visual dome remains present after arrival; the live route projection tints only
entries in `ReachedWaypoints` gray.

### Visual progress uses the authoritative live binding

An authored target path is resolved once by the USD behavior projection into the
vessel's `TargetBindings` map. The route visualizer associates that exact
path-to-entity binding with the authored marker entity; it does not scan marker
paths, reinterpret a path relative to another prim, or rely on ECS query order.
Runtime-only targets use their explicit `RuntimeWaypointBinding` instead. If an authored binding is
unavailable, the marker keeps its authored appearance and no visited state is
inferred. This makes gray/green progress a projection of the same live target
identity that drives the behavior tree, so a waypoint cannot oscillate because
two unrelated path representations happen to match.

### Waypoints are not children of the vessel

A route is in WORLD space. Parented under the rover, the waypoints would ride along
as it drives — the route would chase the vehicle. They live in the scene root's
`Route` scope, and the XML names them by path.

### Resolution happens at compile time

`compile_behavior_xml` resolves each `target` prim path to that prim's live
`GlobalTransform`, bakes the coordinates into the compiled tree, and **recompiles
whenever a referenced prim moves**. So dragging a pin re-routes the rover, while the
hot path (`drive_autopilots`) stays a plain coordinate chase with no per-tick lookups.

`BehaviorSpec` therefore needed **no new variant**: the prim reference exists only in
the XML/JSON intermediate and is gone by the time a tree is built.

A tree naming a deleted waypoint **refuses to compile** and keeps its last good route
— it must never silently bake `[0,0,0]` and drive the rover into the world origin.

## Interaction — document-backed and runtime-only routes

For a rover mounted in an authored USD document, **no new command verbs** are
needed. The `PlaceWaypoint` intent paired with the primary pointer action
(Alt+LMB in the bundled keymap) lowers to `ApplyUsdOps`: ordered
`AddPrim`/`SetTranslate` operations for the marker, optional mission-program
construction (`AddPrim` + `SetApiSchemas`), and `SetAttribute` for
`info:sourceCode`. The document journals it as one undo unit and the live projector
sees the complete authored shape after the change set, so no ECS component is
patched directly by the editor.

A runtime-spawned asset has no owning `UsdDocument`. The same `PlaceWaypoint`
intent therefore does not
guess the active document or write a path into an unrelated scene. It extends the
vessel's mirrored `AutopilotBehaviorSpec` through the existing
`SetAutopilotBehavior`/`EngageAutopilot` commands. The resulting route is
read-only runtime geometry: it is visible and drives the same behaviour-tree
autopilot, but has no authored marker to drag or persist until the user mounts or
authors it in a document.

The Spawn palette's Waypoint entry is the UI affordance for this same route
operation: it arms a ground placement and appends to the currently possessed or
selected vessel. It does not create a free-standing marker, because a marker
without a vessel and an ordered mission leg cannot follow or be followed by
anything. API/Rhai callers use `AddRuntimeWaypoint` with an explicit vessel.
That command creates the shared USD marker instance and uses a 2 m geometric
arrival radius inside its authored 2.5 m sensor radius, keeping route progression
and collision-backed `waypoint.reached` events on the same physical target.

Everything else about a waypoint is *already implemented*, by code that knows nothing
about waypoints:

| Interaction | Mechanism |
|---|---|
| Move a pin | The ordinary transform gizmo — it's a selectable prim |
| Delete a pin | The ordinary Delete key → `RemovePrim` |
| Undo | The document's typed inverse ops |
| Inspect | Its attributes are ordinary prim parameters |
| Persist | Saved to `.usda` |
| Journal / replay | `DomainKind::Usd`, lossless (forward, inverse) pairs |
| Network | Replicates on the USD document plane |

That is the whole point: **the feature mostly stops existing.**

## Ownership boundary

The editor owns authoring intent and submits typed USD operations for document-backed
routes. The autopilot owns the compiled behavior and runtime control state. The
Command Deck and generic billboard renderer are read-only projections of those
authoritative sources; they do not maintain a second waypoint list or draw a second
route annotation.

## What correctly stays in ECS

**Scene data goes to USD; control authority does not.** Whether an autopilot is
*engaged*, and who possesses a vessel, are runtime session state (a `SessionRegistry`
claim), not scene description — the same way possession isn't a USD attribute.
`EngageAutopilot` / `DisengageAutopilot` stay as they are.

The line: *the route* is authored; *whether we're driving it right now* is not.

## The tick-rate trap this design would otherwise have reintroduced

`Sequence` resets its children the instant it completes, and `Repeat::forever` resets
the lap — so a rover parked inside a waypoint's radius completes a lap **every tick**
and re-fires that waypoint's tools at 60 Hz. `build_patrol` guards its own legs; a
hand-authored `sequence[drive_to, run_tool]` — which is exactly what this XML compiles
to — would have walked straight back into it.

So the guard is now a general rule in `build_sequence_children`: **a `run_tool` fires
on the arrival edge of the nearest preceding `drive_to` in its sequence.** The drive
leaf arms a latch while it is genuinely en route; firing consumes it. Parked ⇒ never
re-armed ⇒ never re-fires; a real lap drives away and back ⇒ fires once per lap.

## Still open

- **`patrol()` in rhai** still emits `SetAutopilotBehavior{spec_json}` rather than
  authoring prims. This is intentional for runtime-only vessels; a future document
  authoring command may offer an explicit “promote route to USD” operation.
- Runtime-only pins are visible but intentionally not draggable or right-click
  editable: there is no USD prim/document to own those edits. Promotion to authored
  USD is still a separate authoring feature.
- **Ctrl+Z does not undo a spawn or a gizmo move today** — the editor's `UndoStack` is
  a separate ECS-only stack, while the real (typed, invertible) undo lives on the
  document host. Waypoints ride the document path, so re-pointing Ctrl+Z at
  `DocumentHost::undo()` would fix undo for waypoints, spawns and moves in one move.
  Pre-existing, but this design leans on it.
