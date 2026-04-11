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
2. **`sync_gizmo_transforms`** — update `GlobalTransform` from `Transform` so the mesh renders at the correct position (the gizmo modifies Transform in `Last`, after PostUpdate's GlobalTransform propagation)
3. **`restore_gizmo_dynamic`** — restore dynamic body when drag ends

### Why GlobalTransform Must Be Synced

`global_transform_propagation_system` runs in `PostUpdate`, but the gizmo modifies `Transform` in `Last`. Without syncing, `GlobalTransform` is stale and the mesh renders at the old position while the gizmo is at the new position — causing visual mismatch and over-dragging.

### Why Position Is NOT Synced

Initially, we synced `Position` from `Transform` to prevent Avian3D writeback from overwriting. However, this caused the gizmo library to double-apply transforms on subsequent frames (it reads the synced Position via Transform and adds the cumulative delta on top). Position sync was removed — the kinematic body state prevents writeback interference.

### System Schedule

```
PostUpdate:
  1. Avian3D Writeback: Position → Transform (stale, but body is kinematic so no change)
  2. global_transform_propagation: GlobalTransform = parent * Transform (stale)

Last:
  3. update_gizmos: Transform = gizmo_result (new)                    ← transform-gizmo-bevy
  4. draw_gizmos: renders gizmo at new position                       ← transform-gizmo-bevy
  5. capture_gizmo_start: makes body kinematic                        ← our code
  6. sync_gizmo_transforms: GlobalTransform = parent * Transform      ← our code
  7. restore_gizmo_dynamic: restores dynamic when drag ends           ← our code
```

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
