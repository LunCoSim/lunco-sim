# Efficiency & Maintainability — the North Star

> Status: Active · Audience: all contributors — the one principle, the tier ladder, and the five substrates (B–E in full)

> Umbrella for the caching/perf work. Frames the detailed docs
> (`caching-and-precompute-strategy.md`) under one principle, so the whole
> workspace moves the same direction instead of accreting one-off
> optimizations. Substrates B, C, D and E are specified **in this file**
> (§ Substrate B–E below); Substrate A lives in code (`lunco-core/derived.rs`).

## The one principle

> **Separate structure from state. Place every derived value at the cheapest
> correct tier. Invalidate on change — never on a clock.**

Almost every perf and maintainability problem we've found is a violation of this:

| Symptom | The violation |
|---|---|
| `propagate` rebuilt a string-keyed wire snapshot every tick | recomputing **structure** at **state** cadence |
| `sync_collider` rebuilt a collider every frame | derived value not memoized against its input |
| ports re-resolve name→backend every read | **resolution** (structure) fused with **transfer** (state) |
| horizon shadows re-baked every load; USD stages re-flattened every load | deterministic **artifact** recomputed instead of cached |
| USD animation samplers re-derive topology every frame | per-entity **structure** recomputed per frame |
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
  refreshed on change. Idioms: dirty-flag component (`LastColliderVolume`),
  plan component built on `Added<>` (USD `AnimationPlan`).
- **Tier 2 — change-compiled resource.** A structure-stable global fabric
  compiled once and rebuilt only on `Changed`/`Added`/`RemovedComponents` of its
  source. Idiom: `CompiledWiring`. **This tier has a shared helper** — Substrate A.
- **Tier 3 — content-addressed cache.** Expensive deterministic artifacts baked
  to disk/RAM, keyed by content hash + LOD + variant. Substrate:
  `lunco-precompute` (bake-or-load). Reference impl exists (`derived_layers.rs`).

The **determinism firewall** overrides all of it: never cache a stateful
integrator's output or reorder a schedule that encodes a data dependency
(`ControlDacSet.before(Propagate)`). Live sim math stays f64; wire quantization
is not sim precision.

## The five shared substrates

Build these once; every subsystem adopts them instead of re-solving. Each is a
face of the one principle. A is summarized here (its home is the code); B–E are
specified in full in the sections below.

### A. `RebuildOnChange<Source, Value>` — change-detected derivation (tier 2) ✅ landed

Generalizes `CompiledWiring` into a reusable type in `lunco-core`
(`derived.rs`): *"cache `Value` computed from `Source`; rebuild it **only when
`Source` changes**, never per tick."* One method, `get_or_rebuild(world,
rebuild)`. A private `ChangeDetector<S>` caches a `SystemState` so
`Changed<S>`/`RemovedComponents<S>` detection works **inside exclusive systems**
(where normal change-detection params don't exist), with a forced first-run.
`propagate_connections` uses it. Kills
the per-tick-recompute class and gives one review-checklist item: *"does this
system recompute structure at state cadence? → `RebuildOnChange` it."*

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

## Non-goals (protect these)

- **No big-bang rewrite.** Each substrate is independently shippable and
  measurable (frame-time / startup A/B on the headless server).
- **Don't touch the mature subsystems** (networking, terrain) except to adopt a
  substrate — they already embody the principle; they're the reference.
- **Don't optimize across the determinism firewall.** Prediction/replication
  correctness outranks any per-tick saving.
- **No abstraction beyond the five substrates.** The point is *fewer* ways to do
  a thing, not a framework.

---

## Substrate B — `lunco-precompute`, the content-addressed cache (tier 3)

The **tier-3** rung of the derived-data ladder — a content-addressed **disk**
cache for expensive *pure* derivations:

1. RAM memo (per-entity component / `Local`) — e.g. `AnimationPlan` (0.5).
2. Change-compiled resource — `RebuildOnChange` / `Fnv1a` keys (Substrate A/E).
3. **Content-addressed disk cache — `bake_or_load` (this).**

Run `bake(input) -> output` once, persist the output under a key that is a hash
of the *content + parameters*, and on every later run — or every peer — load it
back instead of recomputing. Byte-identical bake ⇒ byte-identical key ⇒ cache
hit, with zero coordination.

### API

```rust
pub trait Bake {
    type Output;
    const NAMESPACE: &'static str;          // e.g. "terrain/derived"
    fn key(&self) -> u64;                    // Fnv1a fold of content+params, version-first
    fn bake(&self) -> Self::Output;          // the expensive pure fn — miss only
    fn store(dir: &Path, out: &Self::Output) -> StorageResult<()>;  // one+ store_blob
    fn load(dir: &Path) -> Option<Self::Output>;                    // validate → None = miss
}

