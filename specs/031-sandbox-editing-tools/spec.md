# Feature Specification: Sandbox Editing Tools

**Feature Branch**: `031-sandbox-editing-tools`
**Created**: 2026-04-09
**Status**: Draft
**Input**: Add spawn panel, gizmo tools, terrain editing, and parameter editing to the sandbox

## Problem Statement

The USD-based rover sandbox currently loads a fixed scene with predefined rovers and terrain. Users cannot add new objects, modify terrain, or interact with objects beyond possession. This limits the sandbox's usefulness as a testing and prototyping environment. Users need a way to dynamically place objects, build terrain, and manipulate entities without editing USD files.

## User Scenarios & Testing

### User Story 1 - Spawn Panel with Object Palette (Priority: P1)

As a sandbox user
I want a panel listing spawnable objects (rovers, solar panels, balls, ramps, walls)
So that I can click in the scene to place them

**Why this priority**: This is the core editing capability. Without it, nothing else is possible. It's the first thing users need to build scenes dynamically.

**Independent Test**: Can be fully tested by opening the panel, selecting an object, clicking in the scene, and verifying the object appears at the clicked location.

**Acceptance Scenarios**:

1. **Given** the sandbox is running, **When** I open the spawn panel, **Then** I see a categorized list: Rovers (skid, ackermann), Power (solar panel), Props (ball static, ball dynamic), Terrain (ramp, wall)
2. **Given** I have selected an object from the palette, **When** I click on the ground or a surface, **Then** the object appears at that location with default parameters
3. **Given** I have selected an object, **When** I move my mouse before clicking, **Then** I see a ghost/preview of the object at the raycast hit point
4. **Given** I spawned an object, **When** I press Escape or click "Cancel", **Then** spawn mode exits without placing anything

---

### User Story 2 - Gizmo Tools for Manipulation (Priority: P2)

As a sandbox user
I want to select objects and use gizmos to move, rotate, and apply forces
So that I can position and test objects without editing files

**Why this priority**: Once objects exist, users need to position and interact with them. This is the second most important capability after spawning.

**Independent Test**: Can be fully tested by selecting an object, using translation gizmo to move it, rotation gizmo to orient it, and force tool to push it, verifying changes in the scene.

**Acceptance Scenarios**:

1. **Given** an object is selected, **When** I activate the translate gizmo, **Then** I see 3-axis arrows and can drag them to move the object
2. **Given** an object is selected, **When** I activate the rotate gizmo, **Then** I see rotation rings and can drag them to rotate the object
3. **Given** an object is selected, **When** I activate the force tool, **Then** I can click and drag to apply a force vector to the object
4. **Given** no object is selected, **When** I activate a gizmo tool, **Then** the tool is inactive until I select an object

---

### User Story 3 - Parameter Inspector Panel (Priority: P2)

As a sandbox user
I want to see and edit parameters of selected objects (mass, friction, spring stiffness, motor power)
So that I can tune behavior without restarting or editing files

**Why this priority**: Enables rapid iteration and tuning. Critical for the sandbox's purpose as a testing environment.

**Independent Test**: Can be fully tested by selecting an object, seeing its parameters in a panel, changing a value, and observing the effect in simulation.

**Acceptance Scenarios**:

1. **Given** a rover is selected, **When** I open the inspector panel, **Then** I see chassis parameters (mass, damping) and per-wheel parameters (spring K, damping C, rest length)
2. **Given** I change a parameter value, **When** I press Enter or click Apply, **Then** the new value takes effect immediately in simulation
3. **Given** a solar panel is selected, **When** I open the inspector, **Then** I see its power output and orientation parameters

---

### User Story 4 - Terrain Building Tools (Priority: P3)

As a sandbox user
I want to add ramps and walls to the scene
So that I can create obstacle courses and test environments

**Why this priority**: Enhances testing scenarios but is not essential for basic functionality.

**Independent Test**: Can be fully tested by selecting a ramp from the palette, placing it, and verifying rovers can drive on it.

**Acceptance Scenarios**:

