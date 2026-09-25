---
name: luncosim-architecture
description: Review or build a LunCoSim feature that crosses USD, Modelica, Avian, Rust, or Rhai. Use this when adding a reusable component, sensor, actuator, controller, generated Modelica network, USD schema, or runtime projection, and when removing legacy paths, shims, compatibility fallbacks, or custom fields that duplicate an OpenUSD standard.
---

# LunCoSim architecture

Use this skill before changing a reusable engine feature. Keep the authored
system declarative and composable, and make each concern live in its native
representation:

Before calling an architectural capability missing or impossible, use
[**capability-discovery**](../capability-discovery/SKILL.md). Search the relevant
skills and architecture docs, identify the standard or project owner, inspect
registrations and callers, check maintained dependencies, and verify the
production/API surface when relevant. Classify the result as found,
implemented-but-unwired, present on another branch/version, not found in the
searched scope, or externally blocked. Do not create a second owner, fallback,
or compatibility shim because the first search was incomplete.

| Concern | Authoritative owner | Runtime role |
|---|---|---|
| Scene structure, identity, topology, frames, connections, component parameters | USD | Rust projects the composed stage; it does not invent missing topology |
| Continuous equations, state, control laws, filters, physical networks | Modelica | Rumoca compiles and steps the model |
| Rigid-body collision, contacts, forces, torques, joints | Avian through USD-authored physics | Rust exposes the engine's generic mechanics and executes them |
| Mission phases, events, policy, objectives | Rhai or behaviour trees | Task/event orchestration in production; `on_tick` is test-only for sampled verdicts |
| Engine mechanisms, projection, scheduling, hot paths | Rust | Generic implementation; no vehicle- or sensor-name special cases |

For cross-domain execution, read
[`62-deterministic-runtime-and-async-boundaries.md`](../../docs/architecture/62-deterministic-runtime-and-async-boundaries.md).
Keep parsing, source resolution, and immutable preparation off the UI/fixed
schedule when inputs can be captured by revision. Admit results only at an
owner boundary in stable identity order. Keep live-world hooks and physics
inside their deterministic schedule. A Rhai scenario that depends on Modelica
ports or events declares the participating entity ids in
`simulation_dependencies(me, ctx)`, where `ctx` is the validated scenario
parameter map. The hook returns a map with `modelica_entities: [ids]` and
`required_inputs: [#{ owner, identity }]`. The owner resolves Modelica ids once
per source/parameter revision and adds them to the shared causal barrier.
Required input keys refer to producer namespaces registered in the generic
`SimulationDependencyStates` resource. The scenario holds its existing
`ScriptPreparation` key until every required input is Ready; it retries only
after an owner-published state revision, and the visible wait reason names the
input. A missing producer is a terminal diagnostic; a producer's Failed state
reports its errors. Omitting the hook declares no Rhai dependencies. The plan
runs before mutable top-level initialization, so derive it from `me`, scenario
parameters, and read-only world queries. Top-level initialization runs in its
own `Initialization` phase after the plan commits. Dependency planning may
resolve identities but cannot access live ports, issue commands, mutate the
world, or emit events. Invalid or unresolved Modelica ids fail that source
revision with a diagnostic. Unbarriered port access and Modelica event delivery
fail visibly. Physics operations that can accumulate into shared bodies use
`PhysicsOrderKey` from the instance root and authored prim path. Joint solving,
motor warm-start, custom prismatic correction, raycast and jointed tire forces,
and raycast mass-property folds consume stable key order. The production
`multi_rover_stress_20` Rhai gate compares physics and Modelica state across
repeated single-thread and default-pool runs; two four-run matrices matched all
recorded snapshots on the same build. This is fixture-specific evidence. Do not
claim whole-simulation replay determinism while the remaining reviewed gaps
are open.

Scenario actor compile submission, completion commit, and hook execution use the
source-owned `GlobalEntityId` component directly; the Update-synchronized API
lookup index is not an ordering source. A local-only host without that component
uses its Bevy entity key only within the current World and has no cross-session
replay identity. Author observable multi-actor ordering checks in Rhai scene
tests; reserve Rust tests for the generic ordering and async commit mechanisms.

For file-backed documents, use `PreparedFileBacked` with the shared
`DocumentRegistry::open_prepared_file` path when source parsing moves to a
worker. The registry remains authoritative for path identity, dirty-document
preservation, and lifecycle events. Owners fence results against the exact
source/document revision before committing them. The default USD Twin scene
uses this path for native USDA parse and persistent-source serialization;
runtime sidecar restore and the browser worker transport remain separate open
boundaries.

