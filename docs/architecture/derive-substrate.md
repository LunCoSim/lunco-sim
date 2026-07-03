# Derive — the unified derived-artifact substrate

> **HANDOVER — optimization effort (`caching-phase0` worktree).** Design output of the
> 2026-07-03 low-end-FPS pass: idle-spike + camera-motion-stall investigation →
> generalization. Lives here with the substrate code (A–E, Phase 0/0′) it extends.
>
> - **Where the effort stands:** substrates A–E + Phase 0/0′ **landed** on this branch;
>   this doc is the *next* phase (async/GPU tier + `BakeQueue`) — **DESIGN ONLY, not built.**
> - **First task → §11 Phase 1:** build `BakeQueue<CpuExec>` + async `bake_or_load`, then
>   migrate the terrain collider ring (`crates/lunco-terrain-surface/src/collider_ring.rs:160-176`,
>   the unbudgeted sync heightfield cook) onto it **with a coarse always-sync base floor**
>   (invariant §6.2). That one change fixes the camera-motion stall *and* the fall-through
>   risk together, and proves the abstraction on a real physics-coupled artifact.
> - **Context/data:** the Tracy profiling that surfaced the stall and the WASM
>   `AsyncComputeTaskPool`-on-main-thread constraint are the load-bearing findings behind §7.
> - **Sibling design docs** it unifies — `precompute-substrate.md`, `ports-system-design.md`,
>   `hashing-substrate.md`, `mobility-substrate.md`, `caching-and-precompute-strategy.md`,
>   `efficiency-and-maintainability.md` — are currently untracked in `main` (see §10).

> Status: design. Unifies substrates **A** (`RebuildOnChange`), **B** (`lunco-precompute`),
> **C** (Mobility), **D** (ports resolve→handle), **E** (`lunco-hash`) under one primitive.
> Motivating consumer: terrain tile colliders (camera-motion stall). See
> `caching-and-precompute-strategy.md`, `precompute-substrate.md`, `ports-system-design.md`.

## 1. The one primitive

Across the codebase the same shape recurs dozens of times (36 files touch
`AsyncComputeTaskPool`, 14 hand-rolled `Task<>` markers, `poll_once` in 10 crates,
plus every `RebuildOnChange` / `CompiledWiring` / USD→ECS projection):

> **A derived artifact, invalidated by its source's change signal, produced at the
> cheapest correct tier.**

Everything below is *one* concept instantiated at different tiers. We are not adding a
caching system next to the ports cache next to the projection cache — we are naming the
primitive all three already are, so the trigger, the key discipline, and the
invalidation rules are written **once** and every consumer gets WASM-correctness,
generation-guarding, and cache tiers for free.

The primitive has four parts:

| part | question it answers | reused from |
|---|---|---|
| **Trigger** | *when* do we recompute? | `ChangeDetector` in `lunco-core/derived.rs` |
| **Key** | *what* identifies this artifact? (structure only) | `lunco-hash` (fnv1a) + composed-USD identity |
| **Executor** | *how* is it produced? (inline / CPU / GPU) | the tier axis (new) |
| **Cache** | *where* does the result live, and how long? | RAM/VRAM LRU + `lunco-precompute` disk/OPFS |

## 2. The tier axis

The Executor is the *only* thing that varies between consumers. Three tiers, chosen by
the "cheapest correct tier" rule from `efficiency-and-maintainability.md`:

```rust
enum Tier { SyncInline, CpuAsync, GpuAsync }
```

- **`SyncInline`** — degenerates to today's `RebuildOnChange`. For cheap resolutions that
  must be ready *this* tick: port handle tables, `CompiledWiring`, USD→ECS projection.
  No queue, no cache — recompute-on-change, in place.
- **`CpuAsync`** — off-thread `AsyncComputeTaskPool` bake + RAM/disk cache. For expensive
  CPU artifacts that may arrive a few frames late: heightfield colliders, DEM-derived
  surface/normal textures (already a `lunco-precompute` consumer in `derived_layers.rs`),
  LOD tile meshes.
- **`GpuAsync`** — render-graph node writing a `Handle<Image>` + VRAM-resident cache. For
  procedural shader textures worth baking down instead of recomputing per-pixel-per-frame.
  Net-new plumbing; build only when a concrete expensive live shader justifies it.

