---
name: performance-profiling
description: Diagnose or improve LunCoSim FPS, physics time, periodic stalls, Builder versus View differences, terrain/render cost, or Tracy captures. Use when performance must improve architecturally without changing BigSpace, substeps, shadows, terrain quality, or image fidelity.
---

# Performance profiling: measure the owner, then remove avoidable work

Read the current handover in
[`docs/reviews/open-400fps-performance-handover.md`](../../docs/reviews/open-400fps-performance-handover.md)
before changing code. A status-bar FPS number is a symptom, not an attribution.

## Required separation

- Run one production session that you own for the product FPS number, launched
  from the task checkout on a verified free API port. Do this even if another
  user's or agent's session is active; never control, stop, restart, or change
  the scene in a pre-existing session. Report concurrent GPU/CPU workloads and
  classify affected numbers as contention-affected, not clean acceptance. Do
  not use Tracy's instrumented number as acceptance evidence.
- Run a separate Tracy build/capture using the adjacent `../tracy` checkout;
  start `tracy-capture` before the production binary and inspect the settled
  window, not only startup.
- Compare Builder and View with the same scene, camera, rendering-quality
  settings, physics substeps, shadow settings, and terrain assets. A Builder-
  only cost usually means an editor observer, rebuild, projection, or UI path,
  not that physics needs a different global timestep.
- Attribute CPU, GPU, physics, terrain, and UI separately. Never disable
  shadows, lower authored terrain quality, change BigSpace, or change the
  standard substep count to make a graph look better.

## Architecture checks

Search the owning systems for unconditional writes, full-set topology scans,
repeated observer registration, per-frame allocations, polling, and work that
should be gated by a revision/change event. Structural edits should invalidate
structural caches; transform propagation and telemetry output are not by
themselves topology changes. Check both the Builder and View registration paths
before fixing only one.

When a measured UI snapshot builds several indexes from the same entity
population, combine compatible marker reads into the existing query and avoid
another full-population traversal for each marker set. Keep queries over
different populations separate unless measurements show that a broader scan
costs less.
For the entity tree, derive parent and grid facts through indexed lookups along
named candidates' deduplicated ancestor closure instead of copying every scene
entity's `ChildOf` and `Grid` membership into the snapshot.