Lifecycle work that changes authoritative scene state participates in
`lunco-core-runtime::SimulationProgress`. Acquire with a typed owner/operation
key, carry that identity through async preparation, and release it only after
the owning terminal result has been committed. Scene load/restart/clear use
`SceneTransitionId`; same-path transitions still have distinct identities.
The coordinator advances the shared scene generation only for the matching
successful transition and emits `SceneTransitionCommitted`. Scenario and other
Twin-scoped cycle owners read that committed generation; do not maintain a
subsystem-local scene counter or arm lifecycle work from a stale completion.
Lifecycle hooks receive a discrete owner context keyed to the exact transition.
For example, `scene.time.select` runs as `Twin/Lifecycle/Preparation` with no
elapsed clock, and its typed result carries the same `SceneTransitionId` through
application. The time owner ignores stale results and terminal edges after a
replacement begins. One-shot policy inspection uses `Application/Repl/Evaluation`.
Keep participant readiness and `PhysicsHolds` in their owners: they control
local/world physics admission, while `SimulationProgress` controls whether the
shared causal tick may advance. `UsdSceneRuntimePlugin` installs this shared
resource when selected and holds active reference spawns on the mounted primary
scene through closure preparation and live ECS projection. Preview and additive
mounts retain diagnostics without pausing or faulting the primary simulation. A
terminal primary reference failure retains its exact hold and publishes a path
diagnostic plus `RuntimeFault` until scene teardown; inactive or removed,
nonfaulted operations release their own keys. Surface the active wait reason
through the existing status bus. Async data that changes authoritative physics,
including a mounted DEM download/build, also holds an exact terrain entity key
through the committed collider/oracle result; on web the hold spans the coarse
preview and full worker result. The DEM bridge and readiness scan run in
`PreUpdate` before `TimeSpineSet`, including while a Twin manifest scan is
pending, so the first eligible fixed tick cannot precede terrain admission.
UI and presentation schedules remain live.
Physics readiness then admits bodies and joints on fixed steps. An active USD Modelica participant remains
readiness-held through its first successful communication point; a compiled,
intentionally paused model is ready without being stepped. Keep that participant
readiness separate from the shared `SimulationProgress` lifecycle gate.
First-compile intent is admitted by `request_modelica_compiles` in the lifecycle
cycle while solver stepping stays in `FixedUpdate`, so compilation can be
requested while virtual simulation time is held. The Modelica execution owner
consumes typed `CompileRequested` intent and owns document resolution and
worker dispatch; the UI command only chooses the class and publishes intent.
On native, source-root installation, compile requests, Reset, parameter updates,
and cache-invalidating Step auto-init share one single-owner Rumoca actor FIFO.
Results return asynchronously and commit in submission order; actor requests
and solve preparations use bounded admission. A Step that requires a rebuild
resumes only after the matching session and library generation commit.
Compile results must match both worker session and captured document
generation. A result for an edited source revision is discarded, and an active
model keeps its compile-run intent until a current result commits. The execution
owner reconciles active causal models into `SimulationProgress` before
`TimeSpineSet` and releases each exact entity key only after the matching
compile result has been committed. Intentionally paused and noncausal models do
not hold world time. Their first normal co-simulation step uses the per-step
barrier after activation. The root USD loader composes the available dependency
closure before publishing the stage asset, and scene admission ends after
structural projection. CPU mesh construction stays on the presentation path.
Keep owner-specific clock gates separate; production acceptance must still
prove that the first authoritative fixed tick observes each dependency declared
by the scene and scenario, independent of worker completion order.

For engineering requirements, keep the same split at the numerical boundary:
SysML owns typed intent, units, normative tolerances, and requirement/
verification identity; USD owns realized geometry and standard scene facts;
Modelica/Rumoca owns equations and continuous state; Rhai owns the selected
mechanical relation, orchestration, and evidence; Rust owns only reusable,
hot, type-safe f64 mechanisms. The authored
`assets/scripting/tools/mechanical_relations.rhai` library is the extension
point for CAD-like predicates such as distance, coincidence, parallelism,
under/clearance, mirroring, and symmetry. Do not grow a Rust registry of
relation names or product-specific checks.

Use the existing Rhai standard math surface for ordinary scalar operations and
the native Rust bridge for f64 vector validity, dot/cross, clamped cosine,
angle, and native component access. Keep native values in hot loops; arrays are
only explicit interchange boundaries. Use typed predicates (`f64_only`,
`array_is`, `map_is`, `string_is`, `vec3_is_native`) instead of string-based
runtime type protocols. Numerical settings are explicit per dimension and are
resolved once per report/solve; they do not replace a source-owned SysML
tolerance or become a global epsilon.

For Avian-backed physics, keep one numeric admission contract at
`lunco-physics::avian_backend`. The BigSpace bridge owns lifecycle admission of
f64 poses and collider support geometry before the Avian step, `GridSpatialQuery`
reuses the converted-ray predicate, and USD projection reuses shape/AABB and
leaf structure predicates before ECS insertion. Convex colliders are checked by
their actual support points; composite or non-convex shapes use the conservative
AABB boundary. A failed live invariant raises the
shared scene-scoped runtime fault and gates the remaining nested physics
phases. Do not turn these checks into per-call query fallbacks or lint-only
warnings.

### Rhai task callback contract

Task leaves accept anonymous closures (`|me| ...`) or named script callbacks
(`Fn("name")`, declared `fn name(me)`). `me` is the host entity id; both forms
receive persistent state as the driver-bound `this`. The native task driver owns
progression and state transfer. Rhai map parameters are value/copy-on-write
values: a helper that assigns a state field must return the updated map, and
the lifecycle callback must assign it back to `this`. Do not rely on helper
side effects to persist task state.

## Plugin and crate layering

