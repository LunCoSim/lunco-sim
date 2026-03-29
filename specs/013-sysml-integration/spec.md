# Feature Specification: 013-sysml-integration

## Problem Statement
The LunCoSim environment needs to be driven by formalized engineering models. We need to integrate a SysML v2 parser so that the `.sysml` files act as our **Static Architect & Master Specification**. SysML defines the "Signal Wiring Diagram" for our Bevy physical plants and the Stoichiometric Recipes for our factories. **SysML does not simulate; it instantiates.**

## User Stories

### Story 1: Structural Component Instantiation
As a systems engineer, I want the rover's assembly (chassis, wheels, payload) and its "Signal Interface" to be instantiated directly from a SysML v2 model.

**Acceptance Criteria:**
- Bevy entities are spawned based on SysML `part` definitions.
- `Sensor` and `Actuator` components are automatically attached to the correct entities based on the SysML `interface` and `port` mappings.
- The `.sysml` file acts as the primary source of truth for the physical plant's architecture.

### Story 2: Factory Recipe Formalization
As an industrial engineer, I want the formula for processing lunar regolith into oxygen to be defined as a SysML `ItemFlow`.

**Acceptance Criteria:**
- The engine's SysML parser successfully identifies `Item` definitions (e.g., `Regolith`, `Power`, `Water`).
- `ItemFlow`s between blocks are extracted and serialized into Bevy `ResourceRecipe` structs, declaring exactly how much mass and energy is required to output a manufactured product.

### Story 3: Bi-Directional SysML Generation (Future Scope)
As a systems engineer, after I interactively design a functional regolith-cracking factory inside the visual engine, I want the engine to export the exact stoichiometry and interface requirements back out as a valid `.sysml` file.

**Acceptance Criteria:**
- The engine supports a "SysML Serialization" hook that reads `ResourceRecipe` and `ChemicalSystem` components from the ECS.
- The engine outputs a valid SysML v2 syntax tree capturing the new block relationships and flows.
