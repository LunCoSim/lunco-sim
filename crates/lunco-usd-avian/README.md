# lunco-usd-avian

A reusable bridge between **OpenUSD Physics Schemas** and **Avian3D** for the Bevy game engine.

## Rationale
OpenUSD (Universal Scene Description) is the industry standard for 3D scene exchange, including complex physics properties defined via `USDPhysics`. Avian3D is a popular physics engine for Bevy. This crate provides an automated way to map USD-authored physics properties directly to Avian components, ensuring that "if it's defined in USD, it just works in Bevy."

By separating this into its own crate, we allow other Bevy + Avian projects to benefit from standard USD physics support without pulling in LunCo-specific simulation logic.

## Key Functions & Features

### 1. `UsdAvianPlugin`
The main Bevy plugin that sets up the physics mapping logic. It registers necessary types and observers.

### 2. Bevy Observers (0.18+)
Instead of a heavy manual loader, this crate uses high-performance observers that react to the addition of a `UsdPrimPath` component. 
*   **Automatic Mapping**: When an entity is tagged with a USD path, the crate looks up the corresponding Prim in the USD stage.
*   **RigidBody Mapping**: Maps `physics:rigidBodyEnabled` to Avian's `RigidBody::Dynamic`.
*   **Collider Mapping**: Maps USD primitives (like `Cube`) to Avian `Collider` components.

### 3. Components
*   **`UsdPrimPath`**: The "tag" component that links a Bevy entity to a specific Prim in a USD stage.
*   **`UsdStageResource`**: A component/resource that holds the `openusd` stage reader.

## Current Limitations
*   **Parser Maturity**: Relies on the `openusd` crate (native Rust), which currently has limited support for complex ASCII (`.usda`) property blocks.
*   **Primitive Support**: Currently focuses on basic primitives (Cube, Cylinder).
