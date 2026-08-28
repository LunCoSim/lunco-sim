---
name: build-usd-scene
description: >
  Assemble or edit LunCoSim's 3D world: load scenes, spawn objects, place,
  move, rotate, scale, tune, or clear them. USE THIS SKILL for requests such as
  "put a lander near that crater", "spawn rovers", "load the Moon scene", "add
  rocks", "move this", "set its mass or material", or "build a scene with X
  and Y". For the agent mid-code: `LoadScene`, `SpawnEntity`, `MoveEntity`,
  `SetObjectProperty`, a catalog `entry_id`, a `.usda` file, or a placement/
  lighting issue. Project-specific: USD is the source of truth projected to ECS;
  use the fixed Y-up, right-handed, -Z-forward metre frame, root-qualified
  `lunco://` or `twin://` paths, and catalogued spawnables. Do not use
  `SetDocumentSource` for live edits. Use author-scenario for behavior and
  authoring-vessel-controllers for GNC.
---

# Build & edit USD scenes

The 3D world is **OpenUSD, projected to Bevy ECS** — USD is the source of truth,
the ECS scene is its projection. You build the world by **authoring USD** (via
commands that apply reversible ops), not by mutating ECS directly. Drive it over
the API (`--api`, port **4101**; launch per [`test-via-api`](../test-via-api/SKILL.md)).

Design background: [`21-domain-usd.md`](../../docs/architecture/21-domain-usd.md),
[`usd-source-of-truth.md`](../../docs/architecture/usd-source-of-truth.md).

Before assembling a scene, choose its world/time contract. The complete option
matrix is in [`assets/tutorials/README.md`](../../assets/tutorials/README.md);
the short version is below.

## Choose the scene's lighting and time model

| Scene contract | Author | Use it when |
|---|---|---|
| Fixed instructional world | A real `DistantLight` reference such as `lunco://lighting/sun.usda`, with an authored rotation; omit `LunCoEpochAPI` and `SolarSystem`. | Teaching UI, spawning, or basic controls where changing sunlight is not the subject. |
| Ephemeris world | Apply `LunCoEpochAPI`, author `double lunco:time:epochJd = …`, and reference `lunco://celestial/solar_system.usda` under `SolarSystem`; author the site anchor on the scene root when needed. | Teaching a real lunar day, Earth tracking, orbital motion, or any feature whose result depends on celestial time. |
| Existing world | Reference or payload the authoritative scene that already owns gravity, lighting, time, and celestial content. | Adding a lesson or assembly whose subject is behaviour, not scenery. |
| UI-only lesson | Omit the payload; the tutorial launcher clears an outgoing lesson scene before showing the UI-only lesson. | Teaching menus, commands, or workbench concepts. |

Do not combine a fixed light with an implicit orbital provider. `LunCoEpochAPI`
without an authored `lunco:time:epochJd` is a lint error (`epoch-api-missing-time`):
set the epoch on the same scene root, or remove the celestial opt-in. A fixed
light is a complete scene contract, not a temporary fallback.

For tutorial payloads, use the fixed contract for onboarding scenes such as
`first_drive.usda`, and the ephemeris contract for `driving_basics.usda` and
`slope_test.usda`. `rover_variants.usda` reuses `driving_basics.usda`, while
the lander mission reuses `scenes/luncosim/lander_ops.usda`; do not copy their
environment or silently choose a second clock.

## The one coordinate frame (spec 009)

The engine runs in **one fixed canonical frame: Y-up, right-handed, −Z-forward,
SI metres, f64.** Any external asset (USD `upAxis`/`metersPerUnit`, glTF, Blender)
is converted **once, at the importer** — never branch on convention in your own
placement math. A `position` you pass to `SpawnEntity` is Y-up metres.

## The command surface

