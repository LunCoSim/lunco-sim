# Feature Specification: 013-sysml-integration

## Problem Statement
The LunCoSim environment needs to be driven by formalized engineering models. We need to integrate a SysML v2 parser so that the `.sysml` files act as our **Static Architect & Master Specification**. SysML defines the "Signal Wiring Diagram" for our Bevy physical plants and the Stoichiometric Recipes for our factories. **SysML does not simulate; it instantiates.**

## Technical Mapping: SysML v2 to Bevy ECS

To ensure the simulation is a true digital twin, the engine performs a direct 1:1 mapping between SysML v2 language primitives and Bevy ECS architectural components.

| SysML v2 Concept | Bevy ECS / LunCoSim Implementation | Role |
| :--- | :--- | :--- |
| **`part`** | **Link** (Entity with `f64` Transform) | A structural component (e.g., chassis, wheel). |
| **`port`** | **Port** Component (`Digital` / `Physical`) | An interaction point for signals or physical flows. |
| **`connection`** | **Wire** (Signal) or **Joint** (Spatial) | The logical or physical link between two ports or parts. |
| **`interface`** | **PortType** & **Unit** Metadata | Defines the "Contract" of what a port can send/receive. |
| **`attribute`** | **Component Data** (e.g., `Mass`, `MaxTorque`) | Numerical attributes of a part or port. |
| **`requirement`**| **Verifier Rule** (from Spec `000` / `005`) | Verification logic (e.g., `Rover.Mass < 50kg`). |
| **`ItemFlow`** | **Resource Logic** (Spec `014` / `025`) | The transfer of mass/energy over time (Modelica). |

### 1. The SysML "Blueprint" Logic
- **Instantiation**: When a `.sysml` model is loaded, the `SysML_Parser` iterates over all `part` definitions and spawns corresponding **Link** entities.
- **Wiring**: `connection` definitions in SysML trigger the creation of **Wire** entities in Bevy, establishing the signal flow from the OBC to the Plant.
- **Mounting**: `connection` definitions between parts in SysML that imply spatial constraints (e.g., "A is mounted to B") trigger the creation of **Joint** entities in the `f64` frame tree.

### 2. Semantic Integrity
The engine uses the **Engineering Ontology** to resolve SysML names. 
- If SysML defines `part drive_motor`, the engine looks for a Bevy bundle tagged as `drive_motor` in the asset library.
- If SysML defines a port `out_pwm`, the engine automatically creates a `DigitalPort { port_type: PWM }` on the corresponding entity.

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
- The engine supports a "SysML Serialization" hook that reads `ResourceRecipe` and `ChemicalProcess` components from the ECS.
- The engine outputs a valid SysML v2 syntax tree capturing the new block relationships and flows.
