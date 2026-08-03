# 60 — Clean architecture and USD standards

> Status: Active · Audience: contributors building reusable LunCoSim assets, models, projections, and tools

This is the implementation boundary for new LunCoSim features. It records the
decisions that keep a mission assembled from USD, equations in Modelica, rigid
body behavior in Avian, policy in Rhai, and generic mechanisms in Rust. It is
also the standard-schema gate: a custom LunCo field is justified only when USD
has no semantic owner for it.

## 1. Ownership is the architecture

| Concern | Authoritative representation | What Rust may do |
|---|---|---|
| Prim identity, composition, hierarchy, frames, topology, parameters, connections, material assignment | Composed USD stage | Project it and maintain derived caches |
| Geometry, transforms, cameras, visibility and purpose | `UsdGeom` | Read standard schema and render/project it |
| Materials and connectable graphs | `UsdShade` | Evaluate the authored graph or expose it to a renderer |
| Lights and shadows | `UsdLux` | Apply standard light and shadow schemas |
| Bodies, mass, collision, joints, limits and drives | `UsdPhysics` and the maintained PhysX schema where required | Build Avian entities and apply the authored mechanics |
| Continuous equations, state, filters, controllers, fuel, hydraulics and physical networks | Modelica source or a generated Modelica source | Compile, bind, step and expose generic ports |
| Raw rigid-body observations and force/torque realization | Avian | Publish generic observations and generic actuator ports |
| Mission phases, events, objectives and sequencing | Rhai or behaviour trees | Run the policy mechanism; never own continuous control math |
| Engine scheduling, projection, storage and transport | Rust | Implement generic mechanisms, never one vessel or sensor's semantics |

The ECS is a projection of USD, not a second authored scene. `SimConnection`,
compiled wiring, runtime port samples, and entity IDs are derived state. A
feature that writes those caches directly instead of authoring USD escapes save,
journal, undo, replication, and deterministic reconstruction.

## 2. Standard-schema gate

Use the existing standard before creating a `LunCo*API` or `lunco:*` property:

| Concept | Canonical schema or mechanism |
|---|---|
| Units and transforms | Layer `upAxis`, `metersPerUnit`, `UsdGeom.Xformable` |
| Descriptions and asset identity | USD `doc`, `kind`, `Usd.ModelAPI`, `assetInfo`, and `displayName` metadata |
| Geometry, grouping and visibility | `UsdGeom` typed prims, `Imageable`, `purpose`, `visibility`, `CollectionAPI` |
| Materials and node graphs | `UsdShade.Material`, `Shader`, `NodeGraph`, `ConnectableAPI`, `MaterialBindingAPI` |
| Lighting and shadows | `UsdLux.LightAPI`, concrete light types, `ShadowAPI`, light-list APIs |
| Cameras | `UsdGeom.Camera` and authored `Xformable` transforms |
| Physics | `UsdPhysics` bodies, mass, collision, joints, limits and drives; PhysX schema only for PhysX-specific semantics |
| Graph topology | Typed `inputs:`/`outputs:` attributes and native `connectionPaths`; `UsdShade` connectable conventions for graph nodes |
| Graph editor layout | `UsdUI` node-graph schemas when supported by the reader |

Custom schemas remain appropriate for semantics that core USD does not define:
mission and celestial meaning, geodetic anchors and orbits, engine program
allocation, control-session ownership, terrain generation policy, raw sensor
configuration, and domain-specific vehicle authoring where no standard vehicle
schema exists. They must be narrow and must not repeat a standard property.

The source of the LunCo schema registry is
`crates/lunco-usd/schema/schema.usda`. `generatedSchema.usda` and `plugInfo.json`
are generated artifacts. Edit the source, run `scripts/gen_schema.py`, and test
the generated result. Never hand-edit generated schema files.

### 2.1 Migration rule

When a custom property overlaps a standard property, do a clean cutover:

1. Author the standard property in every asset.
2. Change every reader, projection, command, test, and document to the standard.
3. Delete the custom property and its reader branch in the same change.
4. Regenerate schema artifacts and add a negative test proving the old spelling
   is not accepted as a second source of truth.

Do not retain aliases, dual reads, compatibility branches, shims, or fallback
search paths. A default is valid only when the authoritative standard schema
declares the semantic default and the reader consumes it. A numerical epsilon
used to keep a denominator positive is a numerical guard, not a compatibility
value, and must be named as such.

## 3. Reusable component pattern

A reusable component is a composed USD asset, not a Rust type for one vehicle.

1. Give the asset one clear default prim, `kind = "component"`, standard
   metadata, typed geometry, and explicit units.
2. Use `UsdPhysics` for the actual rigid body, collision, mass, joint, limits,
   and drive. Hierarchy is namespace; a rigid body becomes attached only by a
   joint with authored frames.
