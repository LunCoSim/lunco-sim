# Vessel Limits and Routes — deriving instead of copying

**Status:** HUD derivation + rhai accessors implemented 2026-07-19; routes and
tiers still proposed. Companion to
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

The HUD has the same defect from the other direction. `rover_hud.rs` colours tilt
against fixed `CAUTION_TILT_DEG = 20.0` / `DANGER_TILT_DEG = 30.0`, with an honest
comment explaining why they are generic and inviting the fix:

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
  `lunco:tire:frictionCoefficient` and composed onto the wheel by its `tire`
  variant.
- **Track width** — the wheel entities' own transforms.
- **CoM height** — `physics:centerOfMass`, already authored per variant and loaded
  into the physics body.

So the slip and tip limits are *derivable*, per vessel, at runtime. Publishing
them is not new data — it is refusing to make humans recompute data we have.

## Design: compute at the point of use, store nothing

### The rejected version, and why

The first implementation added a `VesselEnvelope` component to `lunco-mobility`,
recomputed by a change-driven system. **It was built, tested, and reverted the same
day**, because it contradicts the argument this document makes.

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
| Driver HUD (per frame, Rust) | `tilt_bands()` — a free function in `rover_hud.rs`, fed by the wheel query the system already has |
| Lessons (one-shot, rhai) | `slip_limit()` / `slip_limit_or()` / `exceeds_slip()` in `assets/scripting/prelude/vessel.rhai`, reading `WheelRaycast.friction_mu` by reflection |

The cost profile is the right way round: the per-frame consumer is Rust, where six
wheels and an `atan` are cheaper than the layout of the panel they label; the
reflection-crossing consumer is rhai, which only ever asks at configuration time.

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
| Lessons (rhai) | four hand-copied constants | `slip_limit_or(rover, fallback)` |
| Terrain hazard overlay | `cliff_deg` typed per lesson | `cliff_deg` passed from `slip_limit(rover)` |

The overlay case is what makes this worth doing at all: *the ground turns red
exactly where this rover loses traction*, with nobody typing a number. That is
`13_DRIVER_UI_DESIGN.md`'s "the difficulty ladder, rendered" — obtained by deleting
constants rather than by building a panel.

## Routes as USD prims

The same defect, second instance. `route()` — five XZ waypoints — is now
**duplicated verbatim** in two lesson scripts, because rhai scenarios have no
include mechanism. Edit one and the check measures a route nobody drives.

The fix is not to sync them. It is to move the route out of the scripts:

```usda
def Xform "Route" ( kind = "group" )
{
    def Xform "WP_0" ( prepend references = @lunco://vessels/markers/waypoint.usda@ ) { … }
    def Xform "WP_1" ( … ) { … }
}
```

Ordered waypoint prims under a group, read by any lesson that needs them. This
pays for itself three times:

1. **Kills the duplication** — one route, in the scene, read by both lessons.
2. **It is the remote-sensing import format.** The handoff pack already specifies
   waypoints coming back as `[[x,y,z], …]`; waypoint prims are what those become.
   A GIS-authored route becomes a `.usda` overlay, not a script edit.
3. **It is what the path-line gizmo needs anyway** — see below.

Heights: waypoints should carry authored Y, but a route authored from GIS knows
only XZ. Resolve at load by sampling `TerrainHeight` (analytic, answers anywhere
on the crop) rather than requiring the author to supply heights they cannot know.

## Route line: drape, do not span

`lunco-autopilot` mirrors `AutopilotBehaviorSpec` onto the vessel in three places
(`lib.rs:1254`, `:1639`, `:1693`) explicitly so "the UI / path-line gizmo can read
the waypoints". **That gizmo was never written.** The only `linestrip` in the tree
is `cinematic.rs:136`, the camera-path preview.

When it is written it must **drape over the relief, not connect the waypoints**. A
straight chord between two waypoints 651 m apart passes *through* the crater wall:
it renders underground for most of its length, and draws a path the rover does not
take. Sample `TerrainHeight` along each leg at a fixed step — 4 m is the natural
choice, matching the baseline everything else about that site is measured at — and
emit a polyline through those points, lifted slightly to avoid z-fighting.

> Body curvature is a separate, smaller effect: over a 1 km scene the surface falls
> ≈0.29 m below a straight chord (`d²/2R`, R = 1737 km). Draping on the DEM
> subsumes it at this scale, but the same code at moonbase scale must not assume a
> flat datum.

## Difficulty tiers as a variantSet

Third instance of the same shape. `traverse.usda` hardcodes `rover_medium`, so
"run the next tier" means editing the scene, and `ss4`'s `tier` param moves the
scoring without moving the machine.

A `tier` variantSet on `/Traverse/Rover` — the same mechanism the wheels already
use for `tire` — makes the tier a *selection* rather than an edit, and keeps every
prim path identical. That last part is load-bearing: spawning a replacement rover
would orphan `/Traverse/Rover/Comms`, the link node the radio-shadow lesson finds
by path.

## Why this is one document

Three symptoms, one cause: **data that is derivable, or authorable once, was
copied instead.** The envelope, the route, and the tier are the same fix applied to
capability, to geometry, and to configuration. Each one removes constants rather
than adding a feature — which is why they are cheap, and why they keep paying.
