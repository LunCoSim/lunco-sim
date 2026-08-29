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
diagnostic identifies the missing producer/consumer contract. Scene teardown
resets the selection intent, and no engine-created camera can survive into the
next Twin.

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
checks.

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
