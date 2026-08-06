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
| Battery | `power/battery.usda` | reusable physical/nameplate/electrical contribution; the rover electrical layer composes it with loads and synthesizes one acausal domain DAE |
| Motor thermal | `thermal/motor_thermal.usda` | rover-agnostic thermal PARTS (`MotorHeatLoad`/`MotorThermalMass`/`MotorRadiator`); each rover authors its own `Scope "Thermal"` with one heat load per driven motor, compiled to its own DAE separate from `Electrical` — chosen via the rover's `thermal` variantSet |
| Chassis | `mobility/chassis/box_chassis.usda` | collider + panelised hull material (`rover_hull.wgsl`) |
| Headlight | `lights/headlight.usda` | spotlight + casing + glowing lens, self-contained |
| Drive law | `mobility/drive_laws/modelica_{skid,ackermann,six_independent}.usda` | Modelica motor-lag drivetrain, one per steering family (see below) |
| Drivetrain realization | `mobility/physical_drivetrain.usda` | the `physical` variant: articulation root + per-wheel revolute joints. The `raycast` variant is EMPTY — a raycast wheel is the absence of a joint. Wheel MOUNTS are never authored here: the wheel prim is the axle in both realizations, so it belongs to the rover, outside the variantSet. |

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
(`crates/lunco-luncosim-edit/src/selection.rs:315`). Plain **Shift+click is the
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

## Presentation-first delivery order

For a Summer Space School request, author the rover, presentation course, and
any new test scene under the Summer Space School Twin (`space-school-twin/` or
`workshop/`), then load it through that Twin's manifest/reload path. Keep
generic components and engine regression scenes in LunCo only when they are
reusable beyond the school; do not make a presentation rover depend on a
one-off engine scene.

Build the smallest **builtin-drive, raycast** rover path before adding Modelica,
power, thermal, autonomy, or a physical drivetrain. For a lunar presentation,
start with the existing `skid_rover` on the shared `lunar_surface` base: it has
the complete command surface and the least moving runtime parts. The first
acceptance gate is `scenes/tests/drivetrain_parity.usda`: its scenario settles,
drives straight, and steers both builtin-drive realizations, then emits the
real verdict `DRIVETRAIN PARITY: PASS|FAIL`. A rover that merely composes, or
two rovers that are both stationary, do not pass.

Only after that gate passes, add one concern at a time:

1. terrain/course and presentation cameras;
2. the vehicle-specific assembly or rocker-bogie morphology;
3. a Modelica drive-law overlay, proved by `scenes/tests/modelica_drive_law.usda`
   (`MODELICA DRIVE LAW: PASS|FAIL` proves the expected transient lag, not merely
   movement);
4. battery, generation, thermal, then autonomy/story behaviour.

Do not combine these stages. A failed rover with a new terrain, Modelica model,
and scenario has too many owners to diagnose; restore the last passing stage
before adding the next one.

### Coordinate contract for mounted mechanisms

For an antenna, camera gimbal, solar head, or other rover-mounted tracker,
declare one coordinate contract before tuning: world axes, rover/mount local
axes, joint positive axes and order, and the physical boresight. Derive the
Modelica setpoint from that contract; never copy a `-Z forward` formula into a
component whose geometry points along another axis. Validate the target vector,
setpoint, measured joint angles, and rendered boresight together after a full
scene reload.

For a fixed photovoltaic deck, there is no tracker controller to tune. Reference
`components/power/solar_panel.usda` once, author its `inputs:area` and placement
on the rover, use the component's +Y normal unless a different face is explicit,
connect its `connectors:p` to the battery, and include both
in the one `Electrical` collection with the driven loads. Keep the panel's
visual frame and cell surface under that same mounted component; do not add a
second rigid body or a disconnected visual proxy. A horizontal deck uses the
component's +Y collecting face. Verify `power_out`, `cos_incidence`, battery
current and `soc_out`, not only that a panel prim appears in the hierarchy.

### Stability before tuning

