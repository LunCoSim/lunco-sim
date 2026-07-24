# 52 — Connectivity: the gap audit and what closed it

Status: **as-built**. Companion to [49 — the generic link kernel](49-connectivity-link-kernel.md),
which carries the resulting design. This doc is the record of what was wrong, and why each
fix is shaped the way it is.

Driven by the summer space school, which needs a demonstrable, visible, student-drivable
connectivity feature. The audit started from one question — *"can we put a ground station,
a rover and a wall between them in a scene and watch the link drop?"* — and the answer was
**no**: the wall would not have blocked anything.

> **Read this for the reasoning, not the API.** Where this doc and doc 49 / the code
> disagree, doc 49 and the code are current.

The constraints from doc 49 held throughout: **no comms crate, no comms component, no
`lunco:comms:*` vocabulary, and `update_links` stays a regular non-exclusive system.**
Every fix below is checked against those.

---

## 1. The gaps

| # | Gap | Status |
|---|---|---|
| G1 | LOS ignored all scene geometry — a wall did not block a link | **closed** — `LinkOccluder` |
| G2 | Nothing rendered link state at all | **closed** — `link_viz.rs` |
| G3 | Three files advertised `comms:*` ports that no code published | **closed** — claims deleted |
| G4 | `comms-degradation` faked radio shadow by distance to a POI | **closed** — reads real link |
| G5 | The verdict hook id was documented wrong in two places | **closed** |
| G6 | Node identity was a shared `class` string — 3 DSN stations collapsed into 1 node | **closed** — GID |
| G7 | No apiSchema for `lunco:link:*` — undiscoverable | **closed** — `LunCoLinkAPI` |
| G8 | `comms_mast.usda` — "the base's link home" — was not a link node | **closed** |
| G9 | Tests covered range/elevation only | **closed** — 13 kernel + 4 scene tests |
| G10 | Doc 49 had drifted from the code | **closed** |
| G11 | No link budget, no antenna pattern | **deliberate** — doc 49 §8 |
| G12 | *(found mid-flight)* terrain occlusion read `GlobalTransform` — wrong frame | **closed** |

### G1 — LOS ignored all scene geometry

`update_links` occluded against exactly two things: analytic celestial spheres
(`segment_hits_sphere` over the body registry) and DEM terrain (`los_hit` marched over each
`SurfaceOracle`). Nothing else. A wall, a habitat, a lander, a boulder or another rover
standing directly between two antennas **did not block the link** — there was no
`SpatialQuery` and no geometry test of any kind in the link path.

"Rover drives behind an obstacle and loses comms" is the most legible connectivity demo
there is, and it did not work unless the obstacle happened to be terrain.

### G2 — Zero visualization

`LinkState` / `LinkNode` / `link.aos` appeared **nowhere** in `lunco-viz`, `lunco-ui`,
`lunco-render-bevy` or `lunco-web`. The kernel computed correct link geometry — including
real terrain radio-shadow — that a student could not see. `LinkState` is `Reflect`-registered,
so selecting a node and reading a struct in the inspector was the entire UI surface.

For a lesson whose point is "line of sight is geometry, so move to fix it", a number in a
panel is not an answer.

### G3 — Ports that did not exist

Three places promised `comms:*` ports: `skid_rover.usda`, `comms_demo.usda`, and a
comment in `lunco-sandbox/src/lib.rs`. Grepping `comms:` across `crates/**/*.rs` returned
exactly one hit — that comment. There was no `PortRegistry` registration for links, so
`read_port .../comms:route_earth:connected` simply failed. Doc-43-era leftovers that
outlived the comms-crate deletion.

**Resolved as: no ports.** Publishing `comms:*` from Rust would reintroduce precisely the
comms vocabulary doc 49 §1 exists to keep out. The three files now describe what is real —
`LinkState`, `query("Links")`, `link.aos`/`link.los`, and `links.rhai`.

### G4 — The tutorial faked its own subject

`ss3_radio_shadow.rhai` flipped `set_subsystem("comms-degradation", …)` **by distance to
the POI**, while its own header admitted the wiring was missing. Distance is not line of
sight, and a lesson about line of sight must not be narrated over a proximity check.

The deeper problem: `traverse.usda` had **no link nodes at all** and no site anchor, its
terrain is a flat placeholder (the real relief is roadmap P0.2), and its one hill sits at
bearing ~45° while Earth is authored due east — so *nothing in that scene could have cast a
radio shadow*. The fake existed because the geometry didn't.

