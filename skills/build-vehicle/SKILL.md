---
name: build-vehicle
description: >
  How to BUILD A VEHICLE (rover, hauler, wheeled anything) for LunCoSim out of
  the mobility component library — assembly root, wheels, tires, suspensions,
  chassis, lights, variant axes, drive laws, and live parameter tuning.
  USE THIS SKILL when the user asks to "make/build a rover", "add a vehicle",
  "give it different wheels/tires", "swap the drivetrain", "tune wheel physics",
  "make the drivetrain a Modelica model", or asks why a wheel refuses to spawn.
  For a single reusable part use author-usd-component; for scene assembly use
  build-usd-scene; for GNC use authoring-vessel-controllers.
  Project-specific and non-obvious: wheel params are STRICT (a missing attr
  refuses the wheel and names everything missing), defaults live in
  components/mobility/wheel.usda (never in Rust), the raycast/physical split is
  decided per wheel by an authored PhysicsRevoluteJoint, and live edits flow
  ApplyUsdOp → in-place wheel resync (never a respawn).
---

# Build a vehicle

A vehicle is a **thin assembly**: a `kind = "assembly"` Xform root that
references library components and authors only its own decisions — poses,
indices, scale, paint. Components own their defaults; **variants choose
components, they never restate them**.

Working exemplars, simplest first: `assets/vessels/rovers/skid_rover.usda`
(4-wheel skid), `ackermann_rover.usda` (steering), `six_wheel_rover.usda`
(per-wheel port wiring + `driveLaw` variant), `six_wheel_independent.usda`
(fully authored per-wheel mix), `rocker_bogie.usda` (linkage + gear-joint
differential), `rucheyok/` (Z-forward, Modelica electrical).

## The component library (`assets/components/`)

| Part | File | Owns |
|---|---|---|
| Wheel hub | `mobility/wheel.usda` | dimensions, mass, drive/brake/spin dynamics — THE default set every wheel composes |
| Tire | `mobility/tires/*.usda` | grip (`lunco:tire:frictionCoefficient`, `physxVehicleTire:longitudinalStiffness`) + look (wheel.wgsl inputs: lugs, wear, dust) — chosen via the wheel's `tire` variantSet |
| Suspension | `mobility/suspensions/*.usda` | compliance (`lunco:suspension:restLength`, `physxVehicleSuspension:*`) + strut visuals (`lunco:suspensionVisual:role`) |
| Chassis | `mobility/chassis/box_chassis.usda` | collider + panelised hull material (`rover_hull.wgsl`) |
| Headlight | `lights/headlight.usda` | spotlight + casing + glowing lens, self-contained |
| Drive law | `mobility/drive_laws/modelica_{skid,ackermann,six_independent}.usda` | Modelica motor-lag drivetrain, one per steering family (see below) |
| Drivetrain placement | `mobility/raycast_drivetrain.usda` / `physical_drivetrain.usda` | per-variant wheel positions / joints |

## Minimal rover

```usda
def Xform "MyRover" (
    kind = "assembly"
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysxRigidBodyAPI",
        "PhysicsMassAPI", "PhysxVehicleContextAPI",
        "PhysxVehicleTankDifferentialAPI", "LunCoCatalogAPI"]
)
{
    uniform bool lunco:spawnable = true
    float lunco:spawnLift = 1.0
    float physics:mass = 1000.0
    float3 physics:diagonalInertia = (1028, 1354, 341)   # author it — see skid_rover

    def "Controls" ( prepend references = @lunco://vessels/control_profiles.usda@</RoverControls> ) {}

    def Cube "Chassis" ( prepend references = @lunco://components/mobility/chassis/box_chassis.usda@</Chassis> ) {}

    def Cylinder "Wheel_FL" (
        prepend references = [
            @lunco://components/mobility/wheel.usda@</Wheel>,
            @lunco://components/mobility/suspensions/standard.usda@</Suspension>,
        ]
        variants = { string tire = "regolith" }
    )
    {
        double3 xformOp:translate = (-1.0, -0.15, -1.225)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        int lunco:wheel:index = 0
    }
    # …Wheel_FR/RL/RR: index 1/2/3, mirrored translates…
}
```

- `PhysxVehicleContextAPI` on the root ⇒ FlightSoftware + ports
  (`throttle`/`steer`/`brake` intake; `drive_left`/`drive_right`/`steering`).
- `TankDifferentialAPI` ⇒ skid mixing; `AckermannSteeringAPI` (+ root
  `physxVehicleAckermannSteering:maxSteerAngle`, radians) ⇒ steer, front wheels
  = `lunco:wheel:index < 2`.
