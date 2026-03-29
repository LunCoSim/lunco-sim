# Feature Specification: 004-scenario-orchestration

**Feature Branch**: `004-scenario-orchestration`
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

### User Story 3 - Remote Asset Resolution (Priority: P3)
As a project contributor, I want to define huge 3D assets (like a crater or a rover model) inside my scenario file without ever pushing them to GitHub or using Git-LFS, so that the main repository stays lightweight.

**Acceptance Criteria:**
- The scenario file supports Remote URIs for assets (e.g., `crater_tycho = "https://assets.lunco.space/tycho.gltf"`).
- When a scenario loads, the engine checks a local AppData cache (e.g., `~/.lunco/assets/`). If missing, it downloads the asset automatically before spawning the scene (functioning exactly like a Cargo dependency manager).
