# Caching & Precompute — Implementation Plan

> Companion to `caching-and-precompute-strategy.md` (the design). This is the
> build sequence: concrete, ordered, each step independently shippable and
> verifiable. Phases 0 are quick strictly-better wins with **no dependency** on
> the substrate; ship them first. Phases A–F build the universal cache.
>
> Constraints threaded throughout (from repo conventions):
> - CAS / cache logic lives in a **new crate**, never in `lunco-storage`
>   (`lunco-storage` = I/O only).
> - All file writes go through the **Storage API**, never raw `std::fs`.
> - **One canonical form** — when lifting CID out of networking, update call
>   sites; no back-compat re-export shim.
> - Sim-path steps must not cross the **determinism firewall** (design §7).

---

## Phase 0 — Quick strictly-better wins (no substrate, ship first)

Zero visual/behavioral change; pure removal of wasted work. Each is one small PR.

### 0.1 ~~Gate the always-on physics diagnostics plugin~~ — STRUCK (no benefit)
- **Dropped after investigating avian3d 0.6.1.** The ~30 ms spikes are **not
  caused** by `PhysicsTotalDiagnosticsPlugin` — its systems are microseconds
  (`Instant::now`/`elapsed` + one resource write). They bracket the physics step
  (`PhysicsStepSystems::First`/`Last`), so `step_time` *measures* the real
  step-cost spike; it does not create it. Gating/removing the plugin removes the
  measurement, not the cost → **zero FPS gain**.
- Also not cleanly gatable: avian 0.6.1 exposes no runtime toggle; the
  step-timing system is welded into the core `PhysicsStepSystems::Last` set
  (can't run-condition it externally without gating real physics); Bevy plugins
  can't be removed post-startup; a build-time gate reintroduces the "phys reads
  zero on runtime toggle" bug.
- The original audit misread `perf_bridge.rs`'s comment (a profiler span
  capturing the step, phrased as if the plugin were the source). The real
  physics-spike lever is the step itself (`SubstepCount(12)` × solver) — a
  stability/quality knob, not a free win, so out of scope for Phase 0.
- **Done instead:** corrected the misleading comment in `perf_bridge.rs`
  (doc-only, no behavior change); plugin stays always-on.

### 0.2 `sync_collider` volume-dirty gate
- **File:** `crates/lunco-cosim/src/systems/collider.rs:20`
- **Change:** store last-applied `volume` (or gate on `Changed<SimComponent>` +
  compare); rebuild `Collider::sphere` only when `volume` actually changed.
- **Accept:** a steady-volume body allocates zero colliders/frame (add a debug
  counter or step through once); collider still tracks when volume changes.
- **Risk:** none. **Size:** XS.

### 0.3 Compiled connection table + avian port index
- **Files:** `crates/lunco-cosim/src/systems/propagate.rs:51-111`,
  `crates/lunco-core/src/ports.rs:109` (`find_avian_port`).
- **Change:** build a flat `Vec<CompiledWire{ src_slot, dst_slot, scale, offset }>`
  resource, rebuilt only on `Added/RemovedComponents<SimConnection>` (detection
  exists at `lunco-cosim/src/lib.rs:252`). Resolve avian port names → slot index
  once at compile time. Per-tick `propagate_connections` iterates the table:
  no string clones, no `PortRegistry` clone, no `HashMap`.
- **Accept:** cosim tests still green (`cargo test -p lunco-cosim`); values +
  order bit-identical (diff a recorded port trace before/after); host/client
  drive still converges on the headless server.
- **Firewall:** do **not** move `ControlDacSet.before(Propagate)`
  (`lib.rs:118`) or change single-writer semantics (`warn_dual_driven_ports`,
  `lib.rs:271`). **Risk:** low. **Size:** M.

**Milestone 0:** measurable frame-time + smoothness win on low-end, no substrate yet.

---

## Phase 0′ — The per-tick-recompute pattern (audit + sweep)

0.2 and 0.3 are the same defect: **a per-tick/per-frame system recomputes a
value that is a pure function of inputs which change far less often than every
tick, with no change-detection gate.** A repo-wide sweep (2026-07-02) confirms
it recurs — but the codebase is mostly mature: networking and terrain are
**fully gated** (change-detection, `Without<>` markers, throttles, off-thread
generation caches); most of USD/render is too. Only a handful survive.

### The three fix shapes (pick by cardinality)
1. **Compiled resource** (many → one derived table): hoist the fabric into a
   resource rebuilt only on `Changed<>`/`RemovedComponents`. — *0.3
   `CompiledWiring`.* Use for global structures (wiring, indices, scene graphs).