Use a domain `CorePlugin` for headless-safe state, lifecycle, commands, and
runtime mechanisms, then add a separate UI plugin only for panels and visual
presentation. Do not create a UI plugin merely to host API queries. If a
provider belongs to a core domain but importing the API crate would create a
dependency cycle, put the provider in a small `*-api` adapter crate and have
each API-capable composition root install it explicitly. Keep the data/core
crate independent of transport and presentation layers.

For dynamic providers, keep `lunco-hooks` as the single reflected contract and
invocation owner. `HookInvocation` carries the typed runtime context with the
validated values. `lunco-hooks-plugin-api` owns the stable edition-2024 ABI v2
and typed invocation wire helpers; `lunco-hooks-native` owns `libloading`, unsafe
admission, callback limits, and exact-registration teardown. Enable that path
only from an application composition feature. A Twin must explicitly approve a
provider with a Twin-relative `[[native_plugins]]` manifest entry; USD and Rhai
source do not load shared libraries. Native code is trusted process code, so
untrusted bundles require deployment-level signature and isolation controls.

A provider may implement only an existing reflected, installable hook and must
return typed data or a validated action plan. It must not mutate ECS/USD or add
a second domain registry. The owner invokes it through `lunco_hooks::invoke`
with a typed `HookInvocation`: cycle-owned callers supply their runtime
context, while discrete boundaries explicitly use an unclassified context.
The owner validates the result and reports a fault or unconfigured state
according to the hook contract. Policy manifests may mark an optional
feature-owned hook with `skip_when_hook_unavailable`; the runtime reports that
capability in `policy_status().unavailable`, while compile and activation
failures remain in `failed`. Put an eventual expensive terrain provider at the
consumed `lunco-terrain-bake` kernel boundary; keep `lunco-terrain-core` projection-free
and leave `lunco-terrain-surface` as the runtime projection owner. See
[`native-hook-providers.md`](../../docs/architecture/native-hook-providers.md).

Keep the workbench split at the dependency boundary: `lunco-workbench-core`
owns renderer-independent panel/menu/perspective/registration contracts, tab
navigation, source-view commands, scene display state, pending tab-close state,
and the published `WorkbenchSnapshot`; `lunco-workbench-widgets` owns
shell-independent egui controls;
`lunco-workbench` owns `egui_dock`, `bevy_egui`, viewport rendering,
persistence, source editing, and command observers; and
`lunco-workbench-guided-ui` owns the optional Rhai-driven HUD, spotlight,
coach-mark, and guided-recovery surfaces; and
`lunco-workbench-browser` owns the optional Twin/Files panels, browser state,
and built-in filesystem/library sections without depending on the concrete
shell. Rename payloads belong to `lunco-doc-bevy` or `lunco-workspace` according
to the identity they address; the shell retains only picker/execution
observers. Domain UI crates implement contracts from the core crate, read
layout facts from the snapshot, and depend on the concrete shell only when
they use those presentation services. Do not expose or consume the shell's
private `WorkbenchLayout` outside that crate.

Keep application-edge presentation separate from the reusable shell:
lunco-luncosim-presentation owns the final status/environment, terrain-horizon,
USD camera/light, capture, and scene-presentation bridges, while
lunco-luncosim-ui owns window/plugin composition and installs that package at
the boundary. Native updater startup and its rendered surface belong to the
optional `lunco-updater` package, not to the ordinary UI closure. Celestial
body projection and cadence belong to `lunco-celestial-spatial`; ordinary
moving scene objects use standard USD `timeSamples` through
`lunco-usd-bevy-animation`. Do not create a mission-only trajectory component
or clock when composed USD animation expresses the motion.
Celestial state uses the one `lunco-time::CelestialTime` sample, an affine
child of `WorldTime`. It drives ephemerides, body rotation, semantic SunState,
lighting, shadows, geometry queries, and the environment values consumed by
Modelica. A rate up to 100,000× leaves Avian and Modelica at their ordinary
fixed-step cadence. From lunar ground, Earth stays near one sky position
because the Moon is tidally locked; its axial spin still advances the day/night
pattern. BigSpace propagates both the changed Earth grid pose and rotation to
`GlobalTransform`; one cadence gate commits the CelestialTime sample it read.

### Runtime scopes, cycles, and publication boundaries

Use the shared `lunco_runtime_context::RuntimeScope` and
`lunco_core::RuntimeCycleSet` vocabulary when
placing a cross-cutting system. `Core`, `Application`, and `Twin` describe
ownership; `Lifecycle`, `Simulation`, `Interaction`, `Command`, `Repl`, `Telemetry`,
`Ui`, `Presentation`, and `Visualization` describe cadence. These are schedule labels and typed route
metadata, not a new global event bus. Twin-owned resources and completions must
carry their mount generation and be retired at Twin teardown.