| Command | Params | Does |
|---|---|---|
| `LoadScene` | `{path, root_prim}` | Load a USD scene. `path` is a root-qualified `lunco://…` or `twin://…` address. `root_prim` empty = the stage's `defaultPrim`. |
| `ClearScene` | `{}` | Tear down the current scene. |
| `RestartScene` | `{}` | Reload/reset the current scene. |
| `SpawnEntity` | `{target, entry_id, position:[x,y,z], rotation?}` | Instance a catalogued prefab. `entry_id` comes from the **spawn catalog** (`list_bundled` / `ListBundled`). |
| `MoveEntity` | `{…}` | Reposition an existing entity. |
| `SetObjectProperty` | `{entity_id:u64, property, value}` | Set a named property (both strings; value is coerced by property type). |
| `SelectEntity` | `{…}` | Select (drives the gizmo/inspector). |
| `SetPorts` | `{target, writes:[[name,val]]}` | Poke an input port (e.g. drive a spawned rover) — see [`author-scenario`](../author-scenario/SKILL.md) for behaviour. |

Discover the live set with `DiscoverSchema`; discover spawnables with `list_bundled`.

## Recipe

1. **Base:** `LoadScene {path:"lunco://scenes/…/foo.usda", root_prim:""}` for an existing
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
7. **Verify the contract:** run `target/debug/luncosim --validate <scene.usda>`;
   for a composed world, run the authored Rhai scene gate and inspect its
   verdict. `--validate` catches parse/composition/lint failures but does not
   prove that the light remains stable during runtime.

## Gotchas

