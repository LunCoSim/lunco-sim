# Feature Specification: SysML v2 Integration

## Problem Statement
The LunCoSim environment needs to be driven by formalized engineering models rather than hardcoded entities. We need to integrate a SysML v2 parser so that the rover's structural architecture (chassis, wheels, payload) can be directly instantiated in the Bevy simulation from a `.sysml` representation.

## User Stories

### Story 1: SysML Definition Loading
As a systems engineer
I want to define the rover's architecture in SysML v2 and load it into the simulation
So that our simulation entities are mathematically identical to our required engineering architecture.

**Acceptance Criteria:**
- The simulation can read and parse a standard SysML v2 file/JSON export.
- The `001-basic-rover-model` is fully refactored so that its Bevy components (Mass, Colliders, Visuals) are spawned dynamically based on the SysML document.