Fixed by making the claim true: the scene now authors a site anchor, the rover's antenna, an
`Earth` node on the published bearing (az 95°, el 4°), and an `EastRidge` occluder placed by
derivation, not vibe — the POI's line to Earth crosses x = 150 m at y ≈ 11 m, the ridge spans
y ∈ [0, 20] there, and the NW start's line clears the same plane at y ≈ 36 m. So the rover
starts with a link and loses it on the way down, from geometry. The script only reads it.

### G6 — Identity was a label

`node_key` resolved authored `class` → prim `Name` → `node_<index>`. But
`ground_station.usda` authors `class = "earth"`, and `comms_demo.usda` references it
three times (Madrid, Goldstone, Canberra) — so all three collapsed onto the key `"earth"`,
last-write-wins. The graph showed one Earth node and no individual complex was addressable.

The first fix attempted was Name-before-class. **That was also wrong**, and the review that
caught it was right: a `Name` is unique only within its parent, and the `node_<index>`
fallback is stable neither across a reload nor across peers. This project already has a real
identity — the **GID** (`GlobalEntityId`), deterministic from asset + composed path, which
`find()` returns and the API speaks on the wire.

Identity is now the GID; `class` is a routing GROUP (`query("Links").groups`), which is what
it always meant. A node with no GID yet is skipped rather than given a fallback — see doc 49.

