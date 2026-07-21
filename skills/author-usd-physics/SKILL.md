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
- The joint's angular lock is bent a degree or two and stays there.

**The bend alone proves nothing** — measure it, but read it with the load. An
XPBD joint is elastic, so the bend tracks FORCE: a bypassed leg bends ~2°
carrying nothing, and a healthy one bends ~2° carrying 900 N. The tell is the
CONJUNCTION — bending while the spring reads nothing. Stroke is what actually
discriminates, so that is what a test should assert.

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

**Suspect the shape before the physics.** The thief here was a strut modelled as
a unit `Cube` under a non-uniform `xformOp:scale`: a box has corners, and a raked
box's corner hangs `half_thickness * sin(rake)` below its tip, which is what
reached the ground. The same strut as a `Cylinder` with a real `radius` has no
corner to dig in, and the gear went from 0.07 m of travel under 170 N to 0.22 m
under 900 N — the load it was designed for — with the footpads settling flat
instead of hunting a 5..24° band forever.

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

This shipped. Four motors per rover, on every rover in the sandbox, gone on the
first physics step — while the rovers still drove, still steered and still made
their authored top speed. Every parity gate stayed green; the bug was found in a
screenshot of hardware lying on the regolith.

**The rule.** Hierarchy is namespace; a **joint** is attachment.

Ownership follows from that, and it stops at every body boundary in BOTH
directions — USD and avian agree:

| what you author | what it becomes |
|---|---|
| collider with **no** body ancestor | standalone STATIC geometry |
| collider under a body | a piece of that body's compound shape |
| collider under a **nested** body | that nested body's piece — never the parent's |
| a nested body | a SEPARATE body; a **joint** attaches it, or it falls off |

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
  `mount_probe.usda` may keep its body.
- Same answer in every robotics dialect: URDF lumps a fixed-jointed link into its
  parent's inertia, MJCF welds a jointless nested body, and neither has a notion
  of a link inside a link attached to nothing. Reflected rotor inertia belongs to
  the joint (`armature`-style), not to a body of its own.

### When two parts that are NOT jointed must not collide

`JointCollisionDisabled` covers the pair a joint names — parent and child, and no
further. Parts **two joints apart** still collide: a hull and the footpad on the
end of its leg, a wheel and the rocker its bogie hangs from. Author them close
enough and the solver spends every step pushing a vehicle apart from itself.

**Say so declaratively — `PhysicsFilteredPairsAPI` is the standard schema for it**,
and it is implemented:

```usda
def Cube "Pad" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI", "PhysicsFilteredPairsAPI"]
)
{
    # Filtering is SYMMETRIC — one opinion is the whole pair. The hull authors
    # nothing.
    rel physics:filteredPairs = </Lander/Hull>
}
```

