# 49 — Connectivity: the generic link kernel

Status: **as-built**.

## 1. The principle

**Comms is not a subsystem.** There is no comms crate, no comms component, and no
comms vocabulary in the core — and there must not be one. A domain baked into Rust
means every neighbouring domain (sensors, sunlight, relay chains, radar) either grows
its own bespoke module or gets bent through the comms one.

What all of them actually share is **geometry between two points** — range, elevation,
occlusion. That is the reusable mechanism, and it is the only part that lives in Rust.
Connectivity is **authored content over a generic kernel**.

> If you find yourself about to add an `Antenna` component, a `link margin` field, or a
> `lunco:comms:*` attribute to a Rust crate: don't. That existed once and was deleted.
> Author it over the kernel below.

## 2. The split

| Layer | Owns | Where |
|---|---|---|
| **Kernel** (Rust) | The pairwise sweep, the cadence, and the GEOMETRY: range, local elevation, analytic body occlusion, terrain occlusion, authored box occluders. | `lunco-celestial/src/link.rs` |
| **Verdict** (script) | Whether a given pair, with that geometry, counts as a usable link. | `link.connected` hook |
| **Routing / roles** (script) | Reachability, relay chains, which station is "home", link budgets. | `assets/scripting/prelude/links.rhai` |

Nothing in the kernel is "comms": a node is a node, a link is a link. A comms domain
(or a sensor domain, or a line-of-sight domain) is composed on top.

## 3. The kernel

```rust
LinkNode     { max_range_m, min_elevation_deg, class: Option<String> }  // component
LinkOccluder { half_extents, center }                                   // component
LinkState    { peers: Vec<LinkPeer> }                    // component, written per recompute
LinkPeer     { peer: u64, connected, range_m, light_time_s, elevation_deg }
```

`class` is an authored role string. **The core never interprets it** — it is passed
through to the verdict/routing policy and used to GROUP nodes. (This is deliberate:
the moment the core branches on `class == "relay"`, the domain is back in Rust.)

`update_links` (a REGULAR, non-exclusive system) computes, per pair:
range → local elevation at each end → analytic body occlusion (`segment_hits_sphere`
over the body registry) → terrain occlusion → authored occluders. It then publishes
`LinkState` and emits `link.aos` / `link.los` telemetry events on the rising/falling
edges.

### Identity is the GID

`LinkPeer::peer` is a **`GlobalEntityId`** — the same `u64` `find()` returns to a
script and the API speaks on the wire. It is deterministic from the prim's asset +
composed path, so it is identical on every peer and stable across a reload.

Names and classes are **labels, not identities**. `components/comms/ground_station.usda`
authors `class = "earth"`, so a scene referencing it three times (Madrid, Goldstone,
Canberra — `comms_demo.usda` does exactly this) once collapsed all three onto the
key `"earth"`, last-write-wins: the graph showed one Earth node and no individual
complex was addressable. Roles are shared by design; identities cannot be.

Role routing did not regress — `query("Links")` publishes `groups` (`class → [gid]`),
and `links.rhai` accepts either, so `can_reach(rover, "earth")` still means "any Earth
station".

> A node with **no GID yet is skipped**, never given a fallback key. Identity is
> minted in `PostUpdate`, and a runtime-spawned instance takes an extra frame
> (`Provenance::Local` → `Derived`), so an absent GID means "not yet" and the node
> joins within a frame or two. A name/index fallback would MIS-BIND — diverging
> across peers and reloads — which is worse than waiting.

### Frames: grid-absolute, f64, never `GlobalTransform`

Every occlusion test runs in the **grid-absolute (BigSpace root) frame**, which is what
`SolarFramePose::local` is. Occluder and DEM poses come from `lunco_core::coords::world_pose`
(the cell-aware chain walk) — **not** `GlobalTransform`, which is origin-RELATIVE and
shifts by a whole cell whenever the floating origin moves.

