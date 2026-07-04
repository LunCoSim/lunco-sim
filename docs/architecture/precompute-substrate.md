# Precompute substrate (`lunco-precompute`) — Substrate B

*Part of the efficiency/maintainability architecture. Builds on
`hashing-substrate.md` (Substrate E) and `caching-and-precompute-strategy.md`.*

## What it is

The **tier-3** rung of the derived-data ladder — a content-addressed **disk**
cache for expensive *pure* derivations:

1. RAM memo (per-entity component / `Local`) — e.g. `AnimationPlan` (0.5).
2. Change-compiled resource — `RebuildOnChange` / `Fnv1a` keys (Substrate A/E).
3. **Content-addressed disk cache — `bake_or_load` (this).**

Run `bake(input) -> output` once, persist the output under a key that is a hash
of the *content + parameters*, and on every later run — or every peer — load it
back instead of recomputing. Byte-identical bake ⇒ byte-identical key ⇒ cache
hit, with zero coordination.

## API

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

## Contract / firewall

- **Pure input → pure output.** `key()` must capture everything `bake()` reads;
  fold a **format version first** so a math/layout change invalidates old
  entries (content-addressed → no explicit purge).
- **Determinism firewall.** NEVER cache stateful integrator/solver output or
  anything clock-dependent — a "hit" would serve stale physics. This tier is for
  *structure* (meshes, textures, AO/normal layers, flattened stages, colliders),
  never *state*. (Same rule that forbids caching `ControlDacSet` output.)
- **Best-effort.** A failed write only costs a rebake; `bake_or_load` never fails
  the caller for a cache-miss it already satisfied by baking.

## Consumers

- **Landed:** `lunco-terrain-surface/derived_layers.rs` — the reference impl,
  migrated onto `Bake` (`DerivedBake`, `NAMESPACE="terrain/derived"`, two blobs
  `surface.bin`/`normal.bin`). Key is byte-identical to the former inline fold,
  so pre-existing cache entries stay valid.
- **Landed:** `lunco-celestial/horizon_bake.rs` — horizon profiling/shadow bakes
  (`HorizonBake`, `NAMESPACE="celestial/horizon"`, 64KB lookup texture).
- **Planned:** USD stage flat bakes, avian collider/trimesh bakes, obstacle-field grids,
  `lunco-modelica` worker DAE cache.

## Designed, not yet built

- **LOD.** A single content key addresses one resolution; an LOD family keys each
  level (`NAMESPACE` + level in the fold) so coarse levels load first and refine.
  Fits the terrain CDLOD ring and any mip-like artifact.
- **Eviction.** No bound yet — entries accumulate. Needs an enumerate+mtime+remove
  pass over `<root>/<namespace>` (a size/age cap), which `lunco-storage` doesn't
  expose yet. When it lands, **log what is dropped** (no silent purge).
- **CID entries.** `blob_cid` exists; wiring precompute outputs into scenario
  distribution (address a baked asset by CID so peers dedup/verify) is future.