Two things the rel does not require you to get right, because the loader resolves
them: the target may name a **body** or a **collider under one** (a collider folds
into its body's compound, so both resolve to the body), and either end may carry
the opinion. What it will not do is guess — a target that never spawns is
reported by path, and a pair inside one compound body is reported as inert rather
than quietly accepted.

Timing is load-bearing and handled for you: the pair is armed in
`PhysicsSystems::Prepare`, before the first narrow phase, because avian never
re-filters a pair already in the contact graph. A filter that arrived a tick late
would not apply to the contact it was authored to prevent.

There is a strong temptation to make this automatic — "a vehicle never collides
with itself" — and it should be resisted, because *vehicle* is not a thing the
physics knows and every definition of it breaks:

- a rover parked on a lander's deck is two vehicles or one, depending on the
  minute;
- a robotic arm **should** collide with its own base, or it folds through it;
- filtering the whole joint-graph component silently disables contacts an
  articulated mechanism depends on.

Every engine that solved this made it explicit rather than inferred. MuJoCo
filters a body against its parent (which is what `JointCollisionDisabled` is) and
takes the rest as authored `<exclude>` pairs. URDF/MoveIt *precomputes* a pair
list by sampling poses — a tool that emits authoring, not a runtime heuristic.
PhysX filters adjacent articulation links and takes the rest from filtered pairs.

So: parent-child is automatic, everything beyond it is authored, and the linter's
job is to find the pairs that need authoring — not to guess them.

Proven by `scenes/tests/filtered_pairs.usda` (a pad authored 0.5 m inside a hull
it is two joints away from, reporting no contact) against its control
`filtered_pairs_unfiltered.usda` (same rig, rel removed, contact within a
second). The measurement is the CONTACT itself, off `lunco:sensor:contact` —
not how far something moved afterwards.

### When it is not two parts but twenty — `PhysicsCollisionGroup`

A pair is O(n²). Six wheels that must not touch their own rockers is fifteen
rels, and every part added reopens the file. Groups are the O(n) form of the same
statement:

```usda
def PhysicsCollisionGroup "Wheels"
{
    prepend rel collection:colliders:includes = </Rover/Wheels>
    prepend rel physics:filteredGroups = </Scene/Groups/Chassis>
}
```

Membership is a **`UsdCollectionAPI`** — the schema applies
`CollectionAPI:colliders`, so `collection:colliders:includes` /`:excludes` under
the standard `expandPrims` rule: an include brings its subtree, a deeper exclude
takes part of it back out ("the whole vehicle EXCEPT its wheels" is two lines).
`physics:mergeGroup` makes two group prims one group, so two layers can each
contribute members. `physics:invertFilteredGroups` flips the sense: the listed
groups become the only ones this group collides with — including with respect to
itself, so a group that inverts and does not list itself stops colliding
internally.

Adding a group never changes anything outside it: groups take avian layer bits
from 1 up, never bit 0 (the default every ungrouped body keeps) and never bit 7
(the trigger-zone layer).

Both spellings are held to the same answer by `scenes/tests/collision_groups.usda`
— the same rig as the pair test, referenced, filtered the other way, sharing one
control and one scenario.

## 6b. `purpose` — which geometry is the collision geometry

`UsdGeomImageable.purpose` is how a prim says what its geometry is FOR, and it is
INHERITED, so authoring it once on a scope covers everything inside:

| purpose | drawn | collided |
|---|---|---|
| `default` (nothing authored) | yes | yes |
| `render` | yes | only when the body has no `proxy` |
| `proxy` | no | **yes — this is the collision shape** |
| `guide` | no | never |

So a body that describes itself twice — a detailed mesh to look at, a cheap box
to hit — says so in the standard way, and the physics takes the box. A `guide`
prim (debug axis, sensor cone, planned path) is refused a body and a collider
both, whatever schemas are on it.

This is the tool to reach for when a strut's visual shape and its contact shape
want to be different. It is NOT a substitute for §2b: the *frame* problem (a body
that is its own scaled mesh) is separate, and a proxy inherits the same frame.

### Catch it before the screenshot

```bash
cargo run -q -p lunco-sandbox --bin sandbox -j 2 -- --validate assets/vessels/rovers/skid_rover.usda
```

```
[usd/nested-body-no-joint] /SkidRover/Motor_FL — applies PhysicsRigidBodyAPI
inside the body </SkidRover> but no joint names it — …
```

and on the **loaded** scene (which no file describes once you have spawned into
it), the same rules through the verb:

```rhai
cmd("RunLint", #{}); query("LintReport");
```

The rules are authored in `assets/scripting/policy/lint_usd.rhai` — add one there
rather than in Rust. Two gates hold this: `shipped_assets_lint_clean.rs` (every
shipped asset lint-clean) and `scenes/tests/parts_attached.usda` (nothing
drifts >0.5 m from its vessel over a 12 s drive — the behavioural proof, since a
lint cannot simulate). See
[`validate-assets`](../validate-assets/SKILL.md#the-rules-are-authored--the-lint-layer)
and [`docs/architecture/lint-substrate.md`](../../docs/architecture/lint-substrate.md).

## 6c. What this engine does NOT read

Before authoring a schema because a DCC offers it, check it is consumed. The full
table is in [`docs/architecture/21-domain-usd.md`](../../docs/architecture/21-domain-usd.md)
("standard schema this engine does not read yet"); the ones you are most likely
to reach for:

- **`PhysicsArticulationRootAPI`** — authored on three of our rovers and
  deliberately inert. avian has no reduced-coordinate articulation. Keep it for
  PhysX round-trip; do not expect it to change anything here.
- **`UsdGeomPointInstancer` / `instanceable`** — not read. Every copy is a full
  prim tree.
- **`proxyPrim`** — not read; `purpose` on a sibling covers the case we have
  (see §6b).

If you author something from that table, nothing warns you. That is exactly why
the table exists — and why anything on it either gets implemented or gets deleted
from the assets rather than left looking meaningful.

## Verify it, headlessly

`scene_test` runs one authored scene plus its scenario deterministically, and
its exit code comes from a telemetry verdict:

```
cargo run -q -p lunco-sandbox --bin scene_test -j 2 -- \
    --scene scenes/tests/landing_legs.usda --max-ticks 500
```

A physics change is not done until a scene runs clean: **zero** `left the
world`, **zero** `starts violated`, and the scenario's own verdict PASSing. See
[`author-scenario`](../author-scenario/SKILL.md) for writing the verdict.