Cycle labels do not create independent clocks, CPU isolation, or execution
cadence. Rust plugin composition decides which capabilities a host installs;
Bevy's owning schedules provide the actual execution boundary. A typed
`RuntimeExecutionContext` carries the current owner route, cycle, phase, clock
sample, logical sequence, and optional event producer stamp into synchronous
Rhai calls. Scenario hooks and one-shot REPL/tool calls use their owning
contexts; `execution_context()` exposes a read-only Rhai map. Generic hook
calls carry `HookInvocation`; nested `invoke_hook` forwards its active context,
and an isolated Rhai hook reads it from the immutable `runtime_context` map.
`twin.lifecycle` uses the mounted Twin's `TwinId` as its `Twin/Lifecycle`
generation, with explicit `Start`, `Event`, or `Stop` phase and no elapsed
clock; `policy_status().lifecycle.runtime_context` exposes the exact stamp.
Rust call sites without a classified owner use the explicit unclassified hook
entry point. The physics escape policy is a scheduled core simulation hook and
receives its `Behavior` context from `SimTick` and `Time<Fixed>`; its authored
policy rejects off-cycle calls. The scheduled `readiness.action` policy uses
`Core/Simulation/Behavior` with `Time<Fixed>` and the latest `SimTick`, and its
authored policy rejects calls from other cycles. `sim_tick()`,
`dt()`, and `elapsed_seconds()` reject calls outside the simulation cycle as
invocation errors, while missing mandatory simulation clock resources remain
runtime faults. An unclassified invocation has no route; Rhai exposes its
scope, cycle, and generation as unit instead of inventing an owner. Add a
separate schedule driver only when a cycle needs independent cadence or
overload semantics, and keep expensive calculations off
the UI/physics-critical thread.

Native immutable preparation uses `lunco_core_runtime::AsyncWorkAdmission`;
do not add another per-crate priority queue. Each request carries its scope
generation, stable owner identity, source revision, and operation id. Priority
selects which queued job starts; the owner still validates and commits its
typed result at its own boundary. WebAssembly's Bevy async-compute pool runs
cooperatively on the browser main thread, so expensive web work needs an
explicit worker transport rather than this native dispatcher.
Serialize each Modelica step's input assignments in variable-name order;
never let hash-map iteration choose a worker command's observable order.
File-backed Rhai source assets are parsed and const-folded by the asynchronous
asset loader, which publishes their canonical id, exact text, AST, and literal
import dependencies together. Activation owners commit the complete loaded
dependency graph before binding or starting a source; Bevy can report graph
readiness before its `Added` messages are consumed. Startup/Twin tools and the
prelude reuse that AST; scenario workers reuse it only when the source bytes
match, and parse inline roots or uncommitted sources inside shared admission.
Do not compile loaded tool or prelude source synchronously during registration.
The asset publisher retains each loaded `AssetId`'s canonical URI and retires it
on `AssetEvent::Removed`, because `AssetServer::get_path` may fail after the
last handle is released. Ignore the earlier `Unused` edge, and prevent a stale
asset removal from deleting a replacement source with the same URI.
Twin SysML analysis follows the same boundary: `twin.lifecycle` selects the
checked manifest source set, `PrepareTwinSysmlAnalysis` loads source assets and
prepares one immutable snapshot through shared admission, and runtime
`twin://` queries read only the current Twin-id/root/operation result. Pending,
failed, and unprepared snapshots remain visible. This read-only active-Twin work
uses `Interactive` priority and never holds `SimulationProgress`; a scenario
that requires SysML facts declares the typed owner key in its dependency plan.
Open SysML documents use the same bounded `Interactive` admission. Lifecycle
events capture immutable source/origin facts, and `SysmlDocumentAnalyses`
commits only for the exact current document generation and origin URI.
`InspectSysmlDocument` returns `analysis_state` as `pending`, `ready`, or
`failed`; pending diagnostics and error fields are null. Rhai editor verification
must return a retryable pending result and must not call it clean. These editor
analyses do not hold simulation progress. Browser worker transport is explicit
and remains unsupported by this native dispatcher. The Twin analysis producer
registers `sysml.twin-analysis` and publishes each mounted Twin's state under
its exact name. A scenario that needs those facts lists
`#{ owner: "sysml.twin-analysis", identity: twin_name }` in
`required_inputs`; only that scenario's activation hold waits for the async
analysis. The analysis worker itself remains read-only and never acquires a
world-time hold.
Telemetry samples are captured with the fixed tick, then delivered through the
plugin-owned bounded `Telemetry` cycle. Never run subscription, retention, or
logging observers inline with fixed physics; report queue loss through the
telemetry status query and keep simulation progress independent.
The GUI installs `TerrainSurfaceVisualizationPlugin`; server and scene-test
compositions keep terrain physics/query support but omit camera-driven LOD,
visual-map baking, and overlays entirely. A system hidden behind a server-mode
`run_if` still exists in that schedule and is not equivalent to omitting it.
Application builders install the selected capabilities automatically; authors
should not assemble Bevy schedules by hand. Rhai currently declares peer
selection through `@scope host|client|both`, with simulation as its supported
timing. An unknown scope or unsupported timing disables only that scenario and
publishes one document error for its source generation instead of breaking the
host, defaulting to host, or guessing a clock. Cycle and clock selection come
from the Rust owner, not a script directive. A callback error remains visible
and local to its owner; required authoritative hooks hold/fault their owner.
Never panic or silently report success for a failed hook.

