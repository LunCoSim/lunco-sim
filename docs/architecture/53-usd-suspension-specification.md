# 53 — USD Suspension Specification & Alignment

> Status: Active · Audience: contributors on USD loaders, vehicle physics, and rover mobility

This document specifies the canonical USD/Omniverse representation for vehicle wheels and suspensions, and maps them to a unified, non-heuristic Bevy ECS architecture.

---

## 1. The Omniverse/PhysX Vehicle Schema Specification

In the NVIDIA Omniverse / PhysX 5 Vehicle SDK, a wheel assembly is represented by three core API schemas:
1. **`PhysxVehicleWheelAttachmentAPI`**: Serves as the primary connector and attachment point of the wheel/suspension assembly to the parent chassis.
2. **`PhysxVehicleWheelAPI`**: Defines wheel physical dimensions and dynamics (radius, width, mass, moment of inertia, damping rate).
3. **`PhysxVehicleSuspensionAPI`**: Defines suspension compliance (`springStrength`, `springDamperRate`, `travelDistance`, `sprungMass`). Note: there is **no `restLength`** on this API — PhysX models travel as `travelDistance` + `sprungMass`. LunCo's raycast model needs a rest length, so it is authored as a LunCo extension (`lunco:suspension:restLength`).

### LunCo extension APIs
Three concepts the PhysX vehicle schema does not model live in `luncoSchema` (`crates/lunco-usd/schema/schema.usda`), each its own applied API — one per prim role, because an applied schema's properties join the definition of every prim it is applied to:

| API | Property | Applied to | Why it is not PhysX |
| --- | --- | --- | --- |
| `LunCoSuspensionAPI` | `float lunco:suspension:restLength` | suspension prim, beside `PhysxVehicleSuspensionAPI` | PhysX has no `restLength` (`travelDistance` + `sprungMass` instead) |
| `LunCoWheelAPI` | `int lunco:wheel:index` | wheel prim, beside `PhysxVehicleWheelAPI` | PhysX's `index` lives on the WheelAttachment prim, which flat composition lacks |
| `LunCoSuspensionVisualAPI` | `uniform token lunco:suspensionVisual:role` | a strut's moving visual parts | the PhysX vehicle schema is physics-only |

`restLength` is `float` to match the `physxVehicleSuspension:*` attrs it sits beside — and the `travelDistance` it stands in for.

### Canonical Relationship Model
The specification decouples these schemas to allow physical and compliance properties to be shared or configured independently. Rather than relying on scene-graph hierarchy or naming conventions (heuristics), they are bound explicitly via **USD Relationships** (`rel`) defined on the attachment prim:

```usd
def Xform "WheelAttachment_FL" (
    prepend apiSchemas = ["PhysxVehicleWheelAttachmentAPI"]
)
{
    # Explicit USD Relationships linking to property prims
    rel physxVehicleWheelAttachment:wheel = </Rover/Wheel_FL>
    rel physxVehicleWheelAttachment:suspension = </Rover/Suspension_FL>
    
    # Attachment geometry relative to the chassis. `point3f` — NOT `double3`: the
    # frame attrs are the ones assets most often author at the wrong precision, so
    # `physx_vehicle_schemas_register_canonical_properties` pins them.
    point3f physxVehicleWheelAttachment:suspensionFramePosition = (-1.0, -0.15, -1.225)
}
```

---

## 2. Our Current Rovers Analysis

Our existing rover assets (`ackermann_rover.usda`, `six_wheel_independent.usda`, `rocker_bogie.usda`) utilize a **compact flat composition** where the `PhysxVehicleWheelAPI` and `PhysxVehicleSuspensionAPI` are referenced directly into a single cylinder prim representing the wheel:

```usd
# From ackermann_rover.usda
def Cylinder "Wheel_FL" (
    prepend references = [
        @../../components/mobility/wheel.usda@</Wheel>,
        @../../components/mobility/suspensions/standard.usda@</Suspension>,
    ]
)
{
    int lunco:wheel:index = 0            # this wheel's own values, and nothing else
    float physxVehicleWheel:radius = 0.4
}
```

In this simplified composition, the wheel and suspension properties live on the same Prim, so their relationship is implicit.

**The APIs are applied once, on the component prims, and arrive through the arcs.** `wheel.usda`'s `Wheel` applies `PhysxVehicleWheelAPI` + `PhysxVehicleEngineAPI` + `LunCoWheelAPI`; each `suspensions/*.usda`'s `Suspension` applies `PhysxVehicleSuspensionAPI` + `LunCoSuspensionAPI`. `apiSchemas` is a list-op and composes across reference arcs, so all 30 wheels across the seven rovers get their schemas from two files. A rover authors values, never schemas — re-shodding a wheel or retuning a spring is still one line in one place.

