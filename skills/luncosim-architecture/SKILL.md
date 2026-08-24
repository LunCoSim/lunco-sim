---
name: luncosim-architecture
description: Review or build a LunCoSim feature that crosses USD, Modelica, Avian, Rust, or Rhai. Use this when adding a reusable component, sensor, actuator, controller, generated Modelica network, USD schema, or runtime projection, and when removing legacy paths, shims, compatibility fallbacks, or custom fields that duplicate an OpenUSD standard.
---

# LunCoSim architecture

Use this skill before changing a reusable engine feature. Keep the authored
system declarative and composable, and make each concern live in its native
representation:

| Concern | Authoritative owner | Runtime role |
|---|---|---|
| Scene structure, identity, topology, frames, connections, component parameters | USD | Rust projects the composed stage; it does not invent missing topology |
| Continuous equations, state, control laws, filters, physical networks | Modelica | Rumoca compiles and steps the model |
| Rigid-body collision, contacts, forces, torques, joints | Avian through USD-authored physics | Rust exposes the engine's generic mechanics and executes them |
| Mission phases, events, policy, objectives | Rhai or behaviour trees | Event-driven orchestration only; no production `on_tick` control loop |
| Engine mechanisms, projection, scheduling, hot paths | Rust | Generic implementation; no vehicle- or sensor-name special cases |

## Start with the standard-schema audit

Before creating `LunCo*API` or a `lunco:*` property:

1. Inspect the vendored OpenUSD schemas in `crates/lunco-usd/schema/core/` and
   the maintained USD/PhysX schema that actually owns the concept.
2. Use the standard field when it exists: `UsdGeom` for transforms, geometry,
   cameras, visibility and purpose; `UsdShade` for connectable graphs and
   materials; `UsdLux` for lights and shadows; `UsdPhysics` for bodies, mass,
   collision, joints, limits and drives; USD metadata and `assetInfo` for
   descriptions and asset identity; USD connections for graph edges.
3. Add a LunCo schema only for semantics that have no standard owner, such as
   mission/celestial meaning, engine program allocation, a LunCo-specific
   sensor configuration, terrain generation policy, or control-session
   ownership. Keep that schema narrow and do not duplicate standard fields.
4. If a custom field overlaps a standard field, migrate every reader and asset
   to the standard spelling in one change, delete the old field and branch, and
   regenerate the schema artifacts. Do not read both spellings.

The mapping and the current keep/remove decisions are recorded in
[`references/usd-standard-map.md`](references/usd-standard-map.md). The
authoritative engine architecture is
[`docs/architecture/clean-architecture-and-usd-standards.md`](../../docs/architecture/clean-architecture-and-usd-standards.md).

## Build a reusable component

1. Put reusable geometry, physics, parameters, and Modelica source under
   `assets/`; keep scene-specific opinions in the composing scene layer.
2. Give a component one clear default prim and `kind = "component"`; give a
   composed vehicle `kind = "assembly"`. Use `doc`, `displayName`, `assetInfo`,
   `UsdGeom`, `UsdPhysics`, `UsdShade`, and `UsdLux` before adding namespaced
   duplicates.
3. Represent real attachment with a USD physics joint and its authored frames.
   Hierarchy is namespace, not attachment. A mounted rigid body without a joint
   is a free body.
4. Encapsulate each actuator or sensor as a reusable asset. Instantiate and
   connect it in USD. The lander model must not know that a component is called
   an RCS engine, reaction wheel, or altimeter; it consumes local-frame ports.
5. Let Avian apply force or torque at the authored actuator frame. Do not add a
   special Rust emitter for one actuator family, convert to world coordinates in
   Modelica, or maintain a parallel actuator registry.
6. Expose tunable values as typed USD-authored parameters or Modelica
   parameters. Do not hide magic numbers in Rust, Rhai, or a renderer.

## Build a sensor and controller

1. Expose raw built-in Avian observations through generic ports. Rust may know
   that a ray hit happened and publish distance, validity, normal, point, and
   relative velocity; it must not decide that the observation is an
   “altimeter” or “landing sensor”.
2. Mount the sensor and author its connections in USD. Keep sensor placement,
   frames, collision filters, and parameters in the USD asset.
3. Convert raw observations into useful navigation signals in Modelica. Put
   filtering, derivatives, frame conversion, attitude reference, PID, thrust
   mixing, fuel, and actuator dynamics in Modelica. Modelica uses local frames;
   it does not receive a hidden world-coordinate special case.
