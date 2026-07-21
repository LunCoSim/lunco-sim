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
| Suspension | `mobility/suspensions/*.usda` | compliance (`lunco:suspension:restLength`, `physxVehicleSuspension:*`) + strut visuals — ALL suspensions carry them: standard/rocker have the animated Casing/Piston/Spring trio (`lunco:suspensionVisual:role`), rigid a static casing only (zero travel ⇒ no roles) |
| Battery | `power/rover_battery.usda` | traction energy budget (`RoverBattery.mo` + enforcement bridge) — chosen via the rover's `power` variantSet. Distinct from `power/battery.usda`, the physical pack |
| Motor thermal | `thermal/motor_thermal.usda` | per-side motor heat balance (`RoverMotorThermal.mo`), telemetry-only — chosen via the rover's `thermal` variantSet |
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

- `PhysxVehicleContextAPI` on the root ⇒ ActuatorPorts + ports
  (`throttle`/`steer`/`brake` intake; `drive_left`/`drive_right`/`steering`).
- `TankDifferentialAPI` ⇒ skid mixing; `AckermannSteeringAPI` (+ root
  `physxVehicleAckermannSteering:maxSteerAngle`, radians) ⇒ steer, front wheels
  = `lunco:wheel:index < 2`.
- Wheel→port wiring is a USD connection: each wheel connects
  `float inputs:drive.connect` to a `float outputs:<port>` declared on the root,
  and the mix onto those ports is authored as a `DriveMix` child scope — one
  prim per sink port with a `double lunco:factor:<source>` per command source
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

**One no-load speed, so both realizations top out together.**
`physxVehicleEngine:maxRotationSpeed` is THE axle free-spin speed and both kinds
obey it: the physical wheel's velocity motor targets it (`MotorActuator.max_omega`),
and the raycast wheel's drive force now carries a **torque–speed rolloff**
(`drive_force_mag`, `crates/lunco-mobility/src/lib.rs:261`):

```
F = throttle · N · driveForcePerNormal · clamp(1 − ω/ω_max, 0, 1)
ω = (forward_speed / radius) · sign(throttle)
```

so its force falls to zero at the same `ω_max · r`. Top speed is therefore
`ω_max · radius` for either drivetrain: at the authored 12 rad/s and r = 0.4 m,
≈ 4.8 m/s.

Two details that matter if you re-derive it: `ω` is the **hub's ground speed
converted to an equivalent axle rate and signed by the throttle**, not the
wheel's measured spin; and the factor is `clamp(…, 0, 1)`, not `max(0, …)` — the
upper clamp is what stops a reversing wheel receiving *more* than stall force.

There is NO `lunco:wheel:maxDriveOmega`. It used to be a second name for this
same quantity, read only by the physical path, and the two were authored 60 vs
12 — which is why raycast rovers drove ~5× too fast. It is deleted, with no
alias and no shim. Change the top speed in ONE place.

The rolloff is signed: it only bites when the throttle pushes the way the wheel
is already rolling, so braking and reversing keep full force authority.

**Strictness:** every drivetrain/tire attr is required; a wheel missing any
refuses to spawn and the error names ALL missing attrs. That now includes
`physxVehicleWheel:dampingRate` (bearing + rolling drag): it is a physical
property of the hub, so it is authored, never inferred from the drive torque —
the old `peakTorque / maxRotationSpeed` fallback is gone. The ONE number still
not authored per wheel is `physxVehicleWheel:moi`, and only because 0 means
"solid cylinder" and it is DERIVED as ½·m·r² from the authored mass and radius.
You never author them
per vehicle — composing `wheel.usda` + a tire + (for raycast) a suspension is
the complete set. If your wheel refuses to spawn, you dropped one of those
three arcs.

