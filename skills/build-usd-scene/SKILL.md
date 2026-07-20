---
name: build-usd-scene
description: >
  How to author and edit the 3D world in LunCoSim — load scenes, spawn objects,
  place/move/rotate them, and tune their properties, over the API. USE THIS
  SKILL whenever the user asks, in plain words, things like: "put a lander near
  that crater", "spawn a few rovers here", "load the Moon scene", "add some
  rocks / obstacles", "move / rotate / scale this", "set its colour / mass /
  material", "build a scene with X and Y", or "clear the scene and start over".
  Any request to assemble or edit what's IN the 3D world belongs here — the user
  won't say "USD" or "prim". (For the agent mid-code: `LoadScene` / `SpawnEntity`
  / `MoveEntity` / `SetObjectProperty`, an `entry_id` from the spawn catalog, a
  `.usda` file, coordinate placement, or "why did the gizmo grab the wrong
  thing?".) Project-specific and non-obvious: USD is the SOURCE OF TRUTH
  (projected to ECS — you edit the world by authoring it), the engine frame is
  fixed (Y-up, right-handed, −Z-forward, metres), `LoadScene` paths are relative
  to the assets root, spawnable things come from a catalog (`list_bundled`), and
  live edits must NOT go through `SetDocumentSource`. For the vehicle's BEHAVIOUR
  use author-scenario; for its GNC use authoring-vessel-controllers.
---

# Build & edit USD scenes

The 3D world is **OpenUSD, projected to Bevy ECS** — USD is the source of truth,
the ECS scene is its projection. You build the world by **authoring USD** (via
commands that apply reversible ops), not by mutating ECS directly. Drive it over
the API (`--api`, port **4101**; launch per [`test-via-api`](../test-via-api/SKILL.md)).

Design background: [`21-domain-usd.md`](../../docs/architecture/21-domain-usd.md),
[`usd-source-of-truth-ecs-projection-design.md`](../../docs/usd-source-of-truth-ecs-projection-design.md).

## The one coordinate frame (spec 009)

The engine runs in **one fixed canonical frame: Y-up, right-handed, −Z-forward,
SI metres, f64.** Any external asset (USD `upAxis`/`metersPerUnit`, glTF, Blender)
is converted **once, at the importer** — never branch on convention in your own
placement math. A `position` you pass to `SpawnEntity` is Y-up metres.

## The command surface

| Command | Params | Does |
|---|---|---|
| `LoadScene` | `{path, root_prim}` | Load a USD scene. `path` is **relative to the `assets/` root** (do NOT prefix `assets/`). `root_prim` empty = the stage's `defaultPrim`. |
| `ClearScene` | `{}` | Tear down the current scene. |
| `RestartScene` | `{}` | Reload/reset the current scene. |
| `SpawnEntity` | `{target, entry_id, position:[x,y,z], rotation?}` | Instance a catalogued prefab. `entry_id` comes from the **spawn catalog** (`list_bundled` / `ListBundled`). |
| `MoveEntity` | `{…}` | Reposition an existing entity. |
| `SetObjectProperty` | `{entity_id:u64, property, value}` | Set a named property (both strings; value is coerced by property type). |
| `SelectEntity` | `{…}` | Select (drives the gizmo/inspector). |
| `SetPorts` | `{target, writes:[[name,val]]}` | Poke an input port (e.g. drive a spawned rover) — see [`author-scenario`](../author-scenario/SKILL.md) for behaviour. |

Discover the live set with `DiscoverSchema`; discover spawnables with `list_bundled`.

## Recipe

1. **Base:** `LoadScene {path:"scenes/…/foo.usda", root_prim:""}` for an existing
   scene, or start from the loaded default and add to it. `ClearScene` first if
   replacing.
2. **What can I spawn?** `list_bundled` → pick an `entry_id`.
3. **Place it:** `SpawnEntity {entry_id, position:[x,y,z], rotation?}` (Y-up metres).
   The response `data` carries the new entity id.
4. **Adjust:** `MoveEntity` / `SetObjectProperty` (colour, mass, material, scale) /
   `SelectEntity` to inspect.
5. **Confirm:** `CaptureScreenshot` → `/tmp/x.png` → Read it (see
   [`inspect-simulation`](../inspect-simulation/SKILL.md) for reading state back).
6. **Persist:** to make it permanent, author it into the `.usda` scene file under
   `assets/scenes/` (the runtime edits are USD ops; save them into the layer).

## Gotchas