2. **Per-entity plan/memo component**: build a cached derived component once on
   `Added<>` (or rebuild on the source's change event); the hot system reads the
   memo and does only the irreducible per-tick work. — *0.2 `LastColliderVolume`;
   USD-animation plan (0.5).* Use for per-entity derived data.
3. **Deferred-behind-gate**: the guard already exists but the expensive work
   runs *before* it — move the work inside the guard. — *scenario-source clone
   (0.4), Python compile (CQ-217).* Use when a recompile/generation check is
   already present.

### Verified instances (from the sweep)

### 0.4 ScenarioDriver per-tick source/params clone — ✅ DONE (green)
- **File:** `crates/lunco-scripting/src/scenario.rs:251` (`ScenarioDriver::run`,
  `FixedUpdate`).
- **Fixed:** phase 1 now computes the recompile predicate (from the driver's
  per-entity `Fsm` state) and clones `source`/`params` **only when a recompile is
  due**; `work` carries `Option<(source, params)>`, and its `Some`-ness is the
  loop's recompile gate. Zero source clones on the steady path. Shape 3.
- **Note on Substrate A:** a workspace sweep found **no other genuine tier-2
  `RebuildOnChange` consumer** — the two structural fits (`sync_api_registry`,
  entity-tree view) are already gated, and they're *normal* systems where plain
  `Changed<>`/`Added<>` beats `RebuildOnChange` (which exists for *exclusive*
  systems). So A stays scoped to `propagate` — a good outcome, not a gap.
- **Defect:** clones `doc.source` + `doc.params` (full multi-KB strings) for
  every non-paused scenario **every tick**, but they're consumed only in the
  recompile branch gated at `:301` on `!compiled || generation != generation`.
- **Fix (shape 3):** carry only `(entity, gid, generation, authority)` in phase 1;
  re-read the source from `ScriptRegistry` inside the `resource_scope` block
  *only when* the recompile guard fires. Zero source clones on the steady path.
- **Payoff:** N × (multi-KB alloc + memcpy) per tick removed (N = active
  scenarios). **Risk:** none. **Size:** S.

### 0.5 USD animation samplers re-derive topology every frame — ✅ LANDED (worktree `caching-phase0`)
**Done:** `AnimationPlan` component (`lunco-usd-bevy/src/lib.rs`) caches parsed
`SdfPath`, `timeCodesPerSecond`, an `XformDrive` enum (OpOrder/Matrix/Trs{t,r,s}/None),
a `visibility` flag, and `Option<MaterialPlan>` (resolved shader `SdfPath` +
diffuse/geom_color/opacity flags). `plan_usd_animation` derives it once
(`Without<AnimationPlan>` retry-until-stage-loaded gate, empty in steady state);
`clear_animation_plans_on_stage_reload` drops plans on the `UsdStageAsset`
`Modified`/`LoadedWithDependencies` event so they re-derive. Both samplers now
consume the plan and only read values at `t` — zero per-frame `SdfPath` parse,
zero `resolve_bound_shader` 2-hop traversal, zero topology scans. Schedule:
`clear → plan → (sample_xform, sample_material)`, chained.


- **Files:** `crates/lunco-usd-bevy/src/lib.rs:1796` (`sample_usd_animation`),
  `:1872` (`sample_usd_material_animation`), both `Update`, per `UsdAnimated`.
- **Defect:** only the sampled value at time `t` is per-frame; everything else is
  stage-derived structure recomputed each frame: `SdfPath::new(...)` re-parse
  (`:1808`/`:1885`), xform-mode/rotation/time-sample scans, and — worst — a
  2-hop `material:binding → outputs:surface → parent` traversal
  (`resolve_bound_shader`, `:1225`) run every frame purely to locate the shader.
  Binding topology changes only on stage reload.
- **Fix (shape 2):** a per-entity `AnimationPlan` component (cached `SdfPath`,
  resolved shader `SdfPath`/`None`, xform-mode enum, per-channel `timeSamples`
  bitflags) built where `UsdAnimated` is added (next to the already-`Added`-driven
  `bind_animated_to_preview`) or rebuilt on the stage `AssetEvent`. Samplers then
  only sample values at `t`.
- **Payoff:** per animated entity per frame: one `SdfPath` parse + (material) a
  2-hop traversal + 3 field lookups eliminated. Scales with animated-entity
  count × framerate. **Risk:** low (must invalidate on stage reload). **Size:** M.

### 0.3b Fold avian port resolution into `CompiledWiring` — RISK RE-ASSESSED, DEFERRED
- **File:** `crates/lunco-cosim/src/ports.rs:109` `find_avian_port` — per wire
  endpoint per tick, up to 6 component-presence checks + ~30 `name ==` compares.
- **Closer read changes the picture** (2026-07-02):
  1. **The dominant cost is the presence checks (`world.get::<T>` ×6), not the
     name compares.** Those are inherently world-state-dependent, so a static
     `name → &AvianPort` map does *not* remove the real cost — only a per-endpoint
     *resolution cache* (skip straight to the resolved `&'static AvianPort`) does.
  2. **A resolution cache needs a new resolve-handle API on every `PortBackend`.**
     You can't probe a *target* by reading it: write-only inputs (e.g. `force_y`,
     `port.read == None`) return `None` from `read_input`, so "which backend owns
     this write port" can't be discovered by a read. `PortBackend` would need a
     `resolve_output`/`resolve_input(&World, Entity, &str) -> Option<Handle>` on
     all four backends (`lunco-core` API change).
  3. **Invalidation is the determinism-critical part.** The resolved backend/port
     depends on the *endpoint entity's components*, which can change without the
     `SimConnection` changing. So `Changed<SimConnection>` is insufficient —
     invalidation must also fire on `Added`/`RemovedComponents` of ~9 port-bearing
     types (RigidBody, the two joints, three sensors, SimComponent, PhysicalPort,
     DigitalPort). Miss one and a stale resolution diverges host vs client. A
     "try cached backend first" shortcut is **not** precedence-safe (a later
     higher-precedence backend gaining the same name would be skipped).
