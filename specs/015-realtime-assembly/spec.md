# Feature Specification: 015-realtime-assembly

**Feature Branch**: `015-realtime-assembly`  
**Created**: 2026-03-29  
**Status**: Draft  
**Input**: Real-time vehicle assembly (KSP-like) with automatic signal port wiring and SysML-driven structural validation.

## Problem Statement
Engineers and players need to build vehicles interactively — snapping parts together, auto-wiring signals, and immediately testing their designs. The **Design-Test-Break loop** is a core engagement mechanic: assemble a rover, drive it, watch a joint break, understand why, redesign with better materials. This loop is powered by a dedicated **SysML Constraints Violation Plugin** that tracks requirements in real-time.

## User Scenarios & Testing

### User Story 1 - Modular Part Assembly (Priority: P1)
As a user, I want to snap a modular "Wheel" part to a "Chassis" part in real-time, so that I can build a custom rover without opening a CAD tool.

**Acceptance Criteria:**
- The simulation provides a "Construction Mode" UI.
- Parts have defined attachment points and "snap" to each other using Bevy's transform hierarchy.
- Snapping a part automatically creates a fixed physics joint in Avian.

---

### User Story 2 - Automatic Signal and Resource Port Wiring (Priority: P2)
As a user, I want parts to "just work" when attached, so that a wheel automatically connects to the chassis's command signal, and a mining rig automatically hooks up to my factory's fluid input.

**Acceptance Criteria:**
- Assembly tool verifies SysML constraints (e.g., matching a `MotorPort` to a `WheelHub`, or an `O2_Pipe_Out` to an `O2_Tank_In`).
- Connections are realized in Bevy via `avian` Physics joints (Fixed, Prismatic, Revolute).
- Connections dynamically resolve communication signals (e.g., `PortA` sends voltage to `PortB`).
- **Resource/Fluid Ports:** Placing pipe/conveyor pieces creates logistical flow networks between localized Entity inventories.

---

### Story 3: SysML Constraints Violation Plugin (Priority: P1)
As a systems engineer, I want a dedicated plugin that tracks SysML-defined structural and operational requirements in real-time, breaking joints and triggering alerts when constraints are violated. This makes the system engaging and interactive — researchers can quickly test different assemblies and see what fails.

**The Design-Test-Break Loop:**
```
 ┌─── DESIGN ───┐     ┌─── TEST ───┐     ┌─── BREAK ───┐
 │ Snap parts    │────▶│ Drive it    │────▶│ Joint snaps! │
 │ in Assembly   │     │ over rocks  │     │ See WHY      │
 └───────────────┘     └─────────────┘     └──────┬───────┘
       ▲                                          │
       └──── Re-design with better materials ─────┘
```

**This is implemented as a phased plugin (`SysmlConstraintsPlugin`):**

#### Phase 1 — Scalar Stress Limits (Priority: P1)
- Each joint gets a scalar `StressLimit` loaded from SysML.
- If avian reports force > limit → joint breaks.
- Simple, satisfying, shippable.

#### Phase 2 — Multi-Factor Stress (Priority: P2)
- `StressLimit` becomes a computed function:
  ```
  effective_limit = base_stress_limit
                  × material_temperature_factor(T)  // from Modelica (014)
                  × fatigue_cycle_factor(N)          // from usage count
  ```
- Each factor is a simple lookup table or linear interpolation, NOT a coupled PDE.

#### Phase 3 — Full Modelica Structural Model (Priority: P3)
- Stress limits are evaluated by a full Modelica structural model.
- The plugin reads Modelica outputs rather than computing its own factors.

**Acceptance Criteria:**
- The `SysmlConstraintsPlugin` is a **standalone Bevy plugin** built on top of `013-sysml-integration`.
- It reads constraint definitions from the SysML model (stress thresholds, operational limits).
- It monitors ECS state (forces, temperatures, usage counts) every `FixedUpdate`.
- When a constraint is violated: the joint breaks (physics), a notification is raised (`006`), and the violation is logged with the exact SysML requirement ID that failed.
- The plugin is **optional and removable** — the engine runs without it for scenarios that don't need structural validation.

---

### User Story 4 - Formal Architecture Fallback / Export (Priority: P2)
As a systems engineer, I want the custom rover assembled in "Construction Mode" to dynamically serialize and export into a `.sysml` (SysML v2) file, so that impromptu real-time designs are captured as formalized Master Specifications.

**Acceptance Criteria:**
- Exiting Construction Mode triggers a two-way sync that writes the current entity hierarchy and signal port connections back to `.sysml`.
- The real-time assembly UI serves as a visual, intuitive editor for the underlying SysML v2 data model, preventing architecture drift.

## Requirements

### Functional Requirements
- **FR-001**: **Part Metadata**: Every modular part MUST define its physical attributes (mass, collider) and its Signal Ports (Actuators/Sensors).
- **FR-002**: **Real-time Hierarchy Update**: The engine MUST support dynamic parenting/unparenting of physics entities while the simulation is running (or paused in Construction Mode).
- **FR-003**: **Signal Auto-Discovery**: Sub-entities (parts) MUST automatically discover their root `Space System` and register their `Actuator/Sensor` ports to the space system's central multiplexer.
- **FR-004**: **SysML Serialization State**: The engine MUST be able to map its real-time component hierarchy back into a SysML v2 syntax tree and export it.
- **FR-005**: **Plugin Architecture**: The `SysmlConstraintsPlugin` MUST be a separate, optional Bevy plugin that can be added or removed independently.

### Key Entities
- **Modular Part**: A pre-defined entity template (e.g., Solar Panel, Thruster, Wheel).
- **Construction Mode**: A specialized camera/UI state for real-time assembly.
- **Signal Discovery System**: A Bevy system that wires up ports when the hierarchy changes.
- **SysmlConstraintsPlugin**: Plugin tracking SysML requirement violations in real-time.

## Success Criteria
- **SC-001**: A user can assemble a 4-wheel drive rover from individual parts in under 60 seconds.
- **SC-002**: No manual wiring required: The rover is driveable immediately after the last wheel is snapped into place.
- **SC-003**: Driving the rover off a cliff breaks the weakest joint. The violation notification names the specific SysML requirement that was exceeded.
