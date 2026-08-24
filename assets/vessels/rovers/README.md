# Rover Dynamic Parameter Tuning Guide

This directory contains the USD definitions for all surface rovers in LunCoSim. Since LunCoSim follows **Article X of the Project Constitution (The Tunability Mandate)**, all vehicle masses, joints, motor torques, and suspension settings are driven directly by attributes authored in these USD files rather than being hardcoded in Rust.

**One parameter set, two wheel kinds.** Raycast and physical (joint) wheels read the SAME attributes through one strict reader (`lunco-usd-sim/src/wheel_params.rs`); only force generation differs. Every drivetrain/tire attribute is **required** — a wheel missing any refuses to spawn and the error names all of them. The defaults live in `components/mobility/wheel.usda` (+ tires/suspensions), which every wheel composes; a rover authors only its own decisions (pose, standard `physxVehicleWheelAttachment:index`, explicit drive/steer connections, variants). The composed completeness is pinned by `crates/lunco-usd/tests/mobility_composition.rs`.

**Live tuning.** All wheel params carry schema-level slider hints: select a rover, Shift+click a wheel to drill into it, and edit in the Inspector's 🎚 Parameters section. Edits flow `ApplyUsdOp → document → in-place resync` (entities and joints survive). See `skills/build-vehicle/SKILL.md` for the full assembly recipe.

---

## 🛠️ Editing Vehicle Parameters

To tune a rover, edit its corresponding `.usda` file (e.g., [rocker_bogie.usda](file:///home/rod/Documents/luncosim-workspace/main/assets/vessels/rovers/rocker_bogie.usda)). The primary parameters are grouped below:

### 1. Mass & Inertia Properties
These live on the root vehicle or link `Xform` prims:
*   `float physics:mass`: The mass of the link or body (in kg).
    *   *Perseverance-class chassis root:* `720.0` kg; with articulated links,
        wheels, drives, avionics, and camera hardware the composed rover is
        approximately `1,025.0` kg.
    *   *Rockers:* Default is `50.0` kg.
    *   *Bogies:* Default is `30.0` kg.
*   `float physxVehicleWheel:mass`: Standard PhysX wheel mass, default `25.0` kg;
    shared by raycast and physical realizations.
*   `float3 physics:diagonalInertia`: Rotational inertia components $(I_{xx}, I_{yy}, I_{zz})$ about the principal axes. Exposing these ensures correct rotational acceleration and stability during steering.

### 2. Rocker-Bogie Differential Coupling
The differential is a standard `PhysxPhysicsGearJoint` (`Differential`) over the two chassis↔rocker hinges (`physxGearJoint:hinge0 = HingeL`, `hinge1 = HingeR`), coupling `RockerL`/`RockerR` to keep the chassis level. It is softened by the joint's own angular `PhysicsDriveAPI:angular` — a spring-damper, not a rigid gear (which would chatter on terrain). Zero the drive stiffness/damping (via an `over "Differential"`) to disable it.
*   `float drive:angular:physics:stiffness`: Coupling stiffness ($k$, default `15000.0`). Controls how strongly the rockers are forced to mirror each other's pitch.
    > [!WARNING]
    > The gear drive is integrated implicitly at the authoritative physics substep, so there is no asset-specific explicit-penalty limit. Keep stiffness and damping finite and non-negative, author the rocker inertia, and run the USD linter before play.
*   `float drive:angular:physics:damping`: Coupling damping ($c$, default `1500.0`). Prevents the differential from ringing or oscillating.
*   `float drive:angular:physics:targetPosition`: Target for $\theta_{\text{left}} + \theta_{\text{right}}$ (rad, default `0.0`).

### 3. Suspension Parameters (Authored per Wheel)
Even for joint-based physical rovers, the suspension settings are read from standard PhysX/Omniverse schema fields on each `Cylinder` wheel:
*   `float physxVehicleSuspension:springStrength`: Suspension spring constant (default `12000.0` N/m). Lower values make the suspension softer.
*   `float physxVehicleSuspension:springDamperRate`: Suspension damper coefficient (default `2500.0` N·s/m). Prevents the vehicle from bouncing excessively.
*   `float lunco:suspension:restLength`: Uncompressed suspension length (default `0.5` m).

### 4. Drivetrain & Motor Actuation (Authored per Wheel)
Controlling traction and speed:
*   `float lunco:motor:stallTorque` and `float lunco:motor:noLoadSpeed`: Motor-shaft
    torque curve. The optional `lunco:gearbox:ratio`, `:efficiency`, and
    `:maxOutputTorque` reduce it to the axle. These values live on the motor and
    gearbox parts, not on a wheel, so both physical and raycast realizations consume
    one drivetrain contract. See `assets/scenarios/tests/drivetrain_parity.rhai`.
*   `float physxVehicleWheel:maxBrakeTorque`: Braking authority (default `1500.0` N·m) to decelerate or lock the wheels.
*   `float physics:dynamicFriction`: standard `UsdPhysicsMaterialAPI` Coulomb coefficient ($\mu$) — authored on the TIRE (`components/mobility/tires/*.usda`), composed onto the wheel by its `tire` variant, and consumed by both wheel realizations.
*   `float physxVehicleTire:longitudinalStiffness`: Longitudinal tire grip stiffness (default `8000.0` N per unit slip).
*   `float2 physxVehicleTire:lateralStiffnessGraph`: Standard PhysX load graph `(minimum normalized load, maximum stiffness)` in `(ratio, N/rad)`.
*   `float physxVehicleTire:restLoad`: Standard PhysX tire reference load in newtons; required by LunCoSim's shared tire projection.

---

## 📐 Coordinate System Reference
When editing coordinates for translations or joint local anchors (`physics:localPos0` / `physics:localPos1`):
*   **X-axis (Lateral):** Positive is **Right**, Negative is **Left**.
*   **Y-axis (Vertical):** Positive is **Up**, Negative is **Down**.
*   **Z-axis (Longitudinal):** Positive is **Backward**, Negative is **Forward**.
