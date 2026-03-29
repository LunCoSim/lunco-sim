# Feature Specification: 027-terramechanics

**Feature Branch**: `027-terramechanics`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Basic wheel-soil interaction model for lunar regolith.

## Problem Statement
A lunar rover simulation without soil interaction is a toy. The engine needs a basic terramechanics model that governs how wheels interact with the lunar surface — friction, slip, sinkage — to produce realistic driving behavior. This provides the foundational "feel" that makes the simulation credible.

**Phase 1 scope is deliberately Gazebo-level:** A simple parametric model using the Bekker-Wong equations. No deformable terrain, no particle simulation. Those are future enhancements.

> **Note on Scope:** Detailed deformable terrain (tire tracks, digging, particle simulation) is deferred to a future phase. Contact dynamics for manipulation/docking are deferred to potential MuJoCo integration.

## User Stories

### Story 1: Basic Wheel-Soil Contact Model (Priority: P0)
As a rover engineer, I want the wheels to interact with the terrain surface using a parametric friction/slip model, so that the rover behaves realistically on regolith.

**Acceptance Criteria:**
- The engine implements a basic **Bekker-Wong** terramechanics model computing:
  - **Normal pressure-sinkage**: How deep the wheel sinks based on load and soil parameters.
  - **Shear-displacement**: How much the soil resists lateral wheel motion (slip).
  - **Drawbar pull**: Net traction force available for forward motion.
- Soil parameters (cohesion `c`, friction angle `φ`, sinkage moduli `kc`, `kφ`) are configurable per terrain type.
- Default parameters are provided for lunar regolith (JSC-1A simulant values).

### Story 2: Terrain Material Zones (Priority: P1)
As a scenario designer, I want different areas of the terrain to have different soil attributes, so that driving over loose dust feels different from driving over compacted regolith near a crater rim.

**Acceptance Criteria:**
- The terrain mesh supports a `TerrainMaterial` component or texture-map-based lookup that maps surface regions to soil parameter sets.
- The wheel contact system queries the terrain material at the contact point and applies the matching soil parameters.

### Story 3: Slope and Gravity Effects (Priority: P1)
As a systems engineer, I want the rover to struggle on steep slopes and behave correctly in 1/6th gravity, so that mission planning accounts for realistic terrain traversability.

**Acceptance Criteria:**
- The terramechanics model accounts for slope angle in the force calculations.
- Lunar gravity (1.62 m/s²) is the default. Gravity is configurable per scenario for Mars (3.72) or Earth (9.81) testing.
- Wheel load distribution shifts correctly on slopes (downhill wheels bear more weight).

### Story 4: Integration with Modelica (Priority: P2 — Future)
As a thermal engineer, I want wheel-soil friction heating to feed into the Modelica thermal model.

**Acceptance Criteria:**
- The terramechanics friction losses are exposed as a scalar heat source.
- This value is available to `014-modelica-simulation` for thermal coupling.

### Story 5: Deformable Terrain (Priority: P4 — Future Phase)
As a scenario designer, I want the rover to leave visible tire tracks and for ISRU mining equipment to visibly excavate soil.

**Acceptance Criteria:**
- **Deferred.** Future implementation may modify the terrain mesh or heightmap at contact points.
- Particle simulation for excavation visuals is a separate future effort.

## Requirements

### Functional Requirements
- **FR-001**: **Parametric Soil Model**: The engine MUST implement a configurable Bekker-Wong model with tunable soil parameters.
- **FR-002**: **Default Regolith Parameters**: The engine MUST ship with validated lunar regolith defaults (JSC-1A simulant data).
- **FR-003**: **Per-Wheel Computation**: Forces MUST be computed independently per wheel, not per-vehicle.
- **FR-004**: **Plugin Architecture**: The terramechanics system MUST be a swappable Bevy plugin, allowing replacement with more advanced models (e.g., DEM-based, MuJoCo) in the future.
- **FR-005**: **SI Units**: All parameters use SI units. Force in Newtons, pressure in Pascals, angles in radians.

### Key Entities
- **SoilParameters**: Resource or component defining cohesion, friction angle, sinkage moduli.
- **WheelContact**: Component storing per-wheel contact state (sinkage depth, slip ratio, net force).
- **TerrainMaterial**: Component or texture map linking terrain regions to soil parameter sets.
