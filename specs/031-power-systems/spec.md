# Feature Specification: 031-power-systems

**Feature Branch**: `031-power-systems`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Microgrid topology, solar/nuclear generation, and load shedding.

## Problem Statement
While Modelica `014` provides rigorous mathematical solutions for electrical circuits, LunCoSim needs a macroscopic ECS representation of a settlement's power grid to enable interactive base building and player-facing dashboards. `025-isru-resource-economy` handles physical fluid/cargo flow, but we need a dedicated `PowerBus` system to immediately propagate electrical shortages, prioritize critical life support, and manage solar/battery cascades across the settlement graph.

## User Stories

### Story 1: Top-Level Grid Representation
As a base commander, I want to connect different habitats and rovers via universal cables to share a unified power pool.

**Acceptance Criteria:**
- Expanding on `025`, a `PowerGrid` resource maintains a graph of connected electrical nodes.
- Instantaneous power generated (Solar, RTG, Reactor) is summed and compared against instantaneous power demanded (Life Support, Movement, ISRU).

### Story 2: Battery Buffers and Eclipse Handling
As a mission planner, I want batteries to charge during the lunar day and discharge during the 14-day lunar night, maintaining critical base operations.

**Acceptance Criteria:**
- `BatteryBank` entities bridge the gap between generation and demand.
- Visual dashboards alert the user to the "Time until Depletion" based on the current load.
- Seamless integration with the celestial mechanics module (`018`) which automatically drops solar generation to zero when shadowed by the Earth or crater rims.

### Story 3: Smart Load Shedding
As a systems engineer, I want the base to automatically shut down non-essential factories (ISRU) before it shuts down life support (ECLSS) when power is scarce.

**Acceptance Criteria:**
- Nodes on the grid possess a `PowerPriority` flag.
- When demand exceeds supply + battery output, the ECS system automatically sends "Disable" signals down the command bus to low-priority consumers to prevent catastrophic brownouts of P0 elements.

## Implementation Notes
- ECS acts as the semantic graph and priority manager, while the underlying mathematical calculation (e.g., specific voltage drop over a 1km cable) is delegated to the Modelica solver if High-Fidelity mode is requested.