> An earlier `terrain_blocks` inverted a `GlobalTransform` here, so terrain occlusion
> was silently wrong by the floating origin's grid offset: zero near the origin,
> kilometres out at a site like the moonbase — and it also cast a ~1.7e6 m lunar
> coordinate through f32. It was never noticed because nothing tested it. Same failure
> family as the wheel-raycast bug `GridSpatialQuery` exists to prevent.

### Cadence is a runtime parameter, not a build constant

```rust
LinkConfig { interval_s: f64 }   // default 0.25 s (4 Hz)
```
The whole sweep — terrain march included — is gated behind this. It does **not** run
per physics tick. Retune live, from any client or language:

```json
{"command": "SetLinkCadence", "params": {"interval_s": 1.0}}
```

### Terrain occlusion (rille radio-shadow)

The kernel marches `lunco_terrain_core::los_hit` over each DEM's `SurfaceOracle`,
read through a plain `Query<(&GlobalTransform, &DemHeightField)>` — a **read-only
component access**, which is the whole point:

> An earlier version called the `TerrainRaycast` *query provider*, which needs
> `&mut World` and therefore made `update_links` an EXCLUSIVE system. That inserted a
> command-flush sync point that interleaved with twin/terrain despawns and corrupted
> avian's island bookkeeping. **Do not reintroduce an exclusive link system.**

Endpoints are marched in `SolarFramePose::local`, which *is* the terrain oracle frame
(see `pose.rs`). The march is capped to the terrain footprint (`±half_extent`): terrain
can only occlude within its own extent, so a surface↔satellite segment does not march
millions of empty metres.

### Occluders: authored geometry blocks sight-lines

Terrain relief and celestial spheres were once the only things that could sever a link,
so a wall, a habitat, a lander or a parked rover between two antennas did nothing. A box
occluder closes that:

```usda
def Cube "Body" ( prepend apiSchemas = ["PhysicsCollisionAPI", "LunCoOccluderAPI"] ) {
    double3 xformOp:scale = (8, 4, 1)
    bool lunco:occluder = true          # ← blocks sight-lines
    bool physics:collisionEnabled = true # ← blocks bodies (separate fact)
}
```

Three decisions worth keeping:

- **Not derived from colliders.** Opacity is a MATERIAL property, not a collision one: a
  radio-transparent handrail has a collider and must not block; a radio-opaque radome may
  have none. Deriving either from the other is wrong in both directions. The cost is that
  occlusion is opt-in — an untagged wall does not block — which is the honest trade.
- **Not a physics query.** Reading colliders would mean an avian `SpatialQuery` per node
  pair, in the f32 physics frame, against the full broadphase. This is `segment_hits_obb`
  — f64 arithmetic over a handful of authored prims, at the `LinkConfig` cadence, through
  a read-only `Query`, so the system stays non-exclusive (see §7).
- **The box is core UsdGeom `extent`.** No private size vocabulary: `extent` is "a three
  dimensional range measuring the geometric extent of the authored gprim in its own local
  space", and every DCC already authors it. A `lunco:occluder:halfExtentsM` was drafted
  and deleted — a second spelling of a standard attribute is free to disagree with the
  geometry it claims to bound. With no authored extent it falls back to the unit-cube
  convention (`scale/2`), which is how `props/wall.usda` is written.

## 4. The verdict seam

```
hook id: "link.connected"
ctx:     a, b (GIDs), name_a, name_b, class_a, class_b,
         range_m, light_time_s, elev_a, elev_b,
         min_elev_a, min_elev_b, occluded, occluded_by,
         terrain_blocked, occluder_blocked, max_range_m
returns: bool
```

A pure boolean over precomputed geometry — no loops, no queries — so a rhai / Python /
Luau policy stays trivial. With no hook registered, the builtin rule applies
(range ∧ elevation masks ∧ ¬occluded ∧ ¬terrain_blocked ∧ ¬occluder_blocked).

This is why occlusion in Rust does **not** make the feature un-scriptable: the kernel
computes the `terrain_blocked` / `occluder_blocked` *facts*; the script decides what they
*mean*.

> The id is **`link.connected`**. It was documented as `comms.link.connected` in
> `world_bridge.rs` and in the SS3 tutorial — registering that id silently no-ops (no
> error, the builtin verdict just keeps applying), which is the worst failure mode there
> is for someone following the docs.