If a lunar rover tips at launch, inspect the assembled load path before changing
solver settings or adding visual smoothing. High-grip contact forces applied at
the wheel plane create a real pitch moment when the authored
`physics:centerOfMass` is high. The vehicle-level acceptance test should report
travel, maximum tilt, detached descendants, and fixed-step sample count. A
repeatable tilt failure is a geometry/mass/traction defect; a visual jitter with
the body and its labels moving together is a coordinate/transform defect and
needs composed transform inspection.

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
    the solver clock; native USD connections publish `drive_left`/`drive_right`.
  * `drive_laws/modelica_ackermann.usda` (ackermann_rover):
    `RoverAckermannDrivetrain.mo`, ONE shared-axle lag + a `steering`
    passthrough — the built-in Ackermann kernel writes three ports, so the
    law covers all three.
  * `drive_laws/modelica_six_independent.usda` (six_wheel_independent): the
    SAME `RoverDrivetrain.mo` (the law is per-side; fan-out is wiring, not
    physics) with a bridge writing `drive_w0..w2` = left, `drive_w3..w5` =
    right.
  Allocation ownership is derived from the selected allocator's complete
  composed USD output wiring. When every output port the allocator owns has an
  authored producer, that producer (here the Modelica program) owns allocation
  and no imperative `DriveMix` is installed. With no such connections the
  authored built-in allocation applies. A partial set is invalid authoring and
  fails safe; it never falls through to a second controller. The whole law is
  USD + `.mo` + `.rhai` — no sentinel hook or type-specific Rust path. Wheels stay
  port-name-agnostic throughout: each listens to its `lunco:drivePort` (or
  the index-parity default, even ⇒ drive_left / odd ⇒ drive_right); a drive
  law is a VEHICLE-level component that writes those ports by name.
- `power` = **infinite | battery** — does driving cost anything. `infinite`
  is an EMPTY variant (absence of a battery = today's drive-forever default);
  `battery` references reusable battery and motor parts, authors their connector
  topology in the rover file, and lists the actual part paths in a standard
  `CollectionAPI:components` on the `Electrical` Scope. Runtime projects that
  collection as one acausal electrical DAE, with drive commands entering as
  scalar domain-boundary inputs. Brownout
  and current limiting are equations and therefore belong in the projected
  Modelica island. Production Rhai must never scale drive ports per tick.
- `thermal` = **none | basic** — do the motors have temperatures. `none` is
  EMPTY; `basic` authors a `Scope "Thermal"` with its own
  `CollectionAPI:components`, compiled to a SEPARATE generated DAE from
  `Electrical`. Each driven motor gets one `MotorHeatLoad` (from
  `thermal/motor_thermal.usda`); the motor's solved `outputs:heat` crosses into
  the thermal island as a causal `inputs:motor_heat_*` boundary wire (a runtime
  `SimConnection`). The acausal `connectors:port` edges stay inside the thermal
  collection (heat balance per bank). This compiles and publishes
  `motor_temp_left`/`motor_temp_right` (K) REGARDLESS of the `power` variant —
  thermal is decoupled from electrical. See
  `docs/architecture/reviews/2026-07-30-rover-domain-layering.md`. Exemplar:
  rocker_bogie (6 motors), skid_rover (4), six_wheel_rover (6).

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

For iterative modeling, keep one luncosim process running with an explicit
`--api PORT` and use that API. Edit
the USD, then use `OpenFile` for a file-backed asset, `RestartScene` for the
mounted scene, or `ApplyUsdOp` for an in-place authored opinion. Re-run a Rhai
telemetry observer with `RunScenario` and inspect live rover status/ports before
restarting anything.

Object-level USD/reference reload is a planned TODO. Use `RestartScene` while
testing asset edits: it is the supported boundary that reconstructs the prim
tree, Modelica models, and USD connections together. Do not emulate a partial
reload by manually respawning only a visual subtree.

**1. Pre-flight, before launching anything** — composes the whole reference
closure and runs the same strict wheel reader the spawner uses, so a missing
attribute is named in seconds rather than at spawn time
([`validate-assets`](../validate-assets/SKILL.md)):

```bash
target/debug/luncosim --validate assets/vessels/rovers/my_rover.usda
```

**2. Drivetrain parity regression** — the guard that the two realizations stay
matched. `assets/scenes/tests/drivetrain_parity.usda` instantiates
`skid_rover` twice side by side (`drivetrain = "raycast"` at x = −25,
`"physical"` at x = +25) and auto-runs
`assets/scenarios/tests/drivetrain_parity.rhai`: settle 3 s → full throttle straight
12 s → throttle + steer 6 s.

```bash
target/debug/luncosim --api 4101 --scene scenes/tests/drivetrain_parity.usda 2>&1 | tee target/parity.log
grep -E 'DRIVETRAIN PARITY|PARITY FAIL' target/parity.log
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
- ❌ Hand-writing a **wheel** `PhysicsRevoluteJoint` outside a drivetrain
  component — that joint is the raycast/physical discriminator; keep wheel
  hinges in the variant arc. Generic revolute mechanisms (antenna, solar
  tracker, arm) are allowed: their `body1` is not a `PhysxVehicleWheelAPI`, so
  they must not alter drivetrain admission or articulation classification. A
  generic mechanism owns its own hinges and is composed as a root overlay onto
  the vehicle body; do not re-author its hinge in the rover file.
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
