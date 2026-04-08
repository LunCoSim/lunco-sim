# Implementation Plan: Sandbox Editing Tools

## Architecture Overview

The system adds a new crate `lunco-sandbox-edit` that provides:
1. **SpawnCatalog** — registry of spawnable things
2. **Spawn system** — click-to-place via mouse raycast
3. **Selection system** — click to select entities
4. **Gizmo systems** — translate, rotate, force tools
5. **Inspector panel** — parameter editing via EGUI

All spawned entities become children of the Grid (same as USD rovers).

## Technology Stack

### External Crates (All compatible with Bevy 0.18)

| Crate | Version | Purpose | Status |
|-------|---------|---------|--------|
| **transform-gizmo-bevy** | `0.9.0` | 3D transform gizmo (translate/rotate/scale arrows+rings) | New dependency |
| **bevy-inspector-egui** | `0.36.0` | Runtime component inspection & editing via EGUI | **Already in workspace** |
| **avian_pickup** | `0.5.0-rc.1` | Gravity-gun style rigid body pickup (HL2 style) | New dependency |

### Why These Choices

**transform-gizmo-bevy** over custom gizmos:
- Full-featured: translate, rotate, scale modes with proper screen-space projection
- `GizmoCamera` + `GizmoTarget` architecture matches our selection model perfectly
- Events: `GizmoDragStarted`, `GizmoDragging`, `GizmoResult` for undo tracking
- Saves ~2000 lines of custom gizmo rendering and interaction math

**bevy-inspector-egui** (already in workspace):
- Used by `lunco-client` already
- Auto-generates UI from `#[derive(Reflect)]` components
- `WorldInspectorPlugin` for browsing entire world
- `ReflectInspector` widget for embedding in our custom EGUI panels
- No need to manually build sliders for every component type

**avian_pickup** over custom force tool:
- HL2 gravity gun style — click to grab, hold to carry, release to throw
- Handles physics constraints, mass scaling, and force application internally
- Much more polished than a click-drag force vector approach

### Crate Structure
- **`lunco-sandbox-edit`** (new) — spawn catalog, selection, and tool orchestration
  - `catalog.rs` — SpawnCatalog, SpawnableEntry
  - `spawn.rs` — click-to-place system via mouse raycast
  - `selection.rs` — entity selection + tool mode switching
  - `inspector.rs` — EGUI panel wrapping bevy-inspector-egui
  - `undo.rs` — undo stack for gizmo/spawn/param changes
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
    pub tool_mode: ToolMode, // Select, Translate, Rotate, Force
}

pub enum ToolMode {
    Select,
    Translate,
    Rotate,
    Force,
}
```

### UndoAction Enum

```rust
pub enum UndoAction {
    Spawned { entity: Entity },
    Moved { entity: Entity, from: Transform, to: Transform },
    Rotated { entity: Entity, from: Quat, to: Quat },
    ParamChanged { entity: Entity, param: String, old: ParamValue, new: ParamValue },
}
```

## System Design

### Spawn Flow
```
User clicks palette → SpawnState::Active(entry)
User hovers → Raycast from camera → Show ghost at hit point
User clicks → SpawnEntry at hit point → Clear SpawnState
```

### Selection Flow  
```
User in Select mode → Clicks → Raycast picks entity → SelectedEntity.entity = Some(picked)
User in Gizmo mode → Drags gizmo axis → Mutates Transform → Records UndoAction
```

### Inspector Panel
```
SelectedEntity.entity = Some(e) → Query components on e → Show sliders
User changes value → Commands mutates component → Record UndoAction
```

## Gizmo Approach

**Using `transform-gizmo-bevy`** — a mature external crate that provides:
- `TransformGizmoPlugin` — drop-in plugin
- `GizmoCamera` — mark our camera entity
- `GizmoTarget` — mark the selected entity
- `GizmoOptions` — configure which modes are active (Translate, Rotate, Scale)
- Built-in hotkey switching: G=translate, R=rotate, S=scale

We only need to wire:
1. Adding/removing `GizmoTarget` when selection changes
2. Ensuring our camera has `GizmoCamera`
3. Subscribing to `GizmoResult` events for undo tracking

## File Structure

```
crates/lunco-sandbox-edit/
├── Cargo.toml
└── src/
    ├── lib.rs              — Plugin, resource definitions
    ├── catalog.rs          — SpawnCatalog, registration
    ├── spawn.rs            — SpawnState, ghost preview, placement system
    ├── selection.rs        — Mouse pick → entity selection
    ├── gizmo.rs            — Gizmo rendering + interaction (single file for all 3)
    ├── force_tool.rs       — Force application via click-drag
    ├── inspector.rs        — EGUI parameter panel
    └── undo.rs             — Undo stack system

assets/
└── catalog/
    └── spawn_catalog.ron   — Optional: RON-based catalog config
```

## Spawn Sources

Each `SpawnableEntry` has a `SpawnSource`:

| Source | How it spawns |
|--------|--------------|
| `UsdFile("vessels/rovers/skid_rover.usda")` | Load via AssetServer, compose, spawn entities |
| `UsdFile("vessels/rovers/ackermann_rover.usda")` | Same, with wheelType override |
| `Procedural(ProcedureId::SolarPanel)` | Rust factory function |
| `Procedural(ProcedureId::BallDynamic)` | Sphere mesh + RigidBody + Collider |
| `Procedural(ProcedureId::BallStatic)` | Sphere mesh + Collider only |
| `Procedural(ProcedureId::Ramp)` | Cuboid + rotated transform |
| `Procedural(ProcedureId::Wall)` | Cuboid |

## Risk Analysis

| Risk | Mitigation |
|------|-----------|
| Gizmo rendering complexity | Start with simple colored debug lines, iterate |
| USD spawn latency (async loading) | Show "loading" indicator, use ghost placeholder |
| Component inspection genericity | Use trait-based approach: `InspectableComponent` |
| Undo for complex spawns (USD rovers) | Undo spawn = despawn entire entity tree (entity + children) |

## Implementation Phases

### Phase 1: Foundation (spawn catalog + click placement)
- Create `lunco-sandbox-edit` crate
- SpawnCatalog resource with rovers + ball
- Click-to-place via mouse raycast
- Ghost preview

### Phase 2: Selection + Translation Gizmo
- Entity selection via mouse pick
- Translate gizmo (3 arrows)
- Drag-to-move with undo

### Phase 3: Inspector Panel
- EGUI panel showing parameters
- Editable sliders for mass, damping, spring constants
- Real-time parameter application

### Phase 4: Rotation Gizmo + Force Tool
- Rotate gizmo (3 rings)
- Force application via click-drag

### Phase 5: More Spawn Types + Polish
- Solar panel, ramp, wall, static ball
- Undo system
- Keyboard shortcuts
