# Feature Specification: 000-testing-framework

**Feature Branch**: `000-testing-framework`
**Created**: 2026-03-29
**Status**: Draft
**Authority**: Project Constitution (Principle II, VIII, IX).

## Problem Statement
A high-fidelity digital twin must be mathematically and structurally verifiable. For **Stage 1**, we require a **Functional Testing Framework** that enforces architectural integrity and core vessel logic (Action-to-Actuator). Advanced hardware-failure simulation is deferred to future milestones.

---

## 1. Architectural Compliance (Layer Isolation)

The 5-layer model (Actions -> Controller -> FSW -> OBC -> Plant) is a **hard boundary**.

### Isolation Law
- **Compliance Rules**:
    - **Layer N** can only read/write to **Layer N-1** or **Layer N+1**.
    - **Prohibited**: A Level 1 Actuator reading from Level 5 ActionState (bypass).
    - **Prohibited**: A Level 4 Controller writing directly to Level 2 OBC Pins (bypass).
- **Automated Verification**:
    - **Dependency Graph Audit**: CI tools must verify that Bevy Plugins for different layers do not have "layer-skipping" dependencies.

---

## 2. Core Functional Tiers (Stage 1 Mandate)

### Tier 1: Unit Testing (Logic & State)
- **Signal Logic**: Every system that calculates output (e.g., FSW Mixing, Controller Mapping) must pass isolated unit tests with mocked inputs.
- **Persistence (Save/Load)**: Components MUST survive a serialization/deserialization cycle without state drift.
- **Determinism**: All simulation systems must be deterministic (fixed timestep).

### Tier 2: Integration Testing (The Signal Path)
- **Signal Integrity**: Verify the end-to-end path from **Action (L5)** -> **Command (L4)** -> **I/O (L2)** -> **Force (L1)**.
- **Sensor Return Path**: Verify that physical state changes (L1) correctly propagate to the FSW's internal telemetry state (L3).

### Tier 3: Functional Oracles (Headless Validation)
Oracle-based tests run the simulation in **Headless Mode** at high speed.
- **Motion Oracle**: Verify the baseline rover reaches $X$ velocity in $Y$ meters.
- **Braking Oracle**: Verify the baseline rover reaching zero velocity within $S$ seconds of a `Brake` command.
- **Platform Oracle**: Verify that the entire test suite runs and passes in **WASM (Browser)**.

---

## 3. Functional Test Suite (Baseline Rover)

The following matrix defines the exhaustive set of validation cases for the **Stage 1 Baseline Rover**. A "Pass" is only achieved if ALL listed layers reflect the expected state during the simulation step.

### Stage 1: Functional Case Matrix

| ID | Case | Input (L5) | Logic Result (L4/3) | Hardware State (L2) | Physical Result (L3/1) |
|---|---|---|---|---|---|
| **F-01** | Drive Forward | `W` (Hold) | `CMD_DRIVE(1.0)` | `PWM_DRIVE` > 0 | $+Z$ Velocity > 0 |
| **F-02** | Drive Backward | `S` (Hold) | `CMD_DRIVE(-1.0)`| `PWM_DRIVE` < 0 | $+Z$ Velocity < 0 |
| **F-03** | Steer Left | `A` (Hold) | `CMD_STEER(-1.0)`| `PWM_STEER_FL` < 0| Wheel Yaw < 0 |
| **F-04** | Steer Right | `D` (Hold) | `CMD_STEER(1.0)` | `PWM_STEER_FR` > 0| Wheel Yaw > 0 |
| **F-05** | Brake | `Space` | `CMD_BRAKE` | `DIGITAL_BRAKE` = `HIGH` | Ang. Velocity $\to 0$ |
| **F-06** | Idle/Coasting | None | `CMD_IDLE` | All `PWM` = 0 | Free-rolling Inertia |
| **F-07** | Possession | `P` (Toggle) | `Avatar::Possess` | Controller Attached | Input Focus Swap |
| **F-08** | Persistence | Save/Load | `FSW_STATE` persists| `PinStates` persist | Pos/Vel identical |

---

## 4. Future Verification Tiers (TBD)
> [!NOTE]
> The following requirements are deferred beyond Stage 1 and will be introduced as the hardware emulation matured.
- **Signal Noise & Filtering**: Injecting Gaussian noise into OBC inputs.
- **Fault Induction**: Cutting power rails mid-simulation.
- **Environmental Extremes**: High-speed impact stability and Zero-G drift.

---

## Key Entities & Terminology
For a complete definition of all entities (OBC, FSW, etc.), refer to the authoritative **[Engineering Ontology](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/ontology.md)**.
