# 53 — USD Suspension Specification & Alignment

> Status: Active · Audience: contributors on USD loaders, vehicle physics, and rover mobility

This document specifies the canonical USD/Omniverse representation for vehicle wheels and suspensions, and maps them to a unified, non-heuristic Bevy ECS architecture.

---

## 0. Two spring mechanisms — which one to author

The repo has two, and they are not interchangeable. Pick by what the spring carries.

| | **Raycast vehicle suspension** | **Passive prismatic suspension** |
| --- | --- | --- |
| Use for | **raycast wheels** on a reduced vehicle realization | any **physical suspension member** whose load passes through a rigid-body joint: wheels, landing legs, dampers, deployables |
| Authored as | `PhysxVehicleSuspensionAPI` (+ `LunCoSuspensionAPI` for `restLength`), on the wheel prim or on a suspension prim bound from a `PhysxVehicleWheelAttachmentAPI` | `PhysicsPrismaticJoint` with `LunCoPrismaticSuspensionAPI`; the joint owns axis, anchors, limits, and rotational lock, while the material owns only the axial reaction |
| Parameters | `physxVehicleSuspension:springStrength` / `:springDamperRate`, `lunco:suspension:restLength` | standard `drive:linear:physics:{targetPosition,targetVelocity,stiffness,damping,maxForce,type}` plus `lunco:prismaticSuspension:yieldForce`; `physics:lowerLimit` / `:upperLimit` own the stroke |
| Ground contact | a **ray** from the attachment finds the ground; no wheel collider carries the load | ordinary rigid-body **contacts** between the foot/pad collider and the ground |
| Who integrates the spring | `lunco-mobility`'s `apply_wheel_suspension`, analytically (§3.3) | `lunco-cosim`'s passive material constraint in Avian's existing substep solver; no second geometric solve |
| Stroke and reaction read from | the `WheelRaycast` / `Suspension` components | the joint's own cosim ports, `displacement` (m, signed) and `force` (N) — `lunco-cosim`'s `JOINT_DISPLACEMENT_PORT` / `JOINT_FORCE_PORT` |

Both are legitimate realizations of the same authored wheel contract; neither is
inferred from names or selected by a Rust special case. A raycast wheel has no
prismatic joint and its suspension force never appears as a joint reaction, so a
strut's load cannot be read that way. A physical wheel has a rigid carrier and a
revolute wheel joint, so its load and wheel torque pass through the authored
rigid-body topology.

Sections 1–5 below specify the **raycast** mechanism and its shared authoring
contracts. A passive physical member is a `PhysicsPrismaticJoint` with the
standard `PhysicsDriveAPI:linear` plus the narrow
`LunCoPrismaticSuspensionAPI` extension: `physics:lowerLimit` / `:upperLimit`
bound the stroke, `physics:localRot0/1` carry the authored frame, and anchors are
left unauthored when the body transforms already place the joint at zero. The
standard drive carries the target, elastic coefficients, and force capacity;
the extension adds only the yield load and selects one-sided elastic/plastic
material semantics. The native bilateral motor is disabled for this marked
passive role, so it cannot become a second axial mechanism or return energy on
rebound. The joint publishes the material reaction through its output-only
`force` port.
`assets/vessels/landers/descent_lander.usda` uses this passive boundary; a
commandable elevator or actuator may still use the standard drive contract.

### 0.3 Schema reuse decision

The standard schemas were checked before defining the passive extension. The
ownership boundary is:

| Physical fact | Existing owner | LunCo addition |
| --- | --- | --- |
| Rigid-body topology, prismatic axis, frames, limits | `PhysicsPrismaticJoint` / `PhysicsLimitAPI` | none |
| Rest target, stiffness, damping, and maximum force | `PhysicsDriveAPI:linear` | none; do not duplicate these under `lunco:*` |
| Contact friction and restitution | `PhysicsMaterialAPI` | none |
| Raycast-wheel spring, damper, and travel | `PhysxVehicleSuspensionAPI` | `LunCoSuspensionAPI` only supplies the missing raycast `restLength` |
| One-sided compression with irreversible plastic set on an arbitrary rigid-body joint | no standard USD/PhysX schema | `LunCoPrismaticSuspensionAPI` only supplies `yieldForce` and the passive-role marker |

`PhysicsLimitAPI` constrains travel but does not define a material reaction.
`PhysxVehicleSuspensionAPI` belongs to the PhysX wheel/raycast attachment model;
it has no yield or permanent-crush state and is not a substitute for a physical
prismatic landing member. Therefore the custom prismatic API remains only for
the one constitutive fact no inspected standard schema owns. All duplicated
elastic and force-capacity fields have been removed.

