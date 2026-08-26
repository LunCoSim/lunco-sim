# Handover to `main`: telemetry and generated Modelica

Date: 2026-08-24
Source worktree: `usd`
Source branch: `usd`
Last source commit before this handover: `04e7f4555` (`Fix Modelica source ownership and diagram routes`)

This handover belongs to the `usd` worktree. No files in the `main` worktree were
changed by this work. Before integrating, check ancestry and the merge result;
do not replay a commit that is already an ancestor of `main`.

## Executive summary

The intended architecture is now:

```text
composed USD
    │  scene facts: members, classes, ports, parameters, connections
    ▼
lunco-usd-sim domain projection
    │  one generic graph read; no battery/motor/rover classification
    ▼
SynthesizerRegistry → Rhai policy
    │  source + component units + public outputs + diagram layout
    ▼
GeneratedModelicaSource + Modelica compiler/runtime
    │  solver identity and state metadata
    ▼
shared SignalRegistry
    ├── telemetry browser / plots
    ├── API channel listing, history, subscriptions
    └── scripts and other graph consumers
```

The source and visual diagram are two projections of the same generated
Modelica document. A visual-only graph is not authoritative. Modelica owns
continuous equations and state; USD owns the composed topology and authored
metadata; Rhai owns domain synthesis policy; Rust owns generic projection,
validation, lifecycle, and runtime mechanisms.

The follow-up UI review also closed the projection-stall path: the generated
canvas now resolves native icons through the shared Modelica engine, loads
structured source roots asynchronously, and keeps the UI lock-free at
contention points. Editable documents persist node movement as standard
`Placement` annotations; generated documents remain read-only and expose
`Duplicate to edit` so a user cannot make a non-persistent edit to a USD/Rhai
projection. Physical-flow animation consumes declared Modelica `flow` metadata
and live state, not generated electrical names.

The important production rule is that telemetry must preserve the canonical
USD/model identity while exposing the solver state. Solver spelling is an
implementation detail and must not become the user-facing channel identity.

## Current ownership and code locations

### Generated Modelica

- `crates/lunco-usd-sim/src/domain_projection.rs`
  - Reads composed USD network facts.
  - Resolves source classes and reports explicit projection errors.
  - Owns `SynthesizerRegistry`, `GeneratedModelicaSource`, member mappings,
    topology units, and layout validation.
  - Does not contain electrical/rover-specific graph construction.
- `assets/scripting/policy/synth_acausal_network.rhai`
  - Current default synthesis policy.
  - Emits Modelica components, ports, `connect(...)` equations, boundary and
    causal equations, icons, diagram graphics, and deterministic layout.
  - Owns named presentation constants for layout spacing, component extents,
    icon sizing, and diagram typography; repeated source/load members use a
    deterministic near-square bank grid and per-column feeder lanes; these are
    policy controls, not hidden Rust/runtime parameters.
  - Each authored motor/component remains a distinct component in the facts and
    generated topology; a policy may partition connected components into units
    without collapsing the source graph into a visual summary.
- `assets/scripting/policy/synth_actuator_wrench.rhai`
  - Separate registered actuator-domain policy for force/wrench allocation.
  - It is not an electrical special case and is selected through the generic
    synthesizer registry.
- `crates/lunco-scripting/src/lib.rs`
  - Registers the shipped `synth.acausal-network` and
    `synth.actuator-wrench` Rhai hooks at production startup.
- `crates/lunco-hooks-rhai/src/lib.rs`
  - Provides the generic hook compiler and propagates literal top-level Rhai
    constants into policy helper functions; this is shared scripting machinery,
    not generated-electrical policy in Rust.
- `crates/lunco-modelica/src/state.rs`
  - Stores the published generated source, URI, network root, and projection
    error state.
- `crates/lunco-usd-sim/src/cosim.rs`
  - Initializes and schedules generated-source projection, Modelica member
    class resolution, compilation, and publication.
- `crates/lunco-modelica/src/annotations/source.rs`
  - Reads authored `connect(...) annotation(Line(...))` route data from source
    when the parser AST does not retain the connection annotation.
  - This is shared source-backed annotation parsing, not a diagram-specific
    scanner.
- `crates/lunco-modelica/src/index.rs` and
  `crates/lunco-modelica/src/canvas_projection.rs`
  - Build the structural index and diagram projection from the same source and
    AST model, including connection waypoints and ports.

### Telemetry