Every cycle boundary should expose low-cost aggregate duration, work and queue
counts, and missed-budget/overload counts through the existing diagnostics
owner. Use these to target a Tracy capture, then measure frame responsiveness in
a separate unprofiled run. Do not feed a per-frame diagnostics firehose through
the simulation telemetry sampler.

Keep telemetry's two lanes distinct: authoritative events used by Rhai retain
their producer tick and deterministic delivery order; continuous samples are
bounded observations of committed state. The fixed sample boundary captures
due channels into a small typed record. A cached channel plan still requires a
fixed-path walk and live port reads; its samples then go through a bounded
post-simulation cycle. Logging, subscriber fan-out, formatting, serialization,
persistence, and UI plot decimation run outside the physics transaction.
Reducing fixed-path sampling work and measuring its effect on throughput remain
open owner-level work, not a completed guarantee.

Commands and one-shot Rhai/REPL evaluations have independent application
clocks. Record command cadence from the shared `CommandOccurred` publication
and REPL cadence from actual `drain_world_scripts` evaluation. Both use the
wall clock, never simulation time; a queued script is not counted as evaluated
until the exclusive REPL owner runs it. Publish their sequence and timing
through `ApplicationCadence`/`application-cadence` so consumers do not add
their own timers or infer cadence from render frames.

World-bound one-shot Rhai requests remain serial because their verbs read and
mutate the live World. The REPL owns a bounded FIFO (64 pending requests by
default), evaluates one request per `Update`, and applies a 100,000-operation
ceiling per invocation. Queue overflow is a terminal command error. Keep
scenario and hook execution in their owning clocks; do not parallelize callbacks
that still use live-world access.

Publish each fact through its one domain boundary before adding a consumer:
scalar co-simulation endpoints use `PortRegistry`; presentation-ready values
use `EngineExposures`; lifecycle occurrences use typed events or revisioned
resources. Status bars, authored HUDs, telemetry adapters, API readers, and
recorders consume those publications. They must not independently scan engine
diagnostics, query a physics owner, or introduce a widget-specific registry.
The fixed-step and rollback co-simulation paths share one propagation function
and compiled cache; transform propagation remains a separate spatial owner.
Rollback is an instantaneous replay cycle over recorded inputs, so systems in
`RollbackReplay` must not be gated by the live virtual clock's paused/running
condition. Keep its actuation ordering aligned with the fixed path and test the
real replay schedule while `Time<Virtual>` is paused. This body prediction
replay does not reconstruct whole-session commands, Rhai state, or Modelica
state; do not claim whole-simulation replay determinism from it.

For a `ShaderLook` with `vertex_shader`, treat the fragment and vertex sources as
one linked material contract: both stages read the same `@binding(0)` uniform
block, so their `Material` fields, order, and WGSL types must agree. The
fragment schema is the packed layout; do not invent a second vertex schema or
silently select a replacement shader. Validate the requested stage positively:
`sourceAsset` needs `@fragment` and `vertex_shader` needs `@vertex`; a combined
WGSL module is valid for either role. A missing or invalid stage is a structured
runtime diagnostic and an unbound material. Rust must not install a
`StandardMaterial`, neutral shader, or another guessed source as recovery. If a
scenario needs recovery, make it an explicit Rhai policy that can be replaced
or disabled without rebuilding the renderer. Add a cross-file ABI test when
maintaining a multi-stage shader pair. See
[`shader-layers-and-params.md`](../../docs/architecture/shader-layers-and-params.md).

Treat an illumination-bearing grayscale orthophoto as measured imagery, not
intrinsic reflectance. The native `lunco-assets-processing` `kind = "albedo"` pipeline
removes its low-frequency illumination field, anchors local detail at a
neutral material base, and sRGB-encodes the resulting linear colour for the
8-bit PNG contract. The texture loader decodes that material back to linear;
terrain shaders use it directly and have no orthophoto compensation function.
`kind = "map"` remains an analysis/display contrast map and must not be bound
directly as `inputs:albedo_map`. When `weight_albedo` is authored, it owns
terrain colour variation; scale procedural dust/mottle by `1 - weight_albedo`,
while keeping relief normals, roughness, ambient occlusion, and photometry
independent. Heavy raster math stays in Rust; Rhai assembly policy selects and
authors the standard USD material inputs.
The packed surface map's G channel is ambient occlusion: route it through the
shared `terrain_surface_occlusion` helper into Bevy's
`PbrInput.diffuse_occlusion`, never into base albedo. AO is indirect-light
visibility; multiplying it into albedo creates broad false colour patches and
darkens direct sunlight.
The render-side `ShaderLook` binder owns event-driven preparation of filterable
authored RGBA8 maps: it deduplicates off-thread mip generation by image asset
version,
filters colour in linear light, averages scalar maps linearly, renormalizes
normal vectors, and then enables trilinear/anisotropic sampling. Do not skip a
zero-weight map at load time: authored weights are live inputs, while mip
preparation is the separate renderer-owned image lifecycle.

Project-owned persistence policy belongs to the active Twin manifest's generic
settings boundary. A domain may define one namespaced scalar key and expose it
through the existing `SetTwinSetting` path; it must not add a global settings
section or a second cache reader/writer for the same artifact.