---

## 3. Bevy ECS Integration Architecture

To support both the nested Omniverse relationship model and our flat rover composition without duplicating data, the mapping pipeline aligns as follows:

```
[ USD Layer ]                                   [ Bevy ECS Layer ]
WheelAttachment (PhysxVehicleWheelAttachmentAPI) ───► Wheel Entity
     ├── rel:wheel ─────────────────────────────► WheelRaycast component
     └── rel:suspension ────────────────────────► Suspension component
```

### 3.1. Unified Components (Single Source of Truth)
We remove duplicate suspension fields (`rest_length`, `spring_k`, `damping_c`) from `WheelRaycast` and rely entirely on the unified `Suspension` component (`crates/lunco-mobility/src/lib.rs`):

```rust
// Unified Suspension component for both joint-based and raycast wheels
#[derive(Component, Debug, Clone, Reflect)]
pub struct Suspension {
    pub rest_length: f64,
    pub spring_k: f64,
    pub damping_c: f64,
    pub local_axis: DVec3,
}
```

### 3.2. USD Loading Resolution (`lunco-usd-sim`)

**Detection is by applied schema, never by attribute presence.** A prim is a wheel because it applies `PhysxVehicleWheelAPI` (`reader.has_api_schema`), exactly as `PhysxVehicleContextAPI` / `…TankDifferentialAPI` / `…AckermannSteeringAPI` are detected; a prim is a strut visual because it applies `LunCoSuspensionVisualAPI`. Applying the API is the claim; the attributes are its parameters. Sniffing for an attribute instead conflates "declares itself a wheel" with "happens to carry a wheel-ish attr", and silently makes the schema optional.

The loader then resolves a wheel's suspension via `resolve_suspension_params`, a two-step path:
1. **Canonical (Relationship-based):** Pass 1 (`collect_joint_scan_read`) records every `PhysxVehicleWheelAttachmentAPI` prim's `physxVehicleWheelAttachment:wheel` → `:suspension` binding into a `wheel_attachment_targets` map, keyed by `(stage, wheel path)` — a prim path is unique only within its stage, so the same rover loaded twice repeats `/Rover/Wheel_FL`. When the wheel prim is processed in Pass 2, the resolver follows that binding and reads the suspension attrs off the referenced suspension prim.
2. **Flat (Fallback):** If no attachment targets this wheel, the attrs are read directly off the wheel prim — this is LunCo's compact composition, where the wheel references the suspension file directly and the attrs compose onto the wheel prim itself.
3. **Strict validation (§4):** If neither path yields all three params, the resolver returns `None` and the raycast-wheel branch refuses to spawn (no silent defaults).

**Attribute names read:** `physxVehicleSuspension:springStrength`, `physxVehicleSuspension:springDamperRate` (NVIDIA canonical), and `lunco:suspension:restLength` (LunCo extension — PhysX has no equivalent). The canonical names are defined in the reconstructed `crates/lunco-usd/schema/core/physxSchema.usda` and pinned by the `physx_vehicle_schemas_register_canonical_properties` drift test.

### 3.3. Physics & Visual Updates (`lunco-mobility`)
* **`apply_wheel_suspension`:** Queries `(&mut WheelRaycast, &Suspension, &RayHits, &Transform, &ChildOf)` to solve Hooke's spring-damper equations using the `Suspension` component values.
* **`update_suspension_visuals`:** Queries `(&WheelRaycast, &Suspension, ...)` to scale the spring mesh and translate the piston along the suspension travel axis.

### 3.4. Live Tuning & Property Updates (`lunco-scene-commands`)
* The `SetObjectProperty` live mutation system queries the `Suspension` component directly when setting suspension properties (`spring_k`, `damping_c`, `rest_length`).
* This enables live CLI suspension tuning to work uniformly across both **joint-based** and **raycast** vehicles.

---

## 4. Strict Validation: Handling Missing Suspension (No Fallbacks)

Silently inserting default values for missing suspension schemas is forbidden because it hides configuration bugs (e.g. an artist forgetting to add a suspension reference). We enforce a strict, fallback-free validation contract:

### 4.1. Compliant Raycast Wheels (Requires Suspension)
A raycast wheel uses an analytical spring-damper model:

$$F = k \cdot (\text{rest\_length} - \text{distance}) + c \cdot v$$

It **cannot function** without suspension compliance parameters ($k, c, \text{rest\_length}$). 
* If a wheel prim is parsed for raycasting (`PhysxVehicleWheelAPI`) but lacks a resolved `PhysxVehicleSuspensionAPI` (neither on the prim nor referenced via relationships), the loader **fails validation loudly**.
* It logs a compilation error and **refuses to map or spawn the wheel** in the simulation, exposing the asset composition bug immediately.

