# lunco-luncosim-edit-ui

Rendered in-scene editing UI for LunCoSim. The headless ECS mechanisms live
in [`lunco-luncosim-edit-core`](../lunco-luncosim-edit-core); this package
owns the egui/workbench interaction panels, selection bridge, and debug
visualization layers. The focused transform-gizmo adapter lives in
[`lunco-luncosim-edit-gizmo-ui`](../lunco-luncosim-edit-gizmo-ui). The reusable USD prim tree lives in
[`lunco-usd-prim-tree-ui`](../lunco-usd-prim-tree-ui), while the domain-heavy
Inspector and authored USD panels live in
[`lunco-luncosim-edit-inspector-ui`](../lunco-luncosim-edit-inspector-ui).

## Features

- **Spawn System** — click-to-place rovers, props, and terrain with ghost preview
- **Entity Selection** — Left-click replaces, Shift+Left-click extends, and Ctrl+Left-click removes from the selection; each uses the transform gizmo selection owner
- **Entity List** — the active-scene hierarchy reuses a panel-local row buffer and paints only visible rows while keeping expansion keyed to entity identity
- **Live panel view models** — Command Deck and Joint State readouts update only while their panels are visible, using the workbench snapshot as the visibility boundary
- **USD Preview Picking** — clicks in the isolated Editor image map through its focused offscreen camera and select the nearest authored prim-backed part
- **Script-authored click tools** — Rhai tool libraries exposing `on_click(context)` appear in the Tools palette and receive the canonical scene click context
- **Scene pointer previews** — high-frequency pointer samples keep only the newest position per pointer and picking frame; authored tools move view-only previews through a generic typed active-frame transform command without editing USD during hover
- **Prims Navigation** — provided by `lunco-usd-prim-tree-ui`; a newly selected prim opens its ancestors and scrolls into view while unchanged selections leave manual tree scrolling alone
- **Authoring inspection** — the Rhai `authoring_inspection` library composes path-based candidate diffs, structured diagnostic groups, exact selection/reveal/frame navigation, and visual/collision/joint/frame/material/provenance evidence
- **Transform Gizmo** — provided by [`lunco-luncosim-edit-gizmo-ui`](../lunco-luncosim-edit-gizmo-ui), using `transform-gizmo-bevy`; live entities use BigSpace and the scene command, while USD previews use parent-local projection and `ApplyUsdOps`
- **Inspector Panel** — provided by `lunco-luncosim-edit-inspector-ui`; schema-hinted USD fields with units and authored/inherited provenance are prepared as explicit, reviewable USD proposals
- **Appearance and removal** — live material and shader controls update the scene. In a USD preview, **Apply to USD** commits a material draft to the selected layer. Delete removes the live selection. **Delete from mounted USD** explicitly edits its document.
- **Undo** — Ctrl+Z reverts authored document edits

The Assembly Editor is document-scoped. Its native USD tree, connection graph,
parameter/variant/mount views, joint editor, and animation editor keep one
derived view per `UsdPreviewId`; the dock paints the focused lease. Selection
and drilled Inspector targets are restored per lease, and authored edits carry
the lease's explicit document, layer, and projection generation through typed
USD commands. Scripted selection uses `SelectUsdPrim` with that same explicit
preview lease; a USD path is never resolved globally across the live scene and
open documents.

The workbench separates assembly authoring from live base composition. Editor
opens one explicit USD document at a time, such as a rover or lander, and
foregrounds its full-width prim tree in the upper-left navigation surface. The
Twin Browser remains available in the lower-left pane for choosing another
document. Build owns the mounted Twin's general composition tools; a USD
compound rigid-body root is one selectable assembly element there, while its
internal parts are edited in Editor.

The live selection is also readable through the shared public
`InspectSelection` query. It returns stable API ids in selection order, the
current primary id, stable USD paths, and the current stale-entry count. The
query is available to both headful and headless hosts; this UI crate only
adapts viewport gestures to the shared selection resource.

## Gizmo System

The transform gizmo respects `lunco_celestial_spatial_core::OrbitalViewPin.active` only for
the live scene presentation. A focused isolated USD preview owns its own
camera and remains editable while the mounted simulation uses orbital
presentation. Selection remains intact; no second planetary-mode flag or proxy
visibility path is introduced.

### How It Works

