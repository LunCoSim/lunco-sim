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

`clear_scene_entities` (`lunco-usd-sim::cosim`) drives this, and is shared by
`LoadScene` (clear-before-reload) and `ClearScene` (clear-to-empty).

`SceneMountState` records the roots admitted by the current transaction. A
replacement clears that set before deferred despawns; projection systems use
the set to reject late work from the outgoing root. Additive preview mounts
remain explicit and do not become the active running scene.

## One transition boundary

`lunco-core::SceneTransitionIntent` is the typed in-process request boundary.
Higher-level domains such as tutorials emit `Load`, `Clear`, or `Restart`
intents; `lunco-usd` is the only owner that translates them into the concrete
USD commands and resolves scene identity. No subsystem sends a command name or
JSON payload to another subsystem.

The owner publishes `SceneTransitionStarted` before teardown and publishes
`SceneTransitionCompleted` or `SceneTransitionFailed` from the authoritative
transaction. All three scene commands—`LoadScene`, `ClearScene`, and
`RestartScene`—use this boundary, so tutorial/runtime owners cannot miss a
transition merely because it entered through a different command.

Consumers that own transient execution must wind down at `Started` and attach
again only from `Completed`. The tutorial launcher preserves the active
lesson's resolved source across `RestartScene`, then recreates its host and HUD
on the restarted scene; a failed restart clears that lesson and reports the
typed failure.

## Everything else — the `SceneTeardown` schedule

Resources, caches, worker-side handles, and subsystem-derived entity trees are
not covered by the USD prim sweep. They are retired by the dependency-light
`lunco_core::SceneTeardown` schedule, run before the outgoing prims are
despawned and before replacement projection starts.

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

Windowed presentation state follows the same ownership rule. A scene must
author its initial camera selection through `CameraTrack`; avatar or scene-root
projection never creates an implicit view. If the authored camera contract is
missing or unresolved, the viewport remains inactive and the scene-scoped
diagnostic identifies the missing producer/consumer contract. This runtime
presentation check observes the composed, projected result; it is not the
generic loading completion signal. Scene teardown resets the selection intent,
and no engine-created camera can survive into the next Twin.

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

Loading closes from the authoritative asset/projection outcome in `Last`, after
normal projection schedules have finished. `Last`'s deferred-command flush
publishes `SceneTransitionCompleted` or `SceneTransitionFailed` and admits any
pending request; only the following frame's `First` lifecycle phase can execute
it. A second request therefore cannot replace an in-flight stage, and consumers
do not need staging markers, stale-entity guards, retries, or per-frame recovery
checks. Presentation is a separate admission concern: a missing authored
camera leaves the loaded scene in an explicit no-camera state and does not turn
successful USD projection into a false asset-load failure.

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

`UsdVisualProjectionSettings::frame_budget` bounds the ECS binding phase because
entity allocation and Bevy asset mutation are main-thread responsibilities. The
queue marker is the ownership fence: a prepared hierarchy creates one child
under its parent, without a world-wide duplicate-path scan. The same composed
path can legitimately occur in separate mounts or runtime instances, where the
parent hierarchy and instance identity scope it.
For a referenced runtime instance, the root also carries the source asset's
path-remapped prepared plan and the canonical generation at which the
reference was admitted. Descendants reuse that plan through the same queue;
when the canonical generation advances, `reader_for_entity` selects the live
composed stage so local authored opinions cannot be hidden by the snapshot.
`UsdAwaitingStage` remains on queued prims, so the authoritative stage outcome
is retained until the queue is empty. The workbench reports the indeterminate
loading/projecting phase, and a clear transaction reports unloading, rather than
presenting a partially projected scene as ready.

Doc-backed Twin admission is event-driven at the asset boundary. The Twin source
is loaded as `UsdSourceText`; `AssetEvent` marks successful availability and
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

The native `--scene` entry point follows the same boundary: `setup_sandbox` only
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

It is a **schedule**, not a registry, and that choice is the design:

- Bevy already expresses "run these systems at this lifecycle edge" — that is
  `OnExit`. Scene load here is a command rather than a state transition, so this
  is the same idea under an explicit label.
- The reset lives **beside the code that writes the state**. A central registry
  would put every subsystem's cleanup in one file that no subsystem author
  edits, and the state that gets forgotten is always the one whose owner never
  looked there.
- `SceneTeardown` grep-lists everything a reload restores.

```rust
app.add_systems(
    lunco_core::SceneTeardown,
    |mut commands: Commands| commands.remove_resource::<MySceneCache>(),
);
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

## Adding scene-derived state

If you add a resource, cache or external handle that a scene load writes, you
have added a leak until you register its reset. There is no automatic
detection — the schedule is the review surface, and an unregistered resource is
visible as an absence from it.

## See also

- [21 — domain: USD](21-domain-usd.md) — USD as source of truth, ECS as projection
- [`author-usd-physics`](../../skills/author-usd-physics/SKILL.md) — the authoring side, including gravity per scene