## 5. Routing is scripted, not Rust

Routing is a **decision-time** question ("can this rover reach Earth?"), asked on
`link.los` or once a second — not per tick. So it does not belong in the kernel, and
there is no BFS in Rust. Rust exposes exactly one data primitive:

```
query("Links")  ->  { nodes:  [{id, name, class}],        # id = GID
                      adj:    { "<gid>": [gid…] },        # UP links only
                      edges:  [{a, b, range_m, light_time_s}],
                      groups: { class: [gid…] } }
```
A snapshot of the live graph. No traversal. `adj` lists only links that are currently up.

Reachability is authored over it in `assets/scripting/prelude/links.rhai`:

```rhai
links()                  // the raw snapshot
reachable(from, to)      // BFS — direct or multi-hop through relays
link_path(from, to)      // the shortest hop path, as GIDs
link_path_names(from, to)// …the same path as labels, for a human
can_reach(from, station) // "can this rover talk home?"
```

Every helper takes **either a GID or a class**: a GID means that node, a class means the
group of nodes with that role. So `can_reach(find(".../Comms"), "earth")` reads the way
it always did, while each DSN complex is separately addressable. Tune or replace any of
this with no core rebuild.

## 6. Authoring a link node in USD

```usda
def Xform "Relay" (
    prepend apiSchemas = ["LunCoLinkAPI"]
) {
    bool   lunco:linkNode              = 1
    string lunco:link:class            = "relay"
    double lunco:link:maxRangeM        = 2e7
    double lunco:link:minElevationDeg  = 5.0
    int    lunco:orbit:body            = 301      # a KeplerOrbit relay
}
```
A ground station is the same, with `lunco:anchor:body` (a `GeodeticAnchor`) instead of
an orbit. A rover-mounted antenna is the same, with neither — it is a scene-local prim,
posed through the site frame. LEO / lunar-orbit satellites need no new concept: a
satellite is just a `KeplerOrbit` endpoint.

`LunCoLinkAPI` / `LunCoOccluderAPI` are declared in `lunco-usd/schema/schema.usda`, so
these attributes are discoverable (`discover_schema`, the inspector) rather than read
positionally by a reader that alone knows they exist. Core USD has no connectivity or
occlusion schema to reuse — this follows the `LunCoShadowAPI` precedent: name only what
the standard does not, and namespace it.

> A scene-local node needs a **site anchor** on the scene root (`lunco:anchor:*`).
> Without one the pose system cannot resolve it and the kernel never sees it. This is
> why `sandbox_scene.usda` carries no link nodes: it has no site anchor, and adding one
> to the default scene to gain smoke coverage is not worth the behaviour change.

Scenes: `link.usda` (orbital smoke), `comms_wall.usda` (occlusion — a rover, a
mast, and a wall between them), `comms_demo.usda` (the full DSN demo).

## 7. Known issue: scene reload and avian

Reloading a scene with live physics used to crash. It is fixed, but one half of the fix
is a **workaround, not a repair**:

- `[profile.dev.package.avian3d] debug-assertions = false` masks an upstream avian 0.7
  assert (`island.contact_count == 0`, islands/mod.rs:1372) that a batch despawn trips.
  It is debug-only (release never had it) and fires on an island avian deletes on the
  next line. Verified benign: after reload the rover stays finite, rests on terrain, and
  keeps simulating. **Do not attempt to fix it by reordering the teardown** — six
  orderings were tried and all still panic. See `clear_scene_entities` for the analysis.
- The other half was ours and is a real fix: systems touching scene entities through
  `Commands` across a `LoadScene` must use the FALLIBLE forms
  (`try_despawn`/`try_remove`/`try_insert`), because their queries are built before the
  despawn flushes. A plain `remove` panics in `apply_deferred`.

## 8. What is deliberately NOT here

- No link budget (dB, noise, bit-rate). `LinkState` carries `range_m`; a budget is an
  authored policy over it, or a Modelica component — not core Rust.
- No `class` semantics in the core. See §3.
- No routing in Rust. See §5.