`SyncInline` is the sync sibling; `CpuAsync`/`GpuAsync` are the async siblings. **Same
trigger (`ChangeDetector`), same key discipline — different execution and storage.**

## 3. The Executor boundary

```rust
// ---- WHAT: identity + storage policy. Backend-agnostic. -------------------
trait Artifact: Send + 'static {
    fn size_bytes(&self) -> usize;                     // byte-budgeted LRU (RAM or VRAM)
    fn to_bytes(&self) -> Option<Vec<u8>> { None }     // Some => persistable; None => resident-only
    fn from_bytes(_: &[u8]) -> Option<Self> where Self: Sized { None }
}
// Collider, Mesh, Vec<Vec<f64>>  => to_bytes = Some  (persist to disk / OPFS)
// GpuImage (Handle<Image>)       => to_bytes = None  (VRAM-resident, don't persist)

// ---- The recipe. key() folds ONLY structure inputs (see §4). --------------
trait Recipe: Send + Sync + 'static {
    type Output: Artifact;
    const NAMESPACE: &'static str;
    const VERSION: u32;                    // bump when bake() LOGIC changes
    fn key(&self, h: &mut Fnv1a);          // structure only; quantize continuous inputs
}

// ---- HOW: the Executor boundary. The only place CPU/GPU diverges. ---------
trait Executor: 'static {
    type Job: Recipe;
    type Ticket;                                             // in-flight handle
    fn submit(&mut self, cx: &mut ExecCx, job: Self::Job) -> Self::Ticket;
    fn poll(&mut self, t: &mut Self::Ticket)                 // non-blocking completion probe
        -> Option<<Self::Job as Recipe>::Output>;
    fn budget(&self) -> Budget;                              // backend's own currency
}

struct CpuExec<R: CpuRecipe>;   // CpuRecipe: Recipe { fn bake(&self) -> Output; }
//   submit  -> AsyncComputeTaskPool::spawn (native) / cooperative microtask (wasm)
//   poll    -> future::poll_once
//   budget  -> Budget::Cpu { jobs_per_frame, wasm_main_thread_ms }  // GLOBAL cap across CpuExecs on wasm

struct GpuExec<R: GpuRecipe>;   // GpuRecipe: Recipe { fn record(&self, &mut RenderGraph, target: Handle<Image>); }
//   submit  -> allocate Image target + enqueue render-graph node
//   poll    -> "fence signaled / N frames since submit"  (NOT poll_once — GPU has no CPU future)
//   budget  -> Budget::Gpu { gpu_ms_per_frame, vram_bytes }         // competes with the frame render
```

`BakeQueue<E: Executor>` is the generic front half — the valuable, reuse-everywhere part.
Every review invariant (§6) lives here **once**, shared by both backends:

```rust
impl<E: Executor> BakeQueue<E> {
  fn request(&mut self, job: E::Job, gen: Epoch, prio: f32) {
    let key = fold_key(&job);                                     // structure inputs only
    if let Some(a) = self.ram.get(key) { apply(a); return }        // resident hit -> free
    if Output::from_bytes_supported() {
        if let Some(a) = self.disk.load(key) { self.ram.put(key, a); apply(a); return }
    }
    if self.inflight.contains(key) { return }                      // dedup burst -> one bake
    self.pending.push(Req { key, job, gen, prio });
  }
  fn drive(&mut self, exec: &mut E) {                              // per frame, budget + priority gated
    for req in self.pending.drain_within(exec.budget()) {          // nearest/priority first
        self.inflight.insert(req.key, (req.gen, exec.submit(req.job)));
    }
    for (key, (gen, t)) in self.inflight.ready() {
        if source_epoch(key) != gen { self.inflight.remove(key); continue }  // GENERATION GUARD
        let a = exec.poll(t).unwrap();
        self.ram.put_evicting(key, a.clone());                     // byte-budgeted LRU
        if let Some(b) = a.to_bytes() { self.disk.store(key, b) }
        apply(a);
    }
  }
}
```

## 4. Structure vs state — the discriminator

The single rule that decides "just re-feed the shader" vs "must recalculate":

> **An input is _structure_ iff it is folded into `Recipe::key()`. Everything else is
> _state_ and never touches the queue** — it is written straight to the material/component
> each frame by a cheap change-driven system.

- **Structure** (in `key()`): change → new key → cache miss → re-bake. Rare.
- **State** (live uniforms / transforms): change → uniform re-upload through the existing
  handle. ~free. Same artifact, same `Handle<Image>`.

