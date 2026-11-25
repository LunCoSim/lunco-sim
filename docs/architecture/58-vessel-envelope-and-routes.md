# 58 — Vessel Limits and Routes — deriving instead of copying

> Status: Active · Audience: contributors on vehicle capability, HUDs, and routing
>
> HUD derivation and the Rhai accessors are built; the route projection is
> implemented and tiers remain proposed. Companion to
[`57-dem-georeferencing.md`](57-dem-georeferencing.md) — same principle (*one
source of truth, derive the rest*), applied to vehicle capability and to route
data rather than to spatial reference.

Driven out of the Summer Space School twin, where the same physical constant ended
up hand-copied into six files.

## The defect

A rover's **slip limit** is `atan(μ)`. It is not a fact anyone should type.

Today `μ = 0.5` is authored once, correctly, on the tire
(`assets/components/mobility/tires/worn.usda`) — and the *derived* `26.6°` is
then written by hand into:

| Copy | File |
|---|---|
| lesson constant `SLIP_LIMIT_DEG` | twin `sim/tutorials/route_check.rhai` |
| lesson constant `CLIFF_DEG` | twin `sim/tutorials/ss1_the_site.rhai` |
| per-tier `cliff` literal | twin `sim/tutorials/ss4_scored_run.rhai` |
| ladder table | twin `SURVEY.md` |
| tier table | twin `TERRAIN_REPORT.md` |
| prose | `assets/vessels/rovers/variants/rover_medium.usda` |

Retune the tire and six places lie. Worse, they lie *plausibly* — 26.6° stays a
believable number, so nothing looks broken.

The HUD had the same defect from the other direction. The engine exposure producer
coloured tilt against fixed `CAUTION_TILT_DEG = 20.0` /
`DANGER_TILT_DEG = 30.0`, with an honest comment explaining why they are generic
and inviting the fix:

> deliberately not a per-vehicle limit … A rover that wants its own arcs should
> publish them; until it does, these are honest thresholds

Against the school's ladder those generic bands are **inverted**:

| tier | slips at | HUD shows |
|---|---|---|
| awful | 21.8° | amber — already sliding |
| medium | 26.6° | amber — red never arrives before failure |
| easy | 52.4° | red at 30°, with 22° of margin left |

The driver most at risk gets the mildest warning.

## The insight: it is already authored, just not exposed

Nothing new needs to be declared. The engine loads every input already:

- **μ** — `WheelRaycast::friction_mu` (`lunco-mobility/src/lib.rs:322`), read from
  standard `physics:dynamicFriction` and composed onto the wheel by its `tire`
  variant.
- **Track width** — the wheel entities' own transforms.
- **CoM height** — `physics:centerOfMass`, already authored per variant and loaded
  into the physics body.

So the slip and tip limits are *derivable*, per vessel, at runtime. Publishing
them is not new data — it is refusing to make humans recompute data we have.

## Design: compute at the point of use, store nothing

### The rejected version, and why

The first implementation cached the derived limits in `lunco-mobility`,
recomputed by a change-driven system. **It was built, tested, and reverted the
same day**, because it contradicts the argument this document makes.

`atan(min μ)` is one minimum and one arctangent over data already in memory. Caching
it buys nothing — it is not expensive, not shared mutable state, and changes only
when authoring changes — while costing the one thing we were trying to eliminate: a
second representation that can go stale. If a tire variant switched without tripping
`Changed<WheelRaycast>`, the component would keep reporting the old angle. That is
the *same bug as the six hand-copied constants*, relocated into the ECS and given a
system to maintain it.

The rule that falls out:

> Derive at the point of use. Cache a derived value only when the computation is
> expensive **or** a per-frame consumer cannot afford it — and neither is true here.

**`min` μ, not mean** survives from that design: a vehicle slips at its *weakest*
contact, and averaging would flatter a rover with one bald tire.

### What exists instead

| Consumer | Where the derivation lives |
|---|---|
| Driver HUD (reactive runtime exposure) | `tilt_bands()` in `crates/lunco-luncosim-exposures/src/lib.rs`, fed by the wheel query the projector already has |
| Lessons (one-shot, rhai) | `slip_limit()` / `exceeds_slip()` in `assets/scripting/prelude/vessel.rhai`, reading `WheelRaycast.friction_mu` by reflection |

The cost profile is the right way round: the engine producer resolves six wheels
and an `atan` only after relevant ECS inputs change and publishes at a bounded
cadence; the reflection-crossing consumer is rhai, which only ever asks at
configuration time.

`vessel_wheel_summary()` and `vessel_wheels_match()` inspect both `WheelRaycast`
and the vehicle plugin's reflected `PhysicalWheel` contract. The same shared
physical-wheel component remains the render/editor motion owner; inspection
does not introduce a second wheel-parameter store.

**Guidance the prelude states explicitly:** a rhai task that wants this every tick
must read it once into `this` in `on_start`. The limit does not change while you
drive, and each call walks children with one reflected read per wheel.

