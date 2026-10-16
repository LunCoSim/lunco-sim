# 61 — Scene lifecycle and teardown

> Status: Active · Audience: contributors on scene load/unload and `SceneTeardown`

What a scene OWNS, and what has to be given back when it unloads.

## The invariant

> Anything a scene load writes belongs to that scene, and must not be visible to
> the next one.

Loading scene A and then scene B must leave nothing of A in force. When that
fails there is no error to read: the scene simply behaves as though it were
still the previous one — a rover that inherits the last scene's gravity, a
diagnostic that reports a conflict with a prim that no longer exists.

Scene state comes in two shapes, and each has its own mechanism.

## Entities — structural ownership

Entities are despawned. The rule is structural rather than enumerated: a
subsystem TAGS what it spawns, and teardown despawns that set. The celestial
subsystem is the worked example — everything it creates carries
`CelestialDerived`, so a reload removes exactly what it added without teardown
needing to know what a celestial scene contains.

`scene::clear_scene_entities` (`lunco-usd-bevy-runtime-core`) drives this, and is shared by
`LoadScene` (clear-before-reload) and `ClearScene` (clear-to-empty).

`SceneMountState` records the roots admitted by the current transaction. A
replacement clears that set before deferred despawns; projection systems use
the set to reject late work from the outgoing root. Additive preview mounts
remain explicit and do not become the active running scene.

Camera-path gate and playback clocks are linked descendants of their source
curve. Retiring the source reclaims both, including on stage re-projection;
their affine clock-parent references do not establish lifetime ownership.
The camera-path discovery cache uses the registered scene-resource boundary too.

Joint topology follows the native USD resync scope: resyncing a source or any
ancestor, including the stage root, invalidates its cached joints and wheel
facts. Document undo and source replacement must not retain removed constraints
as admission requirements.
The topology index, preparation task store, pending simulation projection work
and ground readiness flag register through `init_scene_resource` at their
owners. Shutdown cancels queued topology jobs and releases their progress keys;
the final reset drops all cached rows and completion handles, preserves only
the monotonic worker operation counter, and rearms source discovery.
Native joint retirement also applies Avian's island wake command, matching
its normal joint-removal hook. Surviving endpoints resume integration when a
sleeping constrained assembly is detached.

## One transition boundary

`lunco-core::SceneTransitionIntent` is the typed in-process request boundary.
Higher-level domains such as tutorials emit `Load`, `Clear`, or `Restart`
intents; `lunco-usd-commands` is the only owner that translates them into the concrete
USD commands and resolves scene identity. No subsystem sends a command name or
JSON payload to another subsystem.

The owner publishes `SceneTransitionStarted` before teardown and publishes
`SceneTransitionCompleted` or `SceneTransitionFailed` from the authoritative
transaction. Each edge carries the coordinator's monotonic
`SceneTransitionId`; the asset outcome, coordinator close, and simulation
progress hold all use that same id. A repeated path is a new transaction, and
a late terminal edge cannot finish or unpause its successor. All three scene commands—`LoadScene`, `ClearScene`, and
`RestartScene`—use this boundary, so tutorial/runtime owners cannot miss a
transition merely because it entered through a different command.

Failed loads leave the viewport in its normal empty-scene presentation. The
typed transition failure and runtime diagnostics retain the cause; the failure
text does not replace the empty-state headline.

An asset-load success is not a scene success. The terminal owner checks
`UsdSceneProjectionFailed` for the running mount, including prims reparented
outside its USD hierarchy, before committing a generation. A rejected root or
prim clears the mount and publishes `SceneTransitionFailed` with the cause.
Preview projection errors remain outside that transaction. Explicit projection
targets must name an existing absolute USD prim (or the stage pseudo-root).

The coordinator advances `completed_generation` only when the active matching
transaction reaches `SceneTransitionCompleted`. It then emits
`SceneTransitionCommitted`; stale completions and failures cannot advance the
scene generation or arm runtime work. Cycle owners use this shared generation
for Twin-scoped execution context instead of maintaining subsystem-local
counters.

`lunco-core-runtime::SimulationProgress` holds the causal clock from the start
edge through the asset and structural-projection terminal edge. CPU mesh builds
and other presentation streaming continue independently; scene geometry still
pending after structural projection is reported under `SCENE_VISUAL_SOURCE`.
Visual-readiness consumers keep reading that source after scene admission, while
the workbench treats terrain and scene-visual progress as a loading notice only
for an admitted or active scene transition. `TimeTransport` keeps the user's
play and rate intent while the gate pauses `Time<Virtual>`. Physics readiness
stays in its existing owner and continues to control physics admission; the
scene progress gate covers lifecycle work that must not consume simulation
ticks.

