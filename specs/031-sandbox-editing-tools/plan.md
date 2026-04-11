# Implementation Plan: Sandbox Editing Tools

## Architecture Overview

The system adds a new crate `lunco-sandbox-edit` that provides:
1. **SpawnCatalog** — registry of spawnable things
2. **Spawn system** — click-to-place via mouse raycast
3. **Selection system** — Shift+click to select entities with immediate gizmo
4. **Gizmo systems** — translate, rotate tools
5. **Inspector panel** — parameter editing via EGUI

All spawned entities become children of the Grid (same as USD rovers).

## Technology Stack

### External Crates (All compatible with Bevy 0.18)

| Crate | Version | Purpose | Status |
|-------|---------|---------|--------|
| **transform-gizmo-bevy** | `0.9.0` | 3D transform gizmo (translate/rotate/scale arrows+rings) | New dependency |
| **bevy-inspector-egui** | `0.36.0` | Runtime component inspection & editing via EGUI | **Already in workspace** |

### Crate Structure
- **`lunco-sandbox-edit`** (new) — spawn catalog, selection, and tool orchestration
  - `catalog.rs` — SpawnCatalog, SpawnableEntry
  - `spawn.rs` — click-to-place system via mouse raycast
  - `selection.rs` — Shift+click entity selection + GizmoTarget management
  - `gizmo.rs` — kinematic/dynamic body switching, Position + GlobalTransform sync
  - `inspector.rs` — EGUI parameter panel
  - `entity_list.rs` — clickable list of scene entities
  - `palette.rs` — spawn palette UI
  - `undo.rs` — undo stack for spawn/transform changes
  - `commands.rs` — SPAWN_ENTITY command message handling
  - `lib.rs` — plugin wiring all subsystems together

## Component Design

### SpawnCatalog Resource

```rust
#[derive(Resource)]
pub struct SpawnCatalog {
    pub entries: Vec<SpawnableEntry>,
}

pub struct SpawnableEntry {
    pub id: String,          // "skid_rover", "solar_panel", "ball_dynamic"
    pub display_name: String, // "Skid Rover", "Solar Panel"
    pub category: SpawnCategory, // Rover, Power, Prop, Terrain
    pub source: SpawnSource,    // UsdFile(Path) or Procedural(ProcedureId)
    pub default_transform: Transform,
}
```

**Design decision**: Catalog is a Resource built at Startup. Each entry knows how to spawn itself — USD files via AssetServer, procedural via factory functions.

### SelectedEntity Resource

```rust
#[derive(Resource, Default)]
pub struct SelectedEntity {
    pub entity: Option<Entity>,
}
```

### Gizmo Lifecycle

```
Shift+Left-click → Select entity → Add GizmoTarget → Gizmo appears immediately
Drag gizmo handle → Body becomes kinematic → Transform updated by gizmo library
                     → Position synced (prevent writeback) → GlobalTransform synced (correct rendering)
Release gizmo handle → Body restored to dynamic → Physics resumes
```

## Gizmo Approach

**Using `transform-gizmo-bevy`** — a mature external crate that provides:
- `TransformGizmoPlugin` — drop-in plugin
- `GizmoCamera` — mark our camera entity
- `GizmoTarget` — mark the selected entity
- `GizmoOptions` — configure which modes are active

We handle:
1. Adding/removing `GizmoTarget` when selection changes
2. Ensuring our camera has `GizmoCamera`
3. Making bodies kinematic during drag (so physics doesn't fight transforms)
4. Syncing `Position` from `Transform` (prevents Avian3D writeback overwrite)
5. Syncing `GlobalTransform` from `Transform` (prevents mesh rendering at stale position)
6. Restoring dynamic bodies when drag ends

## File Structure

```
crates/lunco-sandbox-edit/
├── Cargo.toml
├── src/
│   ├── lib.rs              — Plugin, resource definitions
│   ├── catalog.rs          — SpawnCatalog, registration
│   ├── spawn.rs            — SpawnState, ghost preview, placement system
│   ├── selection.rs        — Shift+click entity selection
│   ├── gizmo.rs            — Kinematic switching, Position/GlobalTransform sync
│   ├── inspector.rs        — EGUI parameter panel
│   ├── entity_list.rs      — Clickable entity list UI
│   ├── palette.rs          — Spawn palette UI
│   ├── commands.rs         — SPAWN_ENTITY command handling
│   └── undo.rs             — Undo stack system
```

## Risk Analysis

| Risk | Mitigation |
|------|-----------|
| Transform overshoot (gizmo + physics conflict) | Body made kinematic during drag, Position synced to prevent writeback |
| Mesh renders at wrong position | GlobalTransform synced from Transform each frame during drag |
| USD spawn latency (async loading) | Show "loading" indicator, use ghost placeholder |
| Undo for complex spawns (USD rovers) | Undo spawn = despawn entire entity tree (entity + children) |

## Implementation Phases

### Phase 1: Foundation (spawn catalog + click placement) ✅ DONE
- Create `lunco-sandbox-edit` crate
- SpawnCatalog resource with rovers + ball
- Click-to-place via mouse raycast
- Ghost preview

### Phase 2: Selection + Translation Gizmo ✅ DONE
- Entity selection via Shift+Left-click
- Transform gizmo appears immediately (all modes)
- Kinematic body switching during drag
- Position + GlobalTransform sync

### Phase 3: Inspector Panel ✅ DONE
- EGUI panel showing parameters
- Editable sliders for mass, damping, spring constants
- Real-time parameter application

### Phase 4: Physics Interaction

_Note: Originally planned to use `avian_pickup` for gravity-gun style grab. Simplified — the gizmo system handles all transform needs directly._

### Phase 5: More Spawn Types + Polish
- Solar panel, ramp, wall, static ball
- Undo system
- Keyboard shortcuts (Escape, Delete)
