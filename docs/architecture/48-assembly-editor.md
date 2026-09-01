# 48 — Assembly editor

> Status: Implemented substrate; production program-attachment gate included · Audience:
> contributors on the Assembly editor and reusable components

The Assembly editor is the authoring surface for assembling and editing a
separate USD document. It projects that document's prims and connections into
the existing canvas, prim-tree, inspector, and isolated USD viewport surfaces.
It does not create a second scene model or a vehicle-specific assembly API.

The workbench exposes this surface through the registered `assembly`
perspective. The registered `terrain_sculpt` perspective exposes the existing
terrain tools and keeps sculpting separate from object assembly. Both are hidden
from the default title-bar switcher, but remain available to authored tutorials
and the typed `ActivatePerspective` command. They are composed from the panels
listed below; they do not introduce a second authoring model.

## Ownership and persistence

- USD owns prim identity, hierarchy, references, variants, transforms, ports,
  connections, mount frames, and authored parameters.
- `lunco-canvas` is a reusable view/projector. Modelica and USD connections use
  different projectors over the same canvas substrate.
- `DocumentId` is the identity of an open assembly/USD file. The existing
  `DocumentRegistry<UsdDocument>` owns its source, generation, origin, undo
  stack, and lifecycle; the Assembly editor never creates a parallel registry.
- `DocumentRegistry::fork` creates a new untitled assembly snapshot with a
  fresh `DocumentId`. `UsdDocument::fork` copies the base/runtime authored
  layers and edit history while creating a document-owned composition cache;
  the host copies undo/redo state by value and the registry attaches a new
  recorder to the same Twin journal. Save-As is the first operation that can
  bind the snapshot to a filesystem path.
- `UsdOp` plus `ApplyUsdOp`/`ApplyUsdOps` is the only write path. It supplies journaling,
  inverse operations, save, undo, and live projection.
- The editor operates on an explicitly opened USD document. The existing
  `OpenFile`/`NewDocument`/`SaveAsDocument` commands provide the file lifecycle;
  the Assembly preview is activated with an explicit `DocumentId` and is
  isolated from the simulation scene.
- ECS entities and view-models are projections. They must not become a second
  source of component topology or authored values.

## Current surfaces

The existing implementation provides the substrate the perspective composes:

| Surface | Responsibility |
|---|---|
| USD prim tree | Browse composed hierarchy and select entity-backed prims |
| Connection canvas | Project typed `inputs:`/`outputs:` and physics joint relationships; write `SetConnection` through `ApplyUsdOp`/`ApplyUsdOps` |
| Inspector | Derive editable fields from the selected prim/components and dispatch typed operations |
| Mount section | Read socket/plug contracts and offer snap or attach actions |
| Behaviour editor | Edit a program's authored source through the scripting command path |
| Component attach | Reference an asset, place it, and author the joint as one typed command |
| Program attach | Discover `.mo`/`.py` sources and lower source, ports, defaults, and wires through `AttachProgram` |

The perspective must remain a composition of these surfaces. The Twin Browser
selects which open document is edited; the USD preview, prim tree, Connections
graph, Inspector, and command handlers consume that same explicit document
binding. The Connections graph is empty until that preview document is selected;
it never infers an editable stage from entity counts or the live simulation.
Do not add a second graph library, a special rover assembly implementation, or
direct ECS mutation for convenience.

### Snapshot and edit contract

An Assembly editor may start from a reusable USD component or an existing
assembly by forking its current document. The fork is a full authored
base-plus-runtime snapshot with a new identity; equal generations do not imply
shared content. Its derived composition memo is private to the fork and keyed
by the document identity plus both layer revisions. The composed USD stage and
resolver remain owned by the existing USD composition path, so the editor does
not introduce a second scene cache or dependency traversal.

The fork has no file identity until Save-As. Its subsequent operations are
journalled under the new document id, while the existing Twin journal remains
the one cross-document history stream. Per-document undo/redo remains on that
document's `DocumentHost`.

### Command and inspection contract

`ForkDocument`, `CloseDocument`, and `DiscardDocument` are the lifecycle
verbs for an Assembly document. Forking returns the new `DocumentId`; closing
removes it from the registry; discarding an untitled document closes it and
discarding a writable file document reads the file through storage off-thread,
then resets the resident document and its undo history. A file identity or
document revision change during the read is rejected rather than applied to a
different document.

`ApplyUsdOp` and `ApplyUsdOps` accept an optional `parent_gen`. When supplied,
the registry rejects a stale request before authoring or journaling. Successful
acks identify the document, target layer, affected paths, new generation,
canonical journal cursor, and change-set id for compound edits; the API response
is sent after the document mutation has committed. Callers that do not have a
causal predecessor pass `null`; they do not use a second edit path.

`InspectUsdDocument` is the read-only agent/human inspection query. It requires
an explicit `doc` and can accept a USD `path`; it reports the document's
authored layers and revisions, local composed prim topology, dependency arcs
from `lunco-usd-compose`, diagnostics, and journal cursor. It never infers an
active document or constructs another resolver/cache.