- **Before typing `lunco:`, name the standard field this would be.** If you can name one, USD already has it — use it. A vendor namespace is only correct where USD has no concept at all, and then it must cover *only* the new part. The lathe schema got this half right: `lunco:lathe:profile`/`throatRadius`/`contour` are legitimate (USD has no surface-of-revolution schema — the parametric gprims are Sphere/Cube/Cylinder/Cone/Capsule/Plane, and `NurbsPatch` is a result format, not a generator), but it also shipped `lunco:lathe:rings` and `lunco:lathe:vOrder`, which were `vVertexCount` and `vOrder` under second names. Sampling density and degree are properties of the patch; only the shape was new. Two spellings of one quantity is the same defect as a duplicate property, reached from the other side.
- **A model as a CHILD a `Scope` applying `LunCoProgramAPI` prim exposes NO readable ports.** Mount a `.mo` as a child prim and it binds and solves — the log says `bound`, the compiler says `OK` — but its outputs are unreadable from every entity a script can reach (`get(x, "stroke")` returns `()` from the owner, its children AND its ancestors). APPLY it to the prim instead (`prepend apiSchemas = ["LunCoProgramAPI"]` + `info:sourceAsset` on the prim itself), which is how the vessel carries `Lander.mo` and how a script reaches its `throttle`. Cost of not knowing this: an entire landing-gear system that compiled, solved, and did nothing visible. **A USD CONNECTION is a different mechanism and does reach a child program**: `.connect` resolves an `outputs:` by PRIM PATH, so `</Vessel/Cone/Photometry.outputs:intensity>` on a child a `Scope` applying `LunCoProgramAPI` wires fine. The restriction is on a script's relationship walk, not on the port graph.
- **A moving part is a JOINT, not a script that rewrites a transform.** If something slides or hinges, author a `UsdPhysics` joint between two bodies and let the solver move it — you get contact, limits, reaction forces and a `force`/`displacement` port for free, and the mechanism is visible in usdview. A script that reconstructs geometry from its own `lunco:param:*` copy of the dimensions has to be kept in step with the authored transform by hand, and silently restores stale geometry the moment the two disagree. The landing gear is the worked example: four `PhysicsPrismaticJoint`s carrying an authored `drive:linear:physics:stiffness`/`:damping`, with no actuator script anywhere.
- **A sprung mechanism is a JOINT DRIVE — author it, then read it back.** Apply `UsdPhysicsDriveAPI:linear` and author `physics:type = "force"` with `drive:linear:physics:stiffness` (N/m) and `:damping` (N·s/m). The canonical USD-to-Avian reader preserves that SI force law and uses Avian's implicit `SpringDamper` realization when the driven body has a positive authored mass; it derives equivalent frequency and damping ratio for stable integration, without creating a second spring or changing the authored coefficients. Its stroke and its load then come off the joint's own `displacement` and `force` ports. **No Modelica model and no rhai script restates the spring** — a second spelling of one spring puts two writers on one fact, and which wins becomes a function of load order. Author `physics:type` explicitly: the coefficients mean newtons under `"force"`, while an `"acceleration"` drive is mass-normalized and has no honest newton readback at all, so its `force` port reads nothing. Missing mass, angular coefficient drives, and negative coefficients are not repaired by a fallback; the USD linter reports the invalid or conditionally unstable authoring.
- **The joint's axis carries the SIGN, and it is the only place that does.** The drive law is `force = stiffness * (targetPosition - displacement)`, so the reaction is always opposite in sign to the displacement. A spring's reaction is positive when COMPRESSED, therefore compression must read NEGATIVE displacement — which fixes the axis to point the way the mechanism EXTENDS. `physics:axis` can only name `"X"|"Y"|"Z"`, so a raked or reversed axis is carried by `physics:localRot0` (quaternions are `(w, x, y, z)`), and the limits follow it: a landing leg is `lowerLimit = -stroke`, `upperLimit = 0`, rest at 0. Get this right in the joint and nothing downstream needs a sign fixup; get it wrong and every consumer grows one.
- **Wire a physical part to PHYSICS, and flight software to SENSORS.** Contact, contact force, position and velocity are collider/body ports — they exist because the thing exists, with nothing to author. Sensors (`lunco:sensor:range` / `:imu` / `:contact`) are authored INSTRUMENTS that read those physics and add mount offset, range limits and out-of-range behaviour; they are what a GNC model should see, because a computer knows only what its instruments report. Getting it backwards is not a style question: gate a landing leg's behaviour on the ALTIMETER — whose datum sits above the pads — and a hand-copied constant has to restate that offset, lighting the legs before touchdown. **A constant in a `.mo` that exists only to translate between two prims' positions means the wire is wrong.** (USD has no standard sensor schema at all — core `UsdPhysics` stops at bodies/colliders/joints, and Omniverse invents its own too: `PhysxContactReportAPI`, `IsaacContactSensor`. `lunco:sensor:*` is the legitimate vendor-namespace case.)
- **Publish the physical quantity, not the driving term.** A strut that reports the force *pressed onto* it reads fully loaded while it is still in the air; the honest number is the spring's own reaction, which is exactly zero until compression starts. Take it from the joint that integrates the spring — `PrismaticJoint`'s `force` port — rather than re-deriving it. When a visualization "happens too early", suspect something is publishing an input rather than a result.
- **Bevy renders an axis-Y `Cone` with its APEX UP.** An in-tree comment claimed the opposite and "corrected" it with a 180° flip, which put a rocket nozzle's apex at the bottom — the ship flew on an ice-cream cone for months. Verify cone orientation in a render before trusting a flip.
- **rhai has no float `pow`.** Exponentiation is registered under the OPERATOR name `**` only (`packages/arithmetic.rs`), so `pow(x, 0.7)` throws `Function not found: pow (f64, f64)` every tick — and because a scenario's error is per-tick and non-fatal, the rest of that function silently never runs. Use `x ** 0.7`.
- **DUPLICATE NAMES ARE SILENT — check these first when an edit breaks something unrelated.** Two prims with the same name in one parent, or the same property authored twice on one prim, are accepted by the Rust `openusd` crate with **no error and no warning** (Pixar's C++ parser rejects both). The later definition wins; the earlier one — and sometimes neighbouring prims — simply cease to exist. Measured symptoms from one real incident: a scope named `Sky` added beside an existing `def Sphere "Sky"` deleted the starfield dome, killed every custom WGSL material in the scene, and left the `LunCoEnvironment` prim unapplied so the whole film rendered ~6 stops overexposed. Nothing in the log said anything. `grep -c 'def .* "Name"'` before hunting shaders.
- **A sphere you add for the Sun or Earth casts a shadow.** Sky bodies are real geometry sitting up-sun: they eclipse the DistantLight and sweep a hard shadow across the ground. Author `bool primvars:doNotCastShadows = true` (the starfield dome does; `big_space_setup.rs` stamps `NotShadowCaster` on the engine's own sun sphere). Better still, declare bodies with `LunCoCelestialBodyAPI` (`lunco:body = 399`) and let the ephemeris place them at true distance.
- **Custom-shader inputs are snake_case** — the ShaderMaterial reflection binds the WGSL struct's field names (`star_density`, `point_size`, `brightness`). A camelCase `inputs:starDensity` is a dead wire: no error, no effect, and hours of "why does tuning the sky do nothing".
- **Exposure and illuminance only mean something together.** The frame's brightness is `illuminance / 2^EV100`, so a scene that copies a `DistantLight` intensity from one file and an `exposureEv100` from another lands stops away from either. Author both on purpose: the sun prim's `inputs:intensity` and the `LunCoEnvironment` prim's `lunco:env:exposureEv100`.
- **Celestial time is an explicit scene choice.** A `SolarSystem` reference makes body poses ephemeris-driven; `LunCoEpochAPI` makes the scene responsible for choosing the mission epoch. Author both the API and `lunco:time:epochJd` together. If the lesson needs repeatable light but not astronomy, use the fixed `DistantLight` contract instead.
- **`LoadScene` path must be root-qualified** — use `lunco://scenes/luncosim/lander_ops.usda` for a shipped asset or `twin://<name>/…` for an opened Twin. Use `OpenFile` for a filesystem path.
- **Spawn `entry_id` must be in the catalog** — an unknown id logs `unknown entry '…'` and no-ops. List first with `list_bundled`.
- **Empty spawn path / root_prim → the `defaultPrim` sentinel**: an empty path means "the stage's default prim". A stage without `defaultPrim` is an invalid scene mount and fails visibly.
- **Spawns land ON the terrain surface.** Placement samples the terrain **height oracle** (analytic, so it works even before a streamed/CDLOD collider tile bakes) — a spawn over un-baked terrain rests on the ground instead of free-falling. The GUI click path terrain-fits the asset's composed `UsdPhysics` collision footprint (slope-aligned, and it considers a physics obstacle under the chassis). An asset without a `defaultPrim` or collision footprint is rejected visibly; placement never invents dimensions or a lift. The API `SpawnEntity` path uses the explicit position supplied by the caller, so pass a real Y.
- **One spawn = one entity.** In a single-player (`Standalone`) session a `SpawnEntity` instantiates exactly one rover; it is not also re-projected from the document (that path is suppressed to avoid a double-instantiation / vanish-on-reload).
- **Gizmo / selection frame:** on a static-USD select, the selectable root is tagged `SelectableRoot` in the **world frame** — not `GridAnchor`. If the gizmo grabs the wrong thing or the wrong frame, that tag is why.
- **Never `SetDocumentSource` for live scene building** — it replaces the whole source and cancels in-flight work. Apply edits as **individual ops** (`SpawnEntity`/`MoveEntity`/`SetObjectProperty`), one at a time.
- **USD → ECS is a projection**, so authored changes flow one way — edit the USD (via ops), and the ECS scene reconciles. Don't hand-mutate ECS transforms expecting them to persist.
- **Behaviour ≠ scene.** Making a spawned rover *do* something (drive, patrol) is a scenario — see [`author-scenario`](../author-scenario/SKILL.md); its self-driving GNC is [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).

## Anti-patterns

- ❌ Passing a bare or absolute filesystem path to `LoadScene`.
- ❌ Guessing an `entry_id` instead of `list_bundled`.
- ❌ `SetDocumentSource` to build a scene incrementally — use per-object ops.
- ❌ Branching placement math on up-axis/units — the frame is fixed; convert only at the importer.
- ❌ Mutating ECS `Transform` directly and expecting USD to remember it — author the USD.