The same *logical quantity* can be either, depending on which artifact consumes it — so
classification is per-`(artifact, input)`, declared explicitly by that artifact's `key()`.
You never ask "did the sun move?"; you ask "did any **structure** key change?" — and the
sun simply isn't in the key for a live-glint shader.

**Tighten change on the data structure.** Split each consumer's backing data so structural
fields are their **own change-tracked component** (`Changed<PanelStructure>` = the re-bake
trigger) and live fields are a separate write path. Then Bevy change detection *alone*
routes the two — no per-frame hashing in the common (state-only) case. This is identical to
ports: the wiring struct is the `Source`; the port values are the hot path.

### Worked example: solar panel

| Input | Class | Where it goes | Cost |
|---|---|---|---|
| sun·normal incidence, power-output fraction (glow/color), thermal tint | **state** | `ShaderMaterial` uniforms, per-frame | ~free |
| articulation/tilt angle (rotates the mesh) | **state** (transform) | `Transform`, not a texture | ~free |
| cell grid layout, texture resolution, base albedo / MSL variant | **structure** | `key()` → `GpuExec` re-bakes detail texture | rare |
| dust / damage accumulation (grows continuously) | **structure, quantized** | `key()` folds `floor(dust/0.1)` → 1 bake per bucket | ~10× total, not per-frame |
| sun angle **if** a *baked* soft-shadow/AO map exists | **structure, quantized** | separate artifact; `key()` folds sun into N angular buckets | 1 bake per bucket |

The sun is **state** for the live glint and **quantized-structure** for a baked shadow map —
two artifacts, two `key()`s, one input. The panel lighting up as the sun moves costs a
uniform write; it never re-bakes.

## 5. Instances — one primitive, many consumers

| Consumer | Tier | Structure (key / resolve) | State (feed / transfer) |
|---|---|---|---|
| **Ports** (Substrate D) | `SyncInline` | `CompiledWiring` — names → slot handles | propagation reads/writes slots |
| **USD → ECS** projection | `SyncInline` | composed prim → ECS components | live sim / predicted values |
| **Terrain colliders** | `CpuAsync` | cook heightfield → `Collider` (key: DEM hash, node, res) | transform placement at spawn |
| **DEM surface/normal textures** (`derived_layers.rs`, already precompute) | `CpuAsync` | bake 512² maps (key: DEM hash) | — |
| **LOD tile meshes** | `CpuAsync` | bake geometry (key: DEM hash, node) | — |
| **Procedural shader textures** (regolith, panel detail) | `GpuAsync` | render → `Handle<Image>` (key: shader+params+res) | material uniforms |
| **Horizon occlusion** | `CpuAsync` | bake visibility (key: DEM hash, **quantized sun**) | — |

Ports and projection are the proof the primitive isn't terrain-specific: they are the same
`RebuildOnChange` shape, already shipped, at the `SyncInline` tier.

## 6. Invariants (must hold, or it breaks *this* sim)

1. **Frame-relative artifacts only.** Never cache absolute/world transforms — cache
   tile-local geometry; re-derive world placement (`GridCell`) on apply. Survives big_space
   recentering; makes recenter free instead of a mass cache invalidation.
2. **Guaranteed coarse floor, async fine detail.** Authoritative physics must never depend
   on an async/cached collider being present. Keep a cheap, always-synchronous coarse base
   collider; stream high-res as *refinement*. Fixes fall-through *and* the
   cache-warmth-affects-authority determinism issue (§7②③).
3. **Generation/epoch guard.** Stamp each request with its source generation; discard the
   result if the source advanced while baking (edit, despawn, LOD change).
4. **Integer-node-derived, version-bound keys.** key = quadtree node + LOD + *post-edit*
   content hash + scene/layer version + `FORMAT_VERSION`. Never float world positions.