pub fn bake_or_load<B: Bake>(bake: &B, root: &Path) -> B::Output;
```

Plus helpers: `key_hex`, `entry_dir`, `store_blob`, `load_blob`, and re-exports
`Fnv1a`/`fnv1a64` (fast key) + `StorageResult`. Feature `cid` adds `blob_cid`
(cross-peer content address for entries that travel on the wire).

Entries live at `<root>/<namespace>/<key-hex>/…`. `root` is passed in
(`lunco_assets::cache_dir()`) so the crate needs no bevy/asset dep — it depends
only on **lunco-hash** (keys) + **lunco-storage** (I/O). `lunco-storage` stays
I/O-only; the CAS *policy* lives here.

### Contract / firewall

- **Pure input → pure output.** `key()` must capture everything `bake()` reads;
  fold a **format version first** so a math/layout change invalidates old
  entries (content-addressed → no explicit purge).
- **Determinism firewall.** NEVER cache stateful integrator/solver output or
  anything clock-dependent — a "hit" would serve stale physics. This tier is for
  *structure* (meshes, textures, AO/normal layers, flattened stages, colliders),
  never *state*. (Same rule that forbids caching `ControlDacSet` output.)
- **Best-effort.** A failed write only costs a rebake; `bake_or_load` never fails
  the caller for a cache-miss it already satisfied by baking.

### Consumers

- **Landed:** `lunco-terrain-surface/derived_layers.rs` — the reference impl,
  migrated onto `Bake` (`DerivedBake`, `NAMESPACE="terrain/derived"`, two blobs
  `surface.bin`/`normal.bin`). Key is byte-identical to the former inline fold,
  so pre-existing cache entries stay valid.
- **Landed:** `lunco-celestial/horizon_bake.rs` — horizon profiling/shadow bakes
  (`HorizonBake`, `NAMESPACE="celestial/horizon"`, 64KB lookup texture).
- **Planned:** USD stage flat bakes, avian collider/trimesh bakes, obstacle-field grids,
  `lunco-modelica` worker DAE cache.

### Designed, not yet built

- **LOD.** A single content key addresses one resolution; an LOD family keys each
  level (`NAMESPACE` + level in the fold) so coarse levels load first and refine.
  Fits the terrain CDLOD ring and any mip-like artifact.
- **Eviction.** No bound yet — entries accumulate. Needs an enumerate+mtime+remove
  pass over `<root>/<namespace>` (a size/age cap), which `lunco-storage` doesn't
  expose yet. When it lands, **log what is dropped** (no silent purge).
- **CID entries.** `blob_cid` exists; wiring precompute outputs into scenario
  distribution (address a baked asset by CID so peers dedup/verify) is future.

---

## Substrate C — `Mobility`, the structure/volatility classifier

### What it is (and honestly, what it isn't)

`Mobility` is the source-agnostic **declared** motion class of a physics body —
`Static` / `Kinematic` / `Dynamic` — set by whichever source spawns it (USD
physics schema, a rhai script, a Modelica model), and projected onto the live
avian `RigidBody`.

Unlike substrates A/B/D/E, C is **not a hot-path optimization**. The per-tick win
it was scoped for — static/kinematic bodies skipping physics work — is *already*
captured: the USD→avian path classifies bodies correctly and avian's solver
already skips `Static`. There is no per-frame mobility re-derivation to fix (spawn
is one-shot; the animated-demotion and `Dynamic`-settling systems are already
change-gated). So C is a **unification / structure-state** play, not a speedup.

### The structure/state split

The point is the north-star split applied to physics bodies:

- **`Mobility` = structure (declared intent).** "This rover IS a dynamic body."
  Stable. Lives in `lunco-core` (no avian dependency), so any source or reader
  sets it downward.
- **`RigidBody` = state (live engine type).** Projected from `Mobility`, but *not
  always 1:1*: a `Dynamic`-declared body spawns transiently `Kinematic` while its
  joints settle (`ShouldBeDynamic` → `activate_dynamic_bodies`), and an animated
  body is demoted to `Kinematic` so the sampler owns its pose.

Recording the declared class separately keeps the stable intent queryable even
while the engine body type is mid-transition — e.g. network-prediction
eligibility should ask "is this *meant* to be dynamic" (`Mobility::Dynamic`), not
read a body that is transiently `Kinematic` during settling.

### Wiring (additive, low-risk)

- **`lunco-core::mobility::Mobility`** — the enum + component. Neutral substrate,
  no avian.
- **USD spawn path** (`lunco-usd-avian`) records `Mobility` at every existing
  classification point (terrain / trigger / collision-child → `Static`;
  `physics:kinematicEnabled` → `Kinematic`; `PhysicsRigidBodyAPI` → `Dynamic`;
  animated-demotion → `Kinematic`). The existing
  `RigidBody`/`ShouldBeDynamic`/settling logic is **unchanged** — `Mobility` is
  added alongside it, so there is zero regression risk to the physics-sensitive
  spawn path.
- **`project_mobility_to_rigid_body`** — maps a declared `Mobility` onto a
  `RigidBody` for bodies the USD path didn't build, gated
  `(Changed<Mobility>, Without<RigidBody>)`. The `Without<RigidBody>` gate means it
  **never** overrides a USD-managed body (including the transient settling
  `Kinematic`); it only serves a rhai / Modelica / editor source that spawns a
  body by declaring mobility alone (one knob, no avian dependency upstream).
  Empty in steady state. Locked by a unit test.

### Follow-ups (deferred)

- **Live mobility flips.** A declared-mobility change on a body that already has a
  `RigidBody` (runtime static⇄dynamic) is out of scope — it needs engine-aware
  transition handling (re-inserting `RigidBody` mid-sim, re-settling joints).
- **Consumers reading intent.** Migrate call sites that inspect `RigidBody` for
  *intent* (e.g. `lunco-networking/sync.rs` prediction eligibility, which can
  misclassify a settling body) to read `Mobility` instead — a correctness
  improvement, done carefully.
- **rhai / Modelica sources.** Expose `Mobility` as a settable field/attribute so
  those runtimes declare mobility directly; the projector already honours it.

---

## Substrate D — Ports: resolve → handle (runtime data-plane)

*The port substrate itself is documented in `lunco_core::ports`.*

### The cost

Every co-sim endpoint is a `(Entity, port name)` addressed through the
`PortRegistry`, which folds over registered `PortBackend`s (first match
wins). A name read/write therefore pays, per call:

- one `world.get::<T>` **presence check per backend** until the owner is found, and
- for the avian backend, up to **six group-presence `get`s + a name scan**
  (`find_avian_port` walked `AVIAN` groups, each gated on a component).

The propagation master runs this **every tick** for every wire source and every
target. For a rover — position/velocity read and `force_y` written each tick on
avian bodies behind the `SimComponent` backend — that's the dominant port cost.
The strings were already removed (0.3's `CompiledWiring`); the remaining cost is
the per-tick backend fold + group scan.

### The model: FMI valueReference (resolve once, exchange by handle)

FMI never exchanges by variable *name* on the hot path — it resolves names to
integer **value references** once, then reads/writes by reference. Substrate D
brings that to ports:

- A backend may expose an **optional** fast path on `PortBackend`:
  `resolve_output`/`resolve_input` (name → opaque `u64` **slot**) and
  `read_slot`/`write_slot` (exchange by slot). `None` ⇒ no fast path.
- The registry resolves an endpoint to a `ResolvedPort { backend, slot }` once,
  then `read_resolved`/`write_resolved` dispatch straight to the owning backend —
  **no fold, no group scan**.
- `ResolvedPort` is **process-local** (like an FMI value reference / a port slot):
  the `slot` is backend-private and MUST NOT be serialized or sent on the wire —
  resolve fresh on every peer. This keeps it inside the determinism firewall.

#### Who opts in

Only backends behind a multi-group scan benefit, so only they implement the fast
path:

| Backend | Fast path? | Why |
|---|---|---|
| `SimComponent` (Modelica map) | no (`None`) | registered first — a name read is already one `get` + map lookup |
| **avian** (bodies/joints/sensors) | **yes** | slot = `(group_index << 16) \| port_index` into `AVIAN`; collapses the 6-group scan to one component access |
| `Port` | no | single fixed port on one component |
| FSW command (map) | no (for now) | map-backed; a fast path needs a name interner (slot can't carry the string) — a documented follow-up |

The name-based avian ops are now **derived** from resolve→slot (the old
`find_avian_port` duplication is gone): `read_output = resolve_output ∘
read_slot`. This is the "name-based API derived from the handle model, no
per-backend duplication" endgame, applied where it pays.

### Correctness

- **Precedence-preserving.** `resolve_*` walks backends in registration order and
  stops at the FIRST owner, so a lower-precedence fast-path backend (avian) can
  never shadow a higher-precedence name-only owner (`SimComponent`) when a name
  collides on one entity. If the winner has no fast path, resolution returns
  `None` and the caller uses the name path — same backend, same result.
  - Outputs are readable, so `read_output.is_some()` detects ownership.
  - Inputs may be **write-only** (avian `force_y` reads `None`), so resolution
    also accepts a backend's own `resolve_input` as the write-ownership
    authority. Precedence holds for our registration order (the readable-input
    backends `SimComponent`/FSW precede the write-only avian one).
- **Stale-handle safe.** A cached handle whose component was removed/swapped since
  the last rebuild makes `read_resolved`/`write_resolved` return `None`/`false`;
  the propagate loop then falls back to the name path (short-circuiting so a
  successful slot write never double-writes). Behaviour is identical to the
  pre-resolve master.
- **Invalidation.** Handles are cached in `CompiledWiring`, rebuilt (via
  `RebuildOnChange`) when the `SimConnection` set changes. A component swap
  without a wiring change is covered by the per-tick fallback above.

### Where it's wired

`lunco-cosim/systems/propagate.rs` — `CompiledWiring::rebuild` resolves every wire
source and every distinct target once; the accumulate/write phases exchange by
handle. One-shot name callers (API `Get`/`SetPorts`, scripting, the inspector) are
unchanged — they don't need resolution and pay no migration cost.

### Follow-ups

- **Map-backed fast path.** A small process-local name interner would let
  `SimComponent`/FSW resolve to a slot too (removing the fold for FSW drive-command
  writes, which currently fall back through the avian scan). Add if profiling
  shows it matters.
- **Register-order invariant.** `ResolvedPort.backend` is an index into the
  registry's fixed startup registration; no backend may be registered after the
  first resolve. (All registration is in plugin `build` today.)

---

## Substrate E — `lunco-hash`, one hashing primitive

### The problem

Three hashing jobs recur across the workspace, and before E they were
hand-rolled and duplicated:

| Job | Where (pre-E) | Algorithm | Stability contract |
|---|---|---|---|
| **Fast change / cache keys** | `lunco-terrain-surface/derived_layers.rs` `cache_key`; scattered `DefaultHasher` in `networking/shared.rs`, `modelica/experiments_runner.rs`, `modelica/.../render.rs`, `lunco-theme` | FNV-1a word-fold **or** std `DefaultHasher` | frozen to *nothing* — bump a format version to invalidate |
| **Cross-peer identity** | `lunco-core/identity.rs` `fnv1a64`→`fold_53`; reference copy in `networking/proto-tests` | byte-wise FNV-1a | frozen to the **wire** (two peers must agree) |
| **Content addressing (CID)** | `lunco-networking/scenario.rs` `cid_for_content` | CIDv1 `raw`(0x55)+sha2-256 | frozen to **IPFS** (`ipfs add --raw-leaves --cid-version 1`) |

The FNV-1a constants (`0xcbf2…` basis, `0x0100…01b3` prime) were literally
copy-pasted between `identity.rs` and `derived_layers.rs`. `DefaultHasher` was
reached for as a "cache key" in several places despite std giving **no**
cross-version/-platform stability guarantee for its algorithm — a latent
portability bug for anything persisted.

### The design: two tiers, one front-end, one firewall

`lunco-hash` is a small, dependency-free crate (the CID tier is behind a `cid`
feature, so the lowest-level crates pull only the fast tier and stay wasm-clean):

- **Fast tier** — `Fnv1a` / `fnv1a64`. Non-cryptographic, folds structured
  fields directly (`h.write_u64(x.to_bits())`) with no serialization. Two write
  granularities share the same math:
  - `write_bytes` — canonical byte-wise FNV-1a (**wire-locked**: network identity).
  - `write_u64` — word-wise xor-multiply fold (numeric cache keys).
- **CID tier** (`content`, feature `cid`) — `content::cid()` /
  `content::cid_from_bytes()`, re-exporting the `Cid` type. CIDv1 raw+sha2-256,
  IPFS-interop, for **on-disk precompute entries and on-wire asset transfer**.

The **firewall**: fast tier = process-local / ephemeral / structured-fold; CID
tier = cross-peer / persisted / byte-content. Making the line explicit stops
anyone reaching for `DefaultHasher` when they need a stable key, or paying sha2
where a change-check will do.

### Why we can't just reuse the CID everywhere

1. **Cost at the wrong cadence.** sha2-256 is ~an order of magnitude slower than
   FNV. Paying it per frame to answer "did this change?" is the
   per-tick-recompute anti-pattern in disguise.
2. **Wrong input shape.** A CID addresses `&[u8]` — you must serialize first. The
   fast tier folds fields directly, no allocation (the repo's "hash, don't
   serialize" idiom).
3. **Different guarantees.** A CID must be collision-resistant *and* IPFS-stable
   (multihash framing, canonical bytes). A local cache key needs none of that.
4. **Independent contracts.** Identity is frozen to the wire, the CID to IPFS, a
   cache key to nothing. One number would entangle three locks — you couldn't
   change cache-key math without risking wire identity or IPFS interop.
5. **Determinism-firewall boundary.** The fast hash is process-local (like port
   slots); the CID must be cross-peer-stable. Same reason the two live in one
   crate with a bright line between them.

Symmetrically, we can't use the fast hash for content addressing: it isn't
collision-resistant for adversarial/wire content and has no IPFS framing.

### Consumers

- **Landed:** `lunco-core/identity.rs` (byte-wise, via `fnv1a64`);
  `lunco-terrain-surface/derived_layers.rs` `cache_key` (word-fold, via `Fnv1a` —
  byte-identical to the old inline fold, so existing cache entries stay valid);
  `lunco-networking/scenario.rs` CID (via `content::cid` — dropped the direct
  `cid`/`multihash-codetable` deps).
- **Next:** **Substrate B `lunco-precompute`** keys its `bake_or_load` disk cache
  with the fast tier and content-addresses persisted blobs with the CID tier —
  one substrate for both, instead of every consumer re-deriving. The remaining
  ad-hoc `DefaultHasher` cache-key sites (`modelica/experiments_runner.rs`) should
  migrate to `fnv1a64` for cross-run reproducibility.

### Stability note

`identity`'s byte-wise fold stays byte-locked to the `networking/proto-tests`
reference (an independent dependency-free copy, kept deliberately). Do not alter
`write_bytes`/`fnv1a64` without updating that reference in lockstep, or two peers
stop agreeing on identity.
