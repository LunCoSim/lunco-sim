# lunco-luncosim-edit

In-scene editing tools for the LunCoSim luncosim: spawn, selection, transform gizmos, and inspector panels.

## Features

- **Spawn System** — click-to-place rovers, props, and terrain with ghost preview
- **Entity Selection** — Shift+Left-click selects entities and shows transform gizmo immediately
- **Transform Gizmo** — translate/rotate via `transform-gizmo-bevy`, converted through BigSpace and committed as one scene command
- **Inspector Panel** — EGUI sliders for transform, mass, damping, and wheel parameters
- **Undo** — Ctrl+Z to revert spawns and transform changes

## Gizmo System

The transform gizmo follows `lunco_celestial::OrbitalViewPin.active`, the
existing planetary presentation fact. While it is active, the editor clears
the maintained transform-gizmo modes so the frontend produces no handles or
picking hits, then restores the user's modes when local scene editing returns.
Selection remains intact; no second planetary-mode flag or proxy visibility
path is introduced.

### How It Works

The gizmo system uses `transform-gizmo-bevy` as a render-space frontend. Its
`GizmoTarget` lives on an unparented proxy; the real entity is never exposed to
the library's `&mut Transform` writer. A drag is a transaction over one exact
f64 pose in `ActivePhysicsFrame`:

```
Shift+Left-click → Select editable entity → Spawn render proxy
Drag gizmo handle → Capture Avian/hierarchy pose → proxy proposes render pose
                     → render pose → active-frame f64 pose → parent-local storage
                     → KinematicDrive and BigSpace bridge keep physics aligned
Release gizmo handle → one TransformEntity command → one USD change set
                     → original body state restored → physics resumes
```

### Coordinate and ownership contract

The gizmo library modifies only the proxy's render `Transform`. The editor must
not calculate deltas, subtract a parent rotation, read `GlobalTransform` as a
physics pose, or write Avian `Position`/`Rotation` directly. The canonical flow
is:

1. **`capture_gizmo_start`** — read `SimulationPoseQuery`, snapshot the active
   frame and body state, then make the body kinematic.
2. **`apply_gizmo_proxy_drag`** — convert the complete proxy pose through
   `render_pose_to_grid_absolute` and
   `position_in_grid_to_parent_local`/`rotation_in_grid_to_parent_local`.
3. **`capture_final_gizmo_pose`** — snapshot the proxy's final `Last`-schedule
   write before release cleanup, because the normal interaction transfer runs
   earlier in `PostUpdate`.
4. **`restore_gizmo_dynamic`** — commit one `TransformEntity`, or restore the
   snapshot on cancel/frame invalidation, then restore physics state.
5. **`TransformEntity`** — the scene-command owner writes the parent-local/cell
   representation and persists translation plus rotation as one USD change set.

Scale handles are disabled until scale has an authored USD and runtime command
contract. The existing BigSpace physics bridge remains the only normal Avian
pose adapter.

### Why Render and Physics Poses Are Split

The proxy is render-frame-owned, while the real entity stores the scene's
parent-local/cell representation. The transaction keeps the exact active-frame
pose separately so BigSpace rebranching and Avian writeback cannot turn a
camera-relative f32 value into a kilometre-scale teleport.

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
