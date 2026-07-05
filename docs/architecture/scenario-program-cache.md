# Scenario program cache

**Status:** implemented (rhai backend) · **Audience:** anyone touching `lunco-scripting` or the event/scenario runtime.

A scenario (`.rhai`) is a *derived artifact of its source*, and its event routing is a *derived artifact of its compiled AST*. Both follow the one rule of the [derive substrate](derive-substrate.md): **derive at the cheapest correct tier, key on structure, invalidate on change — never on a clock.** This doc is the scenario-runtime instance of that pattern; it adds no new infrastructure, only reuses [`lunco-hash`](hashing-substrate.md).

## The firewall: structure vs. state

A compiled scenario has three parts, split by the discriminator *"is it in `key()`?"*:

| Part | Kind | Function of | Shared / cached? |
|---|---|---|---|
| `AST` (prelude-merged) | **structure** | `source` only | ✅ one `Arc`, content-addressed |
| hook mask (`on_start/tick/stop/event` present bits) | **structure** | `AST` | ✅ derived with the AST |
| `scope` (top-level `const` globals) | **state** | seed-run — **touches the world, varies per `params`** | ❌ never |
| `this` (per-entity map) | **state** | runtime | ❌ never |
| event `filter` (`subscribe`) | **state** | `on_start` | ❌ never |

The trap the old code fell into: `compile()` *parsed and ran the top-level body* in one step and stored all of it per-`Entity`. Identical source was re-parsed N times (every rover with the same controller; every tutorial replay). **The AST is pure structure and shareable; the scope is stateful and must not be.** Keeping the parse (shared) strictly apart from the seed-run (per-instance) is what makes the memo determinism-safe.

## What's cached

`RhaiScenarioRuntime` holds `compiled: HashMap<u64, Arc<CompiledProgram>>`, keyed by `fnv1a64(source)` — the [Substrate E](hashing-substrate.md) *fast tier* (local/ephemeral, **never on the wire**; not a CID). `compile()`:

1. `key = fnv1a64(source)` → hit? clone the `Arc` (refcount bump, zero parse). Miss? `engine.compile` + prelude-merge + derive the hook mask → insert.
2. **Always, per entity:** seed a fresh `scope` + `this` (the stateful part).

`CompiledProgram { ast, mask }` carries no per-instance state.

### Invalidation
- **Source edit** bumps the document generation → the driver recompiles with new source → new key → a fresh entry. The old entry is *not* dropped (it's retained for reuse — a replay of the prior version hits it); it goes away only when the whole memo is cleared at the cap (below).
- **Tool-lib / prelude generation** is *not* in the key: it changes the engine's *runtime* module resolution, not the AST parse, so the cached AST stays valid (tool calls resolve at call-time). `maintain()` refreshes the engine; it does **not** clear the memo. (If a future change hot-reloads the *prelude source* merged into every AST, that path must `compiled.clear()`.)
- **Both outcomes cached.** A miss caches the compiled `Arc` *or* the compile-error `Diagnostic`, so a fleet sharing one broken source parses + logs once, not per entity.
- **Eviction:** the memo is retained across entity despawns (for replay reuse), so it is **bounded, not GC'd** — a `COMPILED_CACHE_CAP` (512) triggers a full `clear()` when hit (a cold re-parse on the next compile; the distinct-source working set is far below the cap, so this is rare). A finer byte-budget/LRU is a deferral, same status as the precompute cache's eviction.

### Why no disk tier
rhai's `AST` is **not `Serialize`**, so [`lunco-precompute`](precompute-substrate.md) (Substrate B, content-addressed *disk*) does **not** apply — you cannot `bake_or_load` an AST across process runs. The memo is RAM-only, tier-1. The *source* is already embedded/on-disk at the asset layer; only the parsed form lives in RAM. (Don't reach for the disk cache just because it exists — it's for byte-serializable structure like meshes and flattened stages.)

## Event routing

Two gates decide whether a `TelemetryEvent` enters the VM for an entity's `on_event`, both before any `call_fn`:

1. **Hook mask (structure).** `ProgramMask::event` is derived once at compile. If the program has no `on_event`, the per-event call is skipped entirely — no AST scan, no VM entry. This replaces the old per-`(entity, event)` `ast.iter_functions().any(...)` scan.
2. **Subscription filter (state, opt-in).** `subscribe("name")` / `subscribe_prefix("enter:")`, called in `on_start`, narrow delivery to named events. Default (no `subscribe`) = **all events** — behaviour-identical to before, and *forgetting* to subscribe is safe (you get everything, never a silent drop). Subscribing trades a small footgun (an unnamed event skips `on_event`) for skipping the VM entry on every event it doesn't name.

The filter gates **only the user `on_event`** — the built-in task driver (`__note_task_event`, feeding `wait_for`) still sees every event, so task/mission progress can't be starved by a subscription.

**Names are never inferred from the AST.** Zone events are `enter:<zone>` prefixes, and names can come from `switch` / `.contains` / computed strings — static inference would miss cases, and a missed name is a silently dropped event (a broken lesson). Subscription is therefore explicit-only.

### Implementation note
`subscribe()` needs no entity argument: the driver arms a thread-local accumulator (`SUBS_ACCUM`) before `on_start` and harvests it into the entity's `EventFilter` after — so the verb just pushes names for whichever script is currently running. It is a no-op outside `on_start` (documented: subscribe in `on_start`). The driver is an exclusive system, so the thread-local is single-threaded.

## When to reach for the filter

The hook mask is free and always on. The subscription filter is the *deferred* optimisation — worth adding only when a profile (Tracy) shows `on_event` VM entries dominate: a **dense-event** scene (many sensors emitting `TelemetryEvent` every tick × many agent scenarios). Sparse-event scenarios (tutorials: possess, one zone-enter, a few clicks) should not bother — they pay effectively nothing already.
