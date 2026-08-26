# 48 — Object Builder

> Status: Implemented substrate; production program-attachment gate included · Audience:
> contributors on the Object Builder and reusable components

The Object Builder is the authoring surface for assembling and editing a
simulation object. It projects USD prims and connections into the existing
canvas, prim-tree, inspector, and viewport surfaces. It does not create a second
scene model or a vehicle-specific assembly API.

The workbench exposes this surface through the registered `object_builder`
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
- `UsdOp` plus `ApplyUsdOp`/`ApplyUsdOps` is the only write path. It supplies journaling,
  inverse operations, save, undo, and live projection.
- The builder operates on a document-backed Twin. A raw file path has no runtime
  document layer to persist, so accepting it as an editable builder document
  would discard edits on reload.
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

The perspective must remain a composition of these surfaces. Do not add a second
graph library, a special rover builder, or direct ECS mutation for convenience.

## Mount and attach contract

A reusable component declares a plug frame. A host declares socket frames under
its `Mounts` group. Detection is by the applied LunCo mount schemas, not by loose
attribute names.

```usda
def Xform "Mounts"
{
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

def Xform "Wheel" (
    prepend apiSchemas = ["LunCoMountPlugAPI"]
)
{
    uniform token lunco:mount:plug = "wheel"
    rel lunco:mount:frame = </Wheel/Mounts/hub>
}
```

The attach flow is:

1. Read the composed socket or the component asset's `defaultPrim` plug.
2. Resolve the plug-to-socket placement and rotation in the host's local frame.
3. Lower `AttachComponent` to `AddPrim`/reference, transforms, the typed joint,
   and authored joint relationships.
4. Apply the complete operation set through `ApplyUsdOps`.

Program attachment follows the same author-once rule. `AttachProgram` validates a
complete `ProgramAttachSpec` and lowers it to one USD change set. An empty port
contract is source-only; it is not silently treated as a running cosim model.
The author must add explicit scalar ports and connections before expecting live
value exchange. The palette, Rhai prelude, HTTP API, and future editor all call
this same command.

`AttachSpec::from_mount` and `resolve_mount_placement` own the frame math;
`attach_component_ops` owns the USD lowering. An authored joint anchor wins. An
   implicit anchor is derived only when the authoritative USD input is omitted
   and the component contract defines that default. The editor must never copy a
   transform number into a second joint field.

The same frame contract supports retrofit snap for an already referenced part:
the inspector re-authors its transform and joint anchor without re-referencing
the asset. Invalid socket kinds, axes, relationships, or frames fail closed and
are reported to the author.

## Physics invariants

- A movable mounted part needs a rigid body and a joint. A nested visual or
  collider must not silently become an unconnected body.
- `UsdPhysics` owns bodies, joints, frames, limits, and drives. The builder only
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
