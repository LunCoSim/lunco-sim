# Caching & Precompute Strategy

> Audience: contributors optimizing derived-data pipelines.

A strategy to make LunCoSim run well on low-end machines by **computing each deterministic thing at most once** and reusing it — in RAM within a session, on disk across sessions.

## 0. The one principle

Every computation falls on two axes:

- **Purity** — is the output a pure function of stable, hashable inputs?
- **Volatility** — how often do those inputs actually change? This is the
  **mobility** of the thing: *static* (never), *kinematic* (on a known,
  replayable schedule — animation, scripted path), or *dynamic* (sim-driven,
  emergent, unpredictable).

```
                 inputs change never/rarely      inputs change every tick
 expensive   →   DISK cache (content-addressed)   memoize + dirty-flag (RAM)
 cheap       →   compute at load, keep in RAM      just compute it
```

Caching is only ever correct when the output is a **pure function** of its
declared inputs. The whole discipline is: (1) identify the true input set,
(2) hash it, (3) key the cached result on that hash, (4) bump a
`FORMAT_VERSION` when the *producer* changes. Everything below is an
application of that.

**The universal cache wants a way to know, for any object in any authoring layer
(USD / rhai / Modelica), whether it is static, kinematic, or dynamic.** That
classification informs caching decisions below — whether a thing is bakeable at
all, how aggressively it can be LOD'd/frozen, and whether it is on the
determinism firewall. §2.2 defines it; §2 defines the cache it feeds; LOD (§2.1)
is just one axis of the same cache key, not a special case.