3. Put reusable visuals and simulation models in the asset library. Let a scene
   instantiate them with references and author composition-specific transforms,
   activation, and connections in its own layer.
4. Expose every tunable physical value through typed USD attributes or Modelica
   parameters. Do not hide a mass, force limit, sensor offset, or visual scale
   in Rust or Rhai.
5. Add a composition test and a runtime test. Parsing a `.usda` proves syntax;
   it does not prove that the body, ports, joint, and model actually compose.

For a new engine-backed component, use:
[`skills/author-usd-component/SKILL.md`](../../skills/author-usd-component/SKILL.md),
[`skills/author-usd-physics/SKILL.md`](../../skills/author-usd-physics/SKILL.md),
and [`skills/validate-assets/SKILL.md`](../../skills/validate-assets/SKILL.md).

## 4. Modelica is the continuous-equation boundary

Modelica models own equations and state. They should be ordinary reusable
Modelica models with inspectable parameters, connectors, and icons—not hidden
Rust formulas or per-tick Rhai calculations.

### 4.1 Parameters are not runtime inputs

The source model contract is authoritative:

- `parameter` values are compile-time configuration and belong in the model's
  parameter set;
- `input` values are runtime signals and participate in USD wiring;
- `output` values publish samples and topology to the generic port surface.

When USD exposes an authored constant for a Modelica parameter, contract
classification must route it to parameters before runtime binding. Treating
every USD `inputs:` attribute as a runtime input creates false missing-port
errors and was the cause of the lander recording failure fixed in this
architecture line.

Required runtime ports must fail clearly when absent. Do not manufacture a zero,
reuse an old port name, or add a duplicate connection to preserve a broken
scene.

### 4.2 Local frames and sensor conversion

Modelica uses the local frame defined by the component topology. World-frame
conversion belongs in a sensor or navigation model only when it is an explicit
model input and equation. A generic Rust bridge must not add a special
`force_x = f_world_x` path for a lander.

The sensor path is:

```
Avian raw observation
  -> generic runtime output ports
  -> USD-authored sensor connections
  -> Modelica filtering / frame conversion / navigation
  -> Modelica guidance and control law
  -> USD-authored actuator connection
  -> Avian force, torque, joint, or valve realization
```

Rust may publish a ray hit, distance, validity, normal, point, or relative
velocity. It must not label the sample as an “altimeter”, “landing sensor”, or
other domain-specific semantic. That conversion belongs in the reusable
Modelica sensor/navigation library.

## 5. Actuators are components, not special cases

An RCS jet, main engine, reaction wheel, hydraulic valve, or wheel motor is a
reusable component with an authored local frame and a generic actuation
surface. The vehicle instantiates it and connects it in USD.

- Modelica computes a local command, force, torque, mass flow, valve state, or
  other continuous result.
- USD connects that result to the component's generic input.
- Avian applies the physical effect at the component's authored frame and
  lever arm.
- The visual effect reads the same authored component state; it is not a
  second visual-only actuator path.

Adding a second RCS or a reaction wheel must require only another USD instance
and connections. It must not require a new Rust match arm, a special output
emitter, or a scenario-specific shim.

## 6. Generated networks are a first-class boundary

Runtime model generation is a supported architecture, not a textual workaround.
The generated network must be:

- derived from the USD-authored component graph and native connections;
- emitted as ordinary, inspectable, stable Modelica source;
- reusable and hand-editable after generation;
- composed with standard Modelica components and explicit boundary ports;
- rendered with the model's own Modelica icons and connections;
- compiled and contract-checked before the surrounding USD graph is marked ready.

Keep acausal physical conservation and continuous equations inside Modelica.
Use causal USD connections for cross-domain signals. If separately compiled
components form an algebraic cycle, the runtime must report the explicit
co-simulation delay. Do not hide it in Rhai or suppress the warning. If the
mission requires zero-delay continuous feedback, synthesize one Modelica island
and solve it as one system.

## 7. Verification and handoff

For a reusable engine change, run this order:

1. schema source/generator and asset parse checks;
2. negative composition tests for missing or wrongly named contracts;
3. focused Modelica contract and numerical tests;
4. focused USD connection/projection tests;
5. DEBUG `target/debug/luncosim` built with regular incremental Cargo builds;
6. a real scene run with an explicit API port;
7. readiness, composed topology, port values, solver health, and visual checks.

Report each level separately. A green parser or unit test is not evidence of a
working runtime scene. A clean scene run is not evidence that a visual claim is
correct until a frame or video has been inspected.

Use the production binary directly, preserve a single authoritative path, and
keep unrelated worktree changes intact while applying the cutover.