For startup asset graphs, separate asynchronous source reads from discovery,
composition, and UI/physics admission. Read all known dependencies in each
breadth-first frontier through bounded batches, then merge results in stable
authored order; this avoids serializing independent branches behind one parent
at a time. Preserve the source-change reload graph when using child load
contexts. The USD layer-closure owner and its reload receipts are documented in
[`21-domain-usd`](../../docs/architecture/21-domain-usd.md#composition-closure-and-partial-scene-loading).
Keep CPU-heavy source compilation off asset I/O workers too: register literal
dependencies first, then prepare source ASTs on the existing async-compute pool.
Startup discovery must keep filesystem enumeration, manifest reads and parsing,
and installed-artifact checks off the app thread. Merge the resulting typed
registry snapshot and publish readiness on the owning app schedule. Open Twin
manifest scans can overlap, but commit in discovery order and discard results
when their owning Twin closes. Reuse authored Rhai source classifications for
the same manifest and policy revision; asset-content changes do not require a
second classification pass.

Application policy startup has separate Tracy spans for
`application_policy_source_prepare_offthread`,
`application_policy_compile_offthread`,
`application_policy_prestartup_wait`, and
`application_policy_activation`. Preparation starts during runtime plugin
construction; the compile span includes authored installer-order selection.
`PreStartup` validates that typed order and publishes the prepared hooks before
Startup consumers. Keep source text and callables in the Rust preparation
bundle; the application selector receives hook identities only. Attribute
worker time and residual PreStartup wait separately before changing policy
lifecycle ordering.

Read-only USD projectors use `CanonicalStages::reader_for` or
`reader_for_entity`: generation-zero reads consume the worker-prepared plan, and
later authored generations consume the live canonical stage. Do not call
`get_or_build` just to read startup facts. Use the prepared schema/path indexes to
find initial candidates. Change batches should advance unrelated edits and
invalidate only owners whose consumed paths changed. If a live generation has
no exact worker-prepared plan, snapshot the canonical stage recipe on its owner
thread and submit replacement facts through shared bounded `AsyncWorkAdmission`;
cap this owner's pending stages, then commit only when asset-plan identity and
canonical generation still match. Hold startup progress only until the first
topology index commits. For later generations retain the last committed facts
so current simulation continues, while new or reprojected prim admission stays
queued until replacement facts commit. A required preparation failure,
including a host without worker transport, must be visible through the scene
fault owner. Do not run a whole-stage topology traversal synchronously in
`Update`.

For Twin-open stalls, profile the active Twin policy loader separately from
policy activation. Native manifest and Rhai source reads should run through
bounded `AsyncWorkAdmission`; `twin_policy_source_prepare_offthread` measures
that preparation and `twin_policy_activate` marks the lifecycle-bound commit.
Verify that stale completions are discarded after a Twin switch and that the
active `assets_mounted` plan and first authoritative tick wait for activation,
with the app/UI schedule continuing. Browser builds still use synchronous
WebStorage reads because they do not have a worker transport.

One-shot `RunRhai` and tool callbacks share the prepared `ScenarioDriver`
engine. When profiling a callback stall, separate one-time startup/prelude or
tool-generation refresh from request execution; do not rebuild the engine for
each callback.

For Bevy visibility costs, distinguish the active scene camera from auxiliary
shadow-map subviews. If adapter capability supports GPU culling, put
`NoCpuCulling` on the scene camera so camera frustum work can move to GPU
preprocessing while per-mesh light and shadow visibility remains CPU-owned. Do
not put it on shadow-casting `Mesh3d` entities: Bevy excludes those from its
CPU-built per-light visibility lists. Unsupported adapters retain CPU camera
culling. Measure GPU headroom and compare a clean FPS run plus a separate Tracy
capture; do not trade away shadows or authored quality to reduce CPU time.

Local-light shadow views are filtered at the render boundary: compare a point
light's finite range sphere or a conservative bound of each extracted spotlight
frustum with every active extracted `Camera3d` frustum and compatible
`RenderLayers` before Bevy prepares shadow views. Do not mutate authored lights,
omit offscreen cameras, or infer relevance from the light origin alone.
Uncertain bounds and boundary cases keep the map. This saves only maps
provably irrelevant to all outputs; Bevy's main-world per-light caster
visibility pass is still a separate cost to measure.

Bevy's camera driver also executes `Core3d` for point/spot shadow roots. Admit
camera-only Core3d stage sets only for camera roots; keep the shared shadow
passes and the GPU preprocessing needed by the depth maps active on light
roots. Verify the same retained shadow pass inventory before and after so this
scheduling optimization cannot silently remove shadows.

Treat Bevy `Changed<T>`/`Added<T>` filters as population filters, not free
events: a no-match query can still inspect candidate entities, and separate
`is_empty()` queries can repeat that work. Combine compatible invalidation
sources into one `Or` query when they drive the same decision. If this remains a
hot path, audit every writer before adding a source-owned event/revision/dirty
set; once all writers are accounted for, prefer that signal over an
always-evaluated `Added<T>` population query in a run condition.

Keep readiness checks separate from reconciliation requests. An unresolved
async participant may require a cheap readiness check on later frames, but a
shared revision that wakes full topology or causal-graph work should advance
only when topology or endpoint lifecycle facts actually change—not merely
because readiness is still pending. Guard `ResMut` revisions by comparing the
owner's state before calling a mutating method: Bevy marks the resource changed
on mutable dereference even when an idempotent method leaves its value alone.

`SimComponent` input/output shape is tracked by `lunco-port-core::ScalarPortMap`.
Its identity key changes at insert/remove/clear boundaries, while numeric sample
writes leave it stable; use borrowed-name `set` in hot publishers and strict
`set_existing` for already-declared inputs so stable keys do not allocate. A
strict write separates whether the name exists from whether its value changed.
Callers that bypass Bevy change detection should mark the component changed
only when the sample differs; generic `InputPorts` follows the same rule.
`PortMap<T>` keeps authored names at its boundary and resolves them to dense,
process-local slots for the fixed propagation data plane. Map-backed ports,
static scene-property inputs, shader inputs, Avian ports, and catalogued
link-class outputs use owner slots. Shader live values are predeclared for
driven parameters, and authored shape fingerprints update only at structural
setters. Every owner that can change a declared surface publishes the shared
`PortTopologyRevision`; compiled wire handles are rebuilt on that revision or
on connection changes, and a stale slot is never
retried through a name lookup. Numeric samples do not invalidate the compiled
fabric. Do not re-hash every map when `SimComponent` changes for ordinary
physics values. `PortHolds` advances its own revision only for effective intent
changes; rebuild its target-index projection on that revision or wiring changes
and reuse the aligned value buffer on steady ticks. Do not probe every target
against the hold table or clone its presentation snapshot per tick.
Readable `inputs:*` connection sources use a distinct input-side slot reader;
write-only inputs can be targets but never become fabricated readable sources.
Success diagnostics use entity-indexed borrowed-name lookups and retained tick
scratch. Compiled targets cache surface presence until topology invalidation;
pending/broken snapshots share compiled names as `Arc<str>`, and fault warning
keys are formatted only on first failure rather than every fixed step.

For deferred USD projectors, keep one entity-work set per owner and feed it
from the complete lifecycle boundary: identity arrival, projection readiness,
invalidation, removal, and scene teardown. The same applies to deferred adapter
steps such as wrapping a Modelica model into its shared port surface. Keep a
single bootstrap discovery for entities predating plugin installation, and
retry only work whose authoritative stage/readiness input is still pending.
When a projector caps per-update work, select the bounded prefix in stable owner
order and retain the remainder; avoid sorting the entire pending batch on the UI
thread before applying that cap.
For render-free USD discovery, wake candidates from changed prim identity,
canonical stage generation, or stage-asset events, and keep only transient
runtime prerequisites in the per-frame retry set. Dormant `BasisCurves` without
the owner's relationship must be revisited when their stage changes, not
queried from USD every update.
The celestial admission projector uses the shared `PendingEntityWork` queue for
one bootstrap and later lifecycle arrivals; its idle run condition reads that
owner instead of scanning all USD prims. Keep `CelestialProjected` on the scene
root after its source classification because static-light resolution consumes
that boundary. Ordinary non-root prims do not need a completion command.
Preserve authored-field candidates with a `lunco:` property, the scene root,
`DistantLight` parent-body fills, and non-root `LunCoEpochAPI` diagnostics.
When initial candidate processing spans many owners, queue the bootstrap IDs
once and drain a fixed-size batch in stable entity order per app update; keep
the remainder queued so a large scene cannot monopolize one UI frame.
For USD visual projection, read both the system body and its Tracy
`system_commands` flush: `frame_budget` bounds prim binding but cannot bound
the deferred command batch applied after the system returns. Cap prim work
items per update as well as elapsed time. Direct-child admission has its own
`child_spawn_budget` and `max_child_spawns_per_update`; the parent keeps its
awaiting/projecting markers until its direct children enter the ECS queue, so
scene readiness must remain held while batches drain.
Gate sparse lifecycle work on an existing pending marker or owner queue; an
idle Update should not scan lifecycle state for a request that did not arrive.
For joint admission, use one combined query over the existing
`PendingUsdJoint` and `PendingJointAdmission` markers to gate the readiness
scan. Reconcile candidates before the admission commit so the stable batch
boundary is preserved.
When a worker prepares facts for a composed plan, result commit must use the
same prepared-plan identity for any derived cache. If the canonical owner
records that exact plan as the source of the current live-stage generation,
prepared facts remain valid at that nonzero generation until a live edit
advances it. Later generations require complete change history or
generation-keyed main-thread extraction. Otherwise a cache-key mismatch can
repeat a full-stage scan during result publication.
For dependent-stage refresh, pair `usd_canonical_stage_asset_sync` with
`usd_sim_prepared_topology_cache` and `usd_sim_joint_topology_scan` to verify
that publishing the new asset plan neither reopens the canonical stage nor
repeats its prepared topology extraction.
When initial preparation already builds composed type or API-schema indexes,
run one-time topology and vehicle-output extraction against the prepared reader
on a worker and use its indexed candidate query. Combine overlapping schema
queries and property-prefix checks into one candidate traversal, so live
canonical readers do not walk the stage once per consumer. Return each
candidate's type, API-schema, and property-prefix facts with its path so later
classification does not repeat native schema reads. Build per-prim candidate
sets and authored vehicle-port lists from the prepared snapshot; skip extraction
for unrelated entities while preserving their readiness marker. Do
not materialize every prim path on the UI thread to find a small set of joints,
attachments, vehicle roots, or policy prims. Live canonical edits keep using
their owning-thread reader. Invalidate cached topology from
`UsdSceneChangeBatch`: resynced paths that match an indexed source or currently
carry a relevant schema, and info changes on indexed source prims, require
refresh; unrelated paths only advance the cached generation. Transform-only
info changes on topology source paths also advance the generation without a
topology rebuild; structural resyncs or mixed edits still refresh. Check
intervening changes before accepting prepared worker output. If another live
read or the change observer has already made the cache current at that exact
generation, discard the late prepared result instead of scanning the stage
again. `usd_sim_prepared_topology_cache` records whether this check found a
current-generation cache; pair it with `usd_sim_joint_topology_scan` to confirm
that a repeated reconciliation did not scan the stage.

For whole-index projectors such as USD telemetry, use one initial bootstrap,
then coalesce relevant insert/remove observers into an invalidation flag. Keep
stage-generation and asset-store invalidation as scalar checks. Do not repeat
the same `Added`/`Changed` population filters in both the run condition and the
projector, and do not reproject until the index is invalidated. When structural
projection arrives in bounded batches, keep the index dirty and coalesce further
invalidations until the owning queue settles; clear derived outputs once for
that dirty interval. Keep processed entity identities in the owner's index so
prim progress does not enqueue a deferred ECS marker command for every prim;
clear the set with the stale outputs on invalidation. This still skips ordinary
non-declaration prims on later projector passes without imposing marker insert
and removal flushes on the app schedule.

When a USD reader already exposes `has_authored_attribute`, use it to test one
known property instead of enumerating every attribute name. If one enumeration
feeds multiple derived port sets, derive them together from that single result.
Temporary lookup indexes over immutable ECS queries should borrow path and port
surface data instead of cloning those maps for a one-pass reconciliation.
Build compatible per-entity indexes in one query traversal rather than running
separate full-population passes for each index.
For Modelica telemetry metadata, build `ModelicaIndex::component_name_lookup`
once per dirty metadata batch and resolve all variables through that borrowed
lookup. Do not call the linear `find_component_by_leaf` scan once per variable;
steady sample batches should use the session's cached metadata without walking
the document index.
Cache each runtime producer's stable `SignalRef` identity for the session and
borrow it when recording an existing channel. Avoid reconstructing and cloning
signal paths for every due sample.
For burst channel publication, keep retention depth as a logical limit and let
the history buffer grow with recorded samples. Do not reserve every channel's
full retention window when most new channels contain only their initial sample.
When multiple runtime consumers inspect the same composed owner, acquire its
canonical reader and child list once, derive owned typed facts for each
consumer, then release the stage borrow before mutating ECS. Preserve the same
typed resolution and commit helpers across initial and live projection paths;
do not introduce a second fact cache.
The presentation `project_env_settings` owner retains authored exposure and
bloom facts with the exact stage-plan `Arc` and the matching environment prim
paths. Reuse them across transform-only batches and info edits outside those
paths; environment-path edits, resyncs, generation gaps, or plan replacement
require a fresh composed read. Compare the stage signal without allocating a
replacement vector on steady updates.
When multiple USD consumers need the same composed-stage fact, put its
generation/instance-keyed cache at the shared fact owner and reuse that cache;
clear it at the scene teardown boundary instead of keeping consumer-local copies.
For derived marker sets, compare current membership with the desired set and
apply only additions/removals; unrelated rebuilds must not emit lifecycle churn.
Removal invalidation should be qualified by the entity's authored USD identity
and relevant endpoint capability, not by a generic component removal alone.
Extract a USD program's declared interface once at admission and reuse it for
validation, diagnostics, and publication instead of re-enumerating attributes.
Derive a domain root's synthesizer once alongside member-role discovery and
reuse the root-scoped selection during projection; do not traverse a large
component collection again to repeat its role-schema queries.
In Tracy, compare `domain_member_role_discovery` and
`domain_synthesizer_selection_cache` against the enclosing
`project_domain_islands` interval to attribute any remaining app-thread tail.

Keep invalidation domains distinct: a wiring/topology latch may be raised by
endpoint arrivals and must not automatically trigger domain discovery. Live
canonical edits publish `UsdSceneChangeBatch` with stage generation and
resynced/info paths; route those paths through the owning stage/root/member
index. Transform-only info paths advance the generation without re-reading a
Modelica network; structural and other member/root changes still invalidate
affected roots. Stage-asset changes and missed generation batches requeue only
roots on the affected stage. Reserve the all-prim discovery for initial
admission, and keep entity arrivals on their queued-entity path.
Generated-source document sync should share the `PendingEntityWork` contract,
and its metadata publisher should consume the owner-published dirty flag rather
than adding a parallel change query.

When gating a dependency's multi-system transform schedule, distinguish its
per-update output flags from authoritative input changes. Preserve any required
settle pass after a floating-origin change, and test the change → settle → idle
sequence so a gate neither stays open forever nor closes before propagation.
Keep the settle marker between admitted passes so the idle admission check does
not replace expensive propagation with a full-grid scan.

For physics, distinguish persistent environmental state from transient
commands: use the engine's persistent acceleration or passive non-waking force
contract for gravity and contact support, and reserve waking force/torque writes
for authored drive, braking, or actuator commands. Do not emulate this with a
timer, sleep threshold, or a second cache.

Use the existing cache/revision owner and preserve the USD projection boundary.
Do not add a second cache, a timer, a compatibility path, or a quality fallback.
If the hot path is not established, stop after source inspection and capture a
bounded profile rather than guessing.

## Evidence

For whole fixed-step timing, use the `PhysicsPerformance` query's
`step_time_samples_ms` history in every host that installs the shared USD
physics runtime. It returns retained `PhysicsTotalDiagnostics.step_time`
samples, one per completed physics step; compute p50/p95/p99/max from that
array after the measurement window. The query also returns the current
`step_time_ms` and `step_number`, and the editor publishes the same current
sample through `engine-health.physics_step_ms`. Query once per window because
`PhysicsPerformance` also counts live topology and is not a per-step sampler.

Record the clean FPS window, physics and render timings, Tracy capture path,
scene/settings, and whether the result is startup or settled. Rebuild the
production binary after a code change and repeat one clean A/B plus one Tracy
diagnostic capture. Link the changed owner and state any platform/GPU evidence
that was not available.

For fixed-step versus UI diagnosis, query `SimulationTimingProfile` during the
settled owned run. It summarizes the latest 240 fixed ticks and 240 app-update
fixed-loop bursts: p50/p95/p99/max service time, per-tick realtime service-budget
exceedances, fixed steps per app update, fractional overstep, and simulation-time
demand clipped by `Time<Virtual>::max_delta`. Pair this with
`QueryTelemetryHistory` for `engine.frame_time`, which covers the full app frame.
The profile is read on demand and is not a replay or UI-isolation verdict; a
nonzero clipped-time total means wall-clock demand was already excluded by the
current policy, not that recoverable ticks remain queued.

For CPU outliers, report p50/p95/p99/max for the app-thread frame and the
authoritative fixed-tick transaction, plus fixed steps per app update and
simulation deadline/backlog. Do not infer these from mean Avian time alone or
add overlapping Bevy schedule spans. Bevy drains accumulated fixed schedules
synchronously before `Update`: a fixed `Time<Fixed>` delta is not proof of a
constant wall-clock physics rate, and a catch-up burst delays UI/input. Never
improve UI timing by silently discarding authoritative overstep. If both
wall-clock physics cadence and UI isolation are required, measure the whole
simulation-owner boundary and consume immutable snapshots from the UI/render
side; separate cycle labels alone do not provide thread isolation.

For exclusive systems that borrow `&mut World`, inspect individual Tracy event
timestamps and durations, not just aggregate averages. Align long calls with
Twin-open and readiness milestones: one startup outlier can stall UI even when
the same system is nearly free on settled frames. If the outer system is hot,
attribute time to its internal owner operations before choosing an async
boundary or cache.

When `system_commands` dominates after a producer, inspect the number and shape
of its deferred writes. For a large set of entities receiving the same bundle,
use Bevy's existing batch command when its fallible/overwrite semantics match
the owner, and preserve the owner's iteration order when it determines stable
IDs. Relationship bundles such as `ChildOf` can use the same fallible batch:
Bevy runs their relationship hooks per entity, in batch order, while continuing
past stale entities.

For `process_queued_usd_visuals`, include queue preparation in the frame-budget
review. Reuse its system-local child-key scratch set across updates, and keep
duplicate-child identity checks scoped to the parent being admitted after the
budget check; preparing keys for every queued parent can turn a small projection
slice into a full-queue UI stall.

For live composed-stage discovery of a known USD type, use the typed
`UsdReadObject::prim_paths_matching` query. It preserves live traversal
semantics while avoiding a full materialized path list followed by a separate
type lookup for every prim.

For fixed-step telemetry, keep static channel-presentation facts borrowed in
the per-sample path and allocate their owned signal metadata only when that
channel's metadata is first created or changes.

Dependent USD-stage refresh is owner work behind `sync_twin_overlays`. Snapshot
the base/runtime revisions, serialize each persistent source snapshot once on
bounded workers, share its bytes across dependent stages, and coalesce changed
layers per target stage. Compare recipe bytes on workers and skip unchanged
rebuilds. The main-thread owner validates source revisions, operation/revision,
and target-plan identity before opening the live stage and committing the plan.
Measure serialization and plan preparation separately from the live-stage
build, reset owners, and visual projection; do not move the thread-affine stage
across the worker boundary or let completion order select a commit.

For `project_usd_policies`, a cold cache can use the worker-prepared plan as its
baseline only when every `UsdSceneChangeBatch` from generation zero to the live
generation is present and proves that no policy prim changed. Existing cached
facts use the same complete-generation and affected-path checks when promoted.
A missing batch, generation gap, plan replacement, or policy-affecting change
must use full live extraction. Compare startup and settled edit captures after
changing this path; compile evidence alone does not establish a timing gain.
