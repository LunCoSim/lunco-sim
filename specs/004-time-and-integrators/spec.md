# Feature Specification: 004-time-and-integrators

**Feature Branch**: `004-time-and-integrators`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Time decoupling requirements, Lockstep synchronization, Pluggable Mathematical Integrators (Solvers), and PhysicsMode state machine.

## Problem Statement
Bevy's default `Time` resource is bound to the system clock (real time) and uses implicit global stepping for its systems. To serve as a digital twin and testing platform, the simulation must control its own time passage (`dt`) dynamically. It must be able to run "Faster than Real-time" for ML/Orbital mechanics, or in "Lockstep" for debugging Fprime/ROS nodes. Because time (`dt`) is the fundamental variable for physics simulations, the engine must also support **pluggable mathematical solvers/integrators** allowing users to dictate *how* state progresses over time.

Additionally, this spec establishes the **PhysicsMode state machine** — the foundational mechanism for each entity to be propagated differently (full physics, analytical orbit, or blended transition). This is a core architectural primitive used by `022-fmu-gmat-integration` and `018-astronomical-environment`.

## User Scenarios

### User Story 1 - Fast-Forward Headless Execution (Priority: P1)
As a machine learning or astrodynamics engineer, I want the simulation to run as fast as my CPU allows without dropping physics ticks, so I can simulate thousands of lunar days in minutes.

**Acceptance Criteria:**
- The engine ignores the 60hz VSync cap rendering limit.
- The `Time::delta()` passed to systems is a fixed numeric constant rather than a measurement of wall-clock time.

### User Story 2 - Lockstep Time Sync (Priority: P1)
As a flight software engineer testing SIL nodes (`029`), I want the Bevy simulation to wait for my Fprime FSW to process the last sensor frame before stepping forward.

**Acceptance Criteria:**
- The engine pauses its primary `FixedUpdate` loop until an external "Step" command is received.
- External software receives the simulated timestamp to ensure strict deterministic log alignment.

### User Story 3 - Pluggable Solver & Integrator Architecture (Priority: P1)
As a systems modeler, I want to assign different mathematical integrators (e.g., RK4, Fast-Euler, CVODE) to different entities depending on their need for precision vs performance.

**Acceptance Criteria:**
- The engine implements a global generic `trait Integrator` governing how positional or scalar states update over `dt`.
- External plugins (like Modelica `014` or GMAT `022`) implement this trait, enabling them to hot-swap out local physics bounds on an entity mid-flight.

### User Story 4 - Configurable Tick Rate (Priority: P1)
As a simulation operator, I want to set the physics tick rate per-session without recompiling.

**Acceptance Criteria:**
- The engine exposes a `SimulationConfig` resource configurable via CLI flags or the REPL.
- Supported modes (as defined in the [Engineering Ontology](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/ontology.md)):
  - **Game Mode**: 60 Hz (interactive play, tutorials)
  - **Robotics Mode**: 100–1000 Hz (HIL/SIL testing)
  - **Fast-Forward**: Uncapped CPU-bound (Monte Carlo, ML)
  - **Lockstep**: External clock (Fprime/ROS sync)
- Tick rate is a runtime parameter, NOT a compile-time constant.

### User Story 5 - PhysicsMode State Machine (Priority: P1)
As a systems architect, I want each entity to declare how its physics is propagated (full simulation, analytical orbit, or blended transition), so that the engine can mix high-fidelity local physics with efficient analytical propagation for distant objects.

**Acceptance Criteria:**
- Every entity with physics has a `PhysicsMode` component:
  - `FullPhysics`: Avian RigidBody active. Thrust, collision, contacts.
  - `HybridBlend { blend_factor: f32 }`: Smooth cross-fade between analytical and physics propagators over 3-5 seconds.
  - `OnRails`: No RigidBody. Position driven by orbit equations or external solver (GMAT/Basilisk).
- Transitions are triggered by configurable spatial boundaries (altitude, proximity) or time-warp activation.
- The `HybridBlend` mode eliminates "jitter pop" by gradually weighting inputs from both propagators using the `blend_factor` (0.0 = fully analytical → 1.0 = fully physics).
- During `HybridBlend`, both the analytical propagator and avian forces are active; outputs are weighted and blended.

## Requirements

### Functional Requirements
- **FR-001**: **Time Decoupling**: Simulation time MUST be fully decoupled from wall-clock time.
- **FR-002**: **Deterministic Stepping**: Given identical inputs and seed, the simulation MUST produce bit-identical outputs regardless of tick rate or execution speed.
- **FR-003**: **PhysicsMode per Entity**: Every physics entity MUST have a `PhysicsMode` component governing its propagation method.
- **FR-004**: **Fast World Reset**: The simulation MUST support full world reset in <100ms to enable automated training loops and Monte Carlo batches.