- Wheel→port wiring defaults to index parity; override per wheel with
  `string lunco:drivePort` / `lunco:steerPort`; declare extra ports on the root
  with `lunco:drivePorts` + author the mix with `lunco:driveMix`
  (six_wheel_independent shows the full stack).

## Wheel physics: one parameter set, two realizations

Both wheel kinds read the SAME attributes through ONE strict reader
(`lunco-usd-sim/src/wheel_params.rs`). Only force generation differs:

- **raycast** (default): analytical spring + traction force at the hub.
  Requires a composed suspension.
- **physical**: authored `PhysicsRevoluteJoint` targeting the wheel via
  `physics:body1` ⇒ rigid body + velocity motor. That joint IS the switch —
  the `drivetrain` variantSet on the 4-wheel rovers just references the
  component that authors (or omits) the joints.

**Strictness:** every drivetrain/tire attr is required; a wheel missing any
refuses to spawn and the error names ALL missing attrs. You never author them
per vehicle — composing `wheel.usda` + a tire + (for raycast) a suspension is
the complete set. If your wheel refuses to spawn, you dropped one of those
three arcs.

**Tuning:** all wheel params carry schema-level slider hints — select the
rover, Shift+click a wheel to drill to it, edit in 🎚 Parameters. Edits go
`ApplyUsdOp SetAttribute` → document → in-place resync (same entities, joints
untouched). Never poke `WheelRaycast`/`RevoluteJoint` components directly; the
next document change would overwrite you.

## Variant axes (orthogonal, each choosing a component)

- `drivetrain` = **raycast | physical** — how wheels are realized physically.
- `tire` (per wheel) = **regolith | hard | cleated | worn | bald** — grip+look.
- `driveLaw` = **builtin | modelica** — how throttle/steer become drive port
  values. Exists on ALL rovers; ONE law component per steering family, chosen
  by what ports the built-in kernel it displaces writes:
  * `drive_laws/modelica_skid.usda` (skid_rover, six_wheel_rover,
    rocker_bogie): `RoverDrivetrain.mo` integrates a per-side motor lag on
    the solver clock; the rhai bridge writes `drive_left`/`drive_right`.
  * `drive_laws/modelica_ackermann.usda` (ackermann_rover):
    `RoverAckermannDrivetrain.mo`, ONE shared-axle lag + a `steering`
    passthrough — the built-in Ackermann kernel writes three ports, so the
    law covers all three.
  * `drive_laws/modelica_six_independent.usda` (six_wheel_independent): the
    SAME `RoverDrivetrain.mo` (the law is per-side; fan-out is wiring, not
    physics) with a bridge writing `drive_w0..w2` = left, `drive_w3..w5` =
    right.
  In every case `lunco:driveKernel = "external"` stands the built-in mixing
  down, and the whole law is USD + `.mo` + `.rhai` — no Rust. Wheels stay
  port-name-agnostic throughout: each listens to its `lunco:drivePort` (or
  the index-parity default, even ⇒ drive_left / odd ⇒ drive_right); a drive
  law is a VEHICLE-level component that writes those ports by name.
- Planned: `power` (infinite | battery), thermal — same shape.

## Looks

No `displayColor` on parts that bind a shader — the shader owns the look.
Restyle a rover by overriding paint inputs, exactly like the difficulty tiers:

```usda
over "Chassis" { over "HullLook" { over "Shader" {
    color3f inputs:hull_color = (0.30, 0.72, 0.35)
} } }
```

Tire look lives on the tire component (`wheel.wgsl` inputs `tread_lugs`,
`lug_depth`, `wear`, `dust_amount`) — a tire that grips differently should
LOOK different in the same file.

## Verify

Spawn from the palette (folder = category; needs `lunco:spawnable`), possess,
drive (`test-via-api`): throttle ⇒ position delta; steer ⇒ heading change; both
`drivetrain` variants. `QueryEntity` a wheel prim ⇒ canonical attrs resolved.
Watch the log: wheel refusals and resyncs are loud by design.

## Anti-patterns

- ❌ Authoring `physxVehicleEngine:*`/`lunco:wheel:*` values per vehicle —
  tune the component, or the specific wheel that genuinely differs.
- ❌ Restating component defaults in the assembly (radius 0.4, axis "X",
  displayColor) — delete; composition provides them.
- ❌ A variant that inlines prims instead of referencing a component.
- ❌ Editing wheel components in ECS/Rust for "live tuning" — the document is
  the only writer; use the Inspector sliders or `ApplyUsdOp`.
- ❌ Hand-writing a `PhysicsRevoluteJoint` outside a drivetrain component —
  the joint is the raycast/physical discriminator; keep it in the variant arc.