> **Scope correction (post-build).** This section originally framed the
> `Mobility` classifier as *the* linchpin input to every caching decision. In
> practice it is **not** a hot-path lever: the per-tick static/kinematic skip it
> was scoped for is already captured (the USD→avian path classifies bodies and
> avian's solver already skips `Static`), and USD hands the structure/state split
> to the derive cache directly (see `derive-substrate.md` §8, `mobility-substrate.md`).
> `Mobility` (Substrate C) is a **unification / correctness** play — a queryable
> declared-intent label — not a speedup, and not a prerequisite for the cache.
> The cache keys off USD composed opinions and change signals whether or not C
> lands. Treat §2.2/§3 below as the *design* of a useful cross-layer label, not a
> gating dependency.

The dangerous inverse — **never cache a stateful integrator's output as if it
were pure**. A Modelica ODE step advances state even with constant inputs; a
schedule ordering encodes a data dependency. Those look cacheable and are not
(see §7).

## 1. Foundation that already exists (build on this, don't reinvent)

| Primitive | Where | What it gives us |
|---|---|---|
| Per-platform cache root | `lunco-assets/src/lib.rs:168` `cache_dir()` | `~/.cache/lunco` (Linux) / `~/Library/Caches/lunco` (mac) / `%LOCALAPPDATA%\lunco` (win), `LUNCOSIM_CACHE` override, `cache://` asset source. wasm returns nominal (no FS). |
| Named cache subdirs | `lunco-assets` `cache_subdir`, `textures_dir`, `msl_dir`, … | An established taxonomy under the root. |
| **Content-addressed disk bake (reference impl)** | `lunco-terrain-surface/src/derived_layers.rs:110` `bake_or_load` | FNV-1a over params + **every height sample** → `cache://terrain/derived/<key>/`, load-if-present else bake+write, `CACHE_FORMAT_VERSION` invalidation. **This is the pattern to replicate everywhere.** |
| In-memory async dedup | **The ECS idiom** — a `Task` *Component* + `Without<BakeTask>` in the spawning query (`lunco-environment/src/horizon.rs:274`, `terrain-surface/src/derived_layers.rs:96`, `celestial/src/trajectories.rs:82`, …) | Entity-keyed load with in-flight dedup **for free** — the query filter *is* the pending set. This is what the codebase actually converged on; see the note below. |
| I/O chokepoint | `lunco-storage` `atomic_write` / `read_file_sync` | Cross-target, wasm-aware; tmp+fsync+rename. |
| SHA-256 asset pinning | `lunco-assets/src/download.rs:63` | Integrity + skip-redownload on hash match. |
| rumoca parse cache | `.cache/rumoca/parsed-files/` (content-hash keyed) + `parsed-msl.bin` bincode bundle | Cold-parse avoidance for Modelica. |
| Structural change-detection | `Added<SimConnection>` (`lunco-cosim/src/lib.rs:252`), USD `Without<Marker>` gates | Recompute-only-on-change is already idiomatic here. |
| **Real CIDv1 content-address** | `lunco-networking/src/scenario.rs:54-66` `cid_for_content`/`cid_from_bytes` | IPLD CIDv1 (raw `0x55` + sha2-256), `ipfs add`-compatible; incremental fail-closed verify (`scenario_sync.rs:88-94`). **First real content-addressing in the repo** — but scoped to networking. |
| **OPFS web blob backend** | `lunco-storage/src/opfs_storage.rs` | Working async `read`/`write`/`exists` on wasm via `createWritable` (main-thread-legal). Path-keyed on `StorageHandle::File`. |
| **`inventory` asset-scheme registry** | `lunco-assets/src/asset_sources.rs:41-96` `AssetSchemeProvider` | Per-crate `inventory::submit!`'d URI schemes drained before `AssetPlugin`; `scenario://` uses it. Clean extension point for a `precompute://`/`cache://` reader. |

> **Retired: `lunco-cache`.** An earlier draft of this doc proposed a generic
> `ResourceCache<L: ResourceLoader>` — a `HashMap<K, Task>` + pending map — as the
> RAM tier. It was built, never adopted by a single crate, and **deleted on
> 2026-07-13**. The reason is worth keeping, because the crate's own doc comment
> asserted it was "the abstraction those [~8 bespoke memos] want" and that claim
> then propagated into a code review and a remediation plan before anyone checked
> it:
>
> - **Almost none of those memos have its shape.** Of ~46 cache-like sites in the
>   workspace, ~17 are *synchronous* memos (content-hash → handle; no task, no
>   in-flight window — `ResourceCache` would only add overhead) and ~20 are plain
>   registries. Only ~9 are true async loads.
> - **Of those ~9, only two are keyed HashMaps.** The rest are Entity-keyed, so
>   the pending set is a query filter, not a map. A `Resource<HashMap<K, Task>>`
>   has nothing to attach to.
> - **The one genuine candidate needs strictly more than it offered.** The terrain
>   tile baker (`stream_viz.rs:494`) carries generation-versioning (discard bakes
>   from a superseded terrain gen) and a `MAX_INFLIGHT_BAKES` budget.
>   `ResourceCache::request` has neither — migrating would have been a downgrade.
>
> If you need async dedup, reach for the ECS idiom in the table above. If you need
> a *disk* bake, that is `lunco-precompute::bake_or_load` (§2), which shipped.

**Gaps in the foundation:**
- **No shared hashing util, but now two families in play.** Change-detection
  still uses ad-hoc `std::DefaultHasher` folds (~7 files); durable content-address
  is now sha2-256 (the new CID). Resolve by *role*, not by picking one algorithm —
  see §2 "Two-tier hashing."
- **CID exists but is scenario-scoped and not a disk blob store.** The new CID is
  content-addressed *on the wire* only; **on disk it is uuid+path keyed**
  (`<cache_dir>/scenarios/<uuid>/<rel>`, `scenario_sync.rs:145`) with **no
  dedup** — identical bytes in two scenarios stored twice. A CID-keyed blob
  store is still net-new.
- **OPFS exists but is off-trait.** `OpfsStorage` holds non-`Send` JS values
  across `.await`, so it **cannot `impl Storage`** (the trait is `Send+Sync`) —
  every caller forks at a `#[cfg]` (see `scenario_sync.rs:327` `do_write`).
  There is **no sync web `exists`** (the sync `read_file_sync`/`write_file_sync`
  helpers route wasm to `localStorage`, *not* OPFS — a footgun for any generic
  cache that calls them). No streaming (whole-`Vec` buffering), no eviction.
- **No eviction anywhere.** Both `cache_dir()` and the new `scenarios/` cache
  grow unbounded; cross-session cache-hit detection is explicitly deferred
  (`scenario_sync.rs:29-32`) so a restarted web client re-fetches everything.

## 2. Proposed substrate: one content-addressed cache crate

**Status: shipped.** `lunco-precompute` exists and is adopted by
`lunco-terrain-surface`. It generalizes `derived_layers.rs` into a reusable API:

```rust
/// Load `key`'s artifact from the disk cache, or produce+store it.
/// `key` = domain + FORMAT_VERSION + content-hash of ALL inputs.
fn bake_or_load<T: Cacheable>(key: CacheKey, produce: impl FnOnce() -> T) -> T
```

- **Backing — reuse today's OPFS + CID, don't rebuild.** `bake_or_load` owns a
  single internal native/web fork (mirroring `scenario_sync.rs:327` `do_write`):
  native → `FileStorage` on `AsyncComputeTaskPool`; wasm → the existing
  `OpfsStorage` inherent async `read`/`write`/`exists` + `spawn_local`, result
  channelled back to a Bevy system. **Do not try to make OPFS `impl Storage`**
  (it can't — non-`Send`); the substrate is the *one* place the `#[cfg]` fork
  lives, so every caller stays backend-agnostic. Because OPFS's cheapest
  `exists` is async, **`bake_or_load` grows an async variant** — and the RAM tier
  that fronts it is the ECS idiom (a `Task` component + `Without<BakeTask>`), not
  a generic cache resource. `derived_layers.rs:96` `DerivedBakeTask` is the
  reference: it already wraps a `bake_or_load` in exactly that shape.
- **CID-keyed blob store (this is the dedup the scenario cache lacks).** Store at
  `cache_dir()/precompute/<domain>/<cid>`; identical content in two domains
  hits the same blob. Lift `cid_for_content`/`cid_from_bytes` + the incremental
  fail-closed verify + `safe_rel_path` guard **out of `lunco-networking` into the
  substrate crate**, and invert the dep (networking depends on the substrate).
  One CID impl, shared by distribution and precompute.
- **Two-tier hashing (resolves "which hash").** Split by *purpose*, not taste:
  - **Durable/shareable disk keys → the existing sha2-256 CID.** Already
    implemented, tested, `ipfs add`-interoperable, fail-closed-verified. Bakes
    are infrequent, so sha2 speed is a non-issue. Adopt it — **drop the earlier
    blake3 proposal**; a second crypto-hash family buys nothing but divergence.
  - **In-frame change-detection → keep a fast non-crypto hash** (`DefaultHasher`/
    `xxhash`). sha2 is far too slow for per-tick dirty-flag folds; these never
    leave the process, so cryptographic strength is irrelevant.
- **Eviction.** LRU-by-mtime + a size
  budget in `lunco-settings`, swept on startup, covering **both** `precompute/`
  **and** the existing `scenarios/` tree (which today grows unbounded).
  Regenerable/re-fetchable by definition, so eviction is always safe.
- **Serialization:** raw `bincode`/POD for binary artifacts (meshes,
  heightfields, DAE tables) — **not JSON** (per the "no JSON for internal
  logic" rule; JSON is only for human-facing/journal DTOs).

This is the missing "cache folder to store computed stuff, dynamically" layer:
lazy — compute on first demand, persist, reuse forever after; dynamic —
evictable, versioned, keyed by content so a changed input transparently
produces a new entry. Today's scenario cache is the *distribution* sibling of
this *recompute* cache — different purpose (fetch-once vs compute-once), same
`cache_dir()` root, and after the CID lift, the same content-address primitive.

### 2.1 One universal key — LOD is a dimension, not a special case

LODs are not a separate system; they are the same cached artifact produced at
different fidelities. The key is a tuple, and *any* consumer (terrain tiles,
mesh decimation, baked shadow resolution, Modelica solver step size, obstacle
density) uses the same shape:

```rust
struct CacheKey {
    domain:  &'static str,   // "terrain.tile", "usd.flatten", "modelica.dae", …
    content: Cid,            // sha2-256 CIDv1 (the §2 two-tier rule) — NOT blake3
    lod:     u8,             // fidelity rung: 0 = coarsest, up. Absent ⇒ single-fidelity
    variant: u16,            // platform/feature bits (wasm vs native, quality preset)
}
```

> **How this maps to the code:** Substrate B's `Bake` trait keys on a fast
> **`fnv1a` `u64`** under a `NAMESPACE` dir — *no* `lod`/`variant` (see
> `precompute-substrate.md`). This richer `CacheKey{domain, content: Cid, lod,
> variant}` is the target shape for cross-peer/persisted entries: fast `fnv1a`
> addresses ephemeral/local blobs; the sha2-256 `Cid` tier is reserved for
> on-disk/on-wire artifacts that must be collision-safe and IPFS-interoperable
> (the two-tier firewall, §2). `lod`/`variant` are a later extension.

Consequences that make it *universal* rather than terrain-specific:

- **Progressive serve.** Ask for `lod=N`; if only `lod=N-2` is resident, serve
  it *now* and bake the finer rung in the background — the existing terrain
  progressive-tile + reveal-anim behaviour, generalized. A coarse rung is
  always a valid stand-in for a finer one of the same `content`.
- **LOD ladder is generated top-down and cached per rung.** Decimate once,
  store each rung; revisits are free. Applies equally to meshes, to baked
  shadow-map resolution, and to sim fidelity (a Modelica model can have a
  cheap reduced-order rung for distant/inactive vehicles — see §5.4).
- **`variant` keeps the quality-preset knobs (low-end vs high-end) from
  thrashing one cache** — a "Potato" preset bakes its own smaller rungs and
  they coexist with the high rungs, both content-addressed.
- **Eviction is LOD-aware:** drop fine rungs first under pressure; never evict
  the coarsest resident rung of a visible object (guarantees a stand-in exists).

The in-RAM `LodMeshCache`/`LodMaterials` (`stream_viz.rs:168,200`) already are
this shape for terrain, RAM-only. The substrate turns that into the disk-backed,
cross-domain default.

### 2.2 Mobility classification — the input the cache runs on

A cache entry is only valid while its inputs hold. So the substrate needs, for
every cacheable thing, a **`Mobility`** label and an **invalidation signal**:

```rust
enum Mobility {
    Static,              // inputs never change after load → bake to disk, freeze, aggressive LOD
    Kinematic { key },   // changes on a known replayable schedule (animation / scripted path)
                         //   → cache the *trajectory*, not per-frame recompute; sample it
    Dynamic,             // sim-driven / emergent → never bake; recompute each tick; determinism firewall
}
```

Two hard requirements:

1. **It must span authoring layers** — the same object may be positioned by USD,
   animated by a rhai script, and have its dynamics from Modelica. The label is
   the *join* of all layers' verdicts: **`Dynamic` wins over `Kinematic` wins
   over `Static`** (the most-volatile contributor decides).
2. **It must be a live, change-detected classification, not a load-time guess.**
   Objects flip: a `Static` rock becomes `Dynamic` when kicked; a `Kinematic`
   animation ends and freezes to `Static`; a rhai script starts writing a
   transform it previously only read. A `Mobility` component recomputed on
   change-detection, that **invalidates the object's cache entries on any
   promotion toward more-dynamic**, is the mechanism. Demotion (dynamic →
   static, e.g. a rover parks and sleeps) is a bake *opportunity*: freeze the
   settled state to a static cached pose.

How each layer votes (see §3 for detection detail):

| Layer | `Static` | `Kinematic` | `Dynamic` |
|---|---|---|---|
| **USD** | default xform, no `timeSamples`, no physics body / `kind=component` scenery | attr/xform **has `timeSamples`** (animation flatten already carries these) or explicit `lunco:mobility="kinematic"` | rigid body (non-kinematic), or bound to a cosim wire / port |
| **rhai** | script only reads, or writes only at `Startup` | script writes a transform/port on a **fixed schedule** but from a pure function of time (replayable) | script writes world state per tick from sim/emergent inputs |
| **Modelica** | component is all `parameter`/`constant`, no equations | output is a pure algebraic function of time only | DAE has continuous (`der`) or discrete (`when`/`pre`) **state** |

This table *is* the "way to figure out dynamic/static things in USD/rhai/Modelica"
the design needs; §3 turns each row into a concrete detector.

## 3. Mobility detection — concrete signals per layer

The classifier is a small per-layer analysis feeding one `Mobility` component.
None of it is speculative — each signal already exists in the codebase or is a
one-attribute addition.

### 3.1 USD
- **`timeSamples` = kinematic.** The animation flatten already carries
  `timeSamples`+tcps into the composed stage (project USD animation), and
  animated prims are made **Kinematic** on the physics side. That same signal
  is the `Kinematic` verdict — no new analysis, reuse the flatten output.
- **Physics body = dynamic.** A prim with a non-kinematic rigid body (avian
  `RigidBody::Dynamic`) is `Dynamic`. Kinematic/Static bodies are not.
- **Port/wire binding = dynamic.** A prim whose attribute is a cosim
  `SimConnection` target, or is written by `SetPorts`, is `Dynamic` for the
  duration of that binding (the PortRegistry already knows the target set).
- **Explicit override.** A custom `lunco:mobility` token attr (`static` |
  `kinematic` | `dynamic`) lets authors pin the verdict — cheap escape hatch,
  and the natural place for "this scenery is static, bake it hard."
- **Default:** no timeSamples + no body + no port binding ⇒ `Static`.

### 3.2 rhai
- The world-bridge already routes all script world-writes through a neutral
  path (SetPorts / set_transform / set_input). Track the **write-set** per
  script: which entities/ports/attrs it mutates.
- A script that writes only during a `Startup`/one-shot task ⇒ contributes
  `Static` (it authored initial state, then stops).
- A script writing on the recurring tick-tree (the task sequencer's
  `forever`/`repeat`) ⇒ `Kinematic` if its writes are a pure function of time
  (replayable → cache the trajectory), else `Dynamic`.
- Pragmatic first cut: **any script that writes a given target on a recurring
  schedule marks that target `Dynamic`** (safe over-approximation). Refine to
  `Kinematic` only for scripts provably time-pure (opt-in annotation, e.g.
  `#[kinematic]` on a task, mirroring the USD override).

### 3.3 Modelica
- rumoca already produces the DAE structure (state count, which outputs depend
  on states). Classify per component from that structure:
  - **0 states, output depends only on parameters/constants** ⇒ `Static`
    (evaluate once, freeze).
  - **0 states, output is algebraic in inputs** ⇒ memoizable but not static
    (pure function — safe to cache per distinct input vector; rarely worth it).
  - **≥1 continuous (`der`) or discrete (`when`/`pre`) state** ⇒ `Dynamic`
    (stateful integrator — determinism firewall, never skip/cache the step, §7).
- This is a compile-time property of the model, computed once when the DAE is
  built and stored alongside the (§5.4) cached DAE artifact.

### 3.4 Why this pays off beyond caching
The same `Mobility` label drives more than the cache:
- **Rendering** — `Static` objects go in a static batch / can bake shadows;
  only `Dynamic` casters need real-time CSM (the "bake the static, compute the
  moving" shortcut in §5).
- **Networking** — `Static`/`Kinematic` objects need no per-tick replication
  (ship the schedule once); only `Dynamic` objects enter the snapshot stream.
  This is a direct bandwidth + client-CPU win, orthogonal to FPS.
- **Physics sleep** — `Dynamic` bodies that settle can be demoted and slept.

## 4. Asset / render precompute → disk (biggest cold-start wins)

Deterministic-given-inputs, currently recomputed **every load**. Rank by payoff:

1. **Horizon heightfield bake** — `lunco-environment/src/horizon.rs`
   `start_horizon_bakes`. ~100ms/terrain, module doc literally says "geometry,
   not lighting — never needs re-baking," yet re-baked every session. Bake once,
   key = (mesh/DEM hash, resolution). Highest value / lowest risk. **Pairs with
   the render-side win** of turning the 96-step per-pixel ray-march into a baked
   horizon-angle map (see the shader analysis) — same bake step feeds both.
2. **LOD tile meshes** — `lunco-terrain-surface/src/stream_viz.rs:200`
   `LodMeshCache` is RAM-only, capped 1024, dropped on re-bake. Tile geometry is
   "a pure function of the node" (its own comment). Persist keyed by
   (DEM hash, quadtree node) → instant terrain on revisit.
3. **DEM crop/upscale working grid** — `bake.rs:28/53` `crop_centered`/`resample`,
   deterministic transform, recomputed each load.
4. **Flattened USD stages** — `lunco-usd-bevy/src/compose.rs:41` `compose_to_data`
   runs full PCP compose + `flatten_stage` inside the AssetLoader on every load.
   Cache the flattened `HashMap<SdfPath,SpecData>` keyed by transitive-closure
   content hash. (In-session reuse exists via `loaded_stages.rs`; disk does not.)
5. **Obstacle field** — `lunco-obstacle-field` is fully deterministic from
   `(spec, seed)` (ChaCha8, no entropy) but rebuilt synchronously each load.
   Currently replication-first by design (ship the seed, not the geometry) — so
   this is optional; cache only if load-time stamping shows up in profiles.
6. **Regolith FBM detail → tiling normal/roughness maps** — bake the per-pixel
   noise layers to detail textures (see shader analysis §1). This is a
   precompute of a *per-pixel* cost into a *sampled texture* — the render-side
   analogue of the same principle.

## 5. Simulation / physics caching (the part usually left out)

Physics *can* be cached, but only the **structure-stable, pure** parts — never
the integrator state.

**High-value, low-risk:**

1. **Compiled connection table** — compiles connection topology into a flat index table rebuilt only on connection change (in `lunco-cosim/src/systems/propagate.rs`). Replaces per-tick string cloning and map accumulation with direct index offsets.
2. **Avian port resolution index** — resolves name-based ports once during compile (`lunco-core/src/ports.rs:180` `ResolvedPort`) instead of scanning const tables on every tick read/write.
3. **`sync_collider` volume-dirty gate** — gates `Collider::sphere` rebuilds on volume change (`Changed<>`), eliminating per-frame allocations during steady-state.
4. **On-disk DAE artifact cache** *(future)* — `lunco-modelica/src/worker.rs:303` keeps a per-entity in-memory `CachedModel` (enables instant Reset, no recompile). The compiled DAE/state-space form is deterministic per source but **not persisted across runs** — cold-compile is minutes, warm-init ~10s. Extending `CachedModel` to write compiled blocks to a content-addressed disk artifact (key = source hash) via the §2 substrate would be the single largest *startup* win for model-heavy scenarios.

**Static-scene shortcut (architectural):** the Moon scene is static terrain +
slow sun + a few dynamic movers. Baked horizon shadows (§4.1) already cover
terrain self-shadowing; restrict real-time CSM to dynamic casters. Same
"bake the static, compute only the moving" principle as physics topology caching.

## 6. In-RAM memoization (existing, sound — leave alone)

Terrain derived layers (disk), `LodMeshCache`/`LodMaterials` (RAM), rumoca
session phase caches + MSL bincode bundle, per-entity `CachedModel` for Reset,
Modelica icon/class/negative-resolution caches, USD generation-gated parse
cache. The change-detection idiom (`Added<>`, marker `Without<>`) is sound and
should be the template for new dirty-flags.

## 7. What must NOT be cached / approximated (determinism firewall)

Live sim feeds client prediction + replication; these constraints are hard:

- **Do not reorder or defer port reads.** `ControlDacSet.before(Propagate)`
  (`lunco-cosim/src/lib.rs:118`) exists specifically to kill a 1-tick skew that
  diverged host vs client (the steering-jitter / DAC-determinism bug). Any cache
  that changes read timing or single-writer semantics (`warn_dual_driven_ports`,
  lib.rs:271) reintroduces it.
- **Do not skip Modelica steps on unchanged input.** The model is a stateful
  integrator — state advances under constant input. Only valid at proven steady
  state; not generally safe.
- **Keep sim math f64.** The 1 mm `i32` position quantization (`networking/sync.rs:65`)
  and i16 DAC round (`ports.rs:245`) are *wire/hardware* formats, not sim
  precision — do not fold them into the compute path as an approximation.
- Client role already skips cosim entirely (renders host snapshots), so all
  sim-side caching is host/single-player only.

## 8. Build order

1. **`lunco-precompute` substrate** — generalize `derived_layers.rs`.
   *Reuses today's landings:* lift the sha2-256 CID + fail-closed verify +
   `safe_rel_path` out of `lunco-networking` (invert the dep); reuse the
   existing `OpfsStorage` async backend behind one internal `#[cfg]` fork; keep a
   fast non-crypto hash for change-detection (§2 two-tier). *Net-new:*
   `CacheKey{domain,content,lod,variant}` (§2.1), an async `bake_or_load` fronted
   by the ECS task-component idiom (**not** a generic cache resource — see the
   `lunco-cache` retirement note in §1), CID-keyed blob layout for dedup, and the
   startup LRU/size-budget sweep covering both `precompute/` and `scenarios/`.
2. **`Mobility` component + per-layer detectors** (§2.2, §3) — USD first (reuse
   the animation-flatten `timeSamples` + physics-body signals), then Modelica
   (DAE state count), then rhai write-set. This is the classifier everything
   downstream keys off; build it early.
3. **Horizon bake → disk** (§4.1) — smallest, highest-value, exercises the substrate.
4. **Compiled connection table + avian port index** (§5.1–5.2) — pure per-tick win.
5. **`sync_collider` dirty gate** (§5.3) — trivial, per-frame allocation gone.
6. **LOD tile mesh + flattened USD disk cache** (§4.2, §4.4).
7. **DAE artifact disk cache** (§5.4) — biggest startup win, most integration work.
8. Wire a cache-size budget + "clear cache" into `lunco-settings` / UI.

Each step is independently shippable and measurable on the headless server
(frame-time + startup-time A/B).
