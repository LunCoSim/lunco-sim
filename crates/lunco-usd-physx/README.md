# lunco-usd-physx

Placeholder crate for mapping NVIDIA PhysX-specific USD schemas to Avian3D.

## Rationale
While `lunco-usd-avian` handles standard `USDPhysics`, some assets (especially from Isaac Sim) use advanced NVIDIA schemas like `PhysxVehicleAPI` for suspension and high-fidelity wheel dynamics. This crate will provide the bridge for those specific schemas to keep the core avian bridge lightweight.

## Future Scope
*   **PhysxVehicleAPI**: Mapping complex vehicle dynamics to Avian.
*   **Physics Materials**: Advanced friction and restitution properties.
*   **Joint Drives**: Detailed motor and drive configurations for robotics.