- **Verdict:** the *safe* form is a `lunco-core` resolve-API + broad structural
  invalidation + tests — **M→L and determinism-sensitive**, for a payoff the
  sweep itself called "modest at today's wire counts." **Deferred** until wire/
  port counts grow enough to justify it; the name-based path (0.3) is correct and
  simple. Revisit as a dedicated, fully-invalidated change, not a Phase-0 quick win.

### Tracked / low-priority (not new work here)
- **CQ-217** (`lunco-scripting/src/lib.rs:224-236`): Python source re-parsed +
  recompiled every tick; fix already documented inline (cache code object on
  source revision). Shape 3.
- **`recompute_interest` scaffolding** (`lunco-networking/src/sync.rs:1638`): the
  5 Hz pass rebuilds stable scaffolding (`registry.snapshot()` clone, ownerless
  set, RBAC sessions) that changes only on claim/release/join/leave. Micro-opt
  only — the dominant position pass must run regardless; any cache must still feed
  *current* positions to `compute_interest_sets` or AOI desyncs.

### Convention to stop the pattern recurring
When adding a per-frame/tick system, ask: *does this recompute anything that's a
pure function of inputs stabler than the tick rate?* If yes, gate it (shape 1–3).
The mature subsystems (networking/terrain) are the template — copy their
`Changed<>`/`Without<>`/throttle idioms. Consider a review-checklist item.

**Milestone 0′:** the surviving per-tick-recompute instances closed;
convention documented so new systems don't reintroduce it.

---

## Phase A — Substrate crate `lunco-precompute` (foundation)

Small, because OPFS + a real CID already shipped (2026-07-02). This phase is
mostly **consolidation**.

### A1 Create the crate + lift the content-address primitive
- **New crate:** `crates/lunco-precompute` (deps: `lunco-storage`,
  `lunco-assets` for `cache_dir()`, `sha2`, `bevy` minimal).
- **Move** from `lunco-networking/src/scenario.rs:54-66` into
  `lunco-precompute::cas`: `cid_for_content`, `cid_from_bytes`, the incremental
  fail-closed verify helper, and `safe_rel_path` (`scenario_sync.rs:153`).
- **Invert the dep:** `lunco-networking` now depends on `lunco-precompute` and
  imports these from it. Update all call sites; **no re-export shim** left in
  networking.
- **Accept:** `cargo check -p lunco-networking -p lunco-precompute` green;
  networking scenario-sync tests still pass unchanged.
- **Size:** S.

### A2 `Cid` + `CacheKey` types
- **In `lunco-precompute`:**
  ```rust
  struct Cid([u8; 36]);                 // CIDv1 raw 0x55 + sha2-256, as shipped
  struct CacheKey {
      domain: &'static str,             // "terrain.horizon", "modelica.dae", …
      content: Cid,                     // CID of ALL inputs (incl. FORMAT_VERSION)
      lod: u8,                          // 0 = coarsest; single-fidelity ⇒ 0
      variant: u16,                     // platform/preset bits
  }
  impl CacheKey { fn disk_path(&self) -> PathBuf }  // cache_dir()/precompute/<domain>/<cid>[.lod<n>.v<variant>]
  ```
