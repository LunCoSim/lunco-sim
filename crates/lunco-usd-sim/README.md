# lunco-usd-sim

The **Simulation-Specific Metadata and Logic** bridge for OpenUSD.

## Rationale
While `lunco-usd-avian` handles standard `UsdPhysics` for generic rigid bodies, high-fidelity robotics and vehicle assets (especially those authored in NVIDIA Omniverse or Isaac Sim) use specialized schemas like `PhysxVehicleWheelAPI`. 

Instead of implementing the full, computationally heavy NVIDIA PhysX vehicle math, this crate adopts the NVIDIA schemas strictly as a **Data Contract**. It "intercepts" these tags during the USD parsing phase and "substitutes" them with LunCo's optimized, lightweight simulation models (like Raycast Suspension).

This approach provides:
*   **Interoperability**: Rover models authored for industry-standard simulators work natively in LunCo.
*   **Performance**: Lightweight ConOps physics instead of heavy iterative solvers.
*   **Decoupling**: Keeps the core Avian bridge pure while handling proprietary or complex industry extensions here.

## Key Functions & Features

### 1. `UsdSimPlugin`
The main plugin that observes USD prims and injects simulation-specific behaviors.

### 2. "Duck Typing" USD Physics
The crate identifies specialized prims by looking for specific schema attributes (e.g., `physxVehicleWheel:radius`).
*   **Wheel Intercept**: When a `PhysxVehicleWheelAPI` is detected, the crate injects a `WheelRaycast` component from `lunco-mobility`.
*   **Future Mappings**: Will include `PhysxVehicleTireAPI` for friction parameters and `PhysxVehicleSuspensionAPI` for spring dynamics.

### 3. Priority & Overrides
Simulation-specific behaviors applied by this crate are intended to take priority over standard collision physics. If an object is marked as a Wheel, its standard collider logic should be bypassed in favor of raycast-based ground interaction.

## Implementation Status
*   [x] Basic `PhysxVehicleWheelAPI` intercept.
*   [ ] `PhysxVehicleTireAPI` mapping.
*   [ ] `PhysxVehicleSuspensionAPI` mapping.
*   [ ] Automatic removal/replacement of standard `UsdPhysics` colliders on intercepted prims.
