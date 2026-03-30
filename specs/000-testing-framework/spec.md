# Feature Specification: 000-testing-framework

**Feature Branch**: `000-testing-framework`
**Created**: 2026-03-29
**Status**: Active
**Authority**: Project Constitution (Principle II, VIII, IX).

## Problem Statement
A high-fidelity digital twin must be mathematically and structurally verifiable. For **Stage 1**, we require a **Functional Testing Framework** that enforces architectural integrity and core space system logic (Action-to-Actuator). Advanced hardware-failure simulation is deferred to future milestones.

---

## 1. Architectural Compliance (Layer Isolation)

The 5-layer model (Actions -> Controller -> FSW -> OBC -> Plant) is a **hard boundary**.

### Isolation Law
- **Compliance Rules**:
    - **Layer N** can only read/write to **Layer N-1** or **Layer N+1**.
    - **Prohibited**: A Level 1 Actuator reading from Level 5 ActionState (bypass).
    - **Prohibited**: A Level 4 Controller writing directly to Level 2 OBC Ports (bypass).
- **Automated Verification**:
    - **Dependency Graph Audit**: CI tools must verify that Bevy Plugins for different layers do not have "layer-skipping" dependencies.

---

## 2. Core Functional Tiers (Stage 1 Mandate)

### Tier 1: Unit Testing (Logic & State)
- **Signal Logic**: Every system that calculates output (e.g., FSW Mixing, Controller Mapping) must pass isolated unit tests with mocked inputs.
- **Persistence (Save/Load)**: Components MUST survive a serialization/deserialization cycle without state drift.
- **Determinism**: All simulation systems must be deterministic (fixed timestep).

### Tier 2: Integration Testing (The Signal Path)
- **Signal Integrity (DAC Path)**: Verify that writing an `i16` value (e.g. `32767`) to a `DigitalPort` (L2) correctly scales via the `Wire` to a `PhysicalPort` (L1) (e.g., `32767` -> `Max_Torque`).
- **Sensor Return Path (ADC Path)**: Verify that physical state changes (L1 `f32`) correctly propagate and scale to the OBC's `DigitalPort` (L2 `i16` / `32767`) and are readable by the FSW (L3).
- **Resolution & Quantization**: Verify that low-resolution digital signals (e.g., 16-bit `-32768` to `32767`) result in expected "stepped" physical outputs on the Plant.

### Tier 3: Functional Verifiers (Headless Validation)

Verifiers run the simulation in **Headless Mode** with the visual system and user input plugins disabled.
- **Motion Verifier**: Verify the baseline rover reaches $X$ velocity in $Y$ meters along the **$-Z$ (Forward)** axis.
- **Braking Verifier**: Verify the baseline rover reaching zero velocity within $S$ seconds of a `Brake` command.
- **Platform Verifier**: Verify that the entire test suite runs and passes in **WASM (Browser)**.
- **Mass Range Verifier**: Verify simulation stability across a **mass-sweep range** from **2 kg to 1,000 kg** (ensuring physics engine convergence at diverse scales).

---

## 3. Headless Verification Pattern (Pure CI)

To ensure reliable verification in headless environments (e.g., GitHub Actions, Linux Servers) without GPU dependencies, the following Bevy 0.18.1 pattern is MANDATED:

### Manual Resource Initialization
Headless tests MUST use `MinimalPlugins` and manually initialize the following simulation registries to satisfy downstream plugin dependencies:
- **`AppTypeRegistry`**: Required for Avian reflection and diagnostic tools.
- **`Messages<AssetEvent<Mesh>>`**: Required for collider caching in Avian 0.6.1.
- **`Messages<CollisionStart / CollisionEnd>`**: Required for physical event tracking.
- **`Messages<Input / Mouse / Keyboard>`**: Required for FSW command integration.

---

## 4. Functional Test Suite (Baseline Rover)

The following matrix defines the exhaustive set of validation cases for the **Stage 1 Baseline Rover**. A "Pass" is only achieved if ALL listed layers reflect the expected state during the simulation step.

### Stage 1: Functional Case Matrix (i16/f32 Mapping)

| ID | Case | Input (L5) | Logic Result (L4/3) | Hardware (L2 `i16`) | Physical (L1 `f32`) | Requirement |
|---|---|---|---|---|---|---|
| **F-01** | Drive Fwd | `W` (Hold) | `DRIVE(1.0)` | `PORT_DRIVE` = `32767` | **`-Z`** Force/Torque | $v > 0$ |
| **F-02** | Drive Back | `S` (Hold) | `DRIVE(-1.0)`| `PORT_DRIVE` = `-32768`| **`+Z`** Force/Torque | $v < 0$ |
| **F-03A**| Skid Left | `A` (Hold) | `MIX(-1.0, 1.0)`| `PORT_L` = `-32768` | Diff. Torque | $\theta > 0$ (Yaw Left) |
| **F-03B**| Steer Left | `A` (Hold) | `STEER(-1.0)`| `PORT_STEER` = `-32768`| **$\theta > 0$** (Left) | Wheel Ang > 0 |
| **F-04A**| Skid Right | `D` (Hold) | `MIX(1.0, -1.0)`| `PORT_R` = `-32768` | Diff. Torque | $\theta < 0$ (Yaw Right) |
| **F-04B**| Steer Right | `D` (Hold) | `STEER(1.0)` | `PORT_STEER` = `32767` | **$\theta < 0$** (Right) | Wheel Ang < 0 |
| **F-05** | Brake | `Space` | `BRAKE` | `PORT_BRAKE` = `32767` | Resistance Force > 0 | $v \to 0$ |
| **F-06** | Coasting | None | `IDLE` | All `PORT` = `0` | Zero Torque | $\dot{v} \approx 0$ |
| **F-08** | Persistence| Save/Load | `FSW_STATE` persists| `DigitalPorts` persist | Pos/Vel identical | $\Delta < \epsilon$ |

---

## 4. Mocking & Isolation Strategy

To satisfy the **Testability Mandate (FR-010)**, the engine provides standard mocks:

### Mock OBC (Level 2 Mock)
Allows testing FSW (L3) logic without a physical plant.
- Provides a fixed set of `DigitalPort` entities.
- Records all `i16` writes for assertion: `assert_eq!(obc.get_port("M1"), 255)`.

### Mock Plant (Level 1 Mock)
Allows testing the full FSW->OBC signal path without the `avian` physics engine.
- Replaces `PhysicalPort` with a `ValueTracker` component.
- Asserts that `DAC` scaling is correct: `assert_eq!(plant.get_input("M1"), MaxTorque)`.

---

## 5. Future Verification Tiers (TBD)
> [!NOTE]
> The following requirements are deferred beyond Stage 1 and will be introduced as the hardware emulation matured.
- **Signal Noise & Filtering**: Injecting Gaussian noise into OBC inputs.
- **Fault Induction**: Cutting power rails mid-simulation.
- **Environmental Extremes**: High-speed impact stability and Zero-G drift.

---

## Key Entities & Terminology
For a complete definition of all entities (OBC, FSW, etc.), refer to the authoritative **[Engineering Ontology](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/ontology.md)**.
