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
| Mission phases, events, policy, objectives | Rhai or behaviour trees | Task/event orchestration in production; `on_tick` is test-only for sampled verdicts |
| Engine mechanisms, projection, scheduling, hot paths | Rust | Generic implementation; no vehicle- or sensor-name special cases |

### Rhai task callback contract

Task leaves use one callback form: an anonymous closure `|me| ...`. `me` is the
host entity id, while `this` is the persistent scenario-state map bound by the
native task driver. Named `Fn("...")` pointers are not task leaves. Named Rhai
helpers remain ordinary callable policy and may be invoked explicitly from an
anonymous task closure. This keeps one positional contract and lets the native
kernel bind state without a compatibility path.

## Plugin and crate layering

Use a domain `CorePlugin` for headless-safe state, lifecycle, commands, and
runtime mechanisms, then add a separate UI plugin only for panels and visual
presentation. Do not create a UI plugin merely to host API queries. If a
provider belongs to a core domain but importing the API crate would create a
dependency cycle, put the provider in a small `*-api` adapter crate and have
each API-capable composition root install it explicitly. Keep the data/core
crate independent of transport and presentation layers.

Project-owned persistence policy belongs to the active Twin manifest's generic
settings boundary. A domain may define one namespaced scalar key and expose it
through the existing `SetTwinSetting` path; it must not add a global settings
section or a second cache reader/writer for the same artifact.

## Source-backed program attachment

`AttachProgram` is the one authoring boundary for binding a `.mo`, `.py`,
`.rhai`, or behaviour-tree source to an existing USD prim. Rust owns the
generic command and lowers a complete `ProgramAttachSpec` into one journalled
USD change set. The spec carries the source asset, explicit scalar inputs and
outputs, defaults, native USD connections, and the explicit `realtimeSafe`
promise.

The Models palette, Rhai `attach_program(...)`, HTTP callers, and the Assembly
editor all use this command. None may insert an ECS marker or maintain a
second program registry. An empty port contract is a valid source-only
attachment, but it is not a running scalar cosim participant; the author must
declare the interface before wiring or stepping it.

For derived 3D annotations such as a waypoint route, keep one change-gated
projection snapshot between authored/runtime facts and presentation consumers.
Resolve authored identities through the authoritative binding map, express all
positions in the active physics frame, and sample the authoritative terrain at
intermediate points before creating geometry. Project marker roots before building
the snapshot so authored and runtime target labels share the same terrain pose.
Keep marker-root placement, annotation mesh reconciliation, labels, and look tinting
as separate owners that consume that snapshot. Stable frames must do no route parsing,
binding lookup, terrain sampling, mesh generation, or marker writes; camera-dependent
label projection is the only remaining per-frame presentation work.

Repeated presentation solvers must also be value-idempotent: compare derived
`Transform`/`CellCoord` values before assignment. Bevy marks mutable component
access as changed even when the value is equal, and BigSpace consumes those
signals for dirty-subtree propagation. Guarding an equal write at the producer
is part of the ownership contract; it is not permission to hide a real dirty
input or to add an alternate propagation path.

Use the same owner-local revision/cursor shape for other stable projections:
the Modelica document registry wakes engine sync, telemetry producers pace by
their authoritative model/fixed time, behavior target paths are cached per
entity and invalidated by authored XML or active-frame ancestry, terrain
curvature reacts to its input components, and globe LOD caches pure selection
until camera/LOD/handoff/residency inputs change. These cursors suppress work;
they do not become a second data source or a compatibility fallback.

Apply this contract to all camera pose owners, including shared interaction
easing, mounted USD followers, cinematic path followers, and the persistent
camera origin. Camera selection/mode policy stays in the application; BigSpace
owns only precision representation and derived transform propagation.

### Assembly document snapshots

Assembly editing starts from the existing document system. Use
`DocumentRegistry::fork` and the domain's `ForkableDocument` implementation to
make an untitled document with a fresh identity; do not create a second scene
model or copy a registry. The document implementation copies authored layers
and invalidates private derived state, while `DocumentHost` copies undo/redo
history by value. The registry attaches a recorder for the new id to the same
Twin journal. Save-As is the first path binding. A derived cache must be
document-owned and keyed by all authoritative layer revisions; full USD
composition and dependency resolution stay with `lunco-usd-compose` and its
existing resolver path.

Native Assembly Editor view-models are keyed by the existing `UsdPreviewId`
session. Derive one prim tree, connection canvas, Inspector subview, and
authored USD subview per open session, then paint the session selected by the
focused `UsdPreviewViewId`. Views share the projected stage but own their
camera/render target and never become document identity. Hidden view cameras
remain inactive, and visible targets are bounded by `UsdPreviewRenderBudget`
(2048 px per axis, 4,194,304 pixels per view, and 8,388,608 visible pixels per
frame by default). The shared ECS
selection is only the focused-session projection; keep selection and drilled
targets in editor-owned session state so focus changes cannot apply a command
to a same-named prim in another document. Always carry the session's explicit
`DocumentId`, `LayerId`, and projection generation into a typed USD command.

