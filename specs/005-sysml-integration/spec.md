# Feature Specification: 005-sysml-integration

## Problem Statement
The LunCoSim environment needs to be driven by formalized engineering models. We need to integrate a SysML v2 parser so that the `.sysml` files act as our **Signal Wiring Diagram**, defining the connections between `Sensors`, `Actuators`, and their corresponding `Ports`.

## User Stories

### Story 1: Structural Component Instantiation
As a systems engineer, I want the rover's assembly (chassis, wheels, payload) and its "Signal Interface" to be instantiated directly from a SysML v2 model.

**Acceptance Criteria:**
- Bevy entities are spawned based on SysML `part` definitions.
- `Sensor` and `Actuator` components are automatically attached to the correct entities based on the SysML `interface` and `port` mappings.
- The `.sysml` file acts as the primary source of truth for the physical plant's architecture.
