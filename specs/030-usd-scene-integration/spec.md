# Feature Specification: 030-usd-scene-integration

**Feature Branch**: `030-usd-scene-integration`  
**Created**: 2026-04-03  
**Status**: Draft  
**Input**: Modular USDA scene integration with Isaac Sim compatibility and extensible plugin architecture.

## Problem Statement
Currently, LunCoSim scenes are defined procedurally in Rust code. To enable interoperability with Pixar USD and NVIDIA Isaac Sim, and to support data-driven authoring, we need a modular USDA integration. This feature must be extensible, allowing for pluggable mapping of USD schemas to different simulation engines (physics, logic, rendering) while adhering to the **Headless-First** mandate (Constitution VIII).

## User Scenarios & Testing

### User Story 1 - Modular Rover Definition (Priority: P1)
As a rover engineer, I want to define my rover model in a standalone `.usda` file that can be referenced by multiple scenes.
**Acceptance Criteria:**
- A `rover.usda` file can be loaded into any scene via a USD `reference`.
- Changes in the source model are reflected across all scenes referencing it.
- **Independent Test**: Verify that a change in `rover.usda` propagates to `rover_sandbox.usda` at runtime.

### User Story 2 - Isaac Sim Physics Compatibility (Priority: P2)
As a simulation specialist, I want to use standard `PhysX` and `USDPhysics` schemas so my files are compatible across both Isaac Sim and LunCoSim.
**Acceptance Criteria:**
- Standard `USDPhysics` (RigidBody, Joint) maps to `Avian3D`.
- NVIDIA `PhysxVehicleAPI` attributes (suspension, tires) map to LunCoSim physics components.
- **Independent Test**: Load a vehicle authored in Isaac Sim and verify its performance on the ramp match expectations.

### User Story 3 - Extensible Data Mapping (Priority: P3)
As a developer, I want to add new simulation-specific mapping logic (e.g., for a new sensor type) without modifying the core USD loader.
**Acceptance Criteria:**
- New "adapters" can be registered as plugins.
- Core `lunco-usd-core` parser remains agnostic of specific physics or rendering engines.
- **Independent Test**: Register a custom `UsdAdapter` that logs whenever it sees a specific metadata tag.

## Requirements

### Functional Requirements
- **FR-001**: **Pure-Rust USDA Parser**: MUST implement/integrate a Rust-native parser (`lunco-usd-core`) capable of reading `.usda` and resolving `references`.
- **FR-002**: **Extensible Adapter Architecture**: MUST define a `UsdAdapter` trait to allow pluggable mapping of USD Prims to ECS components.
- **FR-003**: **PhysX/Isaac Sim Mapping**: MUST provide a mapping layer (`lunco-usd-physx`) for standard `USDPhysics` and NVIDIA PhysX schemas (as used in Isaac Sim).
- **FR-004**: **Physic Adapter Plugin (Avian3D)**: MUST provide an optional adapter plugin (`lunco-usd-avian`) that implements the physics mapping for `Avian3D`.
- **FR-005**: **Plugin-based Extensions**: LunCo-specific schemas MUST be implemented as opt-in plugins (`lunco-usd-mapping`).
- **FR-006**: **Bevy Integration Plugin**: The Bevy `AssetLoader` and entity spawning logic MUST be a modular plugin (`lunco-usd-bevy`).
- **FR-007**: **Headless-First Design**: The core parser and adapters MUST be runnable in a headless environment.

### Refined Crate Architecture
- **`lunco-usd-core`**: Parser, DOM, and `UsdAdapter` trait.
- **`lunco-usd-physx`**: Consolidated Physics mapping (Standard + Isaac Sim/PhysX).
- **`lunco-usd-avian`**: Avian3D specific physics implementation.
- **`lunco-usd-mapping`**: Engineering Ontology (`lunco:` namespace) mapping.
- **`lunco-usd-bevy`**: Bevy AssetLoader and client integration.

### Key Entities
- **UsdAdapter**: Trait for custom schema translation.
- **UsdSceneDescriptor**: The in-memory representation of a loaded USDA hierarchy.

## Success Criteria

### Measurable Outcomes
- **SC-001**: 100% decoupling of the core parser from the Bevy framework.
- **SC-002**: Verification of a full 4-rover sandbox scene authored entirely in USDA.
- **SC-003**: 1:1 parity for `springStrength` and `dampingRate` between USD and Avian3D.

## Assumptions
- **Assumption 1**: Focus on ASCII (`.usda`) initially; binary support is out of scope.
- **Assumption 2**: Standard `PhysX` vehicle schemas are the primary target for rover compatibility.
- **Assumption 3**: Coordinate system (`upAxis`) handling is performed at the root entity level.
