# Implementation Tasks: Sandbox Editing Tools

## Phase 1: Foundation — Spawn Catalog + Click Placement

- [ ] 1.1 Create `lunco-sandbox-edit` crate scaffold
  - Create `crates/lunco-sandbox-edit/` with `Cargo.toml` and `src/lib.rs`
  - Define `SpawnCatalog`, `SpawnableEntry`, `SpawnCategory`, `SpawnSource` types
  - Define `SelectedEntity`, `ToolMode`, `SpawnState` resources
  - Define `UndoAction` enum and `UndoStack` resource
  - Register types in `lunco-sandbox-edit` plugin
  - Add crate to workspace `Cargo.toml`
  - Add `transform-gizmo-bevy = "0.9.0"` and `avian_pickup = "0.5.0-rc.1"` to workspace deps
  - **Depends on**: None
  - **Requirement**: FR-001, FR-013

- [ ] 1.2 [P] Implement SpawnCatalog builder
  - Create `catalog.rs` with `SpawnCatalog::default()` that registers:
    - Rovers: skid_rover.usda, ackermann_rover.usda (via `SpawnSource::UsdFile`)
    - Props: ball_dynamic, ball_static (via `SpawnSource::Procedural`)
    - Terrain: ramp, wall (via `SpawnSource::Procedural`)
  - Each entry has display_name, category, source, default_transform
  - **Depends on**: 1.1
  - **Requirement**: FR-001, FR-010, FR-011, FR-012

- [ ] 1.3 Implement click-to-place spawn system
  - `SpawnState` resource: `None | Selecting(EntryId) | Placing(EntryId, hit_point)`
  - System: when `SpawnState::Selecting`, raycast from camera on click → set `Placing`
  - System: when `SpawnState::Placing`, spawn the entry at hit point
  - USD spawns: load via AssetServer, wait for asset, compose, spawn entities
  - Procedural spawns: call factory function (sphere/cuboid mesh + optional RigidBody + Collider)
  - All spawned entities get `ChildOf(grid)` 
  - **Depends on**: 1.2
  - **Requirement**: FR-002, FR-003, FR-010, FR-011, FR-012

- [ ] 1.4 [P] Implement ghost/preview system
  - When `SpawnState::Selecting`, raycast every frame to get hover point
  - Render a ghost (transparent bounding box or simple mesh) at hover point
  - Use Bevy's built-in gizmo debug rendering or a transparent StandardMaterial
  - **Depends on**: 1.3
  - **Requirement**: FR-003

## Phase 2: Selection + Transform Gizmo

- [ ] 2.1 Implement entity selection via mouse pick
  - When NOT in spawn mode and user clicks, raycast from camera
  - First hit entity with `Name` component becomes `SelectedEntity.entity`
  - Click on empty space deselects
  - Add `GizmoCamera` to the avatar camera entity (one-time setup)
  - Add/remove `GizmoTarget` on selected entity based on `SelectedEntity`
  - **Depends on**: 1.3
  - **Requirement**: FR-004

- [ ] 2.2 Integrate transform-gizmo-bevy
  - Add `TransformGizmoPlugin` to the app in `lunco-sandbox-edit`
  - Configure `GizmoOptions` with Translate + Rotate modes enabled
  - Wire hotkey switching: G=translate, R=rotate (via leafwing-input-manager or direct key check)
  - Verify gizmo appears on selected entity and responds to drag
  - **Depends on**: 2.1
  - **Requirement**: FR-005, FR-006

- [ ] 2.3 [P] Hook gizmo events for undo
  - Subscribe to `GizmoResult` events from transform-gizmo-bevy
  - On gizmo drag complete, record `UndoAction::TransformChanged` with old/new transform
  - **Depends on**: 2.2
  - **Requirement**: FR-014

## Phase 3: Inspector Panel

- [ ] 3.1 Create EGUI inspector panel using bevy-inspector-egui
  - Add `inspector.rs` with `inspector_panel()` function
  - Use `bevy_inspector_egui::reflect_inspector::ui_for_entity` to show components
  - Show panel in sandbox UI when `SelectedEntity.entity = Some(e)`
  - Display entity name, type label, and all `#[derive(Reflect)]` components
  - **Depends on**: 2.1
  - **Requirement**: FR-008

- [ ] 3.2 [P] Add runtime parameter mutation for rover components
  - Ensure `Mass`, `LinearDamping`, `AngularDamping`, `WheelRaycast` have `#[derive(Reflect)]`
  - When inspector changes a value, the `Reflect` system automatically updates the component
  - Record `UndoAction::ParamChanged` via bevy-inspector-egui's change detection
  - **Depends on**: 3.1
  - **Requirement**: FR-009

## Phase 4: Physics Interaction (avian_pickup)

- [ ] 4.1 Integrate avian_pickup plugin
  - Add `avian_pickup::PhysicsPickupPlugin` to the app
  - Configure pickup input (e.g., middle mouse button or E key to grab)
  - Verify: click on dynamic rigid body → grab → move mouse → body follows → release → throw
  - **Depends on**: 2.1
  - **Requirement**: FR-007

- [ ] 4.2 [P] Wire pickup to tool mode system
  - When `ToolMode::Force` is active, enable pickup input
  - When in other modes (select, translate, rotate), disable pickup
  - Visual feedback: highlight grabbed object
  - **Depends on**: 4.1
  - **Requirement**: FR-007

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

- [ ] 5.3 Implement undo system
  - `UndoStack` resource: `Vec<UndoAction>`
  - Ctrl+Z pops last action and reverses it:
    - `Spawned { entity }` → `commands.entity(entity).despawn_recursive()`
    - `TransformChanged { entity, old }` → restore old transform
    - `ParamChanged { entity, param, old }` → restore old parameter value
  - **Depends on**: 1.1, 2.3, 3.2
  - **Requirement**: FR-014

- [ ] 5.4 Integrate editing tools into sandbox binary
  - Add `lunco-sandbox-edit` plugin to `rover_sandbox_usd.rs`
  - Add spawn palette panel to existing EGUI sandbox UI
  - Add tool mode toggle buttons (Select, Translate, Rotate, Force)
  - Wire keyboard shortcuts: Escape=cancel spawn, G=translate, R=rotate, F=force, Ctrl+Z=undo
  - **Depends on**: 2.2, 3.1, 4.2, 5.1, 5.2, 5.3
  - **Requirement**: FR-001, FR-015

## Notes

- `[P]` indicates tasks that can be parallelized with siblings
- **transform-gizmo-bevy** uses `bevy_picking` internally — ensure it doesn't conflict with our manual raycast selection
- **bevy-inspector-egui** is already in `lunco-client/Cargo.toml` as workspace dep — no new dep needed
- **avian_pickup** requires `avian3d` which we already have — compatible with our Avian3D 0.6.1
- USD spawn latency: AssetServer loads asynchronously. The spawn system should handle "loading" state with a visual indicator
- All spawned entities get `ChildOf(grid)` — critical for `FloatingOrigin` to work