> If a genuine per-frame rhai consumer ever appears, *that* is when a cached
> component earns its place. Today there is none.

### Falling back honestly

A vessel with no wheels — a lander, a free camera — gets no derived bands, and the
HUD keeps the generic constants, renamed `FALLBACK_*` and documented as the
*unknown-vehicle* case rather than as the truth. The HUD also labels which it is
showing ("slip 27° · tip 63°" vs "generic limits"), because a coloured arc that
means different things on different vehicles and looks identical is worse than no
arc at all.

### 2. One consumer contract, three consumers

### The three consumers

| Consumer | Before | After |
|---|---|---|
| Driver HUD | fixed 20°/30° constants | amber = `atan(min μ)`, red = `atan(half_track / com_height)` |
| Lessons (rhai) | four hand-copied constants | `slip_limit(rover)`, and the caller REFUSES on `()` |
| Terrain hazard overlay | `cliff_deg` typed per lesson | `cliff_deg` passed from `slip_limit(rover)` |

The overlay case is what makes this worth doing at all: *the ground turns red
exactly where this rover loses traction*, with nobody typing a number. That is
`13_DRIVER_UI_DESIGN.md`'s "the difficulty ladder, rendered" — obtained by deleting
constants rather than by building a panel.

## Routes as composed USD plans

The same defect, second instance. `route()` — five XZ waypoints — is now
**duplicated verbatim** in two lesson scripts, because rhai scenarios have no
include mechanism. Edit one and the check measures a route nobody drives.

The fix is not to sync them. It is to move the route out of the scripts:

```usda
def Scope "Route"
{
    def Scope "Program" (prepend apiSchemas = ["LunCoProgramAPI"]) { … }
    def Xform "P0" ( prepend references = @lunco://markers/route_point.usda@</RoutePoint> ) { … }
    def Xform "P1" ( prepend references = @lunco://markers/route_point.usda@</RoutePoint> ) { … }
}
```

Ordered route-point prims under a route-plan scope, read by the sibling Rhai
program and any lesson that needs them. The scope may be inline for a small
fixture, but reusable or imported paths belong in a separate `.usda` route-plan
asset and are composed into the scene. This
pays for itself three times:

1. **Kills the duplication** — one route, in the scene, read by both lessons.
2. **It is the remote-sensing import format.** The handoff pack already specifies
   waypoints coming back as `[[x,y,z], …]`; waypoint prims are what those become.
   A GIS-authored route becomes a `.usda` overlay, not a script edit.
3. **It gives the route tool one stable topology source** — see below.

Point positions, including elevation, are authored in USD. An import tool that
starts with only XZ must resolve and author its elevation before the route is
used; the route program does not invent a second terrain-derived path.

## Route line: derive a bounded runtime annotation

Composed USD points define the initial route; the scene-level Rhai program
owns the active route plan and is the sole source for guidance. The reusable `waypoint_editor` Rhai tool
derives a live ribbon from the active point children. The route program
refreshes geometry when their ordered positions change. The mounted
route owns a keyed presentation object with an emissive, shadowless material.
On a DEM the terrain surface displays its annotation. A scene without terrain
uses the narrow open mesh anchored at the first point. With fewer than two
points the curve is hidden and retains its identity. The generic billboard
renderer displays each marker's authored waypoint label.

USD remains authoritative for authored point identity, initial transforms and
authored mission topology. Live waypoint additions, positions, removals and
colors reside in the program's active plan and native marker/sensor entities. A transient generation gap while a typed edit is being
projected is retried by the route program; a point-name read that is not
synchronized is rejected instead of guessing a duplicate path. A missing
subject, malformed point, or persistent unavailable USD generation is a visible
route-program error and leaves the subject safely braked. The selected program
path, not a hardcoded `/Route`, determines which plan receives an edit or a
control edge. Closing the scene retires the route’s presentation children.

## Difficulty tiers as a variantSet

Third instance of the same shape. `traverse.usda` hardcodes `rover_medium`, so
"run the next tier" means editing the scene, and `ss4`'s `tier` param moves the
scoring without moving the machine.

A `tier` variantSet on `/Traverse/Rover` — the same mechanism the wheels already
use for `tire` — makes the tier a *selection* rather than an edit, and keeps every
prim path identical. That last part is load-bearing: spawning a replacement rover
would orphan `/Traverse/Rover/Comms`, the link node the radio-shadow lesson finds
by path.

## Why composition separates the plan from the scene

Three symptoms, one cause: **data that is derivable, or authorable once, was
copied instead.** The envelope, the route, and the tier are the same fix applied
to capability, to geometry, and to configuration. A reusable route plan is
therefore composed into the physical scene instead of being copied into it.
The scene owns placement, subject identity, and program bindings; the plan owns
ordered points and route policy. Two plans can be composed at distinct scopes
and selected explicitly without adding a vehicle-specific core registry.
