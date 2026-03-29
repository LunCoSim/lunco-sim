# Feature Specification: 028-eclss-human-factors

**Feature Branch**: `028-eclss-human-factors`
**Created**: 2026-03-29
**Status**: Draft (Future Implementation)
**Input**: Biological agents, metabolic sinks, pressurization, and radiation tracking.

## Problem Statement
A true lunar settlement simulation requires modeling its most fragile components: human beings. Treating vehicles and habitats purely as mechanical space systems ignores the driving constraints of Environmental Control and Life Support Systems (ECLSS). While the `014-modelica-simulation` engine handles the core thermodynamics (gas flow, heat exchange), the Bevy ECS needs an explicit representation of biological agents and habitat pressurization states to drive the human-in-the-loop requirements of the settlement.

> **Note on Scope**: This specification is slated for a future phase, building upon the established Modelica thermal and fluid libraries.

## User Stories

### Story 1: Human Agent Entities
As a scenario designer, I want to place Crew Members into habitats, so that their biological presence actively consumes resources and affects the base.

**Acceptance Criteria:**
- The engine defines a `BiologicalAgent` component representing a crew member.
- Agents act as dynamic resource sinks and sources in the ECS. They consume O₂ and Water/Food from the local `Inventory` (or via connected Modelica loops) and output CO₂, Wastewater, and metabolic heat.
- Agents track internal safety metrics: Fatigue, Radiation Dose, and psychological stress (optional).

### Story 2: Habitat Pressurization & Atmospheric Volumes
As a life support engineer, I want the interior volumes of rovers and bases to track pressure and gas mixture, so that hull breaches or airlock cycles have physical consequences.

**Acceptance Criteria:**
- Space Systems define a `PressurizedVolume` component storing the current gas mix (N₂, O₂, trace).
- Overtaxing the air scrubbers or losing pressure below a threshold triggers ECLSS alarms.
- Modelica continuously calculates the pressure delta and flow rates through the ECLSS hardware based on agent consumption.

### Story 3: Radiation Hazards
As a mission planner, I want to see the crew's cummulative radiation dose in various scenarios (e.g., during solar particle events).

**Acceptance Criteria:**
- Solar radiation (`018-astronomical-environment`) acts as an environmental hazard.
- Habitats have a shielding value. Agents accumulate dosage based on duration and shielding effectiveness.

## Implementation Notes
- The ECLSS loop heavily relies on `014` (Modelica) and `025` (ISRU Logistics) for the actual mass/energy balance.
- This specification provides the *state management* and *visualization* layer for these mathematical calculations.