- `crates/lunco-core/src/telemetry.rs`
  - Defines the transport-neutral channel types and `ChannelSource` variants:
    `Port`, `Reflect`, and `Diagnostic`.
- `crates/lunco-signal/src/lib.rs`
  - Owns canonical `SignalRef { entity, path }`, metadata, sample history,
    retention, deadband, and removal lifecycle.
  - The same path on two entities is intentionally two signals.
- `crates/lunco-telemetry/src/lib.rs`
  - Samples declared channels at their configured rates, validates authored
    settings, collapses duplicate declarations, and retains samples in the
    shared `SignalRegistry`.
- `crates/lunco-telemetry/src/api.rs`
  - Provides generic channel discovery, including Modelica component identity
    and separation of same-named local signals on different entities.
- `crates/lunco-modelica/src/runtime_telemetry.rs`
  - Publishes Modelica runtime state into the shared signal path.
  - Retains solver/model identity metadata and does not require a plot binding
    for state to be retained.
- `crates/lunco-modelica/src/engine_resource.rs` and
  `crates/lunco-modelica/src/class_cache.rs`
  - Own the shared source-aware engine handle and generic asynchronous package
    root loading. UI/projection readers do not wait for the engine mutex.
- `crates/lunco-modelica/src/ui/panels/canvas_diagram/ops.rs`
  - Maps the generic canvas `NodeMoved` event to `ModelicaOp::SetPlacement`;
    the document patch/journal path persists the standard Modelica annotation.
- `crates/lunco-modelica/src/ui/panels/canvas_diagram/edge.rs`
  - Animates only declared connector `flow` variables, including multiple
    flow fields, along the actual routed edge polyline.
- `crates/lunco-api/src/queries.rs` and `crates/lunco-api/src/subscription.rs`
  - Expose channel metadata, history queries, and realtime subscriptions.
- `crates/lunco-usd-sim/src/cosim.rs`
  - Projects authored `lunco:telemetry:*` USD declarations into the shared
    generic telemetry sampler. USD remains the owner of those declarations.
- `crates/lunco-viz/src/telemetry_browser.rs` and
  `crates/lunco-modelica/src/ui/panels/telemetry.rs`
  - Consume the shared metadata and registry; they must not reconstruct channel
    identity from solver variable names.

The canonical design references are:

- `docs/architecture/20-domain-modelica.md`
- `docs/architecture/telemetry-subsystem.md`
- `docs/architecture/29-rumoca-workarounds.md`
- `docs/architecture/lander-actuation-modelica.md`

## Telemetry contract for `main`

The user-facing identity of a channel is the pair `(entity, path)`, represented
by `SignalRef`. Metadata supplies the display name, unit, description, group
path, provenance, active state, and Modelica identity where applicable.

There are three important visibility layers:

1. Public authored channels and boundary values. These are the canonical
   operator-facing representation of a measured quantity.
2. Modelica runtime state. This is the complete state/variable view exposed by
   the model runtime, with source asset, qualified class, member variable, and
   canonical USD-facing metadata. It is not limited to variables that happen to
   be plotted.
3. Archived/inactive history. Retired publishers can retain history for
   explicit inspection, but archived entries must not appear in the ordinary
   live tree. The API and browser have an explicit active/archive distinction.

This separation addresses the previous failure modes:

- generated solver names such as long fully qualified paths no longer define
  the normal display label;
- generated member aliases are diagnostic/internal relations, not a second
  public channel;
- duplicate declarations are collapsed by canonical identity rather than
  displayed twice;
- a new solver session clears stale previous history;
- state remains available even when no plot binding exists;
- missing diagnostics are explicit and non-fatal according to the channel
  source contract, not silently substituted with another value.

Telemetry settings remain authored/configured through the generic channel
contract: rate, enabled state, deadband, retention, unit, name, description,
and one of the supported source kinds. Do not add a Rust branch for “battery”,
“motor”, “camera”, “suspension”, or “rover”. If a component needs an operator
channel, author the declaration or expose it through the model/runtime’s
generic metadata path.

## Generated Modelica contract for `main`

The generated document must be inspectable as ordinary Modelica:

- declared component classes and readable, collision-checked instance names;
- typed ports and public boundary ports;
- authored/component parameters and constants;
- executable `connect(...)` equations and causal/boundary equations;
- standard Modelica icon and diagram annotations. The canvas does not invent a generic
  component card when a resolved class has no `Icon`; that is an explicit visual-contract
  diagnostic and the class must be fixed at its Modelica owner.