The lesson recurred immediately: the tier-2 test's first cut matched prims by path leaf and
silently picked `/CommsWallTest/Rover/Comms/Mast` (a cylinder holding the rover's dish)
instead of `/CommsWallTest/Mast` (the base station). Names are not identities, at every layer.

### G12 — Terrain occlusion was in the wrong frame

Found while building the occluder, and the reason radio-shadow never worked: `terrain_blocks`
marched a segment expressed in `SolarFramePose::local` (**grid-absolute**) through
`gt.affine().inverse()` — the inverse of a **`GlobalTransform`**, which is origin-RELATIVE
and shifts by a whole cell whenever the floating origin moves. Wrong by the floating origin's
grid offset: zero near the origin, kilometres out at a site like the moonbase. It also cast a
~1.7e6 m lunar coordinate through f32.

Same failure family as the wheel-raycast bug that `GridSpatialQuery` exists to prevent — and
it survived because nothing tested it (G9). Both terrain and occluders now resolve poses via
`lunco_core::coords::world_pose`, in f64.

---

## 2. Why the occluder is shaped this way

Three decisions, each of which had a plausible alternative.

**Not derived from colliders.** The obvious move — "if it has a collider, it blocks" — is
wrong in both directions. Opacity is a MATERIAL property: a radio-transparent handrail has a
collider and must not block; a radio-opaque radome may have none. The cost is that occlusion
is opt-in and an untagged wall does nothing, which is the honest trade rather than a
convenient guess. Collision and occlusion are authored as two separate facts on the same prim.

**Not a physics query.** Raycasting the avian world would make every collider occlude for
free, but it couples LOS to physics, needs a stepping physics world for a link sweep that
must run headless, and costs an O(pairs) broadphase query. `segment_hits_obb` is f64
arithmetic over a handful of authored prims at the `LinkConfig` cadence, read through a
plain `Query` — so `update_links` stays non-exclusive, which doc 49 §7 requires. (Note the
precision argument does *not* apply: avian re-exports `parry3d_f64`, so a collider cast here
would be f64 too. The reason is coupling, not precision.)

**The box is core UsdGeom `extent`.** A `lunco:occluder:halfExtentsM` was drafted and
deleted. `extent` — "a three dimensional range measuring the geometric extent of the
authored gprim in its own local space" — already exists, is standard, and every DCC authors
it; a private second spelling would be free to disagree with the geometry it claims to
bound. `lunco:occluder` adds exactly the one fact USD has no word for. With no authored
extent it falls back to the unit-cube convention (`scale/2`), which is how `props/wall.usda`
is written, so tagging an existing prop is one line and no measurements.

The same test applied to the rest: core USD has **no** connectivity or occlusion schema to
reuse (`UsdLux ShadowAPI` is light shadows; `VisibilityAPI`/`purpose` is render pruning;
`NotShadowCaster` is a GPU hint and unreadable from a render-free crate). So `LunCoLinkAPI` /
`LunCoOccluderAPI` are legitimately new, following the `LunCoShadowAPI` precedent: name only
what the standard does not, and namespace it.

---

## 3. How it is proven

### Tier 1 — kernel unit tests (`link.rs`, `RunSystemOnce` on a bare `World`)

13 new tests beside the original 4. No scene, no physics, milliseconds to run:

| Test | Pins |
|---|---|
| `occluder_box_severs_link` | a wall across the sight-line severs it — range unchanged, so occlusion is the only cause |
| `occluder_beside_the_segment_does_not_sever` | the control: same nodes, same distance, box moved aside |
| `occluder_respects_rotation` | an OBB, not an AABB — a yawed slab blocks where it actually is |
| `occluder_without_extent_derives_its_box_from_scale` | the `props/wall.usda` path |
| `occluder_honours_an_offset_extent_centre` | geometry occludes where it sits, not where its origin is |
| `same_class_nodes_stay_distinct_by_gid` | the three-DSN-stations regression (G6) |
| `node_without_gid_is_skipped_not_faked` | no invented identity, ever |
| `hook_verdict_overrides_builtin_in_both_directions` | the whole scripting seam, previously untested |
| `hook_ctx_carries_the_documented_keys` | a renamed key silently reverts every policy to the builtin |
| `aos_los_fire_once_per_transition` | edges, not a 4 Hz retrigger |
| `cadence_gate_skips_recompute_within_the_interval` | the gate — every prior test passed `interval_s = 0.0` |
| `zero_cadence_recomputes_every_tick` | the escape hatch those tests rely on |
| + 5 `segment_hits_obb` tests in `geo.rs` | hit/miss, finite segment, rotation, degenerate box, lunar-scale offset |

### Tier 2 — scene contract (`lunco-usd/tests/link_occlusion.rs`)

The kernel's geometry is unit-tested above. What those tests **cannot** see is whether the
authored scene still produces the components the kernel solves over — and *every gap that
actually shipped was of that kind*: the mast that wasn't a node, the ports that didn't
exist, the wall that didn't block. A green kernel says nothing about any of them.

So tier 2 composes real assets through the real USD → ECS pipeline and asserts the contract:
`props/wall.usda` yields a `LinkOccluder` whose box matches its drawn geometry *and* keeps
its collider; `comms_mast.usda` has a node at dish height, not at its base;
`comms_wall.usda` authors exactly two endpoints with distinct roles, exactly one
occluder, and a wall geometrically between them.

### Tier 3 — the scene a student drives

**`assets/scenes/tests/comms_wall.usda`** — rover at z = +20, wall at z = 0, mast at
z = −20. The two nodes are in range the whole time, so a dropped link can only mean
occlusion. Drive left or right and it returns the moment the rover clears the wall's 8 m
width: the way out of a radio shadow is to move.

```
sandbox rhai --api 4101 -e 'print(can_reach(find("/CommsWallTest/Rover/Comms"), "base"))'
```

The same `links.rhai` a student reads in the classroom is the one the sim runs — the point
of doc 49's split.

**The sandbox was deliberately left alone.** Adding a wall + station there as a permanent
smoke surface was the original plan, and it does not work: `sandbox_scene.usda` has no site
anchor, so scene-local link nodes get no solar pose and the kernel never sees them. Adding
one to the default startup scene to gain smoke coverage is a behaviour change to the thing
everyone opens first, for a scene already crowded with balloons, cosim chains and joint
demos. `comms_wall.usda` covers it without the blast radius.

---

## 4. Tele-op refusal: the seam, and why it is shaped this way

"With the link down you cannot drive by hand" is now enforced rather than narrated. Four
decisions, each of which had a plausible alternative that is wrong.

**The seam already existed: `rbac.authorize`.** Nothing new was invented. It is
per-command-type, it sees the target gid, it **fails closed**, and — the property that
matters — it is **allow-path only**: it is consulted after the compiled role/ownership floor
has already said yes, so a policy can *further restrict and can never grant*. The worst a bug
in a refusal policy can do is lock someone out, never let them in. `SetPorts` is the one
actuation command (keyboard, API and autopilot all funnel into it), so gating it is gating
"driving" while possession, cameras and queries stay available.

**The policy is authored rhai, shipped with the course as a `LunCoPolicy` prim** in
`traverse.usda` (`info:sourceAsset = @twin://SummerSpaceSchool/sim/tutorials/teleop_policy.rhai@`
— the same name a `LunCoProgramAPI` and a UsdShade shader use; a `../` relative path is
rejected by the asset server's `UnapprovedPathMode`). A policy is a prim, so
it composes, journals, syncs and undoes like any scene edit — and is **retracted when the
stage goes away**. That scoping is the whole reason:

- `register_hook()` from the lesson's `on_start` would **leak** — there is no
  `unregister_hook` rhai binding, so the rule would survive into every scene loaded after.
- `assets/scripting/policy/` is for GLOBAL built-ins, registered from a hardcoded table in
  `lunco-scripting/src/lib.rs`. A course rule does not belong there.
- `sourcePath` over inline `source`: an `asset` reference is seen by USD's resolver and the
  whole-twin content plane, so the `.rhai` ships and every peer resolves byte-identical — and
  it stays a file a student can open.

**The fact is precomputed into the ctx, and it is generic.** The hook engine is a bare
`Engine::new()` with **no world bridge** — no `find()`, no `query()`, no `can_reach()`. That
is deliberate (hooks must be pure), so a policy sees only its ctx and cannot ask "is the link
up?". The ctx therefore gained `target_control_path_down`, read from a new
`ControlPathRegistry` — exactly the pattern `link.rs` already uses when it passes
`terrain_blocked` in rather than letting the verdict march the terrain.

It is **not** `link_up`. `session.rs` is in `lunco-core`, which cannot depend on
`lunco-celestial` (the dependency runs the other way) and must not learn comms vocabulary
anyway (doc 49 §1). "The control path is down" is generic: a jammer, a dead receiver, a
severed harness and an OBC fault all mean the same thing to a command.

**Nothing in Rust concludes "no link ⇒ no control".** `ss3_radio_shadow.rhai` computes the
geometry fact (`can_reach(radio, "earth")`) and states the consequence (`SetControlPath`);
the policy enforces it. A store-and-forward or delayed-command mission would disagree with
that inference, and is free to. Doc 49's split — kernel = geometry, script = meaning — applied
one layer up.

### The bypass that made it matter

`authorize()` sat only on the **wire** path (`sync.rs`) and the **scripted** path
(`bridge_core.rs`). The local keyboard triggers `SetPorts` **directly** from
`drive_from_bindings`, so in standalone — which is how a student runs it — a policy would have
refused nothing at all. The local path now consults the same policy.

It calls `authorize_policy` (the hook alone), **not** the full `authorize`: the role/ownership
floor is a wire concern, and this loop deliberately drives an *unpossessed* vessel (owner
`None`) which the ownership-gated floor would refuse. Gating the floor there would have broken
ordinary local play. The policy binds everywhere; the floor stays where it belongs.

### The indicator cannot disagree with the refusal

The blackout badge (`lunco-workbench/src/control_status.rs`) reads the **same**
`ControlPathRegistry` the gate refuses on, so what the student sees *is* the cause.

It deliberately does **not** read `comms-degradation`, which is the obvious source and wrong
twice: `SubsystemToggles` is a *progressive-fidelity* switch ("this lesson now simulates comms
degradation"), and `enabled()` **defaults to `true` for an unset key** — so a scene that never
mentions comms would render as permanently blacked out. SS3 was flipping it per-tick as though
it meant "the link is down right now"; that misuse is gone.

---

## 5. What is still open
- **Real relief for `traverse.usda`.** The `EastRidge` is an honest stand-in that makes the
  lesson's claim true today; the DEM crater/rille (roadmap P0.2) is what the lesson is
  really about, and when it lands the ridge should go and the checkpoints be re-heighted.
- **The sun-shadow claim is still approximate.** With the blocky placeholder `Highland`, an 8°
  sun throws a ~390 m shadow WNW that sweeps the centre-*east* rather than covering the POI.
  RADIO-shadow at the POI is exact; the *lighting* shadow the lesson also mentions waits on
  P0.2. (The hill itself was moved: authored at z = −320 it was NE, while four student-facing
  texts and the event task all say "SE highland". Site axes are East = +X, **North = −Z**, per
  `LocalTangentFrame`; the rover's authored NW start was the tiebreak.)
- **`on_skip_tutorial` never issues `StopScenario`.** So "⏹ Stop tutorial" leaves the script
  ticking and `on_stop` never fires. SS3's blackout release is defensive against this (it also
  clears on the normal exit), but the underlying lifecycle gap is real and bites any lesson
  that acquires state.
- **`thermal` is still a write-only subsystem flag** — nothing reads it, exactly as
  `comms-degradation` was.
- **No link budget** (G11) — deliberate, doc 49 §8. If it is ever wanted it is a synthesized
  Modelica domain (doc 37's `comms-link`) or a policy over the published `range_m`, never
  core Rust.
- **No antenna pattern / boresight / Fresnel / multipath.** The `Dish` prims are geometry.
- **`terrain_blocks` and `TerrainRaycastProvider` share ~15 lines** of DEM-frame setup around
  the same `los_hit` kernel. Not worth extracting for two callers; worth it at three.
