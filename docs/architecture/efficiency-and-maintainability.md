# Efficiency & Maintainability — the North Star

> Umbrella for the caching/perf work. Frames the detailed docs
> (`caching-and-precompute-strategy.md`, `caching-implementation-plan.md`,
> ports design) under one principle, so the whole workspace moves the same
> direction instead of accreting one-off optimizations.

## The one principle

> **Separate structure from state. Place every derived value at the cheapest
> correct tier. Invalidate on change — never on a clock.**

Almost every perf and maintainability problem we've found is a violation of this:

| Symptom (this session) | The violation |
|---|---|
| `propagate` rebuilt a string-keyed wire snapshot every tick (0.3) | recomputing **structure** at **state** cadence |
| `sync_collider` rebuilt a collider every frame (0.2) | derived value not memoized against its input |
| ports re-resolve name→backend every read | **resolution** (structure) fused with **transfer** (state) |
| horizon shadows re-baked every load; USD stages re-flattened every load | deterministic **artifact** recomputed instead of cached |
| USD animation samplers re-derive topology every frame (0.5) | per-entity **structure** recomputed per frame |
| regolith FBM / 96-step march recomputed per pixel per frame | pure-of-position **artifact** not baked to a texture |

The mature subsystems already obey it — **networking and terrain swept clean**
(change-detection, `Without<>` markers, throttles, off-thread generation caches,
disk-baked derived layers). They are the template, not the problem. The goal is
to make the rest of the workspace look like them, with **shared substrates** so
nobody hand-rolls the pattern again.

## The derived-data tier ladder

Every derived/cached value sits at exactly one tier, chosen by **volatility ×
cost** (the matrix from the caching doc §0). This is both the efficiency lever
(cheapest correct tier) and the maintainability rule (one decision procedure,
one substrate per tier — stop reinventing):

```
 volatility →     changes per-tick        changes on structure edit      ~never (pure of stable inputs)
 cost ↓
 cheap           just compute it          RAM memo (tier 1)              compute at load, keep in RAM
 expensive       irreducible (live sim)   change-compiled (tier 2)       disk content-cache (tier 3)
```

- **Tier 1 — RAM memo.** Per-entity derived data cached on a component / `Local`,
  refreshed on change. Idioms: dirty-flag component (`LastColliderVolume`, 0.2),
  plan component built on `Added<>` (USD `AnimationPlan`, 0.5).
- **Tier 2 — change-compiled resource.** A structure-stable global fabric
  compiled once and rebuilt only on `Changed`/`Added`/`RemovedComponents` of its
  source. Idiom: `CompiledWiring` (0.3). **This tier has no shared helper yet —
  it should** (see Substrate A).
- **Tier 3 — content-addressed cache.** Expensive deterministic artifacts baked
  to disk/RAM, keyed by content hash + LOD + variant. Substrate:
  `lunco-precompute` (bake-or-load). Reference impl exists (`derived_layers.rs`).

The **determinism firewall** overrides all of it: never cache a stateful
integrator's output or reorder a schedule that encodes a data dependency
(`ControlDacSet.before(Propagate)`). Live sim math stays f64; wire quantization
is not sim precision.

## The five shared substrates

Build these once; every subsystem adopts them instead of re-solving. Each is a
face of the one principle.