- connection routes and port positions;
- a stable mapping back to the USD prim for every component and output.

The shipped electrical policy gives the generated unit a standard diagram-level
power-bus rail and routes each acausal connection through it with ordinary
Modelica `Line` annotations. The policy chooses the highest-incidence member
in each connected unit as the visual bus hub; an authored
`LunCoModelicaTopologyAPI` `storage` role only breaks equal-incidence ties.
Authored `source` members occupy one branch bank and `load` members the
opposite bank, while `neutral` members are packed onto the shorter bank. This
is topology plus typed presentation metadata, not a battery/motor/solar class
rule, and it avoids the misleading appearance of a solar panel wired directly
to a motor. Because the Modelica graph is acausal, these roles never assert a
solver direction; runtime signed `flow` values still determine animation.
The two banks use disjoint lane ranges around the hub, so a horizontal route
cannot be mistaken for a direct source-to-load wire. Member coordinates are
local to their owning unit diagram; root coordinates place the unit instances.
The policy also uses readable generated unit instances (`power_system` or
`power_unit_N`); full USD paths remain the identity mapping, not the display
label.

Electrical activity is covered by the established generic flow animation path
used by the rocket/lander diagrams. The shipped `Pin` declares `flow Real i`;
the standard connector projection records that flow variable, and the canvas
samples live `instance.p.i` values to move directional dots along each
generated `connect(...)` polyline. The Rhai policy therefore emits ordinary
Modelica connectors and routes only; Rust does not need a generated-electrical
animation branch. Non-zero signed current animates, zero current is idle, and
missing state remains an observable diagnostic.

`GeneratedModelicaSource` is the single published source/diagram record. The
editor, compiler, runtime telemetry, and API must use that record rather than
maintaining separate generated graphs. The API query provider supports the
whole projected set and can filter by `network_root`; the source includes the
network root, generated document URI, component paths, member output aliases,
units, and layout.

The generated browser row is a topology-aware entry point: a network with one
unit opens that unit class so the first diagram shows the real member graph;
multi-unit networks open the generated root wrapper so unit boundaries remain
visible. The root wrapper is still available through the normal Modelica source
and class path. Canvas navigation resolves the focused tab, and `FitCanvas`
consumes its deferred request inside the render pass using the actual widget
rectangle, so split root/unit tabs cannot silently fit the wrong view.

Policy changes belong in Rhai. The Rust side should only gather composed facts
once, invoke the selected policy, validate coverage and shape, and publish the
result. Validation includes rejecting undeclared root/unit causal ports,
promotions for outputs missing from the loaded member class, and overlapping
policy placements, so a policy cannot silently widen the USD-authored runtime
interface or publish an unusable diagram. A missing policy, unknown class,
malformed output, or incomplete unit partition is an explicit projection error.
It must not become an empty diagram, compiled-schema fallback, guessed class, or
legacy alias.

The source-root ownership change in `04e7f4555` is also important for generated
models: source-root classes are tracked by authored namespace, not by a
transport identifier, so later package members do not get compiled a second
time or accidentally collide with an already-owned class.

## Verification completed on `usd`

All commands below were run from `/home/rod/Documents/luncosim-workspace/usd`
with `RUSTC_WRAPPER=` and `-j4`.

### Telemetry and API tests

```text
cargo test -p lunco-telemetry --lib -j4
45 passed, 0 failed, 0 ignored

cargo test -p lunco-api --lib -j4
41 passed, 0 failed, 0 ignored

cargo test -p lunco-modelica --lib runtime_telemetry::tests -j4
5 passed, 0 failed, 260 filtered out
```

The telemetry tests cover channel-key identity, Modelica component identity,
same-name separation by entity, authored/command validation, per-channel rate,
deadband, retention, duplicate collapse, removal, stale-session cleanup,
diagnostics, and precise simulation time.

The Modelica runtime telemetry tests cover generated channel naming from
projected member metadata, retention of Modelica solver identity, state
retention without a plot, steady-time sampling, and clearing history on a new
solver session.

### Generated Modelica and USD projection tests

```text
cargo test -p lunco-usd-sim --test hook_synthesizer -j4
15 passed, 0 failed, 0 ignored (baseline before the topology-role edit)

cargo test -p lunco-usd-sim --test domain_projection_reader -j4
6 passed, 0 failed, 0 ignored

cargo test -p lunco-usd-sim --test usd_connection_derivation -j4
14 passed, 0 failed, 0 ignored
```