When the work is an agent-driven human asset edit, use the
[interactive Assembly Editor runbook](../edit-usd-assembly/SKILL.md). The
authoring session is headful and remains visible to the user; every coherent
change is applied through the existing typed/journal path, inspected through
the focused preview and a screenshot, and reviewed with the user before the
next material change or save. This is an operating mode over the existing
ownership model, not a new assembly API.

The render-side camera binder also owns Bevy's clustered-light policy.
Use Bevy's `ClusterConfig::Single` for automatic cameras while the ECS topology
has no point lights, spot lights, light probes, or clustered decals, and follow
those component lifecycle events back to Bevy's normal configuration when one
appears. The camera reconciler must also wait for a positive computed viewport
and positive `Clusters` dimensions before activating a new window camera,
because GPU extraction receives active cameras before the first cluster
assignment. Preserve an explicit `ClusterConfig`; do not add a scene-name
check, per-frame light scan, or alternate lighting implementation.

Scene-root `UsdPrimPath` values may be empty until the stage is parsed. Resolve
that sentinel through the shared USD `defaultPrim` resolver before any domain
projector reads the path; visual and celestial projection must not each invent
their own deferred-path handling or permanently mark an unresolved root.

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
   to the standard spelling in one change, delete the superseded field and branch, and
   regenerate the schema artifacts. Do not read both spellings.

The mapping and the current keep/remove decisions are recorded in
[`references/usd-standard-map.md`](references/usd-standard-map.md). The
authoritative engine architecture is
[`docs/architecture/clean-architecture-and-usd-standards.md`](../../docs/architecture/clean-architecture-and-usd-standards.md).

## Classify a gap before changing the engine

When a report says a capability is missing, verify it against the target
checkout before accepting the claim. Search the shipped asset library, reusable
Modelica packages, composing scenes, and authored tests; read the closest
production exemplar and its consumer. Classify the result as:

```text
engine substrate | reusable asset | mission assembly | external dependency | evidence gap
```

An absent mission asset is not an absent engine capability. A source parse or
documented feature is not runtime evidence. Record the exact source path and
the strongest observed state separately: parsed, composed, contract-ready,
solver-running, numerical behavior, or visual behavior. Only an infrastructure
gap that survives an authored fixture justifies a Rust design.

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
6. Expose tunable physical values as typed USD-authored parameters or Modelica
   parameters; do not hide them in Rust, Rhai, or a renderer. Named constants
   are appropriate for policy-owned presentation geometry, spacing, extents,
   and typography when they are clearly separated from physical parameters.

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
   fallback to an alternate name or a fabricated zero.
5. Wire the model's outputs to generic body/actuator ports through native USD
   connections. Use the same path for autopilot, API, and scenario writes.
6. Use possession and an authored authority signal for manual handoff. Do not
   create a second `manual` flag or bypass the control surface.

## Generated Modelica networks

Generated models are a first-class reusable composition boundary:

### Root and island rule

One `CollectionAPI:components` network root produces one generated Modelica
participant with one public boundary. The synthesizer may partition its graph
into several generated Modelica units, but those units remain inside the same
root and are not additional ECS participants.

Keep acausal conservation connectors (`Pin`, `HeatPort`, `FluidPort`, `Flange`,
and equivalent domain connectors) inside the root whose solver owns their
algebraic equations. A typed scalar USD connection between two roots is causal
and may have a macro-step or one-step delay. It is not an acausal `connect()`.

If zero-delay bidirectional coupling is required, move the coupled components
into one generated Modelica root and solve the combined DAE. Do not add a Rhai
polling loop, duplicate state, or hidden cross-root fallback. Automatic island
fusion is an optional future mechanism; correct network authoring is the
current default.

- USD owns the component graph, instances, port names, parameters, and
  connections.
- The registered generator emits a normal, inspectable Modelica model with
  stable component names, policy-owned unit instance names, and explicit boundary inputs/outputs; runtime-generated
  documents are read-only projections of the authored USD + Rhai policy.
- The generator is selected by an open domain descriptor/registry, not by a
  Rust `if` for “electrical”, “hydraulics”, or one vehicle.
- Acausal equations and physical conservation stay inside Modelica. Causal
  cross-domain signals cross the USD boundary as typed ports.
- Compose reusable Modelica classes through USD before introducing a vehicle- or
  mission-specific `.mo` wrapper. Add new equations only when the maintained
  package and its public contract cannot express the requirement.
- Render the generated Modelica icons and connection graph from the same model
  source; do not create a second visual-only network.
- Make the generated browser entry useful on first click: a single-unit
  network opens its unit class so the member graph is visible, while a
  multi-unit network opens the root wrapper. Keep both classes in the ordinary
  Modelica source/drill-in hierarchy; this is a navigation choice, not a second
  generated graph.
