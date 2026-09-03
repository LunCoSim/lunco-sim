---
name: author-usd-physics
description: >
  Author or diagnose LunCoSim USD physics: rigid bodies, colliders, joints,
  joint frames, drives, gravity, collision filtering, and scene teardown. Use
  for exploding vehicles, wrong hinge or slider motion, detached parts, bad
  contact, or scene-owned physics state. The critical contracts are two joint
  frames, explicit body attachment, standard UsdPhysics fields, and teardown
  for every scene-owned write. Use build-vehicle for mobility assembly.
---

# Authoring physics in USD

Physics is authored in USD and projected onto avian. USD is the source of
truth; the ECS is the projection. Use maintained `UsdPhysics` schemas for
topology, limits, and commandable drives. When USD has no material schema for a
physical concept, use the narrow LunCo applied API that owns that concept. Do
not add a LunCo schema when standard USD already owns the fact.

For a physical landing member, apply `PhysicsDriveAPI:linear` to a
`PhysicsPrismaticJoint`. Author the drive's `stiffness`, `damping`, `maxForce`,
and explicit force type; `targetPosition` and `targetVelocity` retain the
standard USD defaults unless the asset needs to state them explicitly. The
native Avian joint is the sole axial mechanism in the existing substep
schedule. Do not duplicate standard fields under `lunco:*`. Missing or invalid
fields fail projection; they are never replaced by a target, force cap, or
solver-resolution workaround.

For asset-level checks, the authored `assembly_audit` tool can inspect explicit
composed joint body relationships, cardinal axes, optional local frames,
rigid-body/joint coverage, and mass/inertia/collider manifests. Use those
reports to diagnose authored topology before changing solver settings. The
tool does not infer a body from a name, turn a raycast wheel into a rigid body,
or replace the standard USD physics owner.

