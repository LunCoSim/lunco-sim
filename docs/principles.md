# LunCoSim Principles: Digital Twin of the Solar System

## Core Principles

### I. Everything is a Hotswappable Plugin (Core Mandate)
Every feature, from high-level flight software to low-level physics propagators, MUST be implemented as a modular, **hotswappable plugin**. We favor a decoupled architecture where any component can be replaced at runtime without restarting the simulation. This ensures our digital twin is as dynamic as the systems it models.

### II. TDD-First (Non-Negotiable)
Test-Driven Development is our baseline. No feature code is written until a corresponding test exists and fails. This ensures our digital twin remains verifiable as complexity scales from a lunar base to the entire solar system.

### III. SysML v2 as the "Source of Truth"
We use SysML v2 to define high-level system models, interactions, and state. SysML v2 models are not just documentation; they serve as our **SAVE file format** and the structural blueprint for the Bevy ECS hierarchy.

### IV. Mathematical Rigor with Modelica
All complex physical and system dynamics (power, thermal, life support) are modeled using Modelica. Bevy acts as the "glue" that executes these models, while Modelica ensures the mathematical integrity of the simulation.

### V. Multi-Engine Physics Integration
We leverage multiple physics engines (e.g., Rapier for local interaction, custom orbital mechanics for spaceflight) and orchestrate them through Bevy. The architecture must allow seamless handoffs between local and global physics contexts.

### VI. Human-Centric UI/UX
Despite the technical depth, the user experience is paramount. Our UI must be intuitive, high-performance, and designed for clarity in managing vast amounts of telemetry and system data.

### VII. Extensibility & Open Standards
The simulator is built to be extended. We prioritize open standards (SysML, Modelica) to allow researchers and engineers to plug in their own models and missions.

### VIII. Headless-First Architecture (Non-Negotiable)
The simulation core MUST be runnable in a headless environment (no GPU, no windowing). Rendering and windowing systems must be strictly decoupled from the physical simulation. This enables high-speed automated validation, Monte Carlo analysis, and oracle-based TDD across thousands of nodes without graphical overhead.

### IX. Authority of the Engineering Ontology
All simulated entities, signal flows, and architectural layers MUST adhere to the definitions set forth in the [Engineering Ontology](architecture/01-ontology.md). Terminology drift between specifications and implementation is a principle violation.

### X. Everything is a Tunable Parameter (Core Mandate)
Hardcoded magic numbers are considered technical debt. All visual offsets, colors, physics thresholds, and system constants MUST be exposed as tunable parameters via Bevy Resources or Components. This enables fine-grained control for researchers and allows AI agents to explore the simulation's design space without re-compiling.

### XI. Responsive UI Mandate (Non-Blocking)
The user interface MUST remain responsive at all times. Heavy calculations, including celestial trajectory sampling, terrain mesh generation, and physics collider building, MUST be offloaded to background threads using Bevy's `AsyncComputeTaskPool` or similar non-blocking patterns. Synchronous blocking of the main thread for heavy computations is a principle violation.

### XII. Documentation & RustDoc Mandate (Core Mandate)
Undocumented code is considered technical debt. All modules, functions, structs, and enums MUST be documented using Rust's built-in documentation system (`///` and `//!`). Documentation must be concise and prioritize **system maintenance** for both human developers and future AI agents. It should focus on the "why" of the design, providing the necessary context for long-term architectural continuity and AI-assisted maintenance.

## Technical Standards

### Bevy ECS Architecture
- **Systems** must be pure and modular.
- **Resources** are used for global state (e.g., Time, Solar System Constants).
- **Plugins** are the primary unit of modularity.

### State Persistence
- All persistent state must be representable in SysML v2.
- Serialization/Deserialization between Bevy entities and SysML v2 must be automated and verified.

### Quality Gates
- 100% test coverage for math and logic modules.
- Performance benchmarks required for all physics and rendering updates.

### Data Precision
- **Physics**: All physical magnitudes, dimensions, and spatial vectors (forces, torques, axes) MUST use `f64` (double precision) to ensure simulation stability and accuracy at planetary scales.
- **Signals & Control**: Logical control signals, digital-to-analog bridge values (`PhysicalPort`), and command arguments COULD use `f32` (single precision) to optimize for memory and bandwidth in high-frequency messaging.

## Governance
These Principles supersede all ad-hoc development decisions. Any change requires a formal amendment and a migration plan for existing models.

**Version**: 0.6.0 | **Ratified**: 2026-03-29 | **Project**: LunCoSim
