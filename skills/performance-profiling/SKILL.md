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
  Check the profiler listener and connection against the owned PID with
  `ss -ltnp`/`ss -tnp`. A concurrent Tracy client can occupy 8086 and move the
  owned app to the next port; pass that port with `tracy-capture -p`. A capture
  from another process is not evidence for the task scene.
- Compare Builder and View with the same scene, camera, rendering-quality
  settings, physics substeps, shadow settings, and terrain assets. A Builder-
  only cost usually means an editor observer, rebuild, projection, or UI path,
  not that physics needs a different global timestep.
- For a settled DEM quality window, require `TerrainLodStatus.stream.wanted ==
  resident`, `stream.pending == 0`, `derived.active == false`,
  `derived.pending == 0`, and `derived.ready == derived.total`. Record those
  owner facts before and throughout the window alongside camera and quality
  inputs. Simulation readiness alone does not certify optional visual-map
  preparation. Keep this measurement gate out of simulation admission.
  Audit a surface camera with `world_pos` and `world_rotation` in the active
  simulation frame; `visual_foci` reports composed root coordinates, which move
  with celestial ancestors even when the camera is stationary over the terrain.
- Attribute CPU, GPU, physics, terrain, and UI separately. Never disable
  shadows, lower authored terrain quality, change BigSpace, or change the
  standard substep count to make a graph look better.
  In a diagnostics-enabled render host, `lunco-render-bevy` adds
  `lunco_shadow_shared_early`/`late` and `lunco_shadow_camera_early`/`late`
  timestamps around the native shadow passes. Shared events correspond to
  individual point/spot views; camera events include that camera's directional
  cascades. Check all four names and their event counts before attributing total
  shadow cost. Late spans can contain only timestamp overhead. Retain native
  dependency ordering, native-parameter admission before begin and render-context
  buffer order when changing this adapter. Reapply PBR's system-local early/late
  ordering rules after removing the original system; retaining its implicit
  type set alone preserves only dependencies targeting that set. Check the
  actual shadow image before admitting a capture as performance evidence.
  Unsupported timestamp features or
  exhausted per-frame queries leave incomplete GPU evidence, not zero cost.

## Architecture checks

For authored image preparation, trace `lunco-materials` typed
`PreparedShaderImage` loading: native decoding and owned RGBA8 filtering precede
source/child publication. CPU filtering awaits the async compute pool with an
owned image, without blocking an I/O worker or borrowing ECS. The renderer resolves `ShaderTexture` and reacts to
source asset events; it must not snapshot resident pixels. The prepared loaders
register no extensions, preserving native image/glTF settings and decoding.
Prepared children use the native render-only extraction contract: pixels move
into GPU preparation while the main asset retains metadata. Audit CPU readers
before choosing that usage; sampler/descriptor edits must come through source
reload with fresh data. Verify initial load and reload publish complete chains, independent color/scalar/
normal identities, native-image forwarding and invalid-pixel rejection at the
asset seam. `load_raster` accepts physical paths; verify labeled requests reject
before issuing a prepared-root load and importer-owned native image handles
still load, bind and reload. Preserve exact RGBA8 transfer, sampler quality and role identity.
Worker-time gains need separate loading/UI/FPS evidence before an app-level claim.

