# Feature Specification: 025-isru-resource-economy

**Feature Branch**: `025-isru-resource-economy`
**Created**: 2026-03-29
**Status**: Draft

## Problem Statement
While SysML formally defines our resources and Modelica handles the thermodynamic math of factory processing, the Bevy ECS needs an efficient, game-engine-friendly way to manage the macroscopic storage and flow of materials (e.g., Regolith, Oxygen, Water) between entities. We need an In-Situ Resource Utilization (ISRU) architecture that supports dual-state logistics: discrete physical chunks (for rovers to carry) and continuous fluid networks (for pipes and factories).

## User Stories

### Story 1: Unified Inventory Entities
As a game developer, I want a standard `Inventory` ECS component, so that I can easily query how much Oxygen is in any given rover, tank, or habitat.

**Acceptance Criteria:**
- The engine provides a robust `Inventory` component struct that maps Resource IDs (defined by SysML `Item`s) to capacities and current levels.
- The `Inventory` component automatically alters the mass of the parent entity's `RigidBody` in the `avian` physics engine as it fills up or empties out.

### Story 2: Factory Recipes & Processing Node
As a scenario designer, I want processing factories to act as semantic nodes, converting inputs into outputs without my having to manually code the logic.

**Acceptance Criteria:**
- Entities with a `FactoryNode` and `ResourceRecipe` component automatically pull defined resources from their input inventories, wait for the processing time (or pass state to the Modelica solver `rumoca` for continuous processing), and push out defined resources.
- If input resources are missing or output inventories are full, the factory intelligently enters an `Idle` or `Blocked` state and stops drawing operational power.

### Story 3: Dual-State Logistics Flow
As a base-building player, I want to transport resources using both discrete vehicles and continuous pipe networks.

**Acceptance Criteria:**
- **Discrete Logistics:** Vehicles can transfer cargo between inventories via specialized "Cargo Loader" interactions (e.g., a rover dumping regolith into a hopper).
- **Continuous Logic:** Pipes create a `LogisticsNetwork` resource that equalizes fluid levels between connected inventories (e.g., two Oxygen tanks connected by a pipe) every tick, satisfying visual fluid flow without spawning thousands of individual item entities.

## Implementation Notes
- This module must interoperate cleanly with `014-modelica-simulation`. Bevy handles "is there enough regolith to start?", Modelica handles "what is the thermal output of cracking it?".
- The network graphs for pipes should leverage a dedicated graph traversal system outside the main `Update` schedule to ensure performance at scale.
