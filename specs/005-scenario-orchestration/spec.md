# Feature Specification: 005-scenario-orchestration

**Feature Branch**: `005-scenario-orchestration`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Data-driven test configuration using Bevy Scene Notation (BSN) / RON.

## Problem Statement
Compiling Rust code to swap rovers or test scenarios is severely inefficient. We need a declarative markup format that maps out the initial state of the test scenario (environment, lighting, rovers, and starting coordinates) and dynamically instantiates them inside the Bevy ECS.

## User Scenarios

### User Story 1 - Data-Driven Scenario Loading (Priority: P1)
As a test engineer, I want to load a test scenario from a text file, so that I can seamlessly execute different lunar test campaigns.

**Acceptance Criteria:**
- The simulation ingests a standard Bevy **RON** (`.scn.ron`) or **Bevy Scene Notation (BSN)** file.
- The file acts as the semantic "glue", referencing external Master Specifications (e.g., `rover_v2.sysml`, `artemis_iii.tif`).
- The engine dynamically spawns all referenced geometries with the specified configurations (e.g., Sun angle at 45 degrees, Rover starting at X,Y).

### User Story 2 - CLI Execution (Priority: P2)
As a CI/CD operator, I want to pass the scenario file via command line so the integration test executes automatically in the cloud.

**Acceptance Criteria:**
- Engine boots with `cargo run -- --scenario <path_to_bsn/ron>`.

### User Story 3 - CLI Parameter Overrides (Monte Carlo Fuzzing)
As a Data Scientist, I want an external Python runner to execute 10,000 parallel test scenarios varying the rover's mass slightly each time, without needing to rewrite massive `.ron` payload files every iteration.

**Acceptance Criteria:**
- The scenario loader accepts inline JSON-path arguments (e.g., `cargo run -- --scenario base.ron --override Rover.Mass=15.2 Rover.Friction=0.7`).
- These overrides are injected dynamically into the Bevy ECS before the scenario begins ticking, mathematically proving that the engine seamlessly supports external Monte Carlo generation scripts out-of-the-box.

### User Story 4 - Automated Evaluation Oracle (Priority: P1)
As a test engineer, I want to define victory and failure rules alongside my scenario configuration, so that the simulation stops automatically when an endpoint is reached to enforce a Test-Driven Development (TDD) lifecycle without human visual review.

**Acceptance Criteria:**
- The engine implements an `Oracle` system evaluated during the `FixedUpdate` schedule.
- It interprets semantic rules parsed from the BSN/RON setup (e.g., `REQUIRE Rover.Transform.x > 100`, `FAIL_IF Rover.BatteryLevel < 0.05`).
- The Oracle triggers an `AppExit` the moment a boundary is crossed.
- Upon termination, the simulation writes a strict `test_report.json` detailing the exact frame, simulation time, trigger rule, and final sensor states.
- Exits with standard POSIX codes.
