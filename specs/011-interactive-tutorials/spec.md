# Feature Specification: 011-interactive-tutorials

**Status**: Implemented — see [design.md](./design.md) for the as-built reference.

## Problem Statement
While LunCoSim is rigorous enough for high-fidelity engineering simulations, it must also serve as a training platform. We need an interactive tutorial system that teaches users how to assemble rovers, configure the lunar environment, and orchestrate missions. 

> **Architectural Separation:** The interactive tutorial framework MUST be built *on top* of what we have. It is a separate package incorporating game-like interactive elements for engagement. It is **not** part of the simulation core. 

**Why is this needed?**
We will use this simulation for training professional operators, and even children, to understand and run lunar rovers. Engaging, interactive elements are essential to lower the barrier to entry, explaining complex aerospace physics in an intuitive, hands-on manner before the user graduates to headless engineering scripts.

## User Stories

### Story 1: Guided Onboarding
As a new user or game developer, I want to play through an interactive tutorial that introduces me to the basic controls and ECS-based architecture, so that I can quickly start building on top of LunCoSim without being overwhelmed by realistic aerospace physics.

**Acceptance Criteria:**
- The engine supports a "Tutorial Mode" plugin that overlays UI dialogue, instructions, and objectives.
- The tutorial system can detect ECS state changes (e.g., "Rover moved 10 meters", "Sensor attached") and seamlessly transition to the next learning step.

### Story 2: Progressive Simulation Fidelity
As an educator or scenario designer, I want to create a tutorial that gradually ramps up the simulation fidelity, so that students can understand the impact of complex physics one concept at a time.

**Acceptance Criteria:**
- The tutorial orchestrator can dynamically toggle physics/simulation plugins on and off at runtime.
- Complex subsystems (like communication degradation, thermal constraints, or orbital mechanics) can be introduced sequentially rather than all at once.

### Story 3: Scenario Goal Evaluation
As a mission instructor, I want the tutorial system to evaluate whether the user has successfully completed a set of operational engineering goals, so I can provide automated, real-time feedback.

**Acceptance Criteria:**
- The tutorial integrates with the Evaluation Verifier (Spec 005) to passively monitor success/failure conditions (e.g., "Reach Crater Tycho with >20% battery remaining").
- Real-time performance metrics and hints are displayed on the UI.

## Implementation Notes
- Tutorials use the rhai-based `ScenarioRuntime` to load and drive tutorial environments via `.usda` scene files and `.rhai` scenario scripts (see [`docs/scripting-guide.md`](../../docs/scripting-guide.md)). The older `.scn.ron`/`.bsn` format was superseded.
- The UI layer is decoupled — tutorial HUD elements (`hint`, `spotlight`, `objectives_hud`) are scenario-authored and don't add overhead to headless runs.
- Goal evaluation is authored as rhai mission tasks (`task(me)` hook + sequencer) rather than a separate framework. See `prelude/tasks.rhai` and the design.md for details.