These tests prove that:

- a Rhai policy can be the synthesizer;
- Rhai can replace source emission, unit partition, and layout;
- malformed policy output is an authoring error;
- the synthesis result requires explicit `source`, `units`, both layout
  sections, `source_roots`, and `member_output_aliases` fields;
- repeated battery and solar members retain distinct Rhai-owned placements;
- facts describe the complete fixture graph, including classes, connectors,
  constants, and connections;
- the shipped default policy emits both executable topology and visual
  Modelica graphics;
- composed USD collection membership, source-class resolution, pending source
  state, and terminal failures are handled without class substitution;
- native USD connection paths derive generic simulation connections and clear
  when the authored connection is removed;
- the shipped balloon, sun-tracker, antenna, sandbox, lander, and force-wiring
  fixtures remain connected;
- actuator force geometry is projected through the registered
  `synth.actuator-wrench` policy.

The policy-specific contracts live in:

- `assets/scripting/tests/test_generated_acausal_policy.rhai`
- `assets/scripting/tests/test_generated_actuator_policy.rhai`

The acausal policy asset also contains
`test_topology_roles_put_sources_opposite_loads`, covering the typed
source/storage/load-bank algorithm. It can be exercised through the live
`RunRhai` path without rebuilding or restarting native tests.

The Rust test supplies composed facts, registers the same shipped policy used
in production, and invokes the Rhai contract. Assertions for generated source
shape, solver ownership, topology, layout, aliases, and presentation stay in
Rhai so policy changes do not require Rust test rewrites.

### Modelica parser/compiler checks already verified on this `usd` line

```text
cargo test -p lunco-modelica --lib -j4
272 passed, 3 ignored, 0 failed

cargo test -p lunco-modelica --test rumoca_chokepoints -j4
3 passed, 0 failed

cargo test -p lunco-modelica --test package_member_compile -j4
11 passed, 0 failed

cargo test -p lunco-modelica --test rumoca_api_coverage -j4
5 passed, 0 failed

cargo test -p lunco-hooks-rhai -j4
7 passed, 0 failed, 0 ignored

cargo test -p lunco-scripting --test rhai_test_harness -j4
4 passed, 0 failed, 0 ignored

cargo test -p lunco-assets --lib -j4
85 passed, 0 failed, 0 ignored
```

Formatting and production compilation were also verified:

```text
cargo fmt --all && git diff --check
cargo build -p lunco-luncosim --bin luncosim --features ui -j4
```

Both completed successfully. The production UI binary was launched on
API port `4106`; `/api/ready` reported `ready=true`, the actual solar-rover USD
scene was loaded through the typed command, `GeneratedModelicaSource` returned
`error=null` with the expected `connect(...)` star topology, and the generated
unit was opened from the Build perspective and captured through the typed
`CaptureScreenshot` command. The screenshot was inspected, and a typed
`ExecuteCommand`/`Exit` request terminated the process and released the port.

The Modelica compiler also has a regression proof for ordinary qualified-name
lookup: `LunCo.Electrical.Battery` is compiled without manually registering or
loading `LunCo`. The compiler discovers structured `assets/models/<Root>/`
packages from their `package.mo` marker, seats the referenced root on the
unresolved-reference path, and retries through the same standard root-segment
search-path mechanism used for any package. The synthesis schema requires an
explicit `source_roots` field, but root prewarming remains optional to class
discovery; it is not a library-specific workaround.

### Follow-up UI/runtime validation (2026-08-24)

The previous production session exposed a real recursive-lock defect: the
projection held `ModelicaEngineHandle` while calling a helper that attempted
to lock the same mutex. That path was removed. The UI/API readers now use
short `try_lock` reads, and root loading plus icon inheritance resolution run
on the compute task pool. A cold generated root loaded 77 native Modelica
documents, and the `PowerSystem` drill-in completed projection in roughly
24 ms after opening according to the production log.

The production binary was exercised on API port `4108` with the composed
solar-rover scene. The generated `PowerSystem` document reported live
`Battery.p.i`, `Motor_*.p.i`, and `YawHead__SolarPanel.p.i` state values, while
the inspected screenshot showed the authored battery, motor, and solar-panel
icons and the routed bus topology:
`target/generated-modelica-powersystem-icons.png`.