### 4.2. Rigid Wheels (True Rigid Axles)
If a vehicle axle is intended to be completely rigid (no suspension travel or compliance):
* The asset **must** be configured as a physical joint-based wheel (carrying a `PhysicsRevoluteJoint` connecting the wheel body directly to the chassis/axle, with no intermediate `PrismaticJoint` or suspension schema).
* Contact normal forces are resolved natively via the rigid body and collider contacts in the physics engine (`Avian3D`), representing a true rigid-axle chassis without compliance forces.

---

## 5. Suspension Compliance (`PhysxVehicleSuspensionComplianceAPI`) — PLANNED

> **Status: Not yet implemented in the loader or mobility systems.** The schema
> is registered in `core/physxSchema.usda` (with canonical attr types pinned by
> the drift test), and this section specifies the intended ECS mapping. It is
> visual-only for now — see the PhysX caveat below.

High-fidelity vehicle modeling requires simulating how wheel alignment changes dynamically as the suspension travels (e.g., changes in camber and toe under load).

NVIDIA PhysX defines this via the **`PhysxVehicleSuspensionComplianceAPI`** applied to the wheel attachment.

### 5.1. Compliance Attributes in the PhysX Spec
Each attribute is a **graph**: an array of up to 3 points, each pairing a normalized jounce with a value. **Jounce convention:** `0` = max droop (fully elongated), `1` = max compression. The jounce sequence must be monotonically increasing; one point = constant; empty = 0.0.
* **`float2[] physxVehicleSuspensionCompliance:wheelCamberAngle`**: `(jounce, camber)` pairs, radians.
* **`float2[] physxVehicleSuspensionCompliance:wheelToeAngle`**: `(jounce, toe)` pairs, radians.
* **`float4[] physxVehicleSuspensionCompliance:suspensionForceAppPoint`**: `(jounce, x, y, z)` offsets.
* **`float4[] physxVehicleSuspensionCompliance:tireForceAppPoint`**: `(jounce, x, y, z)` offsets.

> Note the array element types: `float2[]` / `float4[]`, **not** `float[]` / `float3[]` — the jounce is packed as the first component. This is the single most common reconstruction/authoring mistake.

### 5.2. Compliance Component Definition (Planned)
A 2-point linear subset of the PhysX graph — endpoints only, linearly interpolated. Field names follow the jounce convention (jounce 0 = max droop, jounce 1 = max compression), not "rest" (which is ambiguous):

```rust
/// Tracks dynamic wheel alignment changes under suspension compression.
/// A 2-point linear subset of PhysX's compliance graph.
#[derive(Component, Debug, Clone, Reflect)]
pub struct SuspensionCompliance {
    /// Camber angle at max droop, jounce = 0 (radians).
    pub camber_at_max_droop: f64,
    /// Camber angle at max compression, jounce = 1 (radians).
    pub camber_at_max_compression: f64,
    /// Toe angle at max droop, jounce = 0 (radians).
    pub toe_at_max_droop: f64,
    /// Toe angle at max compression, jounce = 1 (radians).
    pub toe_at_max_compression: f64,
}
```

> **PhysX caveat — visual-only for now.** In PhysX, camber/toe compliance feeds the
> **tire-force computation** (camber thrust, slip projection), not just the visual
> wheel orientation. This LunCo mapping applies it to `update_suspension_visuals`
> only. Wiring compliance into the tire-force model is future work; until then a
> LunCo vehicle with compliance authored will *look* right but its tire forces
> will not reflect the camber/toe change.

### 5.3. Dynamic Alignment Application
In the `update_suspension_visuals` system, when we calculate the suspension compression ratio:

$$\text{ratio} = \frac{\text{rest\_length} - \text{current\_distance}}{\text{rest\_length}}$$

If a `SuspensionCompliance` component is present on the wheel, we interpolate the toe and camber and apply the corresponding rotations to the visual wheel entity:

```rust
// In update_suspension_visuals
if let Some(compliance) = compliance_opt {
    let camber = compliance.camber_at_max_droop
        + (compliance.camber_at_max_compression - compliance.camber_at_max_droop) * ratio;
    let toe = compliance.toe_at_max_droop
        + (compliance.toe_at_max_compression - compliance.toe_at_max_droop) * ratio;

    // Combine camber/toe with rolling spin_angle to form the visual rotation
    let alignment_rot = Quat::from_rotation_y(toe as f32) * Quat::from_rotation_z(camber as f32);
    visual_tf.rotation = wheel_rotation * alignment_rot * Quat::from_rotation_x(wheel.spin_angle as f32);
}
```

This design keeps the compliance properties fully isolated and modular, allowing the loader to attach them only when authored, without adding complexity to the core physics or standard visual updates.