The local avatar is a runtime kinematic camera embodiment. Its collision
movement uses Avian's existing `MoveAndSlide` query against standard
`UsdPhysics` colliders in `ActivePhysicsFrame`; it does not need a second USD
body schema or a separate collision representation. An explicit Twin setting
may select a documented unsafe policy, but the movement owner reads that
setting directly and remains safe when the setting is omitted, malformed, or
the Twin closes.

The shared semantic input contract lives in `lunco-control-core`, while the
persisted device-to-intent map lives in `lunco-input-core`, and is not
avatar-only: the workbench
owns one app-level local intent surface for editor actions when an isolated
preview has no avatar. Shared actions such as `CancelIntent` read that surface
through the same `InputBindingsSettings` map, while avatar control continues to
use its own surface; neither path may introduce a raw-key or duplicate binding.
The Workbench may use `InputBindingsSettings::input_map_or_empty` only while the
application-owned authored defaults are loading: it must publish the rejected
settings as a status-bar warning, keep the UI host alive with no active
bindings, and replace the empty map when the authored projection becomes valid.

## Source-backed program attachment

`AttachProgram` is the one authoring boundary for binding a `.mo`, `.py`,
`.rhai`, or behaviour-tree source to an existing USD prim. Rust owns the
generic command and lowers a complete `ProgramAttachSpec` into one journalled
USD change set. The spec carries the source asset, explicit scalar inputs and
outputs, defaults, native USD connections, and the explicit `realtimeSafe`
promise.

The Models palette, Rhai `assembly_edit::attach_program(...)`, HTTP callers, and the Assembly
editor all use this command. None may insert an ECS marker or maintain a
second program registry. An empty port contract is a valid source-only
attachment, but it is not a running scalar cosim participant; the author must
declare the interface before wiring or stepping it.

### Generic authoring evidence

Authoring review is a Rhai policy over generic typed substrate. The shipped
`authoring_inspection` library owns candidate comparison, diagnostic grouping,
diagnostic navigation, unified inspection records, and inspection-mode policy;
it does not know about rovers, landers, or other product nouns. Rust only
provides the reusable mechanisms: document-scoped `QueryUsdPrim` and
`InspectUsdDocument` reads, exact preview selection/framing, generic
`SetDiagnosticLayers`, and settings-backed view-only camera presets.

Keep the ownership split explicit:

- USD remains authoritative for identity, topology, standard visual/collision
  facts, joints, frames, materials, connections, and provenance.
- Rhai chooses the affected paths, groups findings, decides which diagnostic
  layers to show, and composes the evidence record for a human or AI review.
- The Editor owns selection and preview leases; `FrameUsdPreviewSelection`
  frames one exact composed path and must not fall back to a whole-stage frame
  when the requested path is unavailable.
- Shared settings own persisted view-only camera presets. Presets never become
  USD camera prims, journal entries, or another scene graph.

Use exact `DocumentId`, `UsdPreviewId`, `UsdPreviewViewId`, edit target, and
projection generation at every boundary. A document-local Editor fork may be
queried through its composed document stage even when it has no mounted Twin
projection; a stale mapped Twin document must still fail visibly. Missing
collision or provenance is an explicit diagnostic record, not a fabricated
default. Keep positive and negative contracts in the production Rhai scene
gate; do not duplicate these observable assertions in Rust unit tests.

### Shared asset catalog discovery

Asset enumeration belongs to `lunco_assets_runtime::discovery` and runs through the
shared asynchronous catalog listing owned by `lunco-scene-catalog`. USD,
WGSL, Modelica, and Python projections are published from one root snapshot;
they must not add a second filesystem walk or a UI-thread scan. A new
manifest/Twin snapshot advances the listing generation, reopens the USD read
set, and drops older metadata completions. This keeps a Twin opened during an
initial scan complete without allowing stale work to populate its catalog.

Asset provisioning follows the same dependency split: `lunco-assets-datasets`
owns declarations and lifecycle state, `lunco-assets-transport` owns native
HTTP retry/resume, `lunco-assets-download` owns verification/extraction and
atomic installation, and `lunco-assets-processing` owns native decode and
baking. `lunco-assets` is only the explicit Bevy worker/CLI composition root.
Its `ProcessorRegistry` selects heavy Rust implementations by authored
`ProcessConfig.kind`; Rhai owns dataset selection and sequencing, with extra
processor values carried in `process.parameters`. Do not grow a central Rust
match or make ordinary runtime readers depend on this provisioning stack.

For derived 3D annotations such as a waypoint route, keep one reusable
presentation tool between authored/runtime facts and presentation consumers.
Resolve authored identities through the authoritative binding map and write the
derived USD view to `@view@` only when the route changes or the view is first
materialized. Keep durable route edits in `@runtime@`, separately from the
source scene. Use standard USD geometry and the existing renderer for depth and
occlusion; do not create a Twin-specific ribbon prim or a per-frame document
edit. A reusable route may be a separate composed USD plan; the tool follows
the selected program's canonical parent scope rather than assuming `/Route`.
The disposable view must use the typed transient USD projection command, so
it is generation-checked and OpenUSD-backed without entering authored undo,
save, or Twin journal history. Marker-root placement, annotation geometry,
labels, and look remain separate owners. Stable frames must do no route
parsing, binding lookup, mesh generation, or marker writes; camera-dependent
label projection is the only remaining per-frame presentation work.