The local avatar is a runtime kinematic camera embodiment, not an authored
rigid body. Its `MoveAndSlide` capsule reuses the standard `UsdPhysics`
colliders projected by the Avian bridge in the active BigSpace frame. Do not
add an avatar collider schema or a second collision representation; see the
[BigSpace physics boundary](../../docs/architecture/45-big-space-correct-usage.md#physics-boundary)
for the frame conversion and Twin-scoped traversal policy.

## 1. A joint is TWO FRAMES, not an axis

This is the single most expensive thing to get wrong, and it fails silently.

A `UsdPhysicsJoint` is defined by a frame on **each** body:

| USD | avian | meaning |
|---|---|---|
| `physics:localPos0` / `localPos1` | `JointFrame::anchor` | where the joint attaches, in each body's local space |
| `physics:localRot0` / `localRot1` | `JointFrame::basis` | how the joint frame is ORIENTED, in each body's local space |
| `physics:axis` | `slider_axis` / `hinge_axis` / `twist_axis` | a CARDINAL axis **of the joint frame** — X, Y or Z is the whole vocabulary |

`physics:axis` can only name a cardinal axis. That is exactly why `localRot`
exists: it is how a mechanism that is not axis-aligned — a landing leg raked 25°
off vertical — says where its axis really points.

**Both halves must cross into the engine.** Every avian joint except the
spherical constrains relative ORIENTATION through `basis1`/`basis2`. An identity
basis therefore demands its body sit square to the other body. Carrying the rake
in the axis alone aims the slider correctly and still wrenches the strut 25° out
of true — the constraint is violated from the first step, the solver resolves it
impulsively through the strut's lever arm, and the vehicle disassembles at
kilometres per second.

**The rule when a body rests at an angle:** if body1 is rotated relative to
body0 in the authored rest pose, that rotation lives in the FRAMES. The joint
holds `rot0 · localRot0 == rot1 · localRot1`, so author whichever side is needed
to make that identity true at rest.

```usda
# LegPX is raked +25° about Z (its own xformOp:rotateXYZ).
def PhysicsPrismaticJoint "LegPX_Spring" (
    prepend apiSchemas = ["PhysicsDriveAPI:linear"]
)
{
    rel physics:body0 = </DescentLander>
    rel physics:body1 = </DescentLander/LegPX>
    uniform token physics:axis = "Y"          # cardinal, IN the joint frame
    quatf physics:localRot0 = (-0.216440, 0, 0, 0.976296)   # 205° about Z: +Y down the strut
    quatf physics:localRot1 = (0, 0, 0, 1)                  # the same 180° flip; the leg body
                                                            # already carries the 25°
}
```

Quaternions in USD are `(w, x, y, z)`. Angles — `physics:lowerLimit`,
`upperLimit`, `coneAngle0Limit`, `coneAngle1Limit` — are **degrees**.

### The diagnostic

The loader measures every joint against its authored frames at build and reports
a violation, seating the body only where the constraint determines it uniquely:

```
[usd-avian] joint /…/PadNX_Weld starts violated by 0.000 m / 0.436 rad — seating
`/…/PadNX` onto the authored joint frame. frame0: localPos0=… localRot0=…,
frame1: localPos1=… localRot1=…, body0 at …, body1 at …
```

`0.436 rad` is 25°. **An angular violation on a raked mechanism is almost always
a missing `physics:localRot1`**: body0's frame was authored off-cardinal and
body1's was left at identity, so the joint demands body1 sit square to body0.

Position is checked for every joint type. Orientation is checked and seated only
where all three rotational DOF are locked (fixed, prismatic); a revolute or
spherical joint leaves rotation free by design, so it is reported and left to
the solver.

## 1b. A rotating assembly is joints plus a controller — never Euler animation

An antenna, solar panel, camera gimbal, arm, or other tracking head has two
separate concerns:

1. **Mechanism:** each moving frame applies `PhysicsRigidBodyAPI` and is joined
   to its immediate predecessor with a standard `PhysicsRevoluteJoint`.
2. **Setpoint law:** a Modelica program computes radians and wires them to the
   joint's native `inputs:angle` port. The cosim joint backend owns the position
   motor only while that port is wired.

**The assembly owns its own hinges.** Compose the assembly's root directly onto
the host body and keep its internal joints and controller connections relative
to that root. The host supplies no duplicate attachment joint; it only chooses
to compose the assembly. Keep the visible mechanism under a unique child path.

This is the direction of dependency: a higher-level rover, lander, tower, or
ground station knows it installs an antenna; the antenna knows only its own
frames, joints, ports, and Modelica controller. The lower-level assembly never
names `Rover`, `Lander`, or any other concrete host.

**Never drive a tracking mechanism by `xformOp:rotateXYZ`.** An authored rest
orientation is allowed where it belongs in the joint frames; continuous yaw or
pitch is a joint state. Euler writes bypass contacts, limits, solver state, and
the measurable `angle` output, then race the physics bridge.

### Coordinate contract for direction-tracking mechanisms

Before authoring or changing a tracker, write down and validate the complete
chain: world direction convention, mount-frame conversion, each joint's positive
axis/order, and the physical boresight of the final geometry. Do not assume the
vehicle's conventional `-Z` forward is the antenna or panel boresight: inspect
the composed asset. Derive the Modelica vector-to-angle equations from that
actual geometry, then verify live that the target vector, controller setpoint,
joint angle, and visible boresight agree. A controller's self-error/"locked"
output alone is not evidence of physical pointing; it can validate the same
wrong convention it commanded.

`PhysicsRevoluteJoint` is a generic mechanism, not a wheel marker. Do not infer
vehicle topology from an antenna, solar tracker, or robotic-arm hinge.

## 2. A prismatic joint CARRIES MOMENT

A `PhysicsPrismaticJoint` locks all three rotational DOF. It is a slider, not a
pin. This has a consequence that is easy to miss and impossible to see in a
screenshot:

Because it carries moment, a sprung leg has **two** ways to absorb a landing:
slide along its axis, which is the one you designed, or bend its angular lock,
which you did not. The second is always available, and the solver will take it
whenever a stray contact makes it cheaper.

### The failure: a second contact steals the load path

A suspension has exactly one intended load path — foot → spring → chassis. Give
the leg **any** second way to reach the ground and that path wins, because it is
rigid and the spring is not. A contact that only *sometimes* touches is worse
than one that always does: it latches on the first frame it grazes and never
lets go.

The diagnostic is a conjunction: `displacement` remains zero, the joint `force`
is absent or near zero under load, and the joint's angular lock is non-zero.
Measure the angular error together with the reaction force; angular error alone
is not enough because the joint solver is elastic.

**Ground clearance is a load-path property, not a styling one.** A raked box
strut's bottom corner hangs `half_thickness * sin(rake)` below its tip, so a
footpad centred on that tip clears it by almost nothing. Millimetres of margin on
a metres-long vehicle is zero margin: a fraction of a degree of tip puts the
strut on the ground. Size the foot so it is the *only* thing that can touch, by a
margin no small rotation can close — and beware that half-measures make it worse,
because a deeper foot demands a larger leg rotation to reach the ground, which
brings the strut down faster than the foot drops.

### Diagnosing it

Measure the angular lock directly from the two body orientations. A prismatic
holds `rot0 · localRot0 == rot1 · localRot1`; the angle between the two authored
joint axes is its constraint error. Then isolate contacts and inspect the
composed collider geometry. A raked box can contact the ground with a corner;
prefer a measured primitive or a deliberately authored proxy when the foot must
be the sole load path. Change solver and friction parameters only after the
geometry and contact ownership are verified.

### Where a joint's rest position sits

Anchors left unauthored are DERIVED from the transform hierarchy, which puts
displacement at exactly 0 in the authored rest pose. A leg authored `-0.8 .. 0.0`
therefore rests **on** its upper limit and travels one way only — by design: 0 is
the fully-extended pose the geometry is drawn in, and the ground can only
compress it.

So a stroke pinned at `0.0000` is not evidence the limits are backwards. Check
the load path first. Widening the limit to make a jammed leg move buries the
actual defect under a range the mechanism never needed.

**Anchors are also why you cannot freely move a body to fix clearance.** The
anchor is derived from the body's origin; move the origin and you move the
joint's zero, silently preloading the spring by the axial component of the shift.
Change the part's *extent*, or the mating part, not the sprung body's origin.

## 2b. Author the body as a FRAME, with real dimensions

### Make the body a frame, not a mesh

A prim that is both the rigid body and the geometry cannot host children, because
its shaping transform applies to them too. Give the body its own frame and put the
geometry inside it:

```usda
def Xform "LegPX" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsMassAPI"])
{
    # origin at the hull anchor, local -Y down the leg
    double3 xformOp:translate = (2.519, 1.388, 0)
    double3 xformOp:rotateXYZ = (0, 0, 25.0)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]

    def Cylinder "Strut" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    {
        uniform token axis = "Y"
        double radius = 0.075
        double height = 7.05                       # spans local y 0 .. -7.05
        double3 xformOp:translate = (0, -3.525, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
    def Cylinder "PadPX" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"])
    {
        double3 xformOp:translate = (0, -7.2, 0)   # 0.15 below the strut's tip
        ...                                        # + a joint, below
    }
}
```

Now every part of the leg is placed by how far down the leg it sits, in ONE frame,
next to its neighbours. "The foot is below the strut's tip" is something a reader
can see and a linter can compute. Place them in world coordinates instead and you
have written the same geometry twice, in two frames — and the copies drift.

### Author DIMENSIONS, not scale

`UsdGeomCube` has only a uniform `size`, so any real box has to be faked with a
non-uniform `xformOp:scale`. That scale then belongs to the prim rather than to
the shape, and everything downstream has to remember it: the frame is unusable for
children, the collider is a scaled shape rather than a measured one, and the part's
true dimensions appear nowhere in the file.

`Cylinder`, `Capsule`, `Cone` and `Sphere` carry `radius` / `height` — the
dimensions themselves, in metres. Prefer them, and prefer a `Mesh` with authored
`extent` to a scaled primitive. A strut is a cylinder; modelling it as a squashed
cube buys nothing and costs the frame.

## 3. Gravity is authored per scene

`UsdPhysicsScene` — the standard prim, `physics:gravityMagnitude` (scene units
per second squared) and `physics:gravityDirection` (a vector in the stage's
frame). Both convert at the boundary like every other authored quantity.

```usda
def PhysicsScene "PhysicsScene"
{
    vector3f physics:gravityDirection = (0, -1, 0)
    float physics:gravityMagnitude = 1.62
}
```

- **This is a lunar simulator.** Scenes are 1.62 unless there is a stated reason
  otherwise. The vehicles' drivetrains, struts and propellant budgets are sized
  for it.
- **ONE per scene.** Two prims that disagree are an authoring error and are
  reported as one; the last read wins, which depends on prim order.
- USD's sentinels are honoured: a NEGATIVE magnitude means "earth gravity", a
  ZERO direction means "the stage's down axis".
- **An orbital scene authors NO `PhysicsScene`.** Gravity there is per-body and
  position-dependent (`Gravity::Surface` + the celestial point-mass model). A
  flat vector would override that and pin every spacecraft to a fictitious
  "down". `assets/scenes/celestial/artemis_2_review.usda` is the worked example.

**Traction is gravity-dependent, and test thresholds must be too.** At 1.62 a
rover is traction-limited well below its drivetrain's `omega_max * r` ceiling
(measured 2.2–2.4 m/s against 4.8). A kinematic ceiling holds at any gravity — a
driven wheel cannot out-run its no-slip speed — but any floor derived from Earth
traction is simply wrong on the Moon.

## 4. A scene owns more than its entities

Anything a scene load writes belongs to that scene. Unloading despawns the
entities; **`SceneTeardown` unloads everything else.** Register teardown for every
scene-owned resource or override so a new scene starts from its own contract.

Add a reset system beside the code that writes the state:

```rust
app.add_systems(
    lunco_core::SceneTeardown,
    |mut commands: Commands| commands.remove_resource::<MySceneCache>(),
);
```

Which disposition is right depends on who owns the value:

- **REMOVE** state that only means something while a scene is loaded — caches,
  provenance records. Absence is its correct empty state.
- **RESTORE** state the app installs at start-up and a scene merely overrides.
  Gravity is the type case: a scene SHOULD override it, and must not leave the
  override behind. Removing it would leave the world with no value at all.

`SceneTeardown` grep-lists everything a reload restores. If you add
scene-derived state and do not register it, you have added a leak.

The shared physics owner clears `PhysicsHolds` and `PhysicsStepRequest` and
restores a zero-delta `Time<Physics>` at this boundary. The time owner clears
fixed overstep during reset, and the controller/core owners clear scene-keyed
input and control-path state. Do not add an asset-local pause flag or carry
step debt into the replacement scene; readiness and deliberate stepping must be
re-authored by the incoming scene.

## 5. Reading the failure modes

| Symptom | Look at |
|---|---|
| `origin.is_finite()` panic in `obvhs` | a body reached ±inf; a raycast was issued from it. The *cause* is upstream — find the first `body left the world` |
| `[physics] body left the world: …` | first escapee names the mechanism that diverged. Bodies at the end of a lever arm (pads, wheels) escape first |
| `joint … starts violated by … rad` | a joint frame; see §1 |
| stroke reads exactly `0.0000` in every regime | a second contact carrying the load (§2). Measure the joint's angular-lock error before touching its limits |
| a spring loads the "wrong way" | almost never the joint. A jammed DOF and a reversed one look identical from the port; §2 tells them apart |
| a scene retains another scene's setting | a resource or override missing from `SceneTeardown` (§4) |
| a part is lying on the ground behind the vehicle | it declared its own body and no joint holds it (§6). `--validate` the asset, or `cmd("RunLint", #{})` the scene |

## 6. A part is not a body

`PhysicsRigidBodyAPI` declares a **body**, and the loader honours it wherever it
appears — ancestry is never consulted, because nesting-plus-joint is exactly how
a wheel is mounted (`Wheel_FL` under the chassis + a `PhysicsRevoluteJoint`).
So a prim that applies it and is jointed to nothing is a **free body inside your
vehicle**, and it leaves:

```usda
def Xform "Rover" (prepend apiSchemas = ["PhysicsRigidBodyAPI"]) {
    def Xform "Motor_FL" (prepend apiSchemas = ["PhysicsRigidBodyAPI"]) { … }   # ❌ falls out
    def Xform "Motor_FL" (prepend apiSchemas = ["PhysicsMassAPI"])      { … }   # ✅ part of the rover
}
```

**The rule.** Hierarchy is namespace; a **joint** is attachment.

Ownership follows from that, and it stops at every body boundary in BOTH
directions — USD and avian agree:

| what you author | what it becomes |
|---|---|
| collider with **no** body ancestor | standalone STATIC geometry |
| collider under a body | a piece of that body's compound shape |
| collider under a **nested** body | that nested body's piece — never the parent's |
| a nested body | a SEPARATE body; a **joint** attaches it, or it falls off |

`physics:collisionEnabled=true` is an opt-in on an already-declared collider;
it does not apply `PhysicsCollisionAPI` by itself. Ordinary vehicle geometry
must carry both. `LunCoTerrainAPI` and `PhysxVehicleWheelAPI` are the explicit
owner-level exceptions because their terrain and vehicle projectors construct
those shapes through their own contracts. `collision-enabled-without-api` is
an error, not a runtime fallback.

Every renderable gprim under a composed `kind = "assembly"` vehicle must also
state who owns its collision contract. A supported enabled
`PhysicsCollisionAPI` shape is the ordinary owner; `PhysxVehicleWheelAPI` is the
wheel-projector owner. Intentional decoration must explicitly author
`physics:collisionEnabled = false` or an inherited `purpose = "guide"`. When a
body has a `purpose = "proxy"` shape, its `purpose = "render"` geometry is the
visual description and is excluded from collision. Leaving visible vehicle
geometry unannotated is an error reported by `vehicle-part-collision-contract`,
not an invitation for a runtime fallback.

Hierarchy is namespace. **The joint is what attaches** — nesting a body without
one is the motor bug (`nested-body-no-joint`), and nesting one *with* a joint is
how a foot mounts on a leg and a wheel on a chassis. Both directions of that rule
matter: fold a nested body's collider into its parent's compound and one shape has
two owners, the compound holding it rigidly while its joint pulls it. They fight
every step until a body leaves the world.


- An internal part (motor, gearbox, battery, panel, lamp) = mass + geometry, **no
  body**. Its colliders fold into the host body's compound, its mass belongs to
  the host.
- A part that must move relative to its host = a body **and** a joint, authored
  together. That is what a mount (`AttachSpec`) writes, and it is why
  `mounting/demo_probe.usda` may keep its body.
- Same answer in every robotics dialect: URDF lumps a fixed-jointed link into its
  parent's inertia, MJCF welds a jointless nested body, and neither has a notion
  of a link inside a link attached to nothing. The complete rotational assembly
  inertia belongs on the moving body's authored inertia contract; a domain
  component that produces torque must not create a second hidden body or
  duplicate shaft state.

### Collision filtering

`JointCollisionDisabled` covers only the bodies named by a joint. For additional
contacts, author `PhysicsFilteredPairsAPI` explicitly; for many-to-many sets,
use `PhysicsCollisionGroup` with `UsdCollectionAPI`. Do not infer a vehicle-wide
self-exclusion: articulated mechanisms often need selected internal contacts.

Filtering is symmetric, targets may name a body or a collider under that body,
and invalid targets or same-compound pairs must remain visible diagnostics. The
filter is installed during `PhysicsSystems::Prepare`, before contact generation.
Verify contact itself in a scene test such as `scenes/tests/filtered_pairs.usda`
or `collision_groups.usda`, not only by checking subsequent motion.

## 6b. `purpose` — which geometry is the collision geometry

`UsdGeomImageable.purpose` is inherited and separates display geometry from
collision geometry:

| purpose | drawn | collided |
|---|---|---|
| `default` (nothing authored) | yes | yes |
| `render` | yes | only when the body has no `proxy` |
| `proxy` | no | **yes — this is the collision shape** |
| `guide` | no | never |

Use `proxy` for an authored collision shape alongside `render` geometry. A
`guide` prim is never a body or collider. `purpose` does not replace the frame
contract in §2b; proxy geometry still inherits its authored frame.

### Validate and run the scene

```bash
target/debug/luncosim --validate assets/vessels/rovers/skid_rover.usda
```

```
[usd/nested-body-no-joint] /SkidRover/Motor_FL — applies PhysicsRigidBodyAPI
inside the body </SkidRover> but no joint names it — …
```

and on the **loaded** scene, run the same rules through the verb:

```rhai
cmd("RunLint", #{}); query("LintReport");
```

The rules are authored in `assets/scripting/policy/lint_usd.rhai` — add one there
rather than in Rust. Pair the lint with a scene test such as
`scenes/tests/parts_attached.usda`, because lint cannot simulate motion. See
[`validate-assets`](../validate-assets/SKILL.md#the-rules-are-authored--the-lint-layer)
and [`docs/architecture/lint-substrate.md`](../../docs/architecture/lint-substrate.md).

### 6b.1 Vehicle-level stability acceptance

Mounted-part linting catches free bodies, but it cannot prove a rover will stay
upright under drive. Add a real flat-ground or production-terrain scene test
that settles the assembled vehicle, drives through its authored control path,
and reports:

- travel and enough fixed-step samples;
- maximum body tilt;
- missing or detached descendants and their worst relative displacement; and
- any domain output that is part of the vehicle acceptance, such as solar power
  and battery state.

On lunar regolith, inspect the force application height and the authored
`physics:centerOfMass` before changing solver parameters. If all driven wheels
reach the friction limit and the centre of mass is high above the contact plane,
the launch pitch moment is physical. Lower or correctly assemble the mass
distribution and rerun the test; do not turn a repeatable tip into a larger tilt
allowance or a rendering filter.

## 6c. What this engine does NOT read

Before authoring a schema because a DCC offers it, check that the importer
consumes it. The full table is in
[`docs/architecture/21-domain-usd.md`](../../docs/architecture/21-domain-usd.md).
Commonly mistaken fields include:

- **`PhysicsArticulationRootAPI`** — avian has no reduced-coordinate articulation;
  do not expect it to change this runtime.
- **`UsdGeomPointInstancer` / `instanceable`** — not read. Every copy is a full
  prim tree.
- **`proxyPrim`** — not read; `purpose` on a sibling covers the case we have
  (see §6b).

An unconsumed field has no runtime effect. Use a supported schema or implement
the owner and its projection before authoring the field.

## Verify it, headlessly

`luncosim test` runs one authored scene plus its scenario deterministically, and
its exit code comes from a telemetry verdict:

```
target/debug/luncosim test \
    --scene scenes/tests/landing_legs.usda --max-ticks 500
```

A physics change is not done until a scene runs clean: **zero** `left the
world`, **zero** `starts violated`, and the scenario's own verdict PASSing. See
[`author-scenario`](../author-scenario/SKILL.md) for writing the verdict.