- **FORMAT_VERSION** is folded into `content` (hash a `(version, inputs)` tuple)
  so a producer change invalidates transparently.
- **Accept:** unit test — same inputs → same path; changed input or version →
  different path.
- **Size:** S.

### A3 `BlobStore` — one internal native/web fork
- **API (async):**
  ```rust
  async fn has(key: &CacheKey) -> bool;
  async fn get(key: &CacheKey) -> Option<Vec<u8>>;
  async fn put(key: &CacheKey, bytes: &[u8]) -> io::Result<()>;
  ```
- **Impl:** the *single* `#[cfg]` fork in the whole codebase for cache I/O,
  mirroring `scenario_sync.rs:327 do_write`:
  - native → `lunco_storage::FileStorage` on `AsyncComputeTaskPool`;
  - wasm → existing `OpfsStorage::{read,write,exists}` via `spawn_local`,
    result returned over a crossbeam channel to a Bevy system.
- **All writes via the Storage API** (atomic write path), never `std::fs`.
- **Accept:** native round-trip test (put→has→get); wasm smoke via the web
  build writing/reading one blob under `precompute/`.
- **Size:** M (the fork plumbing is the bulk).

### A4 `bake_or_load` + RAM tier
- **API:**
  ```rust
  async fn bake_or_load<T: Cacheable>(key: CacheKey, produce: impl FnOnce() -> T) -> T;
  ```
  Flow: check `lunco-cache::ResourceCache` (RAM) → `BlobStore::get` (disk) →
  else run `produce` on the compute pool, then **write-through** RAM + disk.
  `Cacheable` = `{ fn to_bytes(&self)->Vec<u8>; fn from_bytes(&[u8])->Self }`
  (bincode/POD, **not JSON**).
- Layered **under** `ResourceCache` so in-flight dedup + poll-per-frame come free.
- **Accept:** call twice with a counting `produce` → runs once; restart process →
  disk hit, `produce` not run.
- **Size:** M.

### A5 Bevy plugin surface
- `PrecomputePlugin` exposing `bake_or_load` via a `SystemParam` or resource;
  registers the crossbeam-channel drain system.
- **Accept:** a throwaway system bakes+reads a dummy artifact end-to-end in-app.
- **Size:** S.

**Milestone A:** a working async, content-addressed, cross-platform
`bake_or_load` with RAM+disk tiers and CID dedup. No consumer yet.

---

## Phase B — First real consumer: horizon bake → disk (proves the substrate)

Highest value / lowest risk asset bake (design §4.1).

### B1 Key the horizon bake
- **File:** `crates/lunco-environment/src/horizon.rs` (`bake_heightfield`).
- Compute `content` = CID of `(DEM/mesh hash, resolution, FORMAT_VERSION)`.
- **Size:** S.

### B2 Wrap in `bake_or_load`
- Replace the unconditional bake in `start_horizon_bakes` with
  `bake_or_load(key, || bake_heightfield(...))`. Serialize the R32Float
  heightfield as POD bytes + a tiny header (res, extent).
- **Accept:** first load bakes (~100 ms); second load of the same terrain is a
  disk hit (log "horizon cache hit"), shadows identical. A/B startup time on the
  headless server.
- **Firewall:** none (pure geometry). **Size:** S.

### B3 (Stretch) render-side horizon-angle map
- Extend the same bake to also emit the azimuth-binned horizon-angle texture
  (design: replaces the 96-step per-pixel march). Separate follow-up; gated on
  B2 landing. **Size:** L (shader work) — track independently.

**Milestone B:** substrate exercised end-to-end; instant terrain shadows on revisit.

---

## Phase C — Mobility classifier (the cross-layer static/dynamic input)

Design §2.2 / §3. Build the label + one detector at a time; each detector is
useful alone.

### C1 `Mobility` component + join rule
- **Where:** `lunco-core` (shared).
  ```rust
  enum Mobility { Static, Kinematic { key: u64 }, Dynamic }
  ```
- A change-detected system computes the per-entity join of all layer votes
  (`Dynamic > Kinematic > Static`) and **emits an invalidation event on any
  promotion toward more-dynamic** (so caches keyed on the entity drop).
- **Accept:** unit test the join precedence + promotion event.
- **Size:** S.

### C2 USD detector (do first — signals already exist)
- `timeSamples` present (animation flatten already carries these; animated prims
  already forced Kinematic) ⇒ `Kinematic`; non-kinematic rigid body or cosim
  port/wire binding ⇒ `Dynamic`; else `Static`. Optional `lunco:mobility`
  override attr.
- **Files:** USD→ECS projection + `lunco-usd-sim` (port-binding set from
  PortRegistry).
