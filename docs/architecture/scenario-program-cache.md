# Scenario program cache

> Status: Active · Audience: anyone touching `lunco-scripting` or the event/scenario runtime

A scenario (`.rhai`) is a *derived artifact of its source*, and its event routing is a *derived artifact of its compiled AST*. Both follow the one rule of the [derive substrate](derive-substrate.md): **derive at the cheapest correct tier, key on structure, invalidate on change — never on a clock.** This doc is the scenario-runtime instance of that pattern; it adds no new infrastructure, only reuses [`lunco-hash`](efficiency-and-maintainability.md#substrate-e--lunco-hash-one-hashing-primitive).

## The firewall: structure vs. state

A compiled scenario has three parts, split by the discriminator *"is it in `key()`?"*:

| Part | Kind | Function of | Shared / cached? |
|---|---|---|---|
| `AST` (prelude-merged) | **structure** | source, asset identity, installed prelude | ✅ one `Arc`, content-addressed |
| hook mask (`on_start/tick/stop/event` present bits) | **structure** | `AST` | ✅ derived with the AST |
| `scope` (top-level initialization) | **state** | one explicit run per scenario instance; may touch the world | ❌ never |
| `this` (per-entity map) | **state** | runtime | ❌ never |
| event `filter` (`subscribe`) | **state** | `on_start` | ❌ never |

Compilation builds only immutable structure. The top-level body runs later, once
per attached scenario, after dependency planning at the owner boundary. The AST
is shareable; the scope and `this` state are not.

## What's cached

`RhaiScenarioRuntime` holds `compiled: HashMap<u64, CacheEntry>`, keyed by the
[Substrate E](efficiency-and-maintainability.md#substrate-e--lunco-hash-one-hashing-primitive)
fast-tier hash of source bytes, source length, and asset identity. Asset identity
is part of the key because it anchors relative imports. The key is local and
ephemeral; it is never sent over the wire.

On a hit, preparation returns the cached immutable artifact without dispatching
worker work. On a miss, the owner captures the engine, prelude AST, source, and
asset identity, then submits pure parsing/lowering through bounded
`AsyncWorkAdmission`. Concurrent misses for the same key and runtime revision
share one prepared program. Workers do not access the World or execute the
scenario top-level body.

The owner accepts a result only if scene generation, document generation,
parameter revision, and runtime preparation revision still match. It buffers
the complete pending compile set and commits in stable actor order before
`TimeSpineSet`. Both cache hits and misses hold simulation progress through
dependency planning, per-instance initialization, and the first `on_start`.
`CompiledProgram` carries no per-instance state.

`CompiledProgram` includes the full AST, the imports-only hook AST when needed,
the task AST, and the derived hook mask.

### Invalidation
- **Source edit** bumps the document generation → the driver recompiles with new source → new key → a fresh entry. The old entry is *not* dropped (it's retained for reuse — a replay of the prior version hits it); it goes away only when the whole memo is cleared at the cap (below).
- **Tool-library generation** changes the runtime preparation revision and rebuilds the engine. It does not change the source AST; tool modules resolve through the current engine at execution time, so an existing compiled entry remains reusable. In-flight artifacts stamped with the old revision are rejected.
- **Prelude generation** changes the AST itself because prelude functions are merged into scenario programs. Installing a new prelude invalidates the scenario cache and pending work before scenarios compile against it.
- **Both outcomes cached.** A committed miss caches the compiled `Arc` or compile diagnostic. Concurrent identical misses share the same worker result, and a shared compile error is logged once while each affected document receives its diagnostic.
- **Eviction:** the memo is retained across entity despawns (for replay reuse), so it is **bounded, not GC'd** — a `COMPILED_CACHE_CAP` (512) triggers a full `clear()` when hit (a cold re-parse on the next compile; the distinct-source working set is far below the cap, so this is rare). A finer byte-budget/LRU is a deferral, same status as the precompute cache's eviction.

### Why no disk tier
rhai's `AST` is **not `Serialize`**, so [`lunco-precompute`](efficiency-and-maintainability.md#substrate-b--lunco-precompute-the-content-addressed-cache-tier-3) (Substrate B, content-addressed *disk*) does **not** apply — you cannot `bake_or_load` an AST across process runs. The memo is RAM-only, tier-1. The *source* is already available through the runtime asset layer; only the parsed form lives in RAM. (Don't reach for the disk cache just because it exists — it's for byte-serializable structure like meshes and flattened stages.)

## Event routing

Two gates decide whether a `TelemetryEvent` enters the VM for an entity's `on_event`, both before any `call_fn`:

1. **Hook mask (structure).** `ProgramMask::event` is derived once at compile. If the program has no `on_event`, the per-event call is skipped entirely — no AST scan, no VM entry. This replaces the old per-`(entity, event)` `ast.iter_functions().any(...)` scan.
2. **Subscription filter (state, opt-in).** `subscribe("name")` / `subscribe_prefix("enter:")`, called in `on_start`, narrow delivery to named events. Default (no `subscribe`) = **all events** — behaviour-identical to before, and *forgetting* to subscribe is safe (you get everything, never a silent drop). Subscribing trades a small footgun (an unnamed event skips `on_event`) for skipping the VM entry on every event it doesn't name.

The filter gates **only the user `on_event`**. The native scenario runtime keeps
one bounded name/source event projection for both task `wait_for` leaves and the
Rhai mission driver, so task/mission progress cannot be starved by a subscription
and mission state cannot grow with the event stream. Full typed payloads are
constructed only for matching user `on_event` hooks.

The production contract is covered by the authored `rhai_event_delivery` scene
test, which sends a burst larger than the projection and requires mission
completion, plus `rhai_event_delivery_negative`, which proves an absent event
cannot complete an objective. These fixtures exercise the public scenario
surface; they do not call an internal event-delivery helper.

The shared `ScenarioExecutionGate` admits the initial scene lifecycle only after
all world and entity readiness holds clear. Scenario policy can reference
entities other than its attached owner, so `on_start` must not observe a partly
admitted scene. While this gate is closed, scenario passes are disabled and
events are not copied into the bounded inbox. Once the gate opens, a later
entity hold idles only scenarios whose owner or an ancestor is held; unrelated
owners keep running, and a held program resumes without a stop/start cycle.

Fixed-step delivery releases only events whose recorded tick is older than the
current `SimTick` after `SimTickSet`; events stamped at that boundary wait for
a later tick. The paused `Update` pass delivers discrete events on its next
pass without advancing the fixed tick. A program's first or restarted `on_start`
does not receive the batch accumulated before that lifecycle began; it reads
current owner state instead. Once the world-level gate opens, events follow FIFO
delivery. If producers still exceed the fixed capacity, collection latches the
`scripting-telemetry/telemetry-event-overflow` runtime diagnostic, clears the
partial batch, and holds event delivery until the next scene transition resets
the inbox; it never terminates the simulator from an observer.

**Names are never inferred from the AST.** Zone events are `enter:<zone>` prefixes, and names can come from `switch` / `.contains` / computed strings — static inference would miss cases, and a missed name is a silently dropped event (a broken lesson). Subscription is therefore explicit-only.

### Implementation note
`subscribe()` needs no entity argument: the driver arms a thread-local accumulator (`SUBS_ACCUM`) before `on_start` and harvests it into the entity's `EventFilter` after — so the verb just pushes names for whichever script is currently running. It is a no-op outside `on_start` (documented: subscribe in `on_start`). The driver is an exclusive system, so the thread-local is single-threaded.

## When to reach for the filter

The hook mask is free and always on. The subscription filter is the *deferred* optimisation — worth adding only when a profile (Tracy) shows `on_event` VM entries dominate: a **dense-event** scene (many sensors emitting `TelemetryEvent` every tick × many agent scenarios). Sparse-event scenarios (tutorials: possess, one zone-enter, a few clicks) should not bother — they pay effectively nothing already.
