# Missions: BT.CPP XML + USD waypoint prims

## The mistake this corrects

The merged checkpoint feature invented a private domain: `AppendCheckpoint` /
`DeleteCheckpoint` mutated an ECS component, and pins were drawn with Bevy `Gizmos`.
Nothing was authored, so nothing was persisted, journaled, undoable, or replicated —
an Alt+LMB patrol evaporated on scene reload. And no `.usda` in the repo could give a
vessel a `BehaviorSpec` mission at all.

Three statements settle the design:

1. **The behaviour tree is the model.** There is no "checkpoint" concept — a waypoint
   is a spatial leaf of a tree.
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

```xml
<!-- behaviors/rover_patrol.btxml — canonical LunCoSim name; Groot2 opens this -->
<root BTCPP_format="4" main_tree_to_execute="MainTree">
  <BehaviorTree ID="MainTree">
    <Repeat><Sequence>
      <Action ID="drive_to" target="/World/Behaviors/Rover_wp1"/>
      <Action ID="run_tool" tool="science::take_photo"/>
      <Action ID="drive_to" target="/World/Behaviors/Rover_wp2"/>
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

def Scope "Behaviors" {
    def "Rover_wp1" (prepend references = @vessels/markers/waypoint.usda@) {
        double3 xformOp:translate = (10, 0, 3)      # ← drag this; the rover re-routes
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
```

A behaviour tree is a program like any other: a `LunCoProgramAPI` child prim naming the
XML through `info:sourceAsset` (or carrying it in `info:sourceCode`).
The engine that runs it comes from the source's extension, so nothing about the tree
needs a binding of its own — and deleting the prim deletes the mission, which is exactly
what a patrol should be.

Two things fell out of the existing codebase rather than being invented:

- **The pin visual already existed** — `assets/vessels/markers/waypoint.usda`, a
  translucent dome with an arrival trigger zone, had **no Rust reader**. It was built
  for this and never wired up.
- **`BehaviorSpec`'s own doc** already declared JSON its wire format and named "USD
  metadata" as an intended channel.

### Waypoints are not children of the vessel

A route is in WORLD space. Parented under the rover, the waypoints would ride along
as it drives — the route would chase the vehicle. They live in a sibling `Behaviors`
scope, and the XML names them by path.

### Resolution happens at compile time

`compile_behavior_xml` resolves each `target` prim path to that prim's live
`GlobalTransform`, bakes the coordinates into the compiled tree, and **recompiles
whenever a referenced prim moves**. So dragging a pin re-routes the rover, while the
hot path (`drive_autopilots`) stays a plain coordinate chase with no per-tick lookups.

`BehaviorSpec` therefore needed **no new variant**: the prim reference exists only in
the XML/JSON intermediate and is gone by the time a tree is built.

A tree naming a deleted waypoint **refuses to compile** and keeps its last good route
— it must never silently bake `[0,0,0]` and drive the rover into the world origin.

## Interaction — every edit is an existing `UsdOp`

**No new command verbs.** Alt+LMB triggers `ApplyUsdOp` three times: `AddPrim` (the
pin, referencing the marker asset), `SetTranslate` (where it landed), `SetAttribute`
(`info:sourceCode` on the mission's `LunCoProgramAPI` prim — the tree that now
names it).

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

## What was deleted

- `checkpoint_gizmo.rs` — the entire Bevy `Gizmos` pin renderer.
- `AppendCheckpoint`, `DeleteCheckpoint`, the `CheckpointContextMenu` popup, and the
  bespoke right-click delete flow.
- The Command Deck's checkpoint delete/clear buttons — the route readout is now
  strictly a read-only view of the derived spec.

`PatrolDefaults` moved to `lunco-autopilot` (it is domain tuning, not editor state).

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
  authoring prims, so script and mouse currently produce two different forms. Phase 3
  converges them.
- **Read-only pins for non-prim-backed trees** (a tree with baked coordinates, or one
  authored purely in rhai) — project them into the runtime layer so they are visible
  but not draggable. Phase 4.
- **Ctrl+Z does not undo a spawn or a gizmo move today** — the editor's `UndoStack` is
  a separate ECS-only stack, while the real (typed, invertible) undo lives on the
  document host. Waypoints ride the document path, so re-pointing Ctrl+Z at
  `DocumentHost::undo()` would fix undo for waypoints, spawns and moves in one move.
  Pre-existing, but this design leans on it.