- **Accept:** a scene with an animated prim, a dynamic body, and static scenery
  gets the three verdicts; flipping a body to kinematic re-labels it live.
- **Size:** M.

### C3 Modelica detector
- From the rumoca DAE structure: 0 states + param-only ⇒ `Static`; algebraic ⇒
  memoizable; ≥1 `der`/`when` state ⇒ `Dynamic`. Compute once at DAE build,
  store alongside the (Phase E) DAE artifact.
- **Accept:** a stateful model labels `Dynamic`; a constant-only block `Static`.
- **Size:** M.

### C4 rhai detector
- Track the world-bridge write-set per script; Startup-only writes ⇒ `Static`
  contribution; recurring-tick writes ⇒ `Dynamic` (safe over-approx), refine to
  `Kinematic` only for opt-in time-pure tasks.
- **Accept:** a `forever` script writing a transform marks its target `Dynamic`.
- **Size:** M.

**Milestone C:** every object carries a live static/kinematic/dynamic label —
consumed by caching, rendering static-batch, and networking replication skip.

---

## Phase D — LOD generalization + more bake consumers

### D1 LOD as a key dimension
- Make `bake_or_load` honor `lod`; add **progressive serve**: request `lod=N`,
  if only a coarser rung resident, return it now + spawn the finer bake.
- Retrofit terrain's in-RAM `LodMeshCache`/`LodMaterials` (`stream_viz.rs`) to
  persist per-rung via the substrate. **Size:** M.

### D2 Flattened USD stage cache
- `lunco-usd-bevy/src/compose.rs:41` `compose_to_data` → wrap in `bake_or_load`
  keyed by transitive-closure CID; store the flat `HashMap<SdfPath,SpecData>`.
- **Firewall:** invalidate on any source edit (hash covers the closure). **Size:** M.

### D3 DEM crop/resample working grid
- `bake.rs:28/53` → `bake_or_load` keyed by (DEM CID, window, res). **Size:** S.

**Milestone D:** cold-load dominated by disk hits; LOD is one code path.

---

## Phase E — Sim-side disk caches (biggest startup win, most care)

### E1 On-disk DAE artifact cache
- `lunco-modelica/src/worker.rs:303` `CachedModel` → persist the compiled
  DAE/state-space form via `bake_or_load` keyed by **source CID**. Cuts
  cold-compile (minutes) / warm-init (~10 s) to a disk load on repeat runs.
- **Firewall:** cache the *compiled structure*, never step outputs (stateful
  integrator, design §7). **Size:** L (rumoca artifact serialization is the work).

**Milestone E:** model-heavy scenarios start fast on the second run.

---

## Phase F — Eviction + user surface

### F1 Startup LRU sweep
- New system: scan `cache_dir()/precompute/` **and** the existing
  `cache_dir()/scenarios/` tree, evict by mtime to a size budget. Regenerable /
  re-fetchable, so eviction is always safe. Fixes today's unbounded scenario
  growth too.
- **Size:** M.

### F2 Settings + UI
- `CacheSettings` `SettingsSection` (size budget); a "Clear cache" action;
  optional cache-stats readout.
- **Size:** S.

**Milestone F:** bounded, user-controllable cache across all domains.

---

## Dependency graph (what unblocks what)

```
Phase 0  ──────────────────────────────────  (independent, ship anytime)
Phase A ─┬─> Phase B  (horizon bake)
         ├─> Phase D  (USD/DEM/LOD baked via substrate)
         └─> Phase E  (DAE disk cache)
Phase C ──> feeds D (LOD/mobility-gated invalidation), rendering, networking
Phase F  depends on A (shares the cache root taxonomy)
```

## Recommended order
`0.2` → `0.3` → **A1–A5** → **B1/B2** → **C1/C2** →
`F1` → `D1–D3` → `C3/C4` → `E1` → `B3`/`F2`.
*(0.1 struck — see above; no perf benefit.)*

Rationale: bank the zero-risk frame-time wins immediately; stand up the
substrate; prove it with horizon; get the USD mobility label (cheapest, unblocks
render/network wins); cap cache size before baking a lot to disk; then the
heavier LOD/DAE consumers.

## Verification per step
- `cargo check --workspace --tests` + the crate's own `cargo test`.
- Behavioral: drive the affected flow on the **headless sandbox server** and
  read back via MCP (`read_ports`/`watch_ports`, `capture_screenshot`), A/B
  frame-time (Phase 0/B/D) or startup-time (B/E) before vs after.
- Determinism (Phase 0.3, E): compare a recorded port trace host-vs-client for
  bit-identity; confirm predicted drive still converges.