The generic canvas movement path is covered in source by
`SceneEvent::NodeMoved → ModelicaOp::SetPlacement → document patch/journal`.
The generic flow path now tests source-sign inversion, target-endpoint reads,
multiple declared flow variables, and the rule that ordinary scalar values do
not animate a wire. The path performs no per-frame string formatting and
walks each rendered route linearly.

## Test seam fixed in this handover

`usd_connection_derivation::lander_actuator_projection_uses_all_authored_force_geometry`
invokes `ActuatorWrenchSynthesizer` directly. It does not boot the production
scripting plugin, so it previously failed with:

```text
actuator-wrench is selected but its Rhai synthesis policy is not registered
```

The test now registers the shipped `synth_actuator_wrench` policy before direct
projection, exactly matching the production hook contract. This is a test
fixture initialization fix; it does not add a production fallback or move
policy back into Rust. The affected suite is now 14/14.

## What the above does not prove

The evidence above is source, AST, projection, API-contract, and unit/integration
coverage. It does not by itself prove the following visual/runtime cases:

- loading the real `summer-space-school` or `summer-space-twin` scene in the UI;
- seeing a populated telemetry browser for every camera, IMU, antenna, wheel,
  suspension, battery, solar-panel, motor, and lander component;
- proving that all live channels are shown once with no internal/archived
  duplicates in the current UI build;
- visually opening the generated Modelica diagram and seeing every actual port,
  wire, component, and animated energy-flow path;
- a production runtime screenshot proving non-zero motion, camera payload, IMU
  inclination, suspension state, and per-motor electrical/mechanical values;
- terrain material/lighting, NURBS antenna rendering, spawning coordinates,
  waypoint lighting, or camera perspective UI behavior.

Those need a production `target/debug/luncosim` session with an explicit free
API port, the real scene loaded, and checks through the typed API plus visual
inspection. A green `--validate` or a successful build is not runtime proof.

## Recommended acceptance pass on `main`

After integration, run the focused suites above first, then perform one
production-session acceptance pass:

1. Build only `target/debug/luncosim` with `-j4`.
2. Launch it on an explicit free port and wait for `/api/ready`.
3. Load the actual USD scene through the supported typed command/query path.
4. Query `GeneratedModelicaSource` and inspect the returned source, component
   paths, units, member mappings, and layout. Confirm the source itself contains
   the expected `connect(...)` topology and standard annotations.
5. Query `ListTelemetryChannels` and `QueryTelemetryHistory`. Check canonical
   names, group paths, units, Modelica identity, active state, and entity/path
   uniqueness. Confirm camera/IMU/suspension channels are present because the
   composed model actually declares them, not because Rust recognizes their
   names.
6. Subscribe through the typed telemetry API and verify samples carry the
   producing entity and simulation time.
7. Open the generated diagram and telemetry browser. For a single-unit network,
   confirm the browser opens the unit-level member graph; for a multi-unit
   network, confirm the root wrapper remains the first view. Check ports, wires,
   component labels, connection routes, and non-empty live values. For an
   electrical unit, verify at least one non-zero `Pin.i` edge produces moving
   dots and that a zero-current edge stays idle. Exercise `FitCanvas` while the
   drilled-in tab is focused.
8. Exit through the typed API, verify the process and API port disappear, and
   only then replace the session.

For integration bookkeeping, compare the branches before merging:

```text
git merge-base --is-ancestor 04e7f4555 main
git log --left-right --cherry-pick --oneline main...usd
```

The handover commit itself should be included once. Do not create a second
compatibility path for an API or generated schema that has already been
replaced.

## Do not regress

- Do not reintroduce Rust knowledge of battery, motor, solar, camera,
  suspension, rover, or lander classes into generic projection or telemetry.
- Do not generate a diagram independently of the Modelica source.
- Do not infer public telemetry names from Rumoca solver variable spelling.
- Do not publish archived history as active live telemetry.
- Do not hide missing policy, class, connector, source, or unit coverage behind
  an empty graph, guessed class, fallback compiler path, alias, or shim.
- Do not make tutorials open USD layers themselves; pass them the composed stage.
- Do not change Rumoca or other maintained-library workarounds in this handoff.
- Keep the production hook registration and direct-test registration aligned:
  a policy-owned synthesizer must have an explicit registered policy in every
  path that invokes it.
- Keep active native Rhai directories authoritative. Missing packaged asset
  directories select the compiled-in package, but an existing unreadable or
  empty editable directory must fail visibly rather than switching sources.
- Keep Modelica source-root diagnostics terminal for a load attempt: do not
  install a partial package after any member parse error.
