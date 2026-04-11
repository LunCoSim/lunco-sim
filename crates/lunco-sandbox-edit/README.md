# lunco-sandbox-edit

In-scene editing tools for the LunCoSim sandbox: spawn, selection, transform gizmos, and inspector panels.

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
                     → GlobalTransform synced (correct mesh rendering)
Release gizmo handle → Body restored to dynamic → Physics resumes
```

### Critical: No Manual Transform Application

The gizmo library modifies `Transform` directly in its `update_gizmos` system. **Never** manually apply `GizmoResult` deltas to `Transform` — this causes double-application and amplified movement.

Our systems only:
1. **`capture_gizmo_start`** — make body kinematic when drag starts
2. **`sync_gizmo_transforms`** — update `GlobalTransform` from `Transform` so the mesh renders at the correct position
3. **`restore_gizmo_dynamic`** — restore dynamic body when drag ends

### Why GlobalTransform Must Be Synced

`global_transform_propagation_system` runs in `PostUpdate`, but the gizmo modifies `Transform` in `Last`. Without syncing, `GlobalTransform` is stale and the mesh renders at the old position while the gizmo is at the new position.

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
| `gizmo.rs` | Kinematic switching, GlobalTransform sync |
| `inspector.rs` | EGUI parameter panel |
| `entity_list.rs` | Clickable list of scene entities |
| `palette.rs` | Spawn palette UI |
| `commands.rs` | `SPAWN_ENTITY` command message handling |
| `undo.rs` | Undo stack system |
