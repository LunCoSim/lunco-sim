# Rover Dynamic Parameter Tuning Guide

This directory contains the USD definitions for all surface rovers in LunCoSim. Since LunCoSim follows **Article X of the Project Constitution (The Tunability Mandate)**, all vehicle masses, joints, motor torques, and suspension settings are driven directly by attributes authored in these USD files rather than being hardcoded in Rust.

---

## 🛠️ Editing Vehicle Parameters

To tune a rover, edit its corresponding `.usda` file (e.g., [rocker_bogie.usda](file:///home/rod/Documents/luncosim-workspace/main/assets/vessels/rovers/rocker_bogie.usda)). The primary parameters are grouped below:

### 1. Mass & Inertia Properties
These live on the root vehicle or link `Xform` prims:
*   `float physics:mass`: The mass of the link or body (in kg).
    *   *Chassis root:* Default is `300.0` kg.
    *   *Rockers:* Default is `50.0` kg.
    *   *Bogies:* Default is `30.0` kg.
    *   *Wheels:* Default is `25.0` kg.
*   `float3 physics:diagonalInertia`: Rotational inertia components $(I_{xx}, I_{yy}, I_{zz})$ about the principal axes. Exposing these ensures correct rotational acceleration and stability during steering.

### 2. Rocker-Bogie Differential Coupling
The differential is a standard `PhysxPhysicsGearJoint` (`Differential`) over the two chassis↔rocker hinges (`physxGearJoint:hinge0 = HingeL`, `hinge1 = HingeR`), coupling `RockerL`/`RockerR` to keep the chassis level. It is softened by the joint's own angular `PhysicsDriveAPI:angular` — a spring-damper, not a rigid gear (which would chatter on terrain). Zero the drive stiffness/damping (via an `over "Differential"`) to disable it.
*   `float drive:angular:physics:stiffness`: Coupling stiffness ($k$, default `15000.0`). Controls how strongly the rockers are forced to mirror each other's pitch.
    > [!WARNING]
    > To maintain simulation stability, the stiffness must satisfy the explicit-penalty stability limit: $k < \frac{I}{dt^2}$ (where $I$ is the rocker inertia and $dt \approx 1/64$ s). Keeping it under `250000` is recommended.
*   `float drive:angular:physics:damping`: Coupling damping ($c$, default `1500.0`). Prevents the differential from ringing or oscillating.
*   `float drive:angular:physics:targetPosition`: Target for $\theta_{\text{left}} + \theta_{\text{right}}$ (rad, default `0.0`).

### 3. Suspension Parameters (Authored per Wheel)
Even for joint-based physical rovers, the suspension settings are read from standard PhysX/Omniverse schema fields on each `Cylinder` wheel:
*   `float physxVehicleSuspension:springStrength`: Suspension spring constant (default `12000.0` N/m). Lower values make the suspension softer.
*   `float physxVehicleSuspension:springDamperRate`: Suspension damper coefficient (default `2500.0` N·s/m). Prevents the vehicle from bouncing excessively.
*   `float lunco:suspension:restLength`: Uncompressed suspension length (default `0.5` m).

### 4. Drivetrain & Motor Actuation (Authored per Wheel)
Controlling traction and speed:
*   `float physxVehicleEngine:peakTorque`: Maximum motor torque (default `300.0` N·m). High torque allows climbing steep slopes but can cause wheelspin.
*   `float physxVehicleEngine:maxRotationSpeed`: Motor free-spin angular velocity limit (default `60.0` rad/s).
*   `float physxVehicleWheel:maxBrakeTorque`: Braking authority (default `1500.0` N·m) to decelerate or lock the wheels.
*   `double lunco:frictionCoefficient`: Coulomb friction coefficient ($\mu$, default `0.8`).
*   `float physxVehicleTire:longitudinalStiffness`: Longitudinal tire grip stiffness (default `8000.0` N per unit slip).

---

## 📐 Coordinate System Reference
When editing coordinates for translations or joint local anchors (`physics:localPos0` / `physics:localPos1`):
*   **X-axis (Lateral):** Positive is **Right**, Negative is **Left**.
*   **Y-axis (Vertical):** Positive is **Up**, Negative is **Down**.
*   **Z-axis (Longitudinal):** Positive is **Backward**, Negative is **Forward**.