For repeated analytic USD primitives, inspect reflected `Mesh3d` identities
before inferring duplicate preparation or upload cost. `PrimitiveMeshAssets` in
`lunco-usd-bevy-mesh` owns shared preparation and weak native mesh lifetime;
source admission remains in `lunco-usd-bevy`. Preserve the
[immutable mesh contract](../../docs/architecture/render-decoupling.md#immutable-primitive-mesh-assets),
quality replacement and entity-specific edit isolation. Run the authored
`primitive_mesh_sharing.rhai` sandbox gate for equal and unlike dimensions.
Fewer asset IDs alone do not prove fewer GPU draws or faster frames. The retained
`engine.frame_time` series samples raw app-frame values at the fixed 60 Hz
ceiling and may repeat values; collect it before its 240-sample ring truncates
the measurement window. Report it separately from once-per-rendered-frame data.

Search the owning systems for unconditional writes, full-set topology scans,
repeated observer registration, per-frame allocations, polling, and work that
should be gated by a revision/change event. Structural edits should invalidate
structural caches; transform propagation and telemetry output are not by
themselves topology changes. Check both the Builder and View registration paths
before fixing only one.
For frame-contract gate costs, distinguish the full connectivity validator from
its admission/fixed run conditions. The bridge observes native physical and
hierarchy lifecycle events through one change-ticked resource; each consumer
retains its own observation boundary. Preserve insertion, reparenting, removal,
despawn and active-frame switch/removal detection without waking on ordinary
pose values. Validate a disconnected body through the public owner diagnostic
and stopped solver ticks, then verify a replacement scene resumes physics.
For link-sweep costs, attribute `celestial_ephemeris_position` separately from
pair geometry and verdict hooks. The analytic provider's exact-epoch cache also
shares native EMB/Earth VSOP operands across dependent bodies. Preserve native
f64 values and formula order; a new epoch bit pattern invalidates final and
intermediate samples together. Compare public `BodyPosition` results against the
prior exact artifact at adjacent and revisited epochs before claiming a gain.
For an exclusive system, check repeated `World::query` construction separately
from iteration. Retain Bevy's native `QueryState` in the owning system when
the query shape is stable; read current membership each pass and collect
identities before structural mutation. Preserve that system's serial schedule
position and lifecycle ordering. A settled-frame saving also needs live
add/remove or replacement evidence, since queries must admit newly matched
archetypes after startup.
The neutral scenario driver shares its native model query across preparation,
startup, fixed and visualization passes; preserve fresh model/scope/authority
reads and stable actor sorting when changing that owner.
For telemetry retention, separate due recording batches from cheap between-batch
calls. `SignalRegistry::record_scalar_at_rate` admits and appends through one live
history lookup; preserve backwards-time segments, retention, archived-source
reactivation and catalog notifications. A registry benchmark isolates that owner,
but production retention spans and raw physics/frame tails need separate evidence.
Modelica's channel-limit check reads the live scalar count after append and
only hashes history identity for admission when the catalog is full. Preserve
existing-channel recording above a lowered limit and rejection after history
removal; a cached producer identity alone does not prove that history exists.
For a system that queues many compatible ECS component changes, inspect its
`system_commands` flush separately from the system body. Collect changed values
and use the owning crate's existing batch command path where it preserves the
same commit boundary; retain change filtering so a quiet fixed cycle queues
nothing.
In panel paint code, use `PanelCtx::resource` for reads. Its `resource_scope`
temporarily removes and reinserts the resource, which marks it changed even
when the closure only reads it. Scope a mutable borrow only around a real
state transition; otherwise a steady Builder repaint can wake change-detection
systems and full-scene reconciliation on the next Update.
USD authored/composed text painting borrows `UsdViewportState` and its cached
source strings. Check `usd_preview_render_layer_propagation` when diagnosing
preview invalidation; report traversal cost separately from frame time. Opening
several previews must not add bodies, colliders, or joints to the mounted scene.
For SysML requirements rebuilds, inspect `sysml_requirements_rebuild` input
flags. Domain notification publishers check `has_pending_events` before
mutably draining their registry; an empty queue must not invalidate readers.
Workspace document focus and USD editor open/close are not SysML inputs. The
requirements producer compares the active Twin identity, root, manifest, and
file index before reacting to workspace change detection. Verify both an
unrelated editor change (no rebuild) and a Twin/source/evidence change (fresh
projection); do not suppress domain registry invalidation.
Source links use `DocumentRegistry::doc_for_file`, which checks all exact paths
before resolving file aliases. Preserve symlink identity and dirty-document
semantics; do not build a panel-local path registry.
Inspect scenario preparation separately from lifecycle execution. Unchanged
actors borrow their canonical source; only admitted compile/retry work copies
worker inputs. Do not defer initialization or change actor commit order to
reduce this preparation cost.
For Modelica response retention, reuse existing observable-variable keys and
transfer owned input/output buffers after transaction validation. The accepted
step and UI stream are separate consumers, so retain one output snapshot for
the accepted record. Preserve detected-symbol/output precedence and the UI
stream's output-then-symbol order. Measure this response path separately from
fixed-step output publication; fewer copies alone do not prove throughput gain.
Modelica and scripted snapshot publication use `ScalarPortMap::upsert_samples`.
The destination retains slot hints, but validates each live layout and exact
name on every copy. Preserve reordering, same-size source replacement, target
edits, clone/clear/compaction retirement and native-bit no-op detection when
changing that mechanism. Do not infer unchanged source topology from map length
or cached iteration order. Keep Modelica's reflected variable map and the
fixed-step publication boundary unchanged, and measure the actual copy owner.
For structural publication, use the existing `PortTopologyState` owner to
observe expensive auxiliary fingerprints by their own component change ticks.
`observe_if_changed` computes a cold key and recomputes changed owners; sample
updates reuse the recorded key. A participant's new admission must refresh
auxiliary facts that could have changed while it was absent. Verify metadata
edits, same-size declaration replacement, removal and re-admission. Keep direct
backend inspection and write validation on the live contract.
For fixed-step port costs, backend `declare_ports` uses `PortDeclarationQuery`
for both full inspection and exact-name, direction-constrained discovery.
Map-backed owners query the requested name directly; named queries retain only
the first matching direction without constructing owned rows. Preserve the
same declaration order and live metadata checks; topology publication alone
cannot guard an owner edit before its scheduled publication pass.
`write_resolved` uses canonical input-write
preparation once, compares the live owner with its locator, then commits within
the same exclusive World boundary without a single-element batch allocation.
Preserve live precedence, metadata, topology revision, slot, value checks, and
rejection ordering. Separately prepared batches still revalidate every write
before committing any of them.
The shell's steady `WorkbenchSnapshot` check compares borrowed dock and
perspective iterators before materializing owned vectors; preserve that
allocation-free stable path when changing layout publication.
The top-level menu's unique label projection is built only when the menu
registry changes. Width measurement borrows that projection and streams labels
without collecting another vector; compute perspective-tab rows once per UI
pass and reuse them for both width measurement and painting. Do not rebuild the
same responsive menu inputs during steady repaint.
For contributed top-level menus, defer collecting and sorting scripted
contributions until the popup callback runs; a closed menu should only paint its
row and publish its anchor. Use `workbench_custom_menus_render` to inspect that
row path separately from the workbench aggregate.

When a measured UI snapshot builds several indexes from the same entity
population, combine compatible marker reads into the existing query and avoid
another full-population traversal for each marker set. Keep queries over
different populations separate unless measurements show that a broader scan
costs less.
For large Builder hierarchies, derive a lightweight row index from the cached
tree and current expansion state, retain that index across repaints, then use
`ScrollArea::show_rows` to create widgets only for rows in the viewport. Rebuild
the index only after a source revision, active filter/scope change, or a branch
disclosure change reported by the shared `tree::branch_header` helper. Its
header-only allocation preserves the fixed row stride; ordinary recursive
trees use `tree::branch` to allocate their indented body. Keep foldout state keyed
by stable entity/group identity and preserve selection, drag, and tooltip
behavior on painted rows. Immediate-mode widgets are expected to be repainted;
the domain tree and entry records should be borrowed or shared, not cloned into
panel-owned snapshots. Borrow selected-entity state during paint and retain an
owned selection snapshot only when it changes; compare egui temporary values
through borrowed type-map access instead of cloning a cached vector every frame.
Gate panel-owned view models with `WorkbenchSnapshot::is_panel_visible`, and
order their systems after `WorkbenchSnapshotPublishSet`. Hidden dock tabs have
no reader and should not rebuild view data each frame; keep separate cleanup
work transition-driven when a panel closes.
In the Builder Spawn palette, use the catalog owner's revisioned
category-to-entry index. Borrow category labels and spawn entries, and retain
formatted display labels only until that revision changes. Do not rescan all
entries for each open category or build cloned entry groups before egui
determines which categories have a reader.
For Builder Ports, retain matching and expanded/collapsed entity indexes by
topology revision, filter, and expansion state; update the sampling request
only when the expanded entity set changes.
In the Inspector's material part selector, keep the entity index separate from
its display text. Format the active label only for the selected part and format
the other labels only while the dropdown is open. Retain the material-bearing
entity index for projected USD roots by selected root and `UsdStageRevision`;
rebuild it only when either changes. Recompute for non-USD roots or when the
revision resource is unavailable.
When deriving a chosen part's material controls, filter the selected root's
existing material-bearing entity list instead of walking that part's child tree
again.
For live line plots, do not copy and decimate a full history in every UI frame.
Keep one bounded build per binding, snapshot changed histories at a limited
presentation cadence, transform immutable sample snapshots on the async-compute
pool, and keep painting the last completed point buffer while the next build
runs. `ScalarHistory::snapshot()` shares completed chunks and copies only its
bounded open tail on the caller; flatten and decimate that snapshot on the
worker. In Tracy, compare `line_plot_history_snapshot` (app-thread copy time)
with `line_plot_series_build_worker` (worker time and input/output point
counts); several plots can rebuild concurrently even when each panel's paint
span is short. When live curves share an experiment plot, also inspect
`line_plot_scalar_history_snapshot`, `line_plot_scalar_history_build_worker`,
and `multi_series_plot_points_build_worker` to account for both history
flattening and plot-point preparation. Auxiliary Graphs overlays should use
the same snapshot cache and keep painting their last completed buffer while a
new one is built. Key transformed point buffers by source identity, transform
mode, and pixel width; apply line style while painting.
For experiment and live-overlay plots, retain transformed `PlotPoint` buffers
by immutable source identity, log-Y mode, and display width; build them on the
async-compute pool and borrow them while drawing. Min-max decimate time-sorted
series to about one sample per logical display point on that worker so line
geometry does not traverse every retained history sample each repaint,
preserving narrow peaks. Cache each buffer's full-data bounds so egui auto-fit
does not scan all samples on every repaint. Keep experiment variable groups
and positivity summaries in the change-gated view model instead of regrouping
names or scanning all sample values during graph painting. Time-series hover
may use monotone-X interval pruning; keep the dependency's general segment
search for phase-space curves where X can reverse.
For the entity tree, derive parent and grid facts through indexed lookups along
named candidates' deduplicated ancestor closure instead of copying every scene
entity's `ChildOf` and `Grid` membership into the snapshot.
Keep topology invalidation separate from the query-heavy snapshot producer:
mark the revision dirty when scene facts change, then gate snapshot work until
no prior worker is active. This lets a revision change reject an in-flight tree
result without re-entering the full ECS query system just to discard it.
For Builder telemetry, inspect `telemetry_catalog_patch_capture`,
`telemetry_catalog_patch_worker`, and `telemetry_catalog_patch_commit` separately.
Each patch handles at most 64 changed descriptors and relevant ancestor facts. There is one
initial identity enumeration; later notifications update the persistent index, affected alias
groups, and tree paths. Selection, samples, and unchanged hierarchy writes must cause zero descriptor preparation. Use
`InspectTelemetryCatalog` counters and owner timings to verify this in an owned production
session, and `scripts/api/test_telemetry_catalog.py` for repeated metadata edits with authored
Rhai verdicts. Separate initialization from settled frame costs and worker time from app-thread
capture/commit time. Use `workbench_panel_render` child zones inside `render_workbench` to
attribute painting and visible-row index costs independently of descriptor work.

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
Application session metadata such as recents follows the same boundary:
load, normalize, and persist it on workers, merge typed results on the app
schedule, serialize writes, and finish the final write during shutdown.

For Modelica startup, separate Rumoca compile time, prepared-solve cache lookup,
`lower_for_live`, and ordered result commit. When equivalent requests share the
same structural solve key, coalesce in-flight lowering instead of occupying a
second worker; preserve one result position per participant in the existing
stable commit queue. Keep the combined compiler/root/lowering admission bounded
to the solve-pool worker count plus two staged operations, so the serialized
Rumoca owner can compile later programs while solve workers are busy without
building an unbounded DAE backlog.

For wrench-allocation cost, inspect the generated actuator count and sweep
count separately from retained algebraic variables. The maintained allocator
factors fixed geometry through `WᵀW`; preserve bounded cyclic sweep semantics,
iteration count and command limits. Validate the numeric allocation and physical
mission after reassociation, then compare preparation and settled-step profiles.

Confirm generated-network source-set stability across launches: identical
generated text must not acquire unrelated open documents as compile siblings.
Use the runtime-owned bundled `generated/` provenance classifier for structural
identity, and retain authored multi-document compilation. Inspect typed
persistent-cache rejection warnings and the native preparation owner logs.
`cache=memory-hit`, `cache=disk-hit`, and `cache=miss` identify the actual lookup;
compare lookup, lowering, and total preparation durations for the same admitted
solve key. The native `modelica_solve_preparation_job` span records source key,
library revision, solver ID, exact override bit patterns, cache version and disk
eligibility from the captured key. Source equality alone does not prove key
equality or eviction. Compare the distinct admitted working set with the actual
retention limit before blaming cache capacity. Stepper construction timing alone
does not establish persistent reuse.

Count domain discovery and publication separately. Content prim GIDs do not
enter ordinary scene-network namespaces; instance identities and stage/source
events still invalidate their owners. Initial prepared results publish a
bounded four-root prefix only under their own fixed-clock admission holds,
with no prior installed projection. Live replacements remain one per Update,
and repeated roots wait for the ECS publication boundary. Preserve the oldest
request and every generation fence; do not admit physics early to shorten
loading measurements.

Application policy startup has separate Tracy spans for
`application_policy_source_prepare_offthread`,
`application_policy_compile_offthread`,
`application_policy_prestartup_wait`, and
`application_policy_activation`. The activation span separates
`application_policy_clear_registry`,
`application_policy_validate_order`,
`application_policy_validate_prepared_hooks`,
`application_policy_install_prepared_hooks`, and
`application_policy_publish_registry`. Preparation starts during runtime
plugin construction; the compile span includes authored installer-order
selection. Owner filtering and per-hook contract/arity validation run in
`application_policy_filter_manifest_offthread` and
`application_policy_validate_prepared_hook_offthread`. `PreStartup`
validates the selected order and its manifest records, then registers the
prepared callables before Startup consumers. Keep source text and callables in
the Rust preparation bundle; the application selector receives hook
identities only. Compare the activation children before moving additional
registry work across the lifecycle boundary; preserve authored install order
and visible failure reporting.

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

For USD simulation admission, check whether visual-only prims consume the
participant prefix. The vehicle owner excludes ordinary prims using only its
current-generation topology index, alongside existing preview exclusion;
missing/stale facts and runtime-instance plans remain eligible. Keep the same
simulation-work limit, stable stage/instance/path order, and readiness markers.
Inspect physical-wheel fixtures through the shared reflected `PhysicalWheel`
contract, and run simulation queries under their real Twin-owned scenario route.

For route-edit latency, record four separate spans: the bounded Rhai input
hook, the durable route `ApplyUsdOps` and its one incremental projection,
reference-marker admission, and `UpdateUsdCurveView`'s stroke preparation and
terrain index-patch publication. Inspect `surface_annotation_incremental_worker`
separately from app-thread admission and render uploads. Fragment lookup uses
bounded spatial bins;
verify idle camera/terrain changes do not rebuild the stroke index.
Visited-marker recoloring uses the existing route-view key and must not resubmit
unchanged ribbon geometry.
Ribbon presentation preparation is presentation-only and must not issue another
`ApplyUsdTransientOps`, change document generation, or run inside the fixed
scenario event that observes a projected route. Compare the UI hook and fixed
tick against frame/physics budgets; a fast worker result does not excuse a
slow synchronous query or document edit in the input path.
For surface-annotation flicker, inspect `InspectUsdCurveView`'s
`displayed_surface_binding_count` and `surface_texture` across pending revisions.
Route edits and arriving wheel sources retain the displayed texture until the
patch commits; current-revision readiness remains a separate result.
The `route_surface_annotation` gate checks retained bindings and texture identity
alongside long-path publication and missing-coverage rejection. It edits one
vertex after admitting 2002 legs and verifies `index_updated_segments`,
`index_touched_nodes`, `index_patch_bytes`, `index_full_uploads` and actual render
`uploaded_patch_bytes`/`uploaded_patch_batches`, with `uploaded_patch_sequence`
at or beyond the source's `index_upload_sequence`. Ordinary movement must not copy
history, resize the texture or upload its full contents. `InspectVehicleTrail`
reports producer `annotation_edits`; compare that count with active wheels, not
retained sample count. Initial admission, capacity growth and explicit grid reset
are separate full-upload events.

For general `SpawnEntity`/`DeleteEntity`, measure the command, USD add/remove,
live structural reconciliation, and referenced asset admission separately.
Keep edit-to-projection wall time separate from frame time. A new referenced
entity may still be preparing its source plan across frames; each live instance
retains that prepared asset so later matching spawns are warm. Measure the
first spawn, a warm spawn, and deletion separately, while checking render-frame
and fixed-step cadence during each. Use the production spans
`scene_spawn_entity_command`, `usd_apply_ops_document`,
`usd_document_apply_change_set`, `usd_reference_layer_closure_merge`,
`usd_reference_instance_plan_remap`, `usd_reference_root_author`,
`usd_reference_instance_promotion`,
`usd_live_structural_reconcile`, `usd_visual_projection_batch`,
`scene_runtime_spawn_commit`, `usd_live_subtree_despawn`,
`scene_delete_entity_command`,
`scene_delete_entity_persist`, and `usd_apply_one_op_document` to separate
command, document, reference, projection, and removal costs. Warm instances of
one immutable asset recipe should bypass layer-byte cloning and comparison;
asset reloads use a new recipe identity and still merge changed bytes. Path
reconciliation uses the lifecycle-maintained stage/path index, not a full
population scan per changed prim. Raw-file spawn identity collision checks use
the lifecycle-maintained API identity registry and a hash set of queued roots,
not a world scan or a walk of all pending inputs. Document projection waits only for
references named by that edit's typed AddPrim operations; an unrelated
reference must not extend its completion time or document-projection hold. An
authoritative pending reference can retain its separate `SceneReferences`
simulation hold until its own instance is committed. A long completion wait is
not itself a frame stall, but an unready authoritative reference can hold
simulation progress. A normal document-backed spawn projects from the prepared
instance plan and authors only a lightweight live root; `usd_reference_root_author`
must not include reference composition. Measure `usd_reference_instance_promotion`
separately because the first later edit that needs full composed instance facts
performs that one-time promotion. Deleting an unpromoted root must not promote it.
`scene_runtime_spawn_commit` measures each changed prim's live ECS structure
insertion, including deferred reference descendants. `usd_visual_projection_batch`
measures the bounded per-update component and visual projection pass; keep its
frame cost separate from total time until the last descendant is admitted. The
`usd_reference_instance_plan_remap` span must remain independent of asset prim
count: an instance shares the immutable prepared snapshot and carries only its
namespace and root overrides. The stage test verifies snapshot sharing with
`Arc::ptr_eq` and verifies that an instance view exposes only the source asset's
`defaultPrim` subtree.

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
Disjoint layers prove irrelevance even when spotlight bounds are unavailable.
Uncertain bounds and boundary cases keep the map for compatible layers. Restore
the extracted main-world shadow intent before every frame's relevance check;
native light extraction is incremental, so focusing a parked preview must
restore its shadows without requiring a light edit. This saves only maps
provably irrelevant to all outputs; Bevy's main-world per-light caster
visibility pass is still a separate cost to measure.
Count shadow roots independently of active cameras. Invisible extracted local
lights retire their native `PointAndSpotLightViewEntities` through the renderer
adapter; Bevy's cleanup observer removes the associated views. A deferred
retirement must retain a light already re-extracted in that boundary. Verify
one/many parked-preview counts and text-to-Visual reactivation separately from
authored shadow intent or a camera's `is_active` flag.

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
For large procedural terrain fields, profile both scatter execution and its
deferred entity commands. Admit generated body bundles and visual components in
stable bounded batches while physics continues. Keep the terrain's applied
marker pending until bodies and required visuals are committed. Cancel queued
entries on refresh and teardown.
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
Reuse composed network-membership facts when admitting a member's own telemetry
declaration. Its generated wrapper owns the sampled alias; a metadata Scope still
needs a measured target. Use `ListTelemetryChannels` with an exact `name` when
checking one retained channel, so its metadata read stays bounded.

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
The editor-count diagnostic additionally checks it at one-second checkpoints
to reject a stall after an early advance, along with full readiness and stable
physics topology. It also rejects retained errors from `RuntimeDiagnostics`,
including scene-scoped telemetry delivery holds that do not fault physics.
That polling overhead is part of the diagnostic window.
`scripts/perf/usd_editor_tabs.py` compares yaw, pitch, target, distance,
projection, orthographic scale, measured viewport rectangle and window scale
factor both within a window and between equal-camera windows. Its negative
guard tests cover rotation, resizing, faults, retained runtime errors, late
waits and stopped physics.

Record the clean FPS window, physics and render timings, Tracy capture path,
scene/settings, and whether the result is startup or settled. Rebuild the
production binary after a code change and repeat one clean A/B plus one Tracy
diagnostic capture. Link the changed owner and state any platform/GPU evidence
that was not available.

For fixed-step versus UI diagnosis, query `SimulationTimingProfile` during the
settled owned run. It summarizes the latest 240 fixed ticks and 240 app-update
fixed-loop bursts: p50/p95/p99/max service time, per-tick realtime service-budget
exceedances, fixed steps per app update, and retained pending simulation duration
(`fixed_loop.latest_pending_simulation_secs`, whole ticks plus fractional
overstep). The owner drains at most 64 complete cycles per update while admitting
the full running delta. Its separate `Time<Virtual>::max_delta` diagnostic
detects unexpected clipping and should remain zero. Pair this with
`QueryTelemetryHistory` for `engine.frame_time`, which covers the full app frame.
Use `CosimStatus` step diagnostics to separate solver service from end-to-end
response latency. The execution host admits completions in `First`, after
message rotation and before `ClockProjectionSet`; compile requests remain in
the `Update` lifecycle cycle. Preserve session/source/communication-point
validation and the all-causal-participant hold when changing this boundary.
The profile is read on demand and is not a replay or UI-isolation verdict; a
nonzero clipped-time total means wall-clock demand was already excluded by the
clock admission, distinct from the recoverable pending duration.
When changing this boundary, exercise the rendered production
`fixed_budget_recording.rhai` gate with a fresh writable `output_dir`; it runs
25 FPS recording at 64x to require several budgeted updates per logical frame.
Require three saved frames, a witnessed 64-cycle burst and pending duration,
zero clipped demand, and no readback while whole cycles remain queued.

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

When `tick_rhai_scenarios` or `tick_rhai_scenario_visualization` has an outlier,
compare the nested `rhai_user_scenario_hook`, `rhai_user_event_hook`,
`rhai_native_task_tick`, `rhai_mission_event_projection`,
`rhai_prelude_scenario_driver`, and `rhai_user_visualization_hook` spans. Their
fields identify the scripted entity, hook/driver, and queued-event count. If
the outer scenario system is slow while these spans stay short, the time is in
driver traversal or elsewhere in the exclusive fixed pass; do not attribute it
to Rhai evaluation or move authoritative World access to a worker on that basis.

For exclusive systems that borrow `&mut World`, inspect individual Tracy event
timestamps and durations, not just aggregate averages. Align long calls with
Twin-open and readiness milestones: one startup outlier can stall UI even when
the same system is nearly free on settled frames. If the outer system is hot,
attribute time to its internal owner operations before choosing an async
boundary or cache.

For startup `drive_engine_sync` outliers, compare
`modelica_engine_completion_snapshot` (engine lock and parsed-tree snapshot)
with `modelica_engine_completion_install` (generation validation and document
syntax/index installation). The latter identifies the document and generation.
For physics telemetry, `physics_telemetry_source_retention` separates registry
retention from kinematics/contact collection and marks metadata discovery with
`metadata_dirty`. Compare first discovery with settled sampling before changing
the owner or its cadence.
`physics_telemetry_channel_metadata` covers first or changed descriptions;
`signal_scalar_channel_create` covers initial history/map publication, and
`signal_scalar_history_tail_growth` covers tail allocations. The signal registry
owns metadata values; the physics producer retains only a publication flag.
The tail span selects a full tail before bounded eviction and append, so a
capped non-allocating append can also appear. It is not an allocation counter.
Enable these diagnostic spans with
`RUST_LOG=info,lunco_modelica_core=debug,lunco_usd_sim_telemetry=debug,lunco_signal=debug`; they are
disabled by the ordinary info-level filter.

When `system_commands` dominates after a producer, inspect the number and shape
of its deferred writes. For a large set of entities receiving the same bundle,
use Bevy's existing batch command when its fallible/overwrite semantics match
the owner, and preserve the owner's iteration order when it determines stable
IDs. Relationship bundles such as `ChildOf` can use the same fallible batch:
Bevy runs their relationship hooks per entity, in batch order, while continuing
past stale entities.

For `sync_local_gravity_to_avian`, keep the comparison against the existing
`ConstantLinearAcceleration` component and batch only changed values in query
order. Measure its `system_commands` flush separately; removals remain driven
by the existing `RemovedComponents` streams.

For `reconcile_frozen_subtrees`, profile readiness freeze and release flushes
separately from the system body. Batch repeated joint, rigid-body, collider, and
ownership-record inserts in held-root traversal order, with joint disables
queued before endpoint body disables; retain the chained joint-release boundary
after body restoration.

For `pose_to_position`, treat `PhysicsPoseSeeded` as one-time readiness
metadata. Insert it only on the first valid physics-pose write; later external
pose updates still write position/rotation and update the bridge shadow, while
sleeping bodies wake through Avian's `Sleeping` removal hook.

For `apply_pending_forces`, leave already-zero accumulators untouched so idle
bodies do not publish a `PendingForces` change every fixed tick. Clear every
nonzero command after application or while physics is held, and retain the
terminal non-finite fault path.

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
channel's metadata is first created or changes. Reuse one `HashMap::entry` for
each sample's cached identity lookup and mutation, and update its global-owner
association only when the owner identity changes. Add the shared `SignalSource`
owner marker only when the entity first retains a sample; its removal observer
owns history cleanup, so later batches do not need to queue marker writes.

Dependent USD-stage refresh is owner work behind `sync_twin_overlays`. Snapshot
the base/runtime revisions, serialize each persistent source snapshot once on
bounded workers, share its bytes across dependent stages, and coalesce changed
layers per target stage. Compare recipe bytes on workers and skip unchanged
rebuilds. The main-thread owner validates source revisions, operation/revision,
and target-plan identity before opening the live stage and committing the plan.
Measure serialization and plan preparation separately from the live-stage
build, reset owners, and visual projection; do not move the thread-affine stage
across the worker boundary or let completion order select a commit.
For a first-mounted document with a non-empty view layer, profile
`view_ops_since_source_baseline` separately from the global operation journal.
The initial recipe already contains base and runtime layers; only the bounded
view suffix needs replay, while an expired view suffix still requires a full
composed-source rebuild.

For `project_usd_policies`, a cold cache can use the worker-prepared plan as its
baseline only when every `UsdSceneChangeBatch` from generation zero to the live
generation is present and proves that no policy prim changed. Existing cached
facts use the same complete-generation and affected-path checks when promoted.
A missing batch, generation gap, plan replacement, or policy-affecting change
must use full live extraction. Compare startup and settled edit captures after
changing this path; compile evidence alone does not establish a timing gain.

For movement across terrain LOD bands, inspect
`rebind_changed_shader_look` and source reflection before blaming picking or
physics. `shader_source_validate` and `shader_source_schema` execute once per
loaded shader revision; streamed tiles and material replacements reuse their
facts. Mark the exact idle/movement windows in Tracy messages, keep the initial
avatar pose fixed, and seed a pointer in SceneView when reproducing native
picking. Distinguish shader reload events from material/look changes. Verify
invalid-stage diagnostics and hot-reload invalidation as well as frame time.