- Keep generated visual synthesis in the selected Rhai policy: standard root /
  unit `Icon` and `Diagram` annotations, policy-owned placements, and any
  domain-specific presentation belong there. Rust may provide generic source
  loading, class resolution, and typed projection metadata, but must not encode
  a domain poster or duplicate the policy's graph.
- For a power-network policy, make common-bus semantics visible with standard
  Modelica `Line` waypoints and a policy-owned diagram rail. Use adaptive,
  extent-aware placement for repeated members. Derive the visual hub from
  graph incidence, using the typed `LunCoModelicaTopologyAPI` `storage` role
  only to break equal-incidence ties; place `source` and `load` roles on
  opposite deterministic banks and pack `neutral` members onto the shorter
  bank. Do not branch on component class names or let a fixed demo layout
  imply a direct source-to-load wire when the composed graph has many members.
  These roles are presentation metadata, not Modelica solver direction:
  acausal flow can reverse at runtime. Keep source/load lane ranges disjoint
  around the hub so a horizontal route cannot imply a direct connection.
  Member coordinates are local to the
  owning unit diagram; root coordinates place unit instances.
- Reuse the generic Modelica flow animation for electrical networks. A native
  `flow Real` such as `LunCo.Electrical.Pin.i` must be discovered by connector
  metadata and sampled from live node state; Rhai emits ordinary `Pin`/
  `connect(...)` equations and must not grow a generated-electrical animation
  branch. Non-zero signed flow animates in the resolved direction; zero or
  missing state remains idle/diagnostic.
- The flow renderer reads all declared connector flow variables and live
  runtime state keys, not a domain-specific value or generated policy field.
  Precompute lookup keys during projection and keep the per-frame walk linear
  in route segments plus visible dots.
- Keep generated browser metadata explicit: distinguish root boundary inputs and
  outputs from promoted member telemetry, and expose the generated document as
  read-only runtime state with a normal Modelica drill-in path.
- Keep editing semantics honest: an editable Modelica document moves nodes by
  emitting the generic canvas `NodeMoved` event and persisting standard
  `Placement` annotations through `ModelicaOp::SetPlacement`. A generated
  document stays read-only because USD plus Rhai owns its source; expose
  `Duplicate to edit` instead of accepting a non-persistent drag.
- Keep projection responsive: Modelica root loading, parse, and inheritance/icon
  walks run off the UI thread. UI readers use completed caches or a nonblocking
  lock and show an explicit loading/error state until the generic completion
  event requests reprojection. Never hold the engine mutex across painting.
- Validate the returned generated source as a generic strict AST contract:
  exact root name and boundary, required `source`, `units`, `layout.units`,
  `layout.members`, `source_roots`, and `member_output_aliases` fields, no
  undeclared root/unit causal ports, promotions only for outputs present in the
  loaded member class, non-overlapping policy placements, complete policy units,
  native members nested in their owning units, and no direct native members on
  the root. Member-placement overlap is invalid within one unit coordinate
  system; different unit diagrams may legitimately reuse local coordinates.
  Treat missing or loading class definitions as explicit resolver
  states in the canvas, never as a fabricated resolved node.
- Keep generated document lifetime tied to the projection entity. Classify it by
  the `generated/` document origin, retire it on removal/despawn, and keep
  authored document cleanup separate. Structured packages under
  `assets/models/<Root>/package.mo` are ordinary Modelica search-path roots: the
  compiler/editor discovers the root segment of a qualified reference
  generically and loads that package through the shared Modelica engine. A
  policy-declared `source_roots` list may prewarm dependencies, but it must not
  be required for class discovery and Rust must not name a particular library.
  Reproject from the generic completion signal.
- Let Rhai own the required `member_output_aliases` promotion table, including
  the explicit empty-table case. Rust may validate known member/output pairs and
  identifier uniqueness, but must not choose aliases or emit visual source for a
  policy.
- Keep policy contracts in Rhai assets under `assets/scripting/tests/`. The
  Rust host supplies composed facts and invokes the shipped policy; Rhai owns
  assertions about generated source, topology, layout, and presentation.
  Literal top-level Rhai constants are supported inside policy helper functions
  by the shared hook binding, so presentation policy can remain editable without
  adding Rust-side layout parameters.
- A cyclic set of separately co-simulated components is explicit: the runtime
  may report its one-step delay. Do not silence that warning or add a Rhai
  polling bridge. If zero-delay continuous feedback is required, synthesize one
  Modelica network and solve it as one system.

## Replace a superseded contract cleanly

When a mechanism is wrong, perform a clean cutover:

1. Identify the authoritative replacement and write the negative test first.
2. Update source assets, schema source, reader, projection, commands, and docs
   together.
3. Delete the superseded property, alias, compatibility branch, fallback reader,
   and migration-only shim. Do not preserve an invalid contract for compatibility.
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
