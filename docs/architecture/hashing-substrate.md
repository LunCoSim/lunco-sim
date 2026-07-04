# Hashing substrate (`lunco-hash`) — Substrate E

*Part of the efficiency/maintainability architecture. See
`caching-and-precompute-strategy.md`.*

## The problem

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

## The design: two tiers, one front-end, one firewall

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

## Why we can't just reuse the CID everywhere

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

## Consumers

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

## Stability note

`identity`'s byte-wise fold stays byte-locked to the `networking/proto-tests`
reference (an independent dependency-free copy, kept deliberately). Do not alter
`write_bytes`/`fnv1a64` without updating that reference in lockstep, or two peers
stop agreeing on identity.