### A. `RebuildOnChange<Source, Value>` — change-detected derivation (tier 2) ✅ landed
Generalizes `CompiledWiring` into a reusable type in `lunco-core`
(`derived.rs`): *"cache `Value` computed from `Source`; rebuild it **only when
`Source` changes**, never per tick."* One method, `get_or_rebuild(world,
rebuild)`. A private `ChangeDetector<S>` caches a `SystemState` so
`Changed<S>`/`RemovedComponents<S>` detection works **inside exclusive systems**
(where normal change-detection params don't exist), with a forced first-run.
`propagate_connections` now uses it (0.3 refactored onto it, tests green). Kills
the per-tick-recompute class and gives one review-checklist item: *"does this
system recompute structure at state cadence? → `RebuildOnChange` it."*

### B. `lunco-precompute` — content-addressed cache (tier 3)
`bake_or_load(key, produce)`; `CacheKey{domain, content-CID, lod, variant}`;
one native/web I/O fork reusing the shipped OPFS backend; CID-keyed dedup;
LRU/size-budget eviction covering `precompute/` **and** the unbounded
`scenarios/` tree. Lift the real CIDv1 out of `lunco-networking` (invert the
dep). Details: `caching-and-precompute-strategy.md` §2, plan Phase A.

### C. `Mobility` — the structure/volatility classifier (cross-cutting)
`Static | Kinematic | Dynamic`, joined across USD (`timeSamples`, physics body,
port binding) / rhai (write-set) / Modelica (DAE state count). It is the
**authoring-layer face of structure-vs-state**: it tells every consumer which
tier applies and what can be skipped — bake static shadows, replicate only
Dynamic, sleep settled bodies, LOD static geometry hard. Details: strategy doc
§2.2/§3.

### D. Ports: `resolve → handle` (runtime data-plane)
Split resolution (structure) from transfer (state): backends implement one
minimal surface (`resolve(name,dir)->slot`, `read(slot)`, `write(slot)`,
`list`); the registry **derives** name-based access; the hot path caches a
`PortHandle{backend, slot}` and exchanges by integer. Fixes the avian
presence-scan (the only slow backend) and makes adding a backend easier (one
name-matching site, not two). Details: `ports-system-design.md`.

### E. `lunco-hash` — one hashing primitive (underlies B & tier 2)
Two tiers by *purpose*, replacing ~7 ad-hoc `DefaultHasher` folds:
- **durable/shareable keys** → the existing sha2-256 CID (IPLD-interoperable);
- **in-frame change-detection** → a fast non-crypto hash (xxhash/`DefaultHasher`).

## Why this is efficiency *and* maintainability

- **Efficiency:** each derived value lives at the cheapest correct tier; the
  per-tick hot path carries only irreducible live-state work; expensive
  artifacts compute once. Directly serves the original goal — low-end FPS.
- **Maintainability:** one principle, one tier-decision procedure, one substrate
  per tier. A new subsystem *declares* structure + derivation and gets fast +
  cached for free, instead of hand-rolling change-detection, resolution, and
  caching three different ways. Dependency direction becomes a rule: **feature
  crates depend inward on substrate crates** (the CID-lift is the first
  correction of an accidental outward coupling).

## Rollout — incremental, builds on what's green, no big-bang

1. **Done + green:** 0.2 (`LastColliderVolume`), 0.3 (`CompiledWiring`).
2. **Extract Substrate A** from `CompiledWiring`; apply to the sweep's stragglers
   (0.4 scenario clone, 0.5 USD animation plan).
3. **Substrate E** (`lunco-hash`) — tiny, unblocks B.
4. **Substrate B** (`lunco-precompute`) + first consumer horizon-bake (Phase A/B).
5. **Substrate D** (ports resolve/handle) — the clean 0.3b + a better API.
6. **Substrate C** (`Mobility`) — USD detector first; unlocks render/net/physics
   skipping.
7. Consumers: LOD tile/USD-flatten disk cache, DAE artifact cache, eviction + UI.

## Non-goals (protect these)

- **No big-bang rewrite.** Each substrate is independently shippable and
  measurable (frame-time / startup A/B on the headless server).
- **Don't touch the mature subsystems** (networking, terrain) except to adopt a
  substrate — they already embody the principle; they're the reference.
- **Don't optimize across the determinism firewall.** Prediction/replication
  correctness outranks any per-tick saving.
- **No abstraction beyond the five substrates.** The point is *fewer* ways to do
  a thing, not a framework.