Consumers that own transient execution must wind down at `Started` and attach
again only from `Completed`. The tutorial launcher preserves the active
lesson's resolved source across `RestartScene`, then recreates its host and HUD
on the restarted scene; a failed restart clears that lesson and reports the
typed failure.

## Twin-owned scripts and resources

`SceneTeardown` retires programs projected from the outgoing scene. `TwinClosed`
retires longer-lived resources admitted by that Twin: asset-backed tutorial
scenarios, their source handles and documents, queued one-shot requests,
timelines, tool libraries,
Modelica compiler source sets, and Twin-owned HUD state. A Twin tutorial gets
its own script host instead of replacing an application-owned `WorldRoot`
scenario. Each script stores the stable Twin ID separately from the scene
generation, so its final `on_stop` still routes to the outgoing Twin after the
workspace has switched. Pending asset and compiler work is canceled or fenced
at that same owner boundary.

USD-authored policies retire through the projector's existing reconciliation
path during domain shutdown, after projected scripts stop. This restores the
retained application/Twin hooks and reconciles policy-owned synthesizers before
another scene can execute them. The projector's source handles, prepared facts
and previous-source signal are one registered scene resource. Reset requests
one projection pass even when a surviving source has the same asset identity.
Environment projection likewise registers its authored opinions and source
snapshots, releasing retained plans and invalidating its previous-source signal
at the final reset.
The synthesizer bridge reconciles whenever the effective policy registry
changes. Its application-lived bookkeeping records actual registrations, so
it can remove outgoing names even if Twin closure already removed their policy
definitions; it retains no source text, compiled hook or scene plan.

Live inline bindings use the policy registry's existing application/Twin layers,
selected by the caller's typed route. Twin mutation requires the exact active
Twin and committed scene generation; retired shutdown callbacks cannot recreate
a binding. Application callers cannot mutate a Twin-owned or USD-authored
effective policy. Twin closure restores retained application implementations.

The backend registers `ScenarioDriver::retire_removed_model` for attachment
removal. Interactive/API programs attached to retiring hierarchy entities stop
during the removal flush, before registered scene resources reset, even though
their source documents remain independently open. Stable application hosts keep
their explicit application lifetime. Attachment removal reuses the driver's
idempotent stop operation; it cannot defer an outgoing `on_stop` into the next
scene's driver tick.

`TwinClosed` closes every document whose stored runtime owner is that
`LocalTwin`: the workspace owner dispatches `CloseDocument` to the domain owners,
then removes the metadata. Workspace replacement drains these handlers before
`TwinAdded` and also closes loose and library editor documents without deleting
shared source caches; replicated documents follow their replication session.
USD previews snapshot document ownership at the close edge, before deferred
registry removal. `DocBackedTwinScenes` records document-private asset mounts
separately from workspace and preview leases. The final preview lease releases a private mount when no workspace projection
owner remains. `DocumentClosed` releases it regardless of projection leases;
shared workspace authorities remain workspace-owned. `DocumentClosed` is the
sole registry-closure owner for this bookkeeping. Modelica core owns document
closure and linked-entity
retirement in both headless and UI hosts. File reads capture a
`FileDocumentAdmission`, and completions whose admitted runtime owner has
retired are rejected. Pending workspace restore is canceled at that same Twin
edge. Closing a non-active Twin does not clear the active simulation scene.

The USD browser removes each published document row at `DocumentClosed`, before
the next producer update. At active `TwinClosed`, its Scene file owner clears
published rows, refresh requests and capacity waits, and drops the pending task
handle. Scope changes and an empty source set use the same retirement path;
operation identities stay monotonic. A retired preparation cannot occupy the
replacement's pending task slot or publish into it. Shared application library
caches retain their application lifetime.

The shared scene catalog retires closed-mount metadata, spawn entries and shader
choices at `TwinClosed`, preserving engine and surviving-mount entries. It also
invalidates pending listings and metadata batches at that edge, so a late scan
cannot republish the closed source. Spawn category indexes follow the retained
entries. Private document mounts close without `TwinClosed`: the existing
mount-set change detector prunes their catalog rows before dispatching the
replacement listing, including shader choices and queued program listings.

