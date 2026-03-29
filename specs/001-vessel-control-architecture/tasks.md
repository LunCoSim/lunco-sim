# Implementation Tasks: 001-vessel-control-architecture

## Phase 1: Foundation (Workspace Configuration)

- [x] 1.1 Set up multi-crate Cargo workspace
  - Init root `Cargo.toml`.
  - Create members for `crates/lunco-sim-core`, `crates/lunco-sim-physics`, `crates/lunco-sim-obc`, `crates/lunco-sim-fsw`, `crates/lunco-sim-controller`, `crates/lunco-sim-attributes`, and `crates/lunco-sim-client`.
  - Add workspace dependency blocks for Bevy, Avian3D, and Leafwing-Input-Manager.
  - **Depends on**: None
  - **Requirement**: FR-005, Plugin First Mandate

- [x] 1.2 [P] Implement Level 1 & Level 2 primitives (`lunco-sim-core`)
  - Create `DigitalPort` (i16) and `PhysicalPort` (f32) components.
  - Create `Wire` component with value signal scaling logic.
  - Create `CommandMessage` struct and `CommandRegistry`.
  - **Depends on**: 1.1
  - **Requirement**: FR-003, FR-006

## Phase 2: Core Architecture Engines

- [x] 2.1 Implement the OBC Hardware Emulator (`lunco-sim-obc`)
  - Build the system that processes `Wire` links between `DigitalPort` and `PhysicalPort` per simulation tick.
  - Enable scaling equations mapping -32768 to 32767 raw integer bounds to real-world metric scales.
  - **Depends on**: 1.2
  - **Requirement**: FR-003

- [x] 2.2 [P] Implement Avian3d f64 base & Origin Shifting (`lunco-sim-physics`)
  - Setup Avian3D `PhysicsPlugins` configured for double precision.
  - Integrate `big_space` for camera-relative translations.
  - **Depends on**: 1.1
  - **Requirement**: FR-005, FR-007

- [x] 2.3 Implement the General FSW System Pipeline (`lunco-sim-fsw`)
  - Create listener queues for generic `CommandMessage` structs.
  - Create a trait system or core plugin mapping Commands to targeted `DigitalPort` entities.
  - **Depends on**: 1.2
  - **Requirement**: FR-002, FR-008

- [x] 2.4 Implement Mocking Strategy (`lunco-sim-core` or tests)
  - Build `MockOBC` (DigitalPort tracker) and `MockPlant` (Physical tracker) for isolated testing without `avian3d`.
  - **Depends on**: 1.2
  - **Requirement**: FR-010, 000-testing-framework Mocking Strategy

- [x] 2.5 SysML Telemetry Attributes (`lunco-sim-attributes`)
  - Build `AttributeRegistry` (SysML Value Properties) via Bevy Reflection mapping semantic strings to raw runtime ports.
  - Enable MCP/CLI tools to dynamically tweak parameters without generic logic via `SetAttribute`.
  - **Depends on**: 1.2
  - **Requirement**: User Story 4

## Phase 3: Integration & Scenario Generation

- [x] 3.1 Implement Avatar Controller logic (`lunco-sim-controller`)
  - Incorporate `leafwing-input-manager` configuration logic for WASD to logical Intent states.
  - Map specific intents (Forward, Steer Left, Stop) to broad `CommandMessage` events.
  - **Depends on**: 1.1
  - **Requirement**: FR-001

- [x] 3.2 Build the Stage 1 Baseline Rover & Ramp (`lunco-sim-client`)
  - Synthesize the primitive box bodies, wheels, and basic visual/material meshes inside `lunco-sim-client`.
  - Add the 4 Drive Motors and 2 Steering Motors mapped via OBC + FSW plugins.
  - Configure the static plane logic with Avian static collision bodies.
  - **Depends on**: 2.1, 2.2, 2.3, 3.1
  - **Requirement**: Use Story 0 (Stage 1 MVP)

## Phase 4: Headless Validation & Quality Gates (000-TEST)

- [ ] 4.1 Tier 1 Unit Testing & State Persistence
  - [x] Write isolated tests for FSW Mixing and Controller mapping.
  - [ ] Fix any State Drift in persistence save/load serialization loops.
  - **Depends on**: 2.3, 3.1
  - **Requirement**: 000-TEST Tier 1

- [x] 4.2 Tier 2 Integration Testing (DAC / ADC Paths)
  - Verify writing an `i16` DigitalPort scales properly via `Wire` to a `f32` PhysicalPort.
  - Verify return path quantizations.
  - **Depends on**: 2.1
  - **Requirement**: 000-TEST Tier 2

- [ ] 4.3 Tier 3 Functional Verifiers
  - Create automated integration tests executing the `F-01` through `F-08` test cases for the baseline rover.
  - Run the simulation in pure Headless mode.
  - **Depends on**: 3.2
  - **Requirement**: FR-007, FR-010, 000-TEST Functional Matrix

## Notes
- `[P]` tasks can be parallelized.
- TDD validation occurs natively in Phase 3/4 via headless execution.