## Mount and attach contract

A reusable component declares a plug frame. A host applies
`LunCoMountHostAPI` and explicitly lists its socket prims in
`lunco:mount:sockets`; socket paths and attachment-joint paths are relationship
data, not naming conventions. Detection is by the applied LunCo mount schemas,
not by loose attribute names or a required grouping prim.

```usda
def Xform "Base" (
    prepend apiSchemas = ["LunCoMountHostAPI"]
)
{
    rel lunco:mount:sockets = [</Base/Interfaces/wheel_fl>]
    def Xform "Interfaces" {
        def Xform "wheel_fl" (
            prepend apiSchemas = ["LunCoMountSocketAPI"]
        )
        {
            uniform token lunco:mount:socket = "wheel"
            uniform token lunco:mount:joint  = "revolute"
            token lunco:mount:axis = "X"
            double3 xformOp:translate = (1.2, -0.3, 0.9)
            uniform token[] xformOpOrder = ["xformOp:translate"]
        }
    }
}

def Xform "Wheel" (
    prepend apiSchemas = ["LunCoMountPlugAPI"]
)
{
    uniform token lunco:mount:plug = "wheel"
    rel lunco:mount:frame = </Wheel/Interfaces/hub>
}
```

The attach flow is:

1. Read the composed socket or the component asset's `defaultPrim` plug.
2. Resolve the plug-to-socket placement and rotation in the host's local frame.
3. Lower `AttachComponent` to `AddPrim`/reference, transforms, the typed joint,
   and authored joint relationships. The author supplies an explicit joint leaf;
   the lowering records it on the child through
   `lunco:mount:attachmentJoint`.
4. Apply the complete operation set through the USD command's one journal change
   set (`apply_ops_as_change_set`).

Program attachment follows the same author-once rule. `AttachProgram` validates a
complete `ProgramAttachSpec` and lowers it to one USD change set. An empty port
contract is source-only; it is not silently treated as a running cosim model.
The author must add explicit scalar ports and connections before expecting live
value exchange. The palette, Rhai prelude, HTTP API, and Assembly editor all call
this same command.

`AttachSpec::from_mount` and `resolve_mount_placement` own the frame math;
`attach_component_ops` owns the USD lowering. `AttachSpec` has no independent
joint-anchor input: the lowering always derives `physics:localPos0` from the
single placement and authors `physics:localPos1` at the part origin. Direct and
socket attaches therefore share one author-once rule; the editor must never
copy a transform number into a second joint field.

The same frame contract supports retrofit snap for an already referenced part:
the inspector re-authors its transform and joint anchor without re-referencing
the asset. The command owner validates the host body, socket schema, accepted
kind, joint/axis contract, relationship, asset plug, and occupancy; invalid
frames fail closed in the frame resolver and are reported to the author. Socket
occupancy is authored as `lunco:mount:part` in the same attach change set, and a
stale request is rejected before any child is lowered.

`DetachComponent` is the inverse assembly action. Rhai or the editor submits an
explicit `component_path`, `joint_path`, and, for socket attachments,
`socket_path`. Rust verifies the recorded relationships, layer ownership, and
incoming relationship blockers, then lowers socket clearing plus joint and
component-subtree removal through the same one-change-set boundary. It never
guesses a joint from a component name and never silently deletes external
Modelica/electrical/data links. Undo restores the complete topology.

## Physics invariants

- A movable mounted part needs a rigid body and a joint. A nested visual or
  collider must not silently become an unconnected body.
- `UsdPhysics` owns bodies, joints, frames, limits, and drives. The Assembly editor only
  authors those facts; Avian is the runtime projection.
- A component may be reusable only when its default prim, units, mount contract,
  and physical ownership are explicit.
- A parser pass is not an acceptance test. New attach or mount behavior needs
  composition tests, op-lowering tests, and a production scene run.

## Remaining design work

1. Keep canvas layout as an authored decision only if layout persistence is
   required; otherwise keep it UI-local. Do not journal every drag frame by
   default.
2. Keep per-document `DocumentHost` undo authoritative for `Ctrl+Z`. Reserve
   the twin journal's broader undo manager for a separate future twin-wide or
   cross-author command; never wire both to one undo verb.
3. Reduced-coordinate articulation and large assemblies need their own physics
   contract. Mount frames make an assembly authorable; they do not replace a
   solver model for a coupled articulated system.
4. Validate the complete user path against the production binary: open a Twin,
   attach or snap a component, attach a source-backed program, inspect the
   composed joint and ports, save, reload, and confirm the body remains stable.

See [`clean-architecture-and-usd-standards.md`](clean-architecture-and-usd-standards.md)
for the standard-schema gate and
[`50-usd-driven-visuals.md`](50-usd-driven-visuals.md) for reusable visual
components.