4. Treat the parsed Modelica contract as authoritative. A compile-time
   `parameter` is not a runtime input. USD defaults for parameters go to the
   parameter set; only actual Modelica `input` variables enter runtime wiring.
   A missing required port is an authoring error with a diagnostic, not a
   fallback to an old name or a fabricated zero.
5. Wire the model's outputs to generic body/actuator ports through native USD
   connections. Use the same path for autopilot, API, and scenario writes.
6. Use possession and an authored authority signal for manual handoff. Do not
   create a second `manual` flag or bypass the control surface.

## Generated Modelica networks

Generated models are a first-class reusable composition boundary, not a one-off
string workaround:

- USD owns the component graph, instances, port names, parameters, and
  connections.
- The registered generator emits a normal, inspectable Modelica model with
  stable component names, policy-owned unit instance names, and explicit boundary inputs/outputs; runtime-generated
  documents are read-only projections of the authored USD + Rhai policy.
- The generator is selected by an open domain descriptor/registry, not by a
  Rust `if` for “electrical”, “hydraulics”, or one vehicle.
- Acausal equations and physical conservation stay inside Modelica. Causal
  cross-domain signals cross the USD boundary as typed ports.
- Render the generated Modelica icons and connection graph from the same model
  source; do not create a second visual-only network.
- Keep generated visual synthesis in the selected Rhai policy: standard root /
  unit `Icon` and `Diagram` annotations, policy-owned placements, and any
  domain-specific presentation belong there. Rust may provide generic source
  loading, class resolution, and typed projection metadata, but must not encode
  a domain poster or duplicate the policy's graph.
- Keep generated browser metadata explicit: distinguish root boundary inputs and
  outputs from promoted member telemetry, and expose the generated document as
  read-only runtime state with a normal Modelica drill-in path.
- Validate the returned generated source as a generic strict AST contract:
  exact root name and boundary, complete policy units, native members nested in
  their owning units, and no direct native members on the root. Treat missing
  or loading class definitions as explicit resolver states in the canvas, never
  as a fabricated resolved node.
- Keep generated document lifetime tied to the projection entity. Classify it by
  the `generated/` document origin, retire it on removal/despawn, and keep
authored document cleanup separate. Load bundled source roots asynchronously
through the policy-declared `source_roots` and shared Modelica engine, then
reproject from its generic completion signal.
- Let Rhai own the optional `member_output_aliases` promotion table. Rust may
  validate known member/output pairs and identifier uniqueness, but must not
  choose aliases or emit visual source for a policy.
- A cyclic set of separately co-simulated components is explicit: the runtime
  may report its one-step delay. Do not silence that warning or add a Rhai
  workaround. If zero-delay continuous feedback is required, synthesize one
  Modelica network and solve it as one system.

## Remove wrong old behavior cleanly

When the old mechanism is wrong, perform a clean cutover:

1. Identify the authoritative replacement and write the negative test first.
2. Update source assets, schema source, reader, projection, commands, and docs
   together.
3. Delete the old property, alias, compatibility branch, fallback reader, and
   migration-only shim. Do not preserve behavior merely because old scenes used
   it.
4. Regenerate `generatedSchema.usda` and `plugInfo.json` from the authoritative
   source where applicable. Never edit generated schema by hand.
5. Verify parse, composition, contract, projection, solver, and real executable
   behavior. `--validate` proves syntax only; it does not prove runtime wiring.

Permitted defaults are only semantic defaults declared by the authoritative USD
or Modelica schema. A numerical guard such as a positive epsilon is not a
compatibility fallback and must be named as a numerical guard.

## Verification gate

Run the smallest relevant checks first, then the production binary:

```bash
python3 scripts/gen_schema.py
RUSTC_WRAPPER= cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test -p lunco-usd --test schema_generation -j 4
RUSTC_WRAPPER= cargo test -p lunco-modelica --test sensor_contracts -j 4
RUSTC_WRAPPER= cargo test -p lunco-usd-sim --test usd_connection_derivation -j 4
CARGO_INCREMENTAL=1 RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim -j 4
```

For a live feature, launch only `target/debug/luncosim` with an explicit free
API port, verify readiness, inspect ports and composed connections, and run the
scene through the real executable. Report separately:

- parsed and composed;
- contract and topology accepted;
- solver/runtime behavior observed;
- visual behavior observed; and
- warnings that remain, especially algebraic-loop delays.

Never claim that a source parse or unit test proves the scene is physically
correct.