### 0.1 A prismatic strut is only as good as its foot

The two mechanisms fail differently, and this is the difference that matters.

A raycast wheel's spring is fed by a **ray**, so it is immune to what the rest of
the vehicle touches: nothing else can intercept the load. A prismatic strut is
fed by **ordinary contacts**, so it competes with every other collider on the leg.
Give the leg a second way to reach the ground and that path takes the load, because
it is rigid and the spring is not — and it latches on the first frame it grazes.

Consequences that are properties of the mechanism, not of any one asset:

- **The foot must be the leg's only ground contact, by a margin no small rotation
  can close.** A raked box strut's bottom corner hangs `half_thickness · sin(rake)`
  below its tip, so a foot centred on that tip clears it by almost nothing. Size
  the foot to hang below the strut, not to straddle it.
- **Half-measures invert.** Deepening a foot that stays centred on the tip makes it
  worse: reaching the ground then demands a larger leg rotation, which brings the
  strut down faster than the foot drops.
- **A prismatic carries moment**, so a leg denied its axial DOF absorbs the landing
  by bending its angular lock instead. The vehicle ends up level, at a plausible
  height, at rest — and the spring reads nothing.

### 0.2 Author the leg as a frame

A strut's clearance is a relationship between two parts, so author it as one. Make
the leg body an `Xform` **frame** — origin at its chassis anchor, local −Y down the
leg — and put the strut mesh and the foot inside it as children, placed by how far
down the leg they sit. "The foot is below the strut's tip" is then a fact a reader
can see and a linter can compute. Placed in vehicle coordinates instead, the same
geometry is written twice in two frames, and the copies drift silently.

Two things this depends on:

- **The body cannot also be the mesh.** A prim that is both carries the shaping
  transform, and every child inherits it. Splitting the geometry into a child frees
  the frame — which is what makes the foot placeable in it at all.
- **Author dimensions, not scale.** `UsdGeomCube` has only a uniform `size`, so a
  real box needs a non-uniform `xformOp:scale` that then belongs to the prim rather
  than the shape. `Cylinder`/`Capsule` carry `radius`/`height` directly. This is not
  only tidiness: a box has corners, and a raked box's corner hangs
  `half_thickness · sin(rake)` below its tip — which is the part that reaches the
  ground first. Re-authoring one lander's struts as cylinders took its legs from
  0.07 m of travel under 170 N to 0.22 m under 900 N, the load they were sized for,
  and settled footpads that had been hunting a 5–24° band indefinitely.

Ownership then follows the hierarchy, stopping at every body boundary in both
directions: a nested body's collider is that body's, never the parent's compound,
and a nested body is attached by a **joint** or it falls off.

The observable contract, and what `landing_legs_test` asserts: struts **compress**
— negative `displacement`, since the axis points chassis→foot — by a comparable
amount on every leg, and the welded pads remain part of the leg body rather than
introducing an unconstrained rotational state.
Height and tilt distinguish nothing: a gear that absorbed a landing and one that
bent under it both sit level at a plausible height.

Sharing is judged on **stroke**, not on the `force` port. The load a spring carries
is `k·x`; the port reports the whole drive law `k·x + c·v`, and with a stiff damper
a few tenths of a m/s of contact jitter swamps the carried load. Stroke is the
state, and a dead strut still reads ~0.

The joint's angular-lock residual — `joint_lock_error_deg` in the rhai prelude,
computed from the two bodies' orientations independently so it cannot share a bug
with the port it checks — is worth **logging** but not asserting: an XPBD joint is
elastic, so the bend tracks load. A bypassed leg bends ~2° carrying nothing and a
healthy one bends ~2° carrying 900 N. The signal is the conjunction, bending while
the spring reads nothing, and stroke already carries that half.

---

## 1. The Omniverse/PhysX Vehicle Schema Specification

In the NVIDIA Omniverse / PhysX 5 Vehicle SDK, a wheel assembly is represented by three core API schemas:
1. **`PhysxVehicleWheelAttachmentAPI`**: Serves as the primary connector and attachment point of the wheel/suspension assembly to the parent chassis.
2. **`PhysxVehicleWheelAPI`**: Defines wheel physical dimensions and dynamics (radius, width, mass, moment of inertia, damping rate).
3. **`PhysxVehicleSuspensionAPI`**: Defines suspension compliance (`springStrength`, `springDamperRate`, `travelDistance`, `sprungMass`). Note: there is **no `restLength`** on this API — PhysX models travel as `travelDistance` + `sprungMass`. LunCo's raycast model needs a rest length, so it is authored as a LunCo extension (`lunco:suspension:restLength`).