5. **Byte-budgeted eviction on every tier.** RAM and VRAM LRU bounded by bytes, not count.
   No unbounded `cache_dir` growth (today's precompute has *none* — a slow disk leak).
6. **Global (cross-queue) WASM main-thread budget.** On wasm all `CpuExec`/`GpuExec` share
   the main thread; the yield/time budget is global across queues, or one queue's burst
   freezes the others (the `icon_warmer` freeze, generalized).
7. **Wider (sha256/CID) keys for anything persisted or shared.** fnv1a u64 only for
   ephemeral RAM memo; a planet-scale disk/peer cache needs collision-safe keys or it serves
   the wrong artifact silently.

## 7. Global-simulation edge cases

**① Floating-origin recenter vs cached geometry.** Handled by invariant 1 — artifacts are
tile-local, world placement re-derived on apply. Terrain already does this
(`translation_to_grid` at spawn); the generic layer must *enforce* it.

**② Bodies outrun the async ring → fall through.** Synchronous ring builds the 3×3 *this*
frame; deferring it breaks the guarantee. Severe at **teleport / network-spawn /
scenario-reset / scene-load** (9 uncooked tiles at once, no floor since ring mode dropped the
static collider). Mitigation = invariant 2 (coarse floor).

**③ Cache warmth affects the authority.** Netcode is client-predicted + server-authoritative
(not lockstep), so warm-vs-cold cache doesn't hard-desync clients — reconciliation absorbs
it. **But the headless server runs the ring too**; a cold server cache → late authoritative
ground → reconciliation snaps for everyone. Invariant 2 makes authoritative contact
insensitive to cache warmth.

**④ "Static structure" isn't static — this is a live, editable, USD world.** Crater
re-stamps, terrain tools, USD prim edits mutate DEM heights at runtime. Key must be the
*post-edit* content hash (or edits don't reach colliders), and frequent edits thrash the
cache (feeds invariant 5). Content-address on live content; don't treat terrain as immutable.

**⑤ Time/scenario dimension.** Time-dependent bakes (horizon occlusion depends on **sun
position**; anything on USD `timeSamples`) must fold the time coordinate into the key —
quantized, or the cache explodes. Easy to miss because it "looks static."

**⑥ Planet-scale precision.** Keys from float world coords lose precision far from origin and
can diverge between peers near tile boundaries → non-convergent keys. Derive keys from
**integer quadtree node ids** (invariant 4).

**⑦ LOD-seam consistency is a cross-tile invariant the per-node cache can't see.** A cached
low-LOD tile beside a fresh high-LOD neighbor can leave a collider gap → a body drops through
the seam. Needs skirts on collider tiles / seam-aware baking.

**⑧ Cold-start reality.** The cache helps *revisit / reload / patrol*, not first-time
exploration (all misses). For a world where you constantly see new terrain, the
**budget + async (+ coarse floor) is the fix; the cache is the optimization.** Don't
over-invest in the disk tier expecting it to smooth first exploration — it won't.

**⑨ Cross-session / cross-peer world identity.** A cached artifact is valid only if the world
matches (same DEM, georeference, edits). Bind the key to scene/layer version. USD gives this
for free (§8).

## 8. USD as the root of the derive chain

The derive architecture is not a peer of "USD is the single source of truth" — it is the
**mechanism that makes it fast**. The chain:

```
USD Stage (source of truth) ──SyncInline──▶ ECS projection ──CpuAsync/GpuAsync──▶ artifacts
   journaled, per-layer RBAC          (the "2 worlds")        colliders / meshes / textures
```

Every level is a cache of the level above. USD hands the derive cache the four things it
needs:

1. **Identity / key** — a prim's **composed opinion** is USD's content hash. Key artifacts
   off the composed prim, not transient ECS state. Solves ⑨ for free — USD *is* the
   addressing scheme.
2. **Structure/state split, for free** — USD static attributes → structure (bake key);
   `timeSamples` / animated attributes → state (live feed). That is exactly the **Mobility
   classifier (Substrate C)**: the architecture *reads* the split from USD instead of
   imposing it. (Panel cell-layout = static attr = key; articulation/output = `timeSamples`
   / port = feed.)
3. **The change signal** — the **USD-doc-op journal + per-layer generation counter** is the
   invalidation stream. A live-asset edit = a journal op = a layer-gen bump = a targeted key
   invalidation. Gate the expensive composed-hash behind the cheap gen counter — the exact
   `generation()`-before-`composed()` pattern already used in `sync_twin_overlays`.
4. **Peer convergence** — the key derives from *shared* composed USD state, so two peers get
   the **same key** → the CID tier can ship a baked artifact peer-to-peer instead of everyone
   recooking (the asset-sync plane, binary=CID). Layer scope picks the tier: session-layer
   edit → local-only artifact; scene/twin-layer edit → CID-shareable. RBAC and cache scope
   come out aligned.

**Live assets**: key off the asset **content hash**, not its path — a hot-reload/edit bumps
the key and re-bakes only dependents, through the same journaled-invalidation path.

**USD-specific caveats:**
- Key off **authored (USD)** state, never transient ECS/sim state, or predicted values thrash
  the cache. The structure/state split must hold *across* the USD↔ECS boundary.
- Propagate generations through every level: USD layer gen → projection gen → artifact key.
  In the 2-worlds model, an ECS-derived artifact must invalidate when the *projection*
  updates, not only when USD changes. Don't skip a level.
- Composed-hash cost is real; gate it behind the layer-gen counter (caveat 3).

## 9. Efficiency ladder (cheapest first)

The per-frame cost of "re-bake or re-feed?" must be **O(changed), not O(all)**:

1. **Change-driven, never polled.** `RebuildOnChange` / `Changed<Structure>` — untouched
   artifacts cost zero.
2. **Component-level structure/state split** → change detection routes the two without
   hashing in the common (state-only) case.
3. **Fast fnv1a key, only for the changed few** → equal key ⇒ mutation didn't change
   structure ⇒ skip.
4. **Quantize continuous structure inputs** (dust, sun-for-shadows) → key flips only at
   bucket boundaries.
5. **Debounce/coalesce interactive edits** (slider drag) → bake once on settle; in-flight
   dedup by key collapses a burst.
6. **Resident LRU** → flip-flop / revisit is a free hit.

Layers 1–2 are what make it scale: a static scene costs ~nil because nothing is `Changed`.

## 10. Relationship to existing substrates

- **A `RebuildOnChange`** (`lunco-core/derived.rs`) = the `SyncInline` tier + the shared
  `ChangeDetector` trigger for all tiers.
- **B `lunco-precompute`** = the disk/OPFS tier for `CpuAsync` artifacts whose
  `Artifact::to_bytes` is `Some`. Needs the async `bake_or_load` + RAM tier + eviction it
  currently lacks (plan Phases A4/A5).
- **C Mobility** = auto-derives structure vs state from USD (static attr vs `timeSamples`),
  rhai write-sets, physics body class — so consumers rarely hand-declare.
- **D ports resolve→handle** = the canonical `SyncInline` instance; the primitive is the
  generalization of `CompiledWiring`.
- **E `lunco-hash`** = the fnv1a key fold; the CID tier for persisted/shared artifacts.

## 11. Migration / phasing

1. **Build `BakeQueue<E>` + `CpuExec` + async `bake_or_load`/RAM tier/eviction** in
   `lunco-precompute` (plan A4/A5). Ship invariants 1–6 in the generic.
2. **First consumer: terrain collider ring** — migrate `collider_ring.rs` onto
   `BakeQueue<CpuExec>` **and re-introduce the coarse always-sync base floor** (invariant 2).
   Fixes the camera-motion stall and the fall-through risk together. Validates the layer on a
   real, physics-coupled artifact.
3. **Fold in `derived_layers` + `stream_viz`** (already precompute / already budgeted) — 3–4
   near-identical hand-rolls in one crate collapse onto the generic.
4. **Horizon bake** (quantized-sun key) and **`icon_warmer`/`class_cache`** (highest
   WASM-correctness payoff — correct-by-construction).
5. **`GpuExec`** — build only when a concrete expensive procedural shader (regolith) is worth
   baking down; net-new render-graph plumbing, separate platform matrix (WebGL2 = no compute).
6. Leave genuinely one-shot background jobs (file ops, scenario sync, ephemeris fetch) on a
   thin `spawn_job` helper — not every async task is a keyed, cached artifact.

## 12. Open questions

- **Profile sample-vs-cook** for terrain colliders — decides whether the disk tier (2b) beats
  RAM-only for colliders, or only the budget+coarse-floor matters.
- **Collider serialization** — persist the height *samples* (`Vec<Vec<f64>>`, trivially
  serde) rather than the cooked parry `Collider`; re-cook on load. Confirm the cook, not the
  sampling, is the dominant cost first.
- **Bit-determinism of `bake()`** for CID-shared artifacts — parallel/float reductions must be
  order-stable or peers disagree under the same key.
- **VRAM budget sizing** for `GpuAsync` on WebGL2/wasm (4 GB cap, prior OOM history).