The same boundary applies across document domains: user Rhai/Modelica edits
use their `DocumentHost` operation path, while file-backed, USD-embedded, and
generated source refreshes use the shared `FileBacked::reload_base` contract.
External refreshes advance the live generation and invalidate consumers but do
not create a second undo/journal entry. Never replace a host to hot-swap source;
that discards history and breaks the canonical document identity.

Failure-path acceptance must use an owner-local, transient fixture or typed test
command. For example, the Scenarios menu may inject its unavailable presentation
state without removing or replacing `TwinRoots`, the active Twin, or scene data;
the default production path remains unchanged and the detailed cause still flows
through the shared `StatusBus`.

Repeated presentation solvers must also be value-idempotent: compare derived
`Transform`/`CellCoord` values before assignment. Bevy marks mutable component
access as changed even when the value is equal, and BigSpace consumes those
signals for dirty-subtree propagation. Guarding an equal write at the producer
is part of the ownership contract; it is not permission to hide a real dirty
input or to add an alternate propagation path.

Use the same owner-local revision/cursor shape for other stable projections:
the Modelica document registry wakes engine sync, telemetry producers pace by
their authoritative model/fixed time and capture a live `GlobalEntityId` in the
render-free `SignalRegistry` before a source can become archived, behavior target paths are cached per
entity and invalidated by authored XML or active-frame ancestry, terrain
curvature reacts to its input components, and globe LOD caches pure selection
until camera/LOD/handoff/residency inputs change. These cursors suppress work;
they do not become a second data source or a compatibility fallback.

Apply this contract to all camera pose owners, including shared interaction
easing, mounted USD followers, cinematic path followers, and the persistent
camera origin. Camera selection/mode policy stays in the application; BigSpace
owns only precision representation and derived transform propagation.

Celestial body frames, the sky-time readout, and celestial geometry queries use
`CelestialTime`. Terrain, stations, and links remain attached to their physical
body frames, which follow that same sample. Unbound USD animation uses the
interpolated physical-time sample. Keep each station and marker on the one
body-fixed grid; do not add a parallel presentation grid or marker copy.

Scenario telemetry is collected only while `ScenarioExecutionGate` is open.
That gate waits for all initial scene readiness holds because scenarios may
reference entities outside their owner subtree. After admission, entity holds
idle only scenarios inside the held owner subtree. Fixed-step delivery after
`SimTickSet` releases only events stamped before the current tick; paused
`Update` delivers discrete events without advancing that tick. A new scenario
reads current owner state in `on_start` rather than replaying events from before
its lifecycle. Clear the outgoing scene's pending batch when a scene transition
closes the gate.

Workbench perspectives publish scene visibility as layout intent. A perspective
that uses the full window as its 3D presentation must opt into
`Perspective::scene_visible_when_docked()` so opening a transient side or
bottom panel does not deactivate the selected scene camera or paint the themed
backdrop over the whole frame. Central editor perspectives remain hidden unless
their slot intent includes `ViewportPanel`; the Workbench still never writes
`Camera::is_active`, and `lunco-usd-bevy` remains the sole camera reconciler.

### Temporary diagnostic visuals

Temporary camera/collider/dynamics diagnostics are runtime presentation, not
USD facts. Reuse the existing `Gizmos` systems and one Twin-scoped
`DiagnosticVisualLease` store: Rhai/API/UI selects an explicit target and
policy, while Rust resolves `SceneViewport`, `StageView`, Avian collider
realization, BigSpace render poses, bounded snapshots, and lifecycle cleanup.
Do not add a diagnostic USD schema, temporary physics entity, per-frame USD
edit, second camera selector, or global name-only registry. Handles must carry
the Twin/root/mount generation and become stale on `SceneTeardown`, reload,
target deletion, or `TwinClosed`; missing targets and unsupported shapes are
visible errors. The draw pass reads finalized render transforms and never writes
physics or scene state. Existing separate debug toggles must converge on this
lease boundary when the feature is implemented rather than gaining another
toggle API. See
[`docs/architecture/temporary-diagnostic-visuals.md`](../../docs/architecture/temporary-diagnostic-visuals.md).

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
The editor's path-selection command is `SelectUsdPrim`: it requires the
focused `UsdPreviewId` and resolves through that lease's stage and preview-root
hierarchy.

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

Light shadow intent follows the same standard-schema boundary: read
`UsdLuxShadowAPI.inputs:shadow:enable` from the composed stage for every light.
Application possession or graphics settings must not overwrite that authored
intent; renderer-owned resource budgets publish facts and let the authored Rhai
render policy report unmet limits, never suppressing authored casters at the
admission boundary.

Scene-root `UsdPrimPath` values may be empty until the stage is parsed. Resolve
that sentinel through the shared USD `defaultPrim` resolver before any domain
projector reads the path; visual and celestial projection must not each invent
their own deferred-path handling or permanently mark an unresolved root.

## Start with the standard-schema audit

Before creating `LunCo*API` or a `lunco:*` property:

1. Inspect the vendored OpenUSD schemas in `crates/lunco-usd-document/schema/core/` and
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

