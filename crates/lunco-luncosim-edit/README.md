# lunco-luncosim-edit

In-scene editing tools for the LunCoSim luncosim: spawn, selection, transform gizmos, and inspector panels.

## Features

- **Spawn System** — click-to-place rovers, props, and terrain with ghost preview
- **Entity Selection** — Shift+Left-click selects entities and shows transform gizmo immediately
- **Transform Gizmo** — translate/rotate via `transform-gizmo-bevy`; live entities use BigSpace and the scene command, while USD previews use parent-local projection and `ApplyUsdOps`
- **Inspector Panel** — EGUI sliders for transform, mass, damping, and wheel parameters
- **Undo** — Ctrl+Z to revert spawns and transform changes

The Assembly Editor is document-scoped. Its native USD tree, connection graph,
parameter/variant/mount views, joint editor, and animation editor keep one
derived view per `UsdPreviewId`; the dock paints the focused lease. Selection
and drilled Inspector targets are restored per lease, and authored edits carry
the lease's explicit document, layer, and projection generation through typed
USD commands. Scripted selection uses `SelectUsdPrim` with that same explicit
preview lease; a USD path is never resolved globally across the live scene and
open documents.

The workbench separates assembly authoring from live base composition. Editor
opens one explicit USD document at a time, such as a rover or lander, and its
prim tree is the document's authored hierarchy. Build owns the mounted Twin's
general composition tools; a USD compound rigid-body root is one selectable
assembly element there, while its internal parts are edited in Editor.

## Gizmo System

The transform gizmo respects `lunco_celestial::OrbitalViewPin.active` only for
the live scene presentation. A focused isolated USD preview owns its own
camera and remains editable while the mounted simulation uses orbital
presentation. Selection remains intact; no second planetary-mode flag or proxy
visibility path is introduced.

### How It Works

The gizmo system uses `transform-gizmo-bevy` as a render-space frontend. Its
`GizmoTarget` lives on an unparented proxy; the real entity is never exposed to
the library's `&mut Transform` writer. A drag is one transaction with an
explicit owner: live entities carry an exact f64 pose in `ActivePhysicsFrame`,
while USD preview entities carry their parent-local composed pose:

```
Shift+Left-click → Select editable entity → Spawn render proxy
Drag gizmo handle → Capture the owner pose → proxy proposes render pose
                     → live: render → active-frame f64 → parent-local storage
                     → USD: render → parent-local Bevy transform
Release gizmo handle → live: TransformEntity; USD: one ApplyUsdOps change set
                     → owner state restored or projected, then the session ends
```

The presentation owner is selected by `UsdViewportState` and the workbench's
measured `PanelRects`: a visible focused USD preview camera receives the
standard `GizmoCamera` marker and its logical `GizmoOptions::viewport_rect`.
The maintained gizmo picking backend applies that same rectangle before
testing handles, so rendered and interactive coordinates stay in one space.
When no preview owns the editor, `SceneViewport::active_camera` remains the
live window-camera owner. Singleton and separate preview tabs both publish
their offscreen scene ownership through `ScenePickGate`, so the global egui
focus gate cannot suppress a valid preview-handle drag or leak it into the live
scene.

### Coordinate and ownership contract

The gizmo library modifies only the proxy's render `Transform`. The editor must
not calculate deltas, subtract a parent rotation, read `GlobalTransform` as a
physics pose, or write Avian `Position`/`Rotation` directly. The canonical flow
is:

1. **`capture_gizmo_start`** — resolve the focused USD preview lease first. A
   preview snapshots its local `Transform`; a live entity reads
   `SimulationPoseQuery`, snapshots the active frame/body state, and becomes
   kinematic.
2. **`apply_gizmo_proxy_drag`** — live poses use
   `render_pose_to_grid_absolute` and
   `position_in_grid_to_parent_local`/`rotation_in_grid_to_parent_local`;
   preview poses use Bevy's `GlobalTransform::reparented_to`.
3. **`capture_final_gizmo_pose`** — snapshot the proxy's final `Last`-schedule
   write before release cleanup, because the normal interaction transfer runs
   earlier in `PostUpdate`.
4. **`restore_gizmo_dynamic`** — the live owner restores body/interpolation
   state and commits `TransformEntity`; the USD owner restores on Escape or
   stale preview revision, or commits changed local channels as one
   generation-checked `ApplyUsdOps` change set.
5. **Owner boundary** — the scene command owns live parent-local/cell storage;
  `UsdOp::SetTranslate`/`SetRotate`/`SetScale` own preview authoring and
  projection.

USD preview targets expose the standard USD `xformOp:scale` handles and the
Inspector's unitless scale fields. They commit through `UsdOp::SetScale` in the
same generation-checked `ApplyUsdOps` change set as translation and rotation.
Live simulation targets keep scale unavailable because changing a physics
body's scale requires an authored `UsdPhysics` topology/solver contract; the
existing BigSpace physics bridge remains the only normal Avian pose adapter.

### Why Render and Physics Poses Are Split

The proxy is render-frame-owned. Live entities store the scene's
parent-local/cell representation and keep an exact active-frame pose so
BigSpace rebranching and Avian writeback cannot turn a camera-relative f32
value into a kilometre-scale teleport. Preview entities store ordinary
parent-local transforms; their explicit document/generation owner prevents a
stale drag from overwriting a newer USD edit.

## USD Compound Rigid Bodies

Multi-part USD assemblies (solar panels, rovers, houses) follow the OpenUSD standard for compound rigid bodies:

```usda
def Xform "SolarPanel" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI"]   # ONE rigid body
) {
    float physics:mass = 15.0

    def Cube "PanelFrame" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]  # Collider only
    ) { ... }

    def Cube "PanelSurface" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]  # Collider only
    ) { ... }
}
```

**How it works:**
- Parent with `PhysicsRigidBodyAPI` → ONE `RigidBody::Dynamic` + `SelectableRoot`
- Children with `PhysicsCollisionAPI` → shapes collected into parent's `Collider::compound()`
- Children are pure visuals — no independent physics
- Gizmo appears on root, whole assembly moves together
- A vehicle schema stamps `MobilityRoot` on the assembly owner; viewport hits
  resolve that owner before nested spawnable component markers such as a
  mounted battery.

This follows the OpenUSD specification: `PhysicsRigidBodyAPI` on a parent aggregates all descendant colliders into one compound rigid body. No joints needed.

## User Interaction

| Action | Result |
|--------|--------|
| Shift+Left-click on entity | Select entity, show gizmo |
| Shift+Left-click on empty | Deselect |
| Escape | Deselect / cancel spawn |
| Delete | Delete selected entity |
| Drag gizmo handles | Move/rotate entity |
| Click palette entry → click scene | Spawn entity |

## File Structure

| File | Purpose |
|------|---------|
| `lib.rs` | Plugin, resources (`SelectedEntity`, `SpawnState`) |
| `catalog.rs` | `SpawnCatalog`, `SpawnableEntry`, `SpawnCategory` |
| `spawn.rs` | Ghost preview, click-to-place system |
| `selection.rs` | Shift+click selection, `GizmoTarget` management |
| `gizmo.rs` | Kinematic-drive lifecycle and proxy editing |
| `inspector.rs` | EGUI parameter panel |
| `entity_list.rs` | Clickable list of scene entities |
| `ui/spawn_palette.rs` | Spawn palette UI |
| `commands.rs` | `SPAWN_ENTITY` command message handling |
| `undo.rs` | Undo stack system |
