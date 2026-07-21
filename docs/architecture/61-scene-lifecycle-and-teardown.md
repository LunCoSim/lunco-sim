# 61 — Scene lifecycle and teardown

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

## Everything else — the `SceneTeardown` schedule

Resources, caches and worker-side handles are not entities and are not covered
by a despawn. They are unloaded by the `SceneTeardown` schedule
(`lunco_usd_bevy::scene_lifecycle`), run from the same teardown.

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
    lunco_usd_bevy::scene_lifecycle::SceneTeardown,
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