**Tuning:** all wheel params carry schema-level slider hints, so every wheel gets
Inspector sliders with zero per-asset authoring (`SchemaRegistry::ui_hint` →
`produce_usd_param_view`; a per-asset authored `customData` still overrides —
see [`author-usd-component`](../author-usd-component/SKILL.md#adding-a-new-lunco-property--source--regenerate)).

To reach one wheel: select the rover, then **Alt+Shift+click** the wheel — that
drills the Inspector to that subpart's own PRIM
(`crates/lunco-sandbox-edit/src/selection.rs:315`). Plain **Shift+click is the
multi-select toggle** and explicitly *clears* the drill target; it does not drill.
The drill also requires the rover to already be the primary selection.

Edits go `ApplyUsdOp SetAttribute` → document → **in-place resync, never a
respawn**: `wheel_params::claims_edit` recognises the attribute (any
`lunco:wheel:` / `lunco:suspension:` / `lunco:tire:` / `physxVehicle*:` prefix,
plus `lunco:driveKernel`, `lunco:factor:*` on a `DriveMix` term prim, and
`physics:mass` on a wheel prim)
and `resync_wheels_for_stage` updates the live components — same entities, joints
untouched. Never poke `WheelRaycast`/`RevoluteJoint` components directly; the
next document change would overwrite you.

## Variant axes (orthogonal, each choosing a component)

Axes are **opt-in per vehicle** — a rover only has the axes its file declares.
What is actually authored today:

| Rover | `drivetrain` | `driveLaw` | `power` | `thermal` |
|---|---|---|---|---|
| `skid_rover` | ✅ | ✅ | ✅ | — |
| `ackermann_rover` | ✅ | ✅ | — | — |
| `six_wheel_rover` | — | ✅ | ✅ | ✅ |
| `six_wheel_independent` | — | ✅ | — | — |
| `rocker_bogie` | — | ✅ | — | — |

(`tire` is per-wheel, not per-vehicle — it is declared once on
`components/mobility/wheel.usda` and every composed wheel has it.
`differential_rig.usda` and `rucheyok/` are not driveable vehicles and have no
axes.) Adding a missing axis to a rover is a few lines of `variantSet` copied
from an exemplar — that is the intended way to extend, not a Rust change.

- `drivetrain` = **raycast | physical** — how wheels are realized physically.
  Authored on `skid_rover` and `ackermann_rover`.
  Switching it changes fidelity and cost, NOT how fast the rover goes: both
  realizations self-limit at `physxVehicleEngine:maxRotationSpeed · radius`
  (see *Wheel physics* above).
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
  down: it is read first in `derive_drive_mix`
  (`crates/lunco-usd-sim/src/lib.rs`), pre-empting the `DriveMix` scope,
  `TankDifferentialAPI` and `AckermannSteeringAPI`. Note `"external"` is **not a
  Rust sentinel** — it is simply a hook name nothing registers, so the mixer
  finds no hook, writes no ports (fail-safe coast) and warns once. Any
  unregistered name behaves identically; `"external"` is the agreed spelling.
  The whole law is USD + `.mo` + `.rhai` — no Rust. Wheels stay
  port-name-agnostic throughout: each listens to its `lunco:drivePort` (or
  the index-parity default, even ⇒ drive_left / odd ⇒ drive_right); a drive
  law is a VEHICLE-level component that writes those ports by name.
- `power` = **infinite | battery** — does driving cost anything. `infinite`
  is an EMPTY variant (absence of a battery = today's drive-forever default);
  `battery` references `components/power/rover_battery.usda` onto the root:
  `RoverBattery.mo` integrates state-of-charge on the solver clock with a
  realistic consumption shape — an avionics floor (`idle_w = 30 W`) plus
  per-side motor draw proportional to the commanded drive magnitude
  (`motor_w = 250 W` at full command per side, `capacity_wh = 2000`). Its
  `alive` output is a smooth 0..1 cutoff (1.0 until the last ~2% of charge);
  the `rover_battery.rhai` bridge scales `drive_left`/`drive_right` by it —
  a DELIBERATE last-writer clamp that never touches the ports while
  `alive >= 0.999`, so it cannot race the drive kernel/law in the common
  case, and brownouts rather than steps the rover dead at the end.
  Exemplars: skid_rover, six_wheel_rover — other rovers adopt identically
  (same variantSet + reference; works under both `driveLaw` variants since
  the bridge reads the port value, not a specific writer).
- `thermal` = **none | basic** — do the motors have temperatures. `none` is
  EMPTY; `basic` references `components/thermal/motor_thermal.usda`:
  `RoverMotorThermal.mo` per-side first-order heat balance (dissipation
  follows command magnitude, losses follow excess over a 250 K lunar-day
  ambient), publishing `temp_left`/`temp_right` (K) as PURE TELEMETRY ports —
  no bridge, nothing acts on them. Exemplar: six_wheel_rover; same
  choose-a-component shape.

## Looks

**Colour is `primvars:displayColor`, always — the shader CONSUMES it.** One
authored attribute, in the standard USD place, whether the part renders through
plain PBR or through a shader. `rover_hull.wgsl` declares
`//!@engine display_color` and the engine fills it from the prim's composed
`primvars:displayColor` (element 0 — it is a `color3f[]` ARRAY by schema).
Restyle a rover, or a difficulty tier, by overriding that one attribute:

```usda
over "Chassis" { color3f[] primvars:displayColor = [(0.30, 0.72, 0.35)] }
```

Shader `inputs:` are for what displayColor cannot say — `accent_color`,
`panel_scale`, `wear`, `dust_amount`. Authoring `inputs:display_color`
explicitly still wins over the engine fill, but you rarely want that; it hides
the colour from every other tool that reads USD.

Tire look lives on the tire component (`wheel.wgsl` inputs `tread_lugs`,
`lug_depth`, `wear`, `dust_amount`) — a tire that grips differently should
LOOK different in the same file. Tires author their colours as shader `inputs:`
deliberately; that is unchanged.

## Verify

**1. Pre-flight, before launching anything** — composes the whole reference
closure and runs the same strict wheel reader the spawner uses, so a missing
attribute is named in seconds rather than at spawn time
([`validate-assets`](../validate-assets/SKILL.md)):

```bash
cargo run -p lunco-sandbox --bin sandbox -- --validate assets/vessels/rovers/my_rover.usda
```

**2. Drivetrain parity regression** — the guard that the two realizations stay
matched. `assets/scenes/tests/drivetrain_parity.usda` instantiates
`skid_rover` twice side by side (`drivetrain = "raycast"` at x = −25,
`"physical"` at x = +25) and auto-runs
`assets/scenarios/tests/drivetrain_parity.rhai`: settle 3 s → full throttle straight
12 s → throttle + steer 6 s.

```bash
cargo run -j2 --bin sandbox -- --scene scenes/tests/drivetrain_parity.usda 2>&1 | tee /tmp/parity.log
grep -E 'DRIVETRAIN PARITY|PARITY FAIL' /tmp/parity.log
```

It asserts terminal speed ±15 %, peak speed ±15 %, distance ±20 %, yaw magnitude
±35 % with a strict sign check, and that **both** land in `[2.4, 6.0] m/s` — the
absolute band around the authored `ω_max · r ≈ 4.8`. Both-near-zero is a FAIL,
not a pass. **It emits no exit code** — the verdict is the last stdout line
`DRIVETRAIN PARITY: PASS|FAIL`, so grep for it; a green-looking run that never
printed the line means the scenario never reached its verdict.

Run this after ANY change to wheel params, the rolloff, the motor actuator, or
`wheel.usda` defaults — it is the only thing that catches the two realizations
drifting apart.

**3. Interactive** — spawn from the palette (folder = category; needs
`lunco:spawnable` on the `defaultPrim`, see
[`use-asset-library`](../use-asset-library/SKILL.md)), possess, drive
([`test-via-api`](../test-via-api/SKILL.md)): throttle ⇒ position delta; steer ⇒
heading change; both `drivetrain` variants. `QueryEntity` a wheel prim ⇒
canonical attrs resolved. Watch the log: wheel refusals and resyncs are loud by
design.

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
- ❌ Expecting plain Shift+click to drill into a wheel — it is the multi-select
  toggle and clears the drill target. Alt+Shift+click drills.
- ❌ Adding a second name for a quantity that already exists (the
  `maxDriveOmega` mistake) — one attribute, one reader, one place to change it.
- ❌ Overriding a shader `inputs:` to repaint a rover — author
  `primvars:displayColor`; `rover_hull.wgsl` consumes it via `//!@engine
  display_color` ([`use-asset-library`](../use-asset-library/SKILL.md#add-a-shader-wgsl)).
- ❌ Changing wheel physics without re-running the drivetrain parity scene —
  the two realizations drift silently otherwise.
- ❌ Assuming every rover has every variant axis — check the table above; most
  have only `driveLaw`.
