# lunco-usd-avian

A reusable bridge between **OpenUSD Physics Schemas** and **Avian3D** for the Bevy game engine.

## Rationale
OpenUSD (Universal Scene Description) is the industry standard for 3D scene exchange, including complex physics properties defined via `USDPhysics`. Avian3D is a popular physics engine for Bevy. This crate provides an automated way to map USD-authored physics properties directly to Avian components, ensuring that "if it's defined in USD, it just works in Bevy."

By separating this into its own crate, we allow other Bevy + Avian projects to benefit from standard USD physics support without pulling in LunCo-specific simulation logic.

## Key Functions & Features

### 1. `UsdAvianPlugin`
The main Bevy plugin that sets up the physics mapping logic. It registers necessary types and observers.

### 2. Mapping (observers + deferred resolution)
When an entity is tagged with a `UsdPrimPath`, the crate looks up the Prim and maps:
*   **RigidBody** — `PhysicsRigidBodyAPI` / `physics:rigidBodyEnabled` → `RigidBody`, with **mass-properties** `physics:mass` / `physics:diagonalInertia` / `physics:centerOfMass` → the Avian override components (`Mass`/`AngularInertia`/`CenterOfMass`, shared with the runtime mass-props ports).
*   **Colliders** — every `UsdGeom` shape: `Cube`→cuboid, `Sphere`, `Cylinder`, `Cone`, `Capsule`, `Mesh`→trimesh (DEM grids→heightfield), `Plane`→thin cuboid. Compound bodies via child `PhysicsCollisionAPI`.
*   **Joints** — see below. Built by a deferred system (matches `physics:body0/1` paths → entities, gated on Avian island-admission) so it survives async USD loads.

### 3. Joints (the single home for Avian joint construction)
Standard `UsdPhysics` joint prims → Avian joints — `RevoluteJoint`, `PrismaticJoint`,
`FixedJoint`, `SphericalJoint` (cone/twist limits), `DistanceJoint` (min/max). A
generic `PhysicsD6Joint` is **reduced** to the primitive matching its free DOFs
(per-DOF `PhysicsLimitAPI`). `UsdPhysicsDriveAPI` (`drive:{angular,linear}:physics:
{targetPosition,targetVelocity,maxForce}`) configures the joint motor at load.
`wheel_revolute_joint` is the one programmatic joint (the physical-wheel hinge),
exposed here so *all* joint-building lives in this crate. Scalar physics attrs are
read **f32-first** (`read_scalar_attribute`) — Omniverse authors `float`, and a
`::<f64>`-only read silently drops them. Full schema map: [`docs/architecture/21-domain-usd.md`](../../docs/architecture/21-domain-usd.md#physics-joints).

### 4. Components
*   **`UsdPrimPath`**: links a Bevy entity to a Prim in a USD stage.
*   **`PendingUsdJoint`**: a joint awaiting both bodies' entities.

## Current Limitations
*   **Parser Maturity**: Relies on the `openusd` crate (native Rust), which currently has limited support for complex ASCII (`.usda`) property blocks.
*   **D6 joints**: only joints reducible to a single Avian primitive are built; a genuinely multi-DOF D6 (e.g. two free rotations) warns.
*   **Convex hulls**: meshes always build as trimesh (no convex-hull decomposition).
