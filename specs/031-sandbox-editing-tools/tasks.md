# Implementation Tasks: Sandbox Editing Tools

## Phase 1: Foundation — Spawn Catalog + Click Placement

- [x] 1.1 Create `lunco-sandbox-edit` crate scaffold
  - Create `crates/lunco-sandbox-edit/` with `Cargo.toml` and `src/lib.rs`
  - Define `SpawnCatalog`, `SpawnableEntry`, `SpawnCategory`, `SpawnSource` types
  - Define `SelectedEntity` resource
  - Define `UndoAction` enum and `UndoStack` resource
  - Register types in `lunco-sandbox-edit` plugin
  - Add crate to workspace `Cargo.toml`
  - Add `transform-gizmo-bevy = "0.9.0"` to workspace deps
  - **Depends on**: None
  - **Requirement**: FR-001, FR-013

- [x] 1.2 [P] Implement SpawnCatalog builder
  - Create `catalog.rs` with `SpawnCatalog::default()` that registers:
    - Rovers: skid_rover.usda, ackermann_rover.usda (via `SpawnSource::UsdFile`)
    - Props: ball_dynamic, ball_static (via `SpawnSource::Procedural`)
    - Terrain: ramp, wall (via `SpawnSource::Procedural`)
  - Each entry has display_name, category, source, default_transform
  - **Depends on**: 1.1
  - **Requirement**: FR-001, FR-010, FR-011, FR-012

- [x] 1.3 Implement click-to-place spawn system
  - `SpawnState` resource: `Idle | Selecting(EntryId)`
  - System: when `SpawnState::Selecting`, raycast from camera on click → spawn entry
  - USD spawns: load via AssetServer, wait for asset, compose, spawn entities
  - Procedural spawns: call factory function (sphere/cuboid mesh + optional RigidBody + Collider)
  - All spawned entities get `ChildOf(grid)`
  - **Depends on**: 1.2
  - **Requirement**: FR-002, FR-003, FR-010, FR-011, FR-012

- [x] 1.4 [P] Implement ghost/preview system
  - When `SpawnState::Selecting`, raycast every frame to get hover point
  - Render a ghost (transparent sphere) at hover point
  - **Depends on**: 1.3
  - **Requirement**: FR-003

## Phase 2: Selection + Transform Gizmo

- [x] 2.1 Implement entity selection via Shift+Left-click
  - When NOT in spawn mode and user Shift+clicks, raycast from camera
  - First hit selectable entity becomes `SelectedEntity.entity`
  - Click on empty space deselects
  - Add `GizmoCamera` to the avatar camera entity (one-time setup)
  - Add/remove `GizmoTarget` on selected entity based on `SelectedEntity`
  - Set `DragModeActive` to block avatar possession during selection
  - **Depends on**: 1.3
  - **Requirement**: FR-004

- [x] 2.2 Integrate transform-gizmo-bevy
  - Add `TransformGizmoPlugin` to the app in `lunco-sandbox-edit`
  - Configure `GizmoOptions` with all modes enabled (translate + rotate)
  - Gizmo appears immediately on selection (no two-stage workflow)
  - During gizmo drag: body made kinematic, Position + GlobalTransform synced
  - After gizmo drag: body restored to dynamic
  - **Depends on**: 2.1
  - **Requirement**: FR-005, FR-006

- [x] 2.3 [P] Hook gizmo events for undo
  - Transform changes recorded via inspector panel sliders
  - Undo restores old transform
  - **Depends on**: 2.2
  - **Requirement**: FR-014

## Phase 3: Inspector Panel

- [x] 3.1 Create EGUI inspector panel
  - Add `inspector.rs` with `inspector_panel()` function
  - Show panel in sandbox UI when `SelectedEntity.entity = Some(e)`
  - Display entity name, type label, and editable parameters
  - Editable: Transform (X/Y/Z sliders), Mass, Linear/Angular Damping, WheelRaycast params
  - **Depends on**: 2.1
  - **Requirement**: FR-008

- [x] 3.2 [P] Add runtime parameter mutation for rover components
  - Ensure `Mass`, `LinearDamping`, `AngularDamping`, `WheelRaycast` have `#[derive(Reflect)]`
  - When inspector changes a value, the component is updated directly
  - **Depends on**: 3.1
  - **Requirement**: FR-009

## Phase 4: Physics Interaction

_Note: The original plan used `avian_pickup` for gravity-gun style grab. This was simplified — the gizmo system now handles all transform needs directly without a separate pickup tool._

## Phase 5: More Spawn Types + Polish

- [ ] 5.1 Add solar panel spawn entry
  - Procedural spawn: flat panel mesh + optional `SolarPanel` component
  - Inspector shows power output parameter
  - **Depends on**: 1.2
  - **Requirement**: FR-012

- [ ] 5.2 [P] Add ramp and wall spawn entries
  - Procedural: `Collider::cuboid` with mesh
  - Ramp: rotated 17° by default, static rigid body
  - Wall: tall thin cuboid, static rigid body
  - **Depends on**: 1.2
  - **Requirement**: FR-011

- [x] 5.3 Implement undo system
  - `UndoStack` resource: `Vec<UndoAction>`
  - Ctrl+Z pops last action and reverses it:
    - `Spawned { entity }` → `commands.entity(entity).despawn()`
    - `TransformChanged { entity, old }` → restore old transform
  - **Depends on**: 1.1, 2.3, 3.2
  - **Requirement**: FR-014

- [x] 5.4 Integrate editing tools into sandbox binary
  - Add `lunco-sandbox-edit` plugin to `rover_sandbox_usd.rs`
  - Add spawn palette panel to existing EGUI sandbox UI
  - Wire keyboard shortcuts: Escape=cancel spawn, Delete=delete entity
  - **Depends on**: 2.2, 3.1, 5.1, 5.2, 5.3
  - **Requirement**: FR-001, FR-015

## Notes

- **transform-gizmo-bevy** handles transform manipulation directly — no separate pickup tool needed
- **bevy-inspector-egui** is already in `lunco-client/Cargo.toml` as workspace dep
- USD spawn latency: AssetServer loads asynchronously. The spawn system should handle "loading" state with a visual indicator
- All spawned entities get `ChildOf(grid)` — critical for `FloatingOrigin` to work