After validating a generated source, publish it with its parsed interface and
link it to the normal Modelica document. Lifecycle compile admission dispatches
the compile once that document is current, using the same generation/session
fences as authored models. USD projection must not send a direct worker compile.

Scene lifecycle projection, stable entity identity assignment, and API/path
index publication run in that order in `PreUpdate`, before `TimeSpineSet` can
release simulation. Do not add a separate startup identity pass or readiness
poll; a projected reference must resolve by path on the first resumed tick.

Keep acausal conservation connectors (`Pin`, `HeatPort`, `FluidPort`, `Flange`,
and equivalent domain connectors) inside the root whose solver owns their
algebraic equations. A typed scalar USD connection between two roots is causal
and may have a macro-step or one-step delay. It is not an acausal `connect()`.

USD connection projection caches immutable endpoint facts by composed-stage
generation and runtime-instance identity. Reconciliation retains unchanged
`SimConnection` entities and bindings; edited edges use the normal add/remove
lifecycle. Endpoint lifecycle observers, USD edits, and authority changes feed
the shared `UsdWiringDirty` latch, so stable updates do not scan endpoint
populations for `Added<T>` filters. Domain source-class completion invalidates
only roots that use that source. A root is not synthesized until every
referenced member class has a terminal verdict, avoiding repeated graph
extraction across asynchronous asset arrivals. Scene teardown clears the
scene-owned reverse index, pending candidates, and in-flight synthesis tasks;
resolved member-class facts remain asset-owned and reusable. Cosim source discovery and Python
readiness share one lifecycle-coalesced pending-prim set rather than probing
all USD prims for unprocessed markers on stable updates.

Synthesis hooks run with the owning `Twin/Lifecycle/Preparation` context and
active-or-committed scene generation, without an elapsed clock. Capture that
typed context before async dispatch and pass it unchanged to the hook; the
synchronous live-projection path uses the same contract. Fence async results
by that Twin generation and the owning USD generation or prepared plan. Keep
the policy's context check in Rhai and cover off-cycle rejection plus
generated-source publication in a production scene test.

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
  `assets/models/<Root>/package.mo` and Twin-declared `[modelica].paths` are
  ordinary Modelica search-path roots: the compiler/editor discovers the root
  segment of a qualified reference generically and loads that package through
  the shared Modelica engine. A Twin without that section derives package roots
  from its indexed `.mo` files. Each live compile admits required roots through
  the `LoadSourceRoot` worker path before `Compile`; file-backed source assets
  derive requirements from their prepared AST interface, and generated-model
  policies return the required `source_roots` manifest. That manifest does not
  replace composed USD facts for class discovery, and Rust must not name a
  particular library. The live worker rejects unadmitted roots and reports
  failed roots to dependent compiles rather than synchronously rediscovering
  them. Reproject from the generic completion signal.
- Let Rhai own the required `member_output_aliases` promotion table, including
  the explicit empty-table case. Rust may validate known member/output pairs and
  identifier uniqueness, but must not choose aliases or emit visual source for a
  policy.
- Keep policy contracts in authored Rhai scene tests under
  `assets/scenarios/tests/`; reusable standalone assertions may remain under
  `assets/scripting/tests/` and run through `scripts/api/run_rhai_test.sh`.
  The Rust host supplies composed facts and invokes the shipped policy; Rhai
  owns assertions about generated source, topology, layout, and presentation.
  Literal top-level Rhai constants are supported inside policy helper functions
  by the shared hook binding, so presentation policy can remain editable without
  adding Rust-side layout parameters.
- A cyclic set of separately co-simulated components connected by typed scalar
  `SimConnection`s is explicit causal feedback: the fixed-step exchange may
  have a one-step delay, but it is not an unresolved algebraic loop and must not
  emit an algebraic-loop warning. Do not add a Rhai polling bridge. If
  zero-delay continuous feedback is required, synthesize one Modelica network
  and solve it as one system; force-producing cycles remain subject to the
  explicit realtime-safety contract.

## Replace a superseded contract cleanly

When a mechanism is wrong, perform a clean cutover:

1. Identify the authoritative replacement and write a positive test proving
   the replacement contract first. Add a negative case only when rejection or
   safe failure is itself a public contract (for example, the removed spelling
   must be rejected at the public schema boundary).
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
python3 scripts/gen_schema.py
"$LUNCOSIM_BIN" test --scene scenes/tests/sensor.usda
RUSTC_WRAPPER= cargo test -p lunco-usd-sim --test usd_connection_mechanics -j 4
CARGO_INCREMENTAL=1 RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim -j 4
```

For a live feature, launch only `$LUNCOSIM_BIN` with an explicit free
API port, verify readiness, inspect ports and composed connections, and run the
scene through the real executable. Report separately:

- parsed and composed;
- contract and topology accepted;
- solver/runtime behavior observed;
- visual behavior observed; and
- warnings that remain, especially rejected force-loop diagnostics or solver
  warnings.

Keep long USD fixtures and asset-specific assertions in authored `.usda` and
Rhai scene tests. Rust test stages should be minimal and programmatic, and only
cover mechanisms that the production query/command surface cannot observe.

Never claim that a source parse or unit test proves the scene is physically
correct.
