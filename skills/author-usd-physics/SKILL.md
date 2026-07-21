---
name: author-usd-physics
description: >
  How physics is AUTHORED in USD and what the engine does with it — joints,
  joint frames, gravity, and scene teardown. USE THIS SKILL whenever the user
  asks, in plain words, things like: "the lander explodes / flies apart / spins
  off", "my suspension doesn't compress", "the wheel hinges the wrong way",
  "parts of the vehicle shoot off at launch", "the rover sinks through the
  ground", "gravity is wrong in this scene", "the leg is welded at an angle",
  "why is my spring doing nothing", or "the scene keeps the previous scene's
  settings". Also for the agent mid-code: a `PhysicsRevoluteJoint` /
  `PhysicsPrismaticJoint` / `PhysicsFixedJoint` / `PhysicsSphericalJoint` prim,
  `physics:localRot0` / `localPos0` / `physics:axis`, a `UsdPhysicsScene`,
  `physics:gravityMagnitude`, `starts violated by … rad`, `body left the world`,
  an `origin.is_finite()` panic out of `obvhs`, or `SceneTeardown`. These rules
  are project-specific and non-obvious: a joint is TWO FRAMES not an axis, a
  prismatic joint CARRIES MOMENT, and anything a scene writes must be undone on
  unload.
---

# Authoring physics in USD

Physics is authored in USD and projected onto avian. USD is the source of
truth; the ECS is the projection. Nothing below is a LunCo invention — it is
UsdPhysics, mapped one-to-one onto avian's joint model.

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

The signature is unmistakable once you know it:

- `displacement` reads `0.0000` in **every** regime — free fall, impact, rest.
  Touchdown changes nothing.
- `force` reads near zero while the vehicle is demonstrably standing on the leg.
- The vehicle looks perfect: level, at a believable height, at rest.
- The joint's **angular lock is bent by a degree or two** and stays there.

That last one is the tell, and nothing else in a scene reports it. The gear
settled by *bending* rather than *stroking*, so the load never reached the axial
DOF the spring lives on.

**Ground clearance is a load-path property, not a styling one.** A raked box
strut's bottom corner hangs `half_thickness * sin(rake)` below its tip, so a
footpad centred on that tip clears it by almost nothing. Millimetres of margin on
a metres-long vehicle is zero margin: a fraction of a degree of tip puts the
strut on the ground. Size the foot so it is the *only* thing that can touch, by a
margin no small rotation can close — and beware that half-measures make it worse,
because a deeper foot demands a larger leg rotation to reach the ground, which
brings the strut down faster than the foot drops.

### Diagnosing it

Measure the angular lock directly. A prismatic holds `rot0 · localRot0 == rot1 ·
localRot1`, and both sides are readable from the bodies' world orientations, so
the angle between them **is** the constraint's error:

```rhai
// per leg: the joint's free axis, computed from each body independently
let from_chassis = qrot(world_rotation(hull), localRot0_times_Y);
let from_strut   = qrot(world_rotation(leg),  localRot1_times_Y);
angle_deg(from_chassis, from_strut)   // ~0 healthy; >1 means it is bending
```

Then bisect the contacts. Disable one collider at a time and re-run — the one
whose removal restores the stroke is the thief. Do **not** start from the joint:
the joint is usually innocent, and its authoring is where the time goes.

Two hypotheses that look compelling here and are worth ruling out by measurement
before you spend a day on either: solver conditioning (change a body's mass by
20× — if nothing moves, it is not conditioning) and friction (drop μ by 2× — same
test). A steady error that is invariant to both is *geometric*, and geometry means
contact.

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
entities; **`SceneTeardown` unloads everything else.** Without it, loading scene
A then scene B leaves B running with a value A chose — nothing errors, the scene
just behaves as though it were still the previous one.

Add a reset system beside the code that writes the state:

```rust
app.add_systems(
    lunco_usd_bevy::scene_lifecycle::SceneTeardown,
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

## 5. Reading the failure modes

| Symptom | Look at |
|---|---|
| `origin.is_finite()` panic in `obvhs` | a body reached ±inf; a raycast was issued from it. The *cause* is upstream — find the first `body left the world` |
| `[physics] body left the world: …` | first escapee names the mechanism that diverged. Bodies at the end of a lever arm (pads, wheels) escape first |
| `joint … starts violated by … rad` | a joint frame; see §1 |
| stroke reads exactly `0.0000` in every regime | a second contact carrying the load (§2). Measure the joint's angular-lock error before touching its limits |
| a spring loads the "wrong way" | almost never the joint. A jammed DOF and a reversed one look identical from the port; §2 tells them apart |
| a scene behaves like the previous one | a resource that outlived its scene (§4) |

## Verify it, headlessly

`scene_test` runs one authored scene plus its scenario deterministically, and
its exit code comes from a telemetry verdict:

```
cargo run -q -p lunco-sandbox --bin scene_test -j 2 -- \
    --scene scenes/sandbox/landing_legs_test.usda --max-ticks 500
```

A physics change is not done until a scene runs clean: **zero** `left the
world`, **zero** `starts violated`, and the scenario's own verdict PASSing. See
[`author-scenario`](../author-scenario/SKILL.md) for writing the verdict.