### LunCo extension APIs
The concepts the vehicle schemas do not model live in `luncoSchema`
(`crates/lunco-usd/schema/schema.usda`), each as an applied API on its owning
prim:

| API | Property | Applied to | Why it is not PhysX |
| --- | --- | --- | --- |
| `LunCoSuspensionAPI` | `float lunco:suspension:restLength` | suspension prim, beside `PhysxVehicleSuspensionAPI` | PhysX has no `restLength` (`travelDistance` + `sprungMass` instead) |
| `LunCoSuspensionVisualAPI` | `uniform token lunco:suspensionVisual:role` | a strut's moving visual parts | the PhysX vehicle schema is physics-only |
| `LunCoMassContributionAPI` | standard `PhysicsMassAPI` values | a physical child represented as a reduced-realization contribution | standard USD does not declare how a realization folds a child mass into a reduced body |
| `LunCoPrismaticSuspensionAPI` | `float lunco:prismaticSuspension:yieldForce` | a `PhysicsPrismaticJoint` carrying a passive crush cartridge | standard `UsdPhysics` defines bilateral drives, but not one-sided elastic/plastic landing absorption |

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

Our rover assets use the compact composition supported by the standard schemas: the
wheel prim carries `PhysxVehicleWheelAttachmentAPI`, `PhysxVehicleWheelAPI`, and
`PhysxVehicleSuspensionAPI` through the component references. The attachment index
and each causal drive/steer connection are authored on the wheel prim:

```usd
# From ackermann_rover.usda
def Cylinder "Wheel_FL" (
    prepend references = [
        @../../components/mobility/wheel.usda@</Wheel>,
        @../../components/mobility/suspensions/standard.usda@</Suspension>,
    ]
)
{
    int physxVehicleWheelAttachment:index = 0
    float physxVehicleWheel:radius = 0.4
    float inputs:drive.connect = </Rover.outputs:drive_left>
}
```

When wheel and suspension properties are composed onto the same prim, the standard
attachment schema permits the direct self-composition. A separate attachment prim
may instead author the standard `wheel` and `suspension` relationships; neither
case requires a LunCo-specific index or a Rust-side topology guess.

**The APIs are owned at the topology boundary.** `wheel.usda`'s `Wheel` applies `PhysicsRigidBodyAPI` + `LunCoWheelAPI`; each `suspensions/*.usda`'s `Suspension` applies `PhysxVehicleSuspensionAPI` + `LunCoSuspensionAPI`, and each vehicle wheel instance applies `PhysxVehicleWheelAttachmentAPI` + `PhysxVehicleWheelAPI` because it owns the selected wheel, suspension, tire, and attachment index. Motor, gearbox, and shaft equations are authored Modelica components in one containing `CollectionAPI:components` electrical/mechanical network; the wheel consumes only the solved shaft boundary. `apiSchemas` composes across reference arcs, while the vehicle owns the standard attachment contract. A rover authors values and connections, never a private index or fallback rule.

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
2. **Direct self-composition:** If no attachment targets this wheel, the attrs are read directly off the wheel prim. This is LunCo's compact composition, where the wheel references the suspension file directly and the attrs compose onto the wheel prim itself. Both forms are explicit USD topology; the runtime does not select a compatibility path.
3. **Strict validation (§4):** If neither path yields all three params, the resolver returns `None` and the raycast-wheel branch refuses to spawn (no silent defaults).

The same rule applies to standard `Physics*Joint` projection: a prim that
declares a joint but has malformed or unresolved body/frame/drive authoring is
reported as `usd-physics-joint-invalid` and raises the shared physics safety
hold. It is not silently omitted and therefore cannot become an unconstrained
mechanism.

**Attribute names read:** `physxVehicleSuspension:springStrength`, `physxVehicleSuspension:springDamperRate` (NVIDIA canonical), and `lunco:suspension:restLength` (LunCo extension — PhysX has no equivalent). The canonical names are defined in the reconstructed `crates/lunco-usd/schema/core/physxSchema.usda` and pinned by the `physx_vehicle_schemas_register_canonical_properties` drift test.

### 3.2.1. Generic support and activation transaction

A raycast wheel has support geometry but no Avian collider. The mobility producer
publishes the authored probe footprint as `lunco_physics::PhysicsSupportFootprint`;
terrain consumes that shared contract and does not inspect wheel or drivetrain
components. The contract is ordered by the shared `PhysicsSupportSet`: `Publish`
creates the footprint, `Apply` flushes its deferred ECS insertion, and `Consume`
performs support-cache projection and one-time initial placement. This is a runtime
transaction, not a per-frame reseat or an overturn recovery mechanism.