1. **Given** I select "Ramp" from the spawn panel, **When** I click on the ground, **Then** a ramp appears at that location with default dimensions and angle
2. **Given** I selected "Wall" from the spawn panel, **When** I click on the ground, **Then** a wall cuboid appears at that location

---

### User Story 5 - Undo System (Priority: P3)

As a sandbox user
I want to undo my last spawn, move, or parameter change
So that I can correct mistakes without restarting

**Why this priority**: Quality of life feature. Important but not blocking for initial usability.

**Independent Test**: Can be fully tested by spawning an object, pressing Ctrl+Z, and verifying the object is removed.

**Acceptance Scenarios**:

1. **Given** I just spawned an object, **When** I press Ctrl+Z, **Then** the object is removed from the scene
2. **Given** I just moved an object with a gizmo, **When** I press Ctrl+Z, **Then** the object returns to its previous position

---

## Requirements

### Functional Requirements

- **FR-001**: System MUST provide a spawn panel listing available objects organized by category (Rovers, Power, Props, Terrain)
- **FR-002**: System MUST support click-to-place spawning via mouse raycast against collidable surfaces
- **FR-003**: System MUST show a ghost/preview of the selected object at the raycast hit point before placement
- **FR-004**: System MUST support object selection via mouse click on spawned entities
- **FR-005**: System MUST provide a translate gizmo (3-axis arrows) for moving selected objects
- **FR-006**: System MUST provide a rotate gizmo (3-axis rings) for rotating selected objects
- **FR-007**: System MUST provide a force application tool that lets users click-drag to apply forces to selected rigid bodies
- **FR-008**: System MUST provide a parameter inspector panel showing editable properties of the selected object
- **FR-009**: System MUST support runtime modification of tunable parameters (mass, damping, spring constants, motor power)
- **FR-010**: System MUST support spawning rovers from USD definitions (skid and ackermann variants)
- **FR-011**: System MUST support spawning static objects (balls, ramps, walls) as colliders with optional meshes
- **FR-012**: System MUST support spawning dynamic objects (balls with rigid body, solar panels)
- **FR-013**: Spawnable objects MUST be defined in a registry that can be extended without code changes to the core system
- **FR-014**: System MUST support undo for spawn, move, rotate, and parameter change operations
- **FR-015**: The system MUST work in both USD-based scenes and procedurally spawned scenes

### Key Entities

- **SpawnCatalog**: Registry of all spawnable object types, each with a display name, icon, category, USD path or procedural definition, and default parameters
- **SpawnableEntry**: Single entry in the catalog representing one spawnable thing (e.g., "Skid Rover", "Solar Panel", "Ball Dynamic")
- **SelectedObject**: Tracks which entity is currently selected and which tool mode is active (select, translate, rotate, force)
- **GizmoState**: Holds the current gizmo configuration (which axes/rings are visible, drag state, snap settings)
- **EditAction**: Represents an undoable operation (spawn, transform change, parameter change) with enough data to reverse it

## Success Criteria

- **SC-001**: User can spawn any catalog object within 2 clicks (open panel → select → place)
- **SC-002**: Parameter changes take effect within 1 frame of applying them
- **SC-003**: All spawnable categories (Rovers, Power, Props, Terrain) are represented with at least one working example
- **SC-004**: Gizmo manipulation is smooth at 60fps with no visible lag between mouse movement and object response

## Out of Scope

- Saving/loading edited scenes back to USD files
- Multiplayer collaborative editing
- Custom mesh import at runtime
- Terrain deformation (heightmap painting, sculpting)
- Scripting or automation of spawn sequences

## Assumptions

- The existing USD composition system (`UsdComposer::flatten`) will be used for spawning rover definitions
- Bevy's existing gizmo ecosystem or a third-party gizmo crate will be evaluated during planning
- The existing EGUI sandbox UI will be extended for the new panels
- All spawned objects will be children of the existing Grid entity (same as USD rovers)
- The `lunco:wheelType` override mechanism will be used for spawning rovers with different wheel configurations
- Physics (Avian3D) is always enabled in the sandbox
