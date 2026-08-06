# lunco-luncosim-edit

In-scene editing tools for the LunCoSim luncosim: spawn, selection, transform gizmos, and inspector panels.

## Features

- **Spawn System** — click-to-place rovers, props, and terrain with ghost preview
- **Entity Selection** — Shift+Left-click selects entities and shows transform gizmo immediately
- **Transform Gizmo** — translate/rotate via `transform-gizmo-bevy`, no manual transform application needed
- **Inspector Panel** — EGUI sliders for transform, mass, damping, and wheel parameters
- **Undo** — Ctrl+Z to revert spawns and transform changes

## Gizmo System

### How It Works

The gizmo system uses `transform-gizmo-bevy` which **automatically applies transforms** to entities with `GizmoTarget`. We only handle the physics integration:

```
Shift+Left-click → Select entity → Add GizmoTarget → Gizmo appears immediately
Drag gizmo handle → Body made kinematic → gizmo library updates Transform
                     → InteractionSchedule drives Avian Position/Rotation
Release gizmo handle → Drive removed, body restored → Physics resumes
```

### Critical: No Manual Transform Application

The gizmo library modifies `Transform` directly in its `update_gizmos` system. **Never** manually apply `GizmoResult` deltas to `Transform` — this causes double-application and amplified movement.

Our systems only:
1. **`capture_gizmo_start`** — make the body kinematic and install its
   `lunco_physics::KinematicDrive`.
2. **`apply_gizmo_proxy_drag`** and **`drive_gizmo_kinematic_pose`** — apply the
   proxy's render-frame edit and convert it to Avian's global pose in the
   unpaused interface cadence.
3. **`restore_gizmo_dynamic`** — remove the drive, restore the original body
   state, and release the physics hold.

### Why Render and Physics Poses Are Split

The proxy is render-frame-owned, while the real entity's `Transform` remains
the scene/render pose. The standard transform propagation pass renders the
edited pose; Avian pose ownership is kept separately in the drive so physics
writeback cannot erase an interface edit, including while physics is paused.

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
| `palette.rs` | Spawn palette UI |
| `commands.rs` | `SPAWN_ENTITY` command message handling |
| `undo.rs` | Undo stack system |