Ordinary rigid bodies use their Avian collider bounds directly. Both paths use the
live spatial support surface for placement: a DEM uses the resident collider ring,
while a flat authored scene uses its static/kinematic colliders and the local
gravity axis. Missing support is consumed as an unsupported physical state; it is
never converted into a permanent activation hold or a guessed world-up placement.

### 3.3. Physics & Visual Updates (`lunco-mobility`)
* **`apply_wheel_suspension`:** Queries `(&mut WheelRaycast, &Suspension, &RayHits, &Transform, &ChildOf)` to solve Hooke's spring-damper equations using the `Suspension` component values. The resulting ground reaction is applied at the authored ray contact point, not at the axle mount, so the reduced realization retains the contact lever arm and load-transfer moment of the physical wheel.
* **`update_suspension_visuals`:** Queries `(&WheelRaycast, &Suspension, ...)` to scale the spring mesh and translate the piston along the suspension travel axis.

### 3.4. Live Tuning & Property Updates (`lunco-scene-commands`)
* The `SetObjectProperty` live mutation system queries the `Suspension` component directly when setting suspension properties (`spring_k`, `damping_c`, `rest_length`).
* This enables live CLI suspension tuning to work uniformly across both **joint-based** and **raycast** vehicles.

### 3.5. Passive prismatic material projection (`lunco-usd-sim` + `lunco-cosim`)

`LunCoPrismaticSuspensionAPI` is projected only when the prim is a
`PhysicsPrismaticJoint` that also applies `PhysicsDriveAPI:linear`. The standard
drive's `stiffness`, `damping`, and `maxForce`, plus the extension's `yieldForce`,
are required, finite, and dimensionally positive; the standard target/type
defaults remain the USD semantic defaults (`0` and `force`). Malformed or
incomplete authoring fails the prim's projection. The extension is the explicit
passive-role marker: the runtime disables the standard bilateral motor and uses
the standard coefficients in its one-sided material law, so there are not two
active owners of the axial DOF.

The native Avian prismatic joint remains responsible for anchors, alignment,
rotation lock, and limits. `lunco-cosim` adds one material impulse at Avian's
existing substep velocity boundary: compression produces a compressive
reaction, the damping is implicit, and yield advances an irreversible unloaded
reference. The impulse is applied through both bodies' generalized inverse mass,
including anchor lever arms and locked axes. No extra global substeps, outer-tick
force accumulator, transform write, or second geometric joint solve is part of
this contract.

---

## 4. Strict Validation: Handling Missing Suspension (No Fallbacks)

Silently inserting default values for missing suspension schemas is forbidden because it hides configuration bugs (e.g. an artist forgetting to add a suspension reference). We enforce a strict, fallback-free validation contract:

### 4.1. Compliant Raycast Wheels (Requires Suspension)
A raycast wheel uses an analytical spring-damper model:

$$F = k \cdot (\text{rest\_length} - \text{distance}) + c \cdot v$$

It **cannot function** without suspension compliance parameters ($k, c, \text{rest\_length}$). 
* If a wheel prim is parsed for raycasting (`PhysxVehicleWheelAPI`) but lacks a resolved `PhysxVehicleSuspensionAPI` (neither on the prim nor referenced via relationships), the loader **fails validation loudly**.
* It logs a compilation error and **refuses to map or spawn the wheel** in the simulation, exposing the asset composition bug immediately.

### 4.2. Zero-travel and physical wheel topology
A wheel with no travel is still authored explicitly — "no suspension" is never
spelled as "omit the schema", which §4.1 rejects. Two authored realizations are
valid:
* **Raycast, zero travel** — reference `components/mobility/suspensions/rigid.usda`, which applies `PhysxVehicleSuspensionAPI` + `LunCoSuspensionAPI` with `restLength = 0` and a stiff, damped spring. The wheel remains on the raycast path.
* **Physical wheel** — author a rigid suspension carrier with `PhysicsRigidBodyAPI`, connect the chassis to that carrier with a `PhysicsPrismaticJoint`, and connect the wheel body to the carrier with a `PhysicsRevoluteJoint`. A commandable carrier may carry `PhysicsDriveAPI:linear`; a passive crush cartridge carries the standard drive plus `LunCoPrismaticSuspensionAPI`. Zero travel is expressed by the prismatic limits, not by bypassing the carrier or attaching the wheel directly to the chassis.

The reusable `physical_drivetrain.usda` is this second form. The rover owns the
carrier placement; the shared drivetrain owns the joint graph; and the reduced
raycast realization folds the carrier's explicitly applied
`LunCoMassContributionAPI` into its resolved rigid-body owner.

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