SysML document closure uses the same domain command for clean files and dirty
drafts; its Twin source loader owns only pending asset handles. Modelica document
closure cancels parse preparations, save-close continuations, and document-owned
modals before dropping editor state. Source text tabs are not workspace
documents: tabs and queued filesystem opens under the closed Twin root are
retired, including files opened directly by path without an explicit Twin origin.

Core and application scripts keep their own owner scope and stable generation
across Twin scene transitions. Guided presentation clears at transition start
and registers `GuidedOverlay` as a scene resource, so its final reset also drains
writes from shutdown and entity-removal callbacks. Runtime dropdown state uses
the same final reset; only active `TwinClosed` clears it immediately. Objectives
visibility remains an application preference. Scene teardown closes source
documents explicitly marked as scene-owned, and removal of any retiring host
stops its execution. Twin teardown closes programs explicitly marked with that
Twin ID. A shared source or a
currently active Twin does not make an unmarked application script Twin-owned.
The application Tutorials menu captures the active Twin at selection time and
starts that tutorial instance under that Twin's lifecycle owner; this does not
change the scope or lifetime of the application tool module.

Deferred snippet and tool-call admission, ownership validation, and terminal
close results are defined in
[`rhai-integration.md`](rhai-integration.md#how-to-load--run-a-scenario).

Scene-time selection runs once at the completed load/restart edge, after the
stage dependency load and queued structural projection have drained. A
completion notification without a matching loading phase is ignored; reloading
the already-active stage therefore leaves its selected time unchanged. The time
owner holds the physical loop, USD animation sampling, celestial
placement, and USD DEM construction until the composed root policy result is
installed. CPU-generated render meshes may still be streaming at this point;
they are not inputs to the scene-time decision. A clear or failed load leaves
the scene-time gate closed.

## Everything else — the `SceneTeardown` schedule

Resources, caches, worker-side handles, and subsystem-derived entity trees are
not covered by the USD prim sweep. The dependency-light core lifecycle boundary
runs domain `SceneTeardown` shutdown before outgoing prims are despawned, then
resets registered resources after removal callbacks and before replacement
projection starts.

Initialize scene-derived resources with `AppSceneExt::init_scene_resource::<R>()`,
and implement `SceneResource` at the owning type. Initialization installs cleanup
once for that type; a new scene resource does not need an entry in a central
cleanup list. The default `clear_scene` replaces the value with `Default`,
dropping collections, source handles and pending tasks. Mixed-lifetime stores
override it to preserve only explicitly application-owned state and monotonic
publication counters. Settings, shared source caches and application registries
keep their application lifetime.

Scene-derived message queues use the same registration:
`app.add_message::<M>().init_scene_resource::<Messages<M>>()`.
Their reset clears unread payloads after shutdown and entity-removal callbacks,
while preserving Bevy's message counters so surviving readers consume new
messages correctly. Unregistered application queues retain their normal lifetime.
The Modelica execution owner registers `Messages<ModelicaNotice>` this way;
queued lifecycle notices cannot repopulate cleared status after teardown.

`run_scene_teardown_with_entities` first drains domain `SceneTeardown` systems
and their deferred shutdown work, then reclaims the supplied scene hierarchy
and `SceneOwned` entities, flushes removal callbacks, then runs registered
resource resets. `run_scene_teardown` uses the same boundary without a supplied
hierarchy. This ordering prevents terminal callbacks from repopulating an
already-cleared store. Tag scene-owned entities outside the USD hierarchy with
`SceneOwned`; their linked children are reclaimed too. All scene transition
routes must use this owner boundary, including same-path restart and active
Twin closure. Closing an inactive Twin or rejecting a replacement does not run
this global scene cleanup.

Telemetry histories and metadata default to scene lifetime, including archived
publishers and metadata published before the first sample. Only signals
explicitly registered with `SignalRegistry::retain_across_scenes` survive;
engine FPS and frame time use that application lifetime. The registry publishes
normal descriptor removals and keeps its catalog revision monotonic. Modelica
telemetry sessions, physics kinematic cursors, sampler plans, queued delivery,
focus, browser workers/indexes and transient browser requests use the same
resource registration. API-created channel entities carry `SceneOwned`.

`StatusBus` registers as a scene resource at its owner. Clear and reload retire
visible history, progress, outcomes and handles through that boundary. Windowed
and offscreen hosts both install the owning `StatusBusPlugin`; closing
the active Twin invokes the same reset. Publication counters remain monotonic,
and late handle completion cannot replace successor status.

Runtime exposures follow the same lifetime contract. `EngineExposures` removes
scene namespaces, subjects and properties, including hidden surfaces, at the
registered resource reset boundary. Only namespaces explicitly declared with
`EngineExposures::retain_across_scenes` survive; health, application cadence and
camera status use that lifetime. Authored topology/program caches and seminar
trace state are registered scene resources too. `ExposureRefresh` resets all
invalidation domains and requests an immediate first publication. Independent
change-gated producers also wake once when their namespace is absent; dataset
consent uses that condition so surviving application downloads republish while
retired Twin scope data remains removed. Editing away an authored surface
withdraws its namespace through `EngineExposures::remove_surface`.

Independent editor Modelica documents may survive scene reload. The telemetry
owner requests one projection pass when its scene cache resets, so paused
sources republish current state without advancing their solvers. Its typed
document-to-signal attribution outlives live producer removal within the scene;
`DocumentClosed` retires those exact references, including archived history,
without clearing channels owned by another document or producer.

Completed experiment trajectories remain in the experiment registry, while
their active playback is scene-owned. The runner registers `PlaybackEntities`
as a scene resource and the UI publisher tags each playback entity `SceneOwned`.
Reload drops both the map and entities; a later completed run publishes through
a fresh entity. `DocumentClosed`, `TwinClosed`, and replication retirement use
the same publisher-owned playback release path without deleting run history.

Visualization widgets that only receive an egui context store scene-derived
data through `lunco-viz::scene_state::scene_data`, a single scoped egui
`IdTypeMap`. This includes history snapshots, async plot tasks, canvas interest
and value snapshots, pending input writes and plot-drop requests. The viz owner
tracks Bevy egui context insertion, replacement and removal, including contexts already present
at plugin installation. Its registered scene resource removes the entire store
after callbacks and entity retirement, including removed contexts. Hidden
widgets therefore release buffers and queued actions without another paint;
new entries in that store inherit its lifetime. Telemetry drag payloads and
native plot interaction memory retire at the same boundary, so outgoing zoom
bounds cannot hide a restarted clock. Layout preferences and visualization
configurations keep their application lifetime.

Editor cursor modes, armed tool names, context menus, sub-selection and
cinematic targets use the same registered scene lifetime. A replacement disarms
them before another scene can interpret the same tool or asset name. Brush
dimensions remain explicit application preferences. Entity-tree snapshots,
pending tree builds, port snapshots, row indexes and edit drafts are resources
rather than fields on application-owned panel objects; hidden panels therefore
release outgoing data without painting. Tree publication revisions remain
monotonic across reset and are independent of worker submission revisions.
The live Inspector snapshot is a registered scene resource. Parameter drafts
belong to an explicit preview and document, survive unrelated scene reloads,
and retire immediately at `DocumentClosed` or when the preview owner removes
or replaces that session. Draft retirement runs independently of panel paint.
Shared Inspector shader schemas follow their asset lifetime: shader removal
or modification evicts the cached entry independently of panel visibility.

Connections separates the live scene graph from isolated document-preview
graphs. Scene retirement drops the live graph and its async layout/file jobs;
preview state survives only under its document lifetime. Every async result
pins the structural root, stage and optional preview document in a typed source
lease. A matching document/layout revision is insufficient when that lease has
retired. Document closure releases its preview graph and workers. Retiring a
pending view-file operation publishes its terminal command failure before
dropping the handle. File I/O already executing may finish at its captured
file destination, but cannot publish into a replacement graph.

Live document edits use a separate, stage-scoped boundary; they do not invoke
the application-wide `SceneTeardown`. Before a full-stage re-projection, the
generic USD runtime asks registered domain owners to retire their derived state.
An owner promotes a local prim refresh when its physics topology crosses that
subtree. Avian retires joint graph edges, native joint components, and collider
markers before the stage entities are replaced, then normal projection admits
the new constraints. A failed owner preparation aborts replacement, reports a
structured diagnostic, and safety-holds the mounted simulation.

Celestial state follows the same boundary. Its derived tree is retired,
`OrbitalViewPin` is cleared, and `ActivePhysicsFrame` is restored to the
persistent `WorldGrid` unconditionally; this is not inferred from a later frame
with no body declarations, because a restart can replace old declarations with
new declarations in one transaction. The outgoing avatar owns the transactional
camera return state; retaining only the scene-level orbital pin would make a
replacement surface scene present as orbital without a valid return transaction.
Scene-scoped `RuntimeDiagnostics` is cleared at the same boundary. Each producer
then repopulates only its own findings, so a camera, environment, or physics
error from the outgoing scene cannot be displayed as a fact about the replacement.

Scene-requested dataset reads follow the same boundary. Rhai selects datasets
at scene completion; the generic asset runtime loads their canonical URIs and
retires outstanding read completions at `SceneTransitionStarted`. Domain-owned
data derived from delivered text is cleared during `SceneTeardown`. The
application asset lifecycle policy requests only the declared artifacts needed
by the completed scene.

Windowed presentation state follows the same ownership rule. A scene authors
its initial camera selection through `CameraTrack` or an explicit operator or
director request. When none exists, the `camera.default_presentation` Rhai
policy prefers the unique `LocalEmbodiment` camera, then the first camera under
the active root in stable USD-path order. A standalone host may retain generated
framing when that root has no authored camera. Rust validates the policy result
and applies it through the existing camera-selection mechanism. One camera
contract owns readiness and findings; `RuntimeDiagnostics` and Recent Events
are projections of that state, and camera findings do not enter the
runtime-error warning path. Scene teardown resets the selection intent, and no
engine-created camera can survive into the next Twin.

The retained runtime UI uses the same invariant. When an exposure, perspective,
gate, or placement makes a surface invisible, the bridge removes the whole
retained HUI root whenever any HUI state is present. Visibility flags alone are
not ownership: a deferred rebuild or teardown can leave a root whose local
mounted bit is stale, and that root must not survive into the next presentation
state.

## Transition admission and schedule boundary

Scene commands never mutate the world in the caller's schedule. API, UI, Rhai,
and tutorial entry points submit a typed `SceneTransitionRequest` to the single
`SceneTransitionCoordinator`. The coordinator retains one active transaction
and one latest pending request. It publishes `SceneTransitionAdmitted` only from
the `First` lifecycle phase; private lifecycle observers execute that admitted
request, followed by an explicit deferred-command flush before normal projection
schedules run. Public command handlers therefore have one role—submission—and
contain no execution-mode marker, retry branch, or mid-frame mutation path.
Invalid `LoadScene` addresses and a missing asset pipeline return terminal
command rejections with a warning before admission; they never acknowledge a
load that cannot run. Native filesystem paths and file URIs enter through
`OpenFile`.

Loading closes from the authoritative asset/structural-projection outcome in
`Last`, after normal projection schedules have finished. Async CPU mesh builds
are presentation work and do not extend this transaction. `Last`'s deferred-command flush
publishes `SceneTransitionCompleted` or `SceneTransitionFailed` and admits any
pending request; only the following frame's `First` lifecycle phase can execute
it. A second request therefore cannot replace an in-flight stage, and consumers
do not need staging markers, stale-entity guards, retries, or per-frame recovery
checks. Presentation is a separate admission concern: a missing authored
camera leaves the loaded scene in an explicit no-camera state and does not turn
successful USD projection into a false asset-load failure.

Closing the active Twin retires its scene owner immediately: `SceneOwnerRetired`
closes that owner's scene-time gate and pauses its transport, the mount fence
rejects outgoing projection, and pending document admissions and scene requests
are canceled. An in-flight transaction receives its matching failure edge before
`ClearScene` is admitted for the next `First` teardown. Late results cannot
complete that transaction. Closing an inactive Twin only releases its documents
and admissions. Retirement edges carry the same typed transaction identity, so
an old close cannot stop a replacement scene's clock.

USD visual projection is the deliberate exception to all-at-once scene
materialisation. The asset loader composes the fetched layer closure on its
worker and publishes a `UsdStageProjectionPlan`, an immutable `Send` snapshot
of the composed hierarchy, default-time attributes, transforms, bindings, and
animation topology. `sync_usd_visuals` and `process_queued_usd_visuals` perform
only bounded ECS/resource binding from that snapshot; they do not parse USD,
walk a live stage, or resolve composed values on the UI thread. CPU geometry
that is safe to detach remains on the async compute path, and Bevy asset
mutation is committed on the main thread. The live OpenUSD stage is still
`!Send` and is retained only by `CanonicalStages` for authoring and explicit
incremental edits after the initial snapshot.

`UsdVisualProjectionSettings::frame_budget` bounds prim binding because entity
allocation and Bevy asset mutation are main-thread responsibilities. A prim
count cap bounds the deferred command batch applied after that system returns;
the time budget alone cannot bound this flush when many prims bind quickly.
`child_spawn_budget` and `max_child_spawns_per_update` bound direct-child
admission and its separate deferred command flush. Defaults are 4 ms and 64
prim work items per update, plus 1 ms and 128 child entities per update. Each
parent records children in stable composed-path order and keeps its
`UsdSceneAwaitingStage` and `UsdSceneProjectionQueued`
fences until every direct child has entered the ECS queue. Admission re-reads
the current composed stage. Bevy child-set changes refresh that parent's live
child keys, so a structural sink that creates a child while the continuation is
waiting cannot produce a second projection. The same composed path can
legitimately occur in separate mounts or runtime instances, where the parent
hierarchy and instance identity scope it.
A referenced runtime instance admits its native reference arc before sibling
topology. The scene owner retains source bytes and addresses;
descendant composed reads use the same stage owner as the containing scene.
Mesh work sourced from that immutable instance plan is fenced by the plan and
stage identity, not by later generations of the containing document; runtime
placement and disposable presentation edits therefore cannot cancel an
unrelated marker or component mesh task. Geometry read from the canonical
stage remains generation-fenced and is discarded when its source changes.
`UsdSceneAwaitingStage` remains on queued prims and on parents with child
continuations, so the authoritative stage outcome is retained until the
projection queues are empty. The workbench reports the
indeterminate loading/projecting phase, and a clear transaction reports
unloading, rather than presenting a partially projected scene as ready.

Authored control and program projection use the same composed reader contract:
the prepared projection plan supplies facts before the live canonical stage is
installed, and the canonical reader supplies later edits. Reference namespaces
and root opinions come from native USD composition. Prepared plans capture
those facts directly; referenced runtime entities use the live scene reader.

Doc-backed Twin admission is event-driven at the asset boundary. The Twin source
is loaded as `UsdSourceBytes`; `AssetEvent` marks successful availability and
`AssetLoadFailedEvent` closes the pending document transaction with its error.
Only after that terminal event does the owner publish the composed document
overlay and submit `LoadScene`. Referenced stage assets use the same event
boundary before their reference is authored onto a live stage. This keeps source
bytes in the asset pipeline and removes frame-count timeouts, per-frame load-state
polls, and main-thread filesystem reads from scene admission.

After admission, document edits wake the single `twin_projection` owner through
`DocumentChanged`; stage lifecycle events wake it when an edit is waiting for a
prepared asset. The owner consumes that wake while authoring the typed delta and
the live projection sink refreshes ECS. It does not scan document generations on
the render loop, and the viewport does not maintain a second edit path.

The native `--scene` entry point follows the same boundary: `setup_luncosim` only
resolves the owning root and queues the shared asynchronous Twin scan. The
filesystem walk and `TwinMode::open` index never run in `Startup` on the UI
thread. A scan failure is surfaced through the shared `TWIN_OPEN_FAILED`
telemetry edge; the explicit startup guard turns it into a fatal startup result,
while later user-requested opens remain recoverable.

Teardown also avoids duplicate ownership work: the grid hierarchy is despawned
recursively, while the stage-identity sweep only handles active-scene prims
that are outside that hierarchy (for example, a detached camera during a
reparenting window). This keeps the ownership fence required for deferred
reparenting without queuing the same scene prim twice.

Scene safety state follows the same boundary. `lunco_core::RuntimeFaults` records
the first terminal physics/runtime failure for the active scene, while
`PhysicsHolds::SAFETY_FAILURE` stops unsafe stepping. The USD simulation owner
clears both in `SceneTeardown` before the replacement scene integrates. A bad
scene therefore stops safely, but cannot become a process-wide load lock; there
is no restart-process fallback or mutation-rejection shim in the scene commands.

The safety sequence is intentionally one-way within a scene: a terminal fault
pauses native physics with a zero delta and holds shared simulation admission.
Interaction and presentation remain available for diagnostics and lifecycle
commands. The invalid scene is not repaired or resumed. `SceneTeardown` clears the fault and its safety hold, and
the physics owner separately resets scene-owned holds and the physics clock.
The time owner registers `SimulationStepControl` as scene state, so final
reset retires shared deliberate-step demand and manual holds after terminal
callbacks and before replacement admission. This is recovery of the session boundary, not recovery of invalid runtime state. The contract is covered
by `lunco-physics::terminal_runtime_fault_pauses_physics_until_cleared` and
`lunco-usd-sim::fault_then_scene_reload_can_admit_a_replacement_runtime`.

Possession claims follow the same identity boundary. `SessionRegistry` is keyed
by stable `GlobalEntityId` values, so `lunco-avatar` clears claims for outgoing
`UsdPrimPath` entities during `SceneTeardown`, before deferred despawn. The
replacement scene can therefore reuse deterministic ids without inheriting a
prior lesson's driver, while non-scene ids remain untouched.

The same boundary clears rollback markers, prediction input logs, host jitter buffers, applied-input acknowledgements, render-leading drive
values, simulated intents, and control-path registry. Physics clears its holds
and restores a zero-delta physics clock; the time owner retires shared step
demand. `SimTick`, `Time<Fixed>` and admitted fixed-time balance remain continuous;
the time owner reanchors the replacement mission epoch at the current tick.
These are scene-owned control surfaces, not session identity; leaving any of them
behind would allow a replacement entity or reused global id to receive an old
command or remain silently held.

It is a **schedule**, not a registry, and that choice is the design:

- Bevy already expresses "run these systems at this lifecycle edge" — that is
  `OnExit`. Scene load here is a command rather than a state transition, so this
  is the same idea under an explicit label.
- The reset lives **beside the code that writes the state**. A central registry
  would put every subsystem's cleanup in one file that no subsystem author
  edits, and the state that gets forgotten is always the one whose owner never
  looked there.
- `SceneResource` identifies registered state; `SceneTeardown` identifies custom
  domain shutdown, restoration and cross-resource work.

```rust
use lunco_core::AppSceneExt;

impl lunco_core::SceneResource for MySceneCache {}
app.init_scene_resource::<MySceneCache>();
```

`add_systems` creates the schedule on first use, so no crate has to initialise
it or coordinate with the others.

### Remove, or restore?

Which disposition is right depends on who OWNS the value.

| | When | Why |
|---|---|---|
| **REMOVE** | State that only means something while a scene is loaded — caches, provenance records, "which prim set this" bookkeeping | Absence is its correct empty state |
| **RESTORE** | State the app installs at start-up and a scene merely OVERRIDES | Removing it would leave the world with no value at all |

Gravity is the type case for the second. A scene SHOULD override it — that is
what its `UsdPhysicsScene` is for — and must not leave the override behind. The
app registers its own start-up value as the baseline:

```rust
.insert_resource(SANDBOX_GRAVITY)
.add_systems(
    SceneTeardown,
    |mut commands: Commands| commands.insert_resource(SANDBOX_GRAVITY),
)
```

`PhysicsSceneGravity` — the record of WHICH prim set gravity — is the first
case. Carried into the next scene it would make a fresh `PhysicsScene` look like
a conflicting duplicate of a prim that no longer exists.

## Verification

USD stage changes reconcile the running projection and every explicit preview
root through one structural projector. Indexed stage/path candidates are scoped
to the consuming preview hierarchy; subtree refresh updates all instances.
Framing also requires that preview's own hierarchy. Preview readiness requires
both the document's consumed projection cursor for the exact stage and completed
visual work under that root. Document and cursor changes wake the readiness
producer even when no mesh or transform changed.
Consumers wait for this readiness before composed preview inspection, then
compare the document generation. An allocated asset handle does not establish
that its dependency recipe has finished preparation.

The generic viewport seams use one shared explicit quality-profile fixture and
wait for exact document generations. The production
`usd_material_edit_projection` Rhai gate creates and removes geometry in two
same-document preview leases and checks projected mesh bounds through framing;
removed geometry must disappear from both views without replacing their stage.
`preview_domain_isolation` uses a closed electrical fixture with explicit solar
inputs. It verifies that preview copies and rejected connections cannot admit a
solver or replace the live network. Its root-rename plan includes `SetDefaultPrim`
in the same compound edit so the preview's projection target remains valid.

During stepped execution, the production scene runner stops at a terminal
`RuntimeFaults` boundary.
Its existing expected-fault registry distinguishes authored negative fixtures
from unexpected failures; it does not pump frozen simulation ticks while a
terminal fault is active. Same-process recovery still needs an owned API
session or the generic lifecycle seam.

Generic `scene_lifecycle` tests at the core, scripting, exposure, guided UI and
runtime UI owners cover repeated reset, removal/shutdown callbacks, independent documents,
pending preparation and preserved application state. These are engine ownership
invariants; authored Rhai retains control over script behavior. The production
`scripts/api/test_twin_session_retirement.py` driver exercises Twin replacement
and full reload through the public API with verdicts in
`assets/scenarios/tests/twin_session_retirement.rhai`. Its exposure probe authors
an unsaved registered surface field with `ApplyUsdOp`, proves publication,
renames the namespace and proves withdrawal, then verifies removal across Twin
replacement, same-path reopening and `LoadScene` while application namespaces
survive.

Deferred Modelica compile admission validates its pinned source owner and
participant before consulting worker failures or publishing diagnostics.
Retirement cancels the request; it cannot raise a fault after scene teardown.
Current requests still report actual compiler and worker failures.
`scripts/api/test_twin_loading_replacement.py` covers early replacement and
clear, late physics/compile admission, and recovery after a terminal Modelica
initialization fault. It verifies immediate and delayed empty-state readiness,
physics retirement, and a freshly projected successor after each clear.

`scripts/api/test_modelica_lifecycle.py` runs the parameterized native-worker
retirement and source-duplication gates in sequential isolated sessions. The
native-worker gate's Command host verifies exact scene projection before
signaling its retained Application orchestrator. Authored Rhai proves cancellation of running and queued
outgoing jobs, source retirement, same-name successor isolation, and rejection
of imports from the closed owner.

USD document Save and Save-As publish `DocumentSaved` from the command owner
after the durable write and saved-baseline update. `lunco-usd-commands` alone
mirrors save/change/close events into workspace metadata in every host; the UI
owns browser presentation. This keeps dirty indicators and document retirement
consistent across native API, headless scene tests, and workbench consumers.

Native removal observers must inspect Bevy's destination archetype before
reactivating a disabled constraint. A removal during despawn has no destination;
a compound removal may also remove the joint or its identity. Avian's joint
admission observer therefore requires a surviving enabled native joint and
identity in the destination archetype. This applies to preview closure and
cancelled admission as well as full scene retirement. Teardown queries include
Bevy-disabled entities, which still own native components and removal hooks.

The physics owner registers scene-derived continuous contacts and diagnostics,
constraint order and position scope, dynamic island masses, escape bounds and
reports and holds. The time owner retires shared step demand. Final teardown
drops retained collider geometry and re-arms topology discovery even if no replacement physics tick occurs.
Physics integration settings retain their explicit application lifetime.

Live authoring resolves its document from the entity's exact projected stage
lease in `DocBackedTwinScenes`. The shared composed reader supplies prim validity and
composition-arc ownership for move, rotation, complete pose, property, connection
and deletion persistence. A retired stage cannot acquire another document by
reusing its name, and editor focus does not select a scene mutation destination.

`lunco-usd-commands` registers pending document-dependency preparations as a
scene resource. Its teardown observer first cancels admission keys, rejects
outstanding command receipts and releases `UsdDocumentAuthoring` holds; resource
reset then drops continuations and result slots. Workers hold weak publication
slots and cannot retain completed recipes after their request retires. Document
close applies the same boundary to the addressed document. Independent document
work explicitly survives unrelated scene teardown, retaining its completion
channel; closing its owning Twin retires it immediately. Source leases capture
scene generation only for stages in registered scene mounts. Before commit, the
owner validates its exact document and stage lease, including resolver revision.
Pending discards use that same lease and scene-resource boundary. Closing their
document or saving it rejects and drops the pending source read; a stale discard
cannot reset newer opinions or history.
The render-free scene owner supplies `is_authoritative_scene_stage` to both
projection and document preparation. It reads the exact active mount and stage
identity, so preview/import work cannot hold the simulation, and an old root
loses authority before its deferred despawn even when a new root reuses its path.

Async exposure preparation and guided HUD cards register scene resources at their
owning packages. Exposure invalidation cancels queued admission and drops its
result receiver immediately; running workers cannot publish into a retired
scene. HUD cards, pointer subscriptions and context-menu roots tag their detached
callback entities `SceneOwned`, so final teardown also retires endpoints created
by terminal callbacks. Camera exposure retention remains an explicit producer
contract.


## See also

- [21 — domain: USD](21-domain-usd.md) — USD as source of truth, ECS as projection
- [`author-usd-physics`](../../skills/author-usd-physics/SKILL.md) — the authoring side, including gravity per scene
