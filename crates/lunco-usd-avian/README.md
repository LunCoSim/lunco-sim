# lunco-usd-avian

A reusable bridge between **OpenUSD Physics Schemas** and **Avian3D** for the Bevy game engine.

## Rationale
OpenUSD (Universal Scene Description) is the industry standard for 3D scene exchange, including complex physics properties defined via `USDPhysics`. Avian3D is a popular physics engine for Bevy. This crate provides an automated way to map USD-authored physics properties directly to Avian components, ensuring that "if it's defined in USD, it just works in Bevy."

By separating this into its own crate, we allow other Bevy + Avian projects to benefit from standard USD physics support without pulling in LunCo-specific simulation logic.

Runtime physics projection is isolated from authored lint fact extraction.
`lunco-usd-avian-reader` owns the shared composed-stage geometry, joint, and
physics-attribute readers. `lunco-usd-avian-lint` uses that reader package for
facts consumed by the Rhai USD lint policy, while this package owns runtime
entity construction and lifecycle.

## Key Functions & Features

### 1. `UsdAvianPlugin`
The main Bevy plugin that sets up the physics mapping logic. It registers necessary types and observers.

### 2. Mapping (observers + deferred resolution)
When an entity is tagged with a `UsdPrimPath`, the crate looks up the Prim and maps:
*   **RigidBody** — an applied `PhysicsRigidBodyAPI` is the only thing that makes a prim a body; its own `physics:rigidBodyEnabled` (default true) says whether that body is simulated → `RigidBody`, with **mass-properties** `physics:mass` / `physics:diagonalInertia` / `physics:centerOfMass` → the Avian override components (`Mass`/`AngularInertia`/`CenterOfMass`, shared with the runtime mass-props ports).
*   **Colliders** — USD `Cube`, `Sphere`, `Cylinder`, `Cone`, and `Capsule` map to matching analytic shapes. A finite `Plane` remains a zero-thickness triangle surface. `Mesh` uses its composed indexed points/topology and the shared typed Avian approximation contract: `none`, `convexHull`, `convexDecomposition`, and `boundingCube`. The standard `boundingSphere` and `meshSimplification` tokens are recognized by the USD schema but are not implemented by Avian, so projection rejects them explicitly. `none` creates a triangle mesh and is rejected on dynamic bodies. `boundingCube` currently uses the source mesh's local-axis-aligned vertex bounds. DEM grids use heightfields. Compound bodies use child `PhysicsCollisionAPI` geometry.
*   **Joints** — see below. The deferred USD projector matches `physics:body0/1` paths to entities, then passes normalized facts to `lunco-usd-avian-joints`, which owns native admission so it survives async USD loads.

### 3. Joints
Standard `UsdPhysics` joint prims → Avian joints — `RevoluteJoint`, `PrismaticJoint`,
`FixedJoint`, `SphericalJoint` (cone/twist limits), `DistanceJoint` (min/max). A
generic `PhysicsD6Joint` is **reduced** to the primitive matching its free DOFs
(per-DOF `PhysicsLimitAPI`). `UsdPhysicsDriveAPI` (`drive:{angular,linear}:physics:
{targetPosition,targetVelocity,maxForce}`) configures the joint motor at load.
The USD projector owns schema interpretation; `lunco-usd-avian-joints` owns the
native constructors, collision suppression, seating, solver-island admission,
and detach ordering. Its `wheel_revolute_joint` path is shared by synthesized
vehicle hinges. Scalar physics attrs are read **f32-first** (`read_scalar_attribute`)
— Omniverse authors `float`, and a `::<f64>`-only read silently drops them. Full
schema map: [`docs/architecture/21-domain-usd.md`](../../docs/architecture/21-domain-usd.md#physics-joints).

### 4. Components
*   **`UsdPrimPath`**: links a Bevy entity to a Prim in a USD stage.
*   **`PendingUsdJoint`**: a joint awaiting both bodies' entities.

## Current Limitations
*   **Parser Maturity**: Relies on the `openusd` crate (native Rust), which currently has limited support for complex ASCII (`.usda`) property blocks.
*   **D6 joints**: only joints reducible to a single Avian primitive are built; a genuinely multi-DOF D6 (e.g. two free rotations) warns.
*   **NURBS collision**: `UsdGeomNurbsPatch` is not directly a USD Physics mesh collider. Use the explicit `PlanNurbsCollisionProxy` query with a positive `deviation_tolerance_m` in canonical metres and the `nurbs.rhai` proposal to author an invisible, source-linked `UsdGeomMesh` proxy. The physical cook refines the surface and trim tessellation until the symmetric sampled vertex-to-triangle distance between successive levels meets that target, then records the selected resolution and measured estimate. Avian re-cooks those values and rejects stale or edited proxy geometry. The estimate is not a certified upper bound on exact NURBS surface deviation. Unsupported approximation tokens are rejected instead of silently mapped to another shape.