- **Before typing `lunco:`, name the standard field this would be.** If you can name one, USD already has it — use it. A vendor namespace is only correct where USD has no concept at all, and then it must cover *only* the new part. The lathe schema got this half right: `lunco:lathe:profile`/`throatRadius`/`contour` are legitimate (USD has no surface-of-revolution schema — the parametric gprims are Sphere/Cube/Cylinder/Cone/Capsule/Plane, and `NurbsPatch` is a result format, not a generator), but it also shipped `lunco:lathe:rings` and `lunco:lathe:vOrder`, which were `vVertexCount` and `vOrder` under second names. Sampling density and degree are properties of the patch; only the shape was new. Two spellings of one quantity is the same defect as a duplicate property, reached from the other side.
- **A model as a CHILD `def LunCoProgram` prim exposes NO readable ports.** Mount a `.mo` as a child prim and it binds and solves — the log says `bound`, the compiler says `OK` — but its outputs are unreadable from every entity a script can reach (`get(x, "stroke")` returns `()` from the owner, its children AND its ancestors). APPLY it to the prim instead (`prepend apiSchemas = ["LunCoProgramAPI"]` + `lunco:program:sourceAsset` on the prim itself), which is how the vessel carries `Lander.mo` and why `flame.rhai` can read `throttle`. Cost of not knowing this: an entire landing-gear system that compiled, solved, and did nothing visible.
- **A moving part is a JOINT, not a script that rewrites a transform.** If something slides or hinges, author a `UsdPhysics` joint between two bodies and let the solver move it — you get contact, limits, reaction forces and a `force`/`displacement` port for free, and the mechanism is visible in usdview. A script that reconstructs geometry from its own `lunco:param:*` copy of the dimensions has to be kept in step with the authored transform by hand, and silently restores stale geometry the moment the two disagree. The landing gear is the worked example: four `PhysicsPrismaticJoint`s with an authored `drive:linear:physics:stiffness`/`:damping` replaced a per-tick actuator outright.
- **A sprung mechanism is a JOINT DRIVE — author it, then read it back.** Apply `UsdPhysicsDriveAPI:linear` and author `physics:type = "force"` with `drive:linear:physics:stiffness` (N/m) and `:damping` (N·s/m); avian integrates that spring directly, in SI, with no conversion. Its stroke and its load then come off the joint's own `displacement` and `force` ports. **No Modelica model and no rhai script restates the spring** — a second spelling of one spring puts two writers on one fact, and which wins becomes a function of load order. Author `physics:type` explicitly: the coefficients only mean newtons under `"force"`, and an `"acceleration"` drive has no honest newton readback at all, so its `force` port reads nothing.
- **The joint's axis carries the SIGN, and it is the only place that does.** The drive law is `force = stiffness * (targetPosition - displacement)`, so the reaction is always opposite in sign to the displacement. A spring's reaction is positive when COMPRESSED, therefore compression must read NEGATIVE displacement — which fixes the axis to point the way the mechanism EXTENDS. `physics:axis` can only name `"X"|"Y"|"Z"`, so a raked or reversed axis is carried by `physics:localRot0` (quaternions are `(w, x, y, z)`), and the limits follow it: a landing leg is `lowerLimit = -stroke`, `upperLimit = 0`, rest at 0. Get this right in the joint and nothing downstream needs a sign fixup; get it wrong and every consumer grows one.
- **Wire a physical part to PHYSICS, and flight software to SENSORS.** Contact, contact force, position and velocity are collider/body ports — they exist because the thing exists, with nothing to author. Sensors (`lunco:sensor:range` / `:imu` / `:contact`) are authored INSTRUMENTS that read those physics and add mount offset, range limits and out-of-range behaviour; they are what a GNC model should see, because a computer knows only what its instruments report. Getting it backwards is not a style question: gate a landing leg's behaviour on the ALTIMETER — whose datum sits 3.3 m above the pads — and a hand-copied `contact_alt` has to restate the geometry, gets it wrong, and lights the legs 3.9 m before touchdown. **A constant in a `.mo` that exists only to translate between two prims' positions means the wire is wrong.** (USD has no standard sensor schema at all — core `UsdPhysics` stops at bodies/colliders/joints, and Omniverse invents its own too: `PhysxContactReportAPI`, `IsaacContactSensor`. `lunco:sensor:*` is the legitimate vendor-namespace case.)
- **Publish the physical quantity, not the driving term.** A strut that reports the force *pressed onto* it reads fully loaded while it is still in the air; the honest number is the spring's own reaction, which is exactly zero until compression starts. Take it from the joint that integrates the spring — `PrismaticJoint`'s `force` port — rather than re-deriving it. When a visualization "happens too early", suspect something is publishing an input rather than a result.
- **Bevy renders an axis-Y `Cone` with its APEX UP.** An in-tree comment claimed the opposite and "corrected" it with a 180° flip, which put a rocket nozzle's apex at the bottom — the ship flew on an ice-cream cone for months. Verify cone orientation in a render before trusting a flip.
- **rhai has no float `pow`.** Exponentiation is registered under the OPERATOR name `**` only (`packages/arithmetic.rs`), so `pow(x, 0.7)` throws `Function not found: pow (f64, f64)` every tick — and because a scenario's error is per-tick and non-fatal, the rest of that function silently never runs. Use `x ** 0.7`.
- **DUPLICATE NAMES ARE SILENT — check these first when an edit breaks something unrelated.** Two prims with the same name in one parent, or the same property authored twice on one prim, are accepted by the Rust `openusd` crate with **no error and no warning** (Pixar's C++ parser rejects both). The later definition wins; the earlier one — and sometimes neighbouring prims — simply cease to exist. Measured symptoms from one real incident: a scope named `Sky` added beside an existing `def Sphere "Sky"` deleted the starfield dome, killed every custom WGSL material in the scene, and left the `LunCoEnvironment` prim unapplied so the whole film rendered ~6 stops overexposed. Nothing in the log said anything. `grep -c 'def .* "Name"'` before hunting shaders.
- **A sphere you add for the Sun or Earth casts a shadow.** Sky bodies are real geometry sitting up-sun: they eclipse the DistantLight and sweep a hard shadow across the ground. Author `bool primvars:doNotCastShadows = true` (the starfield dome does; `big_space_setup.rs` stamps `NotShadowCaster` on the engine's own sun sphere). Better still, declare bodies with `LunCoCelestialBodyAPI` (`lunco:body = 399`) and let the ephemeris place them at true distance.
- **Custom-shader inputs are snake_case** — the ShaderMaterial reflection binds the WGSL struct's field names (`star_density`, `point_size`, `brightness`). A camelCase `inputs:starDensity` is a dead wire: no error, no effect, and hours of "why does tuning the sky do nothing".
- **Exposure and illuminance only mean something together.** The frame's brightness is `illuminance / 2^EV100`, so a scene that copies a `DistantLight` intensity from one file and an `exposureEv100` from another lands stops away from either. Author both on purpose: the sun prim's `inputs:intensity` and the `LunCoEnvironment` prim's `lunco:env:exposureEv100`.
- **`LoadScene` path is relative to `assets/`** — `"scenes/sandbox/lander_test.usda"`, never `"assets/scenes/…"`.
- **Spawn `entry_id` must be in the catalog** — an unknown id logs `unknown entry '…'` and no-ops. List first with `list_bundled`.
- **Empty spawn path / root_prim → the `defaultPrim` sentinel**: an empty path means "the stage's default prim", not an error.
- **Spawns land ON the terrain surface.** Placement samples the terrain **height oracle** (analytic, so it works even before a streamed/CDLOD collider tile bakes) — a spawn over un-baked terrain rests on the ground instead of free-falling. The GUI click path terrain-fits the footprint (slope-aligned, `max(oracle, raycast)` so an obstacle rock under the chassis lifts it); the API `SpawnEntity` path snaps `y` to the surface (+ the asset's `lunco:spawnLift`) **only when DEM terrain covers `(x,z)`** — over a flat scene, or when you intend an altitude, the `position` you pass is used exactly. So pass a real Y; don't assume it's ignored.
- **One spawn = one entity.** In a single-player (`Standalone`) session a `SpawnEntity` instantiates exactly one rover; it is not also re-projected from the document (that path is suppressed to avoid a double-instantiation / vanish-on-reload).
- **Gizmo / selection frame:** on a static-USD select, the selectable root is tagged `SelectableRoot` in the **world frame** — not `GridAnchor`. If the gizmo grabs the wrong thing or the wrong frame, that tag is why.
- **Never `SetDocumentSource` for live scene building** — it replaces the whole source and cancels in-flight work. Apply edits as **individual ops** (`SpawnEntity`/`MoveEntity`/`SetObjectProperty`), one at a time.
- **USD → ECS is a projection**, so authored changes flow one way — edit the USD (via ops), and the ECS scene reconciles. Don't hand-mutate ECS transforms expecting them to persist.
- **Behaviour ≠ scene.** Making a spawned rover *do* something (drive, patrol) is a scenario — see [`author-scenario`](../author-scenario/SKILL.md); its self-driving GNC is [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).

## Anti-patterns

- ❌ Prefixing `LoadScene` paths with `assets/`.
- ❌ Guessing an `entry_id` instead of `list_bundled`.
- ❌ `SetDocumentSource` to build a scene incrementally — use per-object ops.
- ❌ Branching placement math on up-axis/units — the frame is fixed; convert only at the importer.
- ❌ Mutating ECS `Transform` directly and expecting USD to remember it — author the USD.