The sibling `lunco-luncosim-edit-gizmo-ui` package uses `transform-gizmo-bevy`
as a render-space frontend. Its
`GizmoTarget` lives on an unparented proxy; the real entity is never exposed to
the library's `&mut Transform` writer. A drag is one transaction with an
explicit owner: live entities carry an exact f64 pose in `ActivePhysicsFrame`,
while USD preview entities carry their parent-local composed pose:

```
Left-click → Replace selection → Spawn render proxy
Shift+Left-click → Extend selection
Ctrl+Left-click → Remove only the clicked entity
Drag gizmo handle → Capture the owner pose → proxy proposes render pose
                     → live: render → active-frame f64 → parent-local storage
                     → USD: render → parent-local Bevy transform
Release gizmo handle → live: TransformEntity; USD: one ApplyUsdOps change set
                     → owner state restored or projected, then the session ends
```

On the press frame, the editor captures the current occlusion-aware gizmo-proxy
hit and retains primary-pointer ownership through release or cancellation. The
preview camera cannot consume that gesture as a pan, including after the cursor
leaves the image.

The presentation owner is selected by `UsdViewportState` and the workbench's
measured preview image rectangle: a visible focused USD preview camera receives
the standard `GizmoCamera` marker and its logical `GizmoOptions::viewport_rect`.
The maintained gizmo picking backend applies that same rectangle before
testing handles, so rendered and interactive coordinates stay in one space.
When no preview owns the editor, `lunco_viewport_core::SceneViewport::active_camera` remains the
live window-camera owner. Singleton and separate preview tabs both publish
their offscreen scene ownership through `ScenePickGate`, so the global egui
focus gate cannot suppress a valid preview-handle drag or leak it into the live
scene.

### Coordinate and ownership contract

The gizmo library modifies only the proxy's render `Transform`. The live scene resolves the render proposal through the active physics frame.
The isolated USD preview measures the gesture in parent-local space.

1. **`drive_gizmo_drag` / `capture_gizmo_start`** — the current hover-map hit
   latches the proxy before the preview pan pass. The following `Update` system
   resolves the focused USD preview lease. A preview snapshots its local
   `Transform`; a live entity reads
   `SimulationPoseQuery`, snapshots the active frame/body state, and becomes
   kinematic.
2. **`apply_gizmo_proxy_drag`** — live poses use
   `render_pose_to_grid_absolute` and
   `position_in_grid_to_parent_local`/`rotation_in_grid_to_parent_local`;
   preview poses use Bevy's `GlobalTransform::reparented_to`.
3. **`capture_final_gizmo_pose`** — snapshot the proxy after its `Last`-schedule
   write on every transaction frame, including release, because the normal
   interaction transfer runs earlier in `PostUpdate`.
4. On release, the live-scene gizmo restores body and interpolation state and
   dispatches `TransformEntity` with interactive intent. The isolated USD preview
   applies the render gesture delta to the composed, double-precision pose at
   its displayed sample time. The shared pose planner commits the changed
   translation, rotation or scale in one generation-checked document transaction.
   Escape restores the original pose for the same preview revision. A closed
   or superseded preview discards the gesture.

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
| Left-click on entity | Replace selection, show gizmo |
| Shift+Left-click on entity | Extend selection |
| Ctrl+Left-click on entity | Remove only the clicked entity |
| Left-click on empty | Keep selection unchanged |
| Escape | Deselect / cancel spawn |
| Delete | Remove the selected entity from the live scene |
| Drag gizmo handles | Move/rotate entity |
| Click palette entry → click scene | Spawn entity |

## File Structure

| File | Purpose |
|------|---------|
| `lib.rs` | Interactive UI adapters and shared selection bridge |
| `ui/mod.rs` | `SceneEditUiPlugin` and interaction-panel registration |
| `selection.rs` | Semantic click selection and gizmo-selection marker management |
| `entity_list.rs` | Clickable list of scene entities |
| `ui/spawn_palette.rs` | Spawn palette UI |
| `diagnostic_visuals.rs` | Camera/collider debug leases, generic inspection-layer leases, and visualization commands |
| `physics_viz.rs`, `physics_gizmo.rs`, `joint_viz.rs` | Rendered physics diagnostics |

The sibling `lunco-luncosim-edit-gizmo-ui` package owns the
`transform-gizmo-bevy` frontend, render-space proxies, and live/USD pose
transaction lifecycle. Keeping that external frontend outside this package
means panel changes do not recompile the gizmo adapter.

The USD-specific panels and prim tree are separate production packages so
hosts that only need scene interaction do not compile the full authored USD
Inspector surface.
