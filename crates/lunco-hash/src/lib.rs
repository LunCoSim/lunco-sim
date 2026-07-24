//! The workspace hashing substrate — **two tiers behind one field-streaming
//! front-end**, with a firewall between them.
//!
//! Three hashing jobs recur across the codebase; they are *not* the same job and
//! must not collapse into one number (see `docs/architecture/hashing-substrate.md`):
//!
//! 1. **Fast change / cache keys** — "did this content change?" / "what's the
//!    disk-cache key for this bake?". Process-local, non-adversarial, runs at
//!    frame/tick cadence over large buffers. Wants a *cheap* non-cryptographic
//!    hash that folds structured fields directly (no serialization). → [`Fnv1a`]
//!    / [`fnv1a64`].
//! 2. **Cross-peer identity** — a deterministic id from provenance, byte-locked
//!    so two independent peers agree with zero coordination. Same FNV-1a as (1),
//!    but its stability is a *wire contract* (see `lunco-core`'s `identity`).
//! 3. **Content addressing** — an IPFS-interop, collision-resistant address of a
//!    blob's *bytes*, carried on disk/wire. → [`content`] (feature `cid`).
//!
//! ## Why not reuse the CID (sha2-256) for the fast tier?
//!
//! - **Cost/cadence.** sha2-256 is ~an order of magnitude slower than FNV; paying
//!   it per frame to answer "did this change?" is the per-tick-recompute
//!   anti-pattern in disguise.
//! - **Input shape.** A CID addresses `&[u8]` — you must serialize first. The
//!   fast tier folds fields directly (`h.write_u64(x.to_bits())`), no allocation.
//! - **Guarantees.** A CID must be collision-resistant *and* IPFS-stable
//!   (multihash framing, canonical bytes). A local cache key needs none of that.
//! - **Independent contracts.** Identity is frozen to the wire, the CID to IPFS,
//!   a cache key to *nothing* (bump a format version to invalidate). One number
//!   would entangle three locks.
//!
//! So: fast tier = local/ephemeral/structured-fold; CID tier =
//! cross-peer/persisted/byte-content. This crate owns both and the line between.

#![cfg_attr(not(feature = "cid"), no_std)]

/// FNV-1a 64-bit offset basis. Public so a caller can reproduce the fold by hand
/// if it must, but prefer [`Fnv1a`].
pub const FNV1A_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
pub const FNV1A_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Streaming FNV-1a 64-bit hasher — the fast tier.
///
/// Fixed constants, identical on every platform: unlike `std`'s `DefaultHasher`
/// (whose algorithm carries **no** cross-version/-platform stability guarantee),
/// a value folded here is reproducible across runs and machines — which is what
/// a *persisted* or *cross-peer* key requires. Two write granularities share the
/// same math:
///
/// - [`write_bytes`](Self::write_bytes) — canonical byte-wise FNV-1a. This is the
///   variant network identity is **byte-locked** to (`lunco-core`'s `identity`
///   + the `lunco-networking` proto-tests). Do not alter it.
/// - [`write_u64`](Self::write_u64) — fold a whole 64-bit word in one xor-multiply
///   step. Cheaper and endianness-independent for numeric cache keys where you
///   control both ends (NOT a wire-stable byte framing).
#[derive(Clone, Copy, Debug)]
pub struct Fnv1a {
    state: u64,
}

impl Fnv1a {
    /// A fresh hasher seeded with the FNV-1a offset basis.
    pub const fn new() -> Self {
        Self {
            state: FNV1A_OFFSET_BASIS,
        }
    }

    /// Canonical byte-wise FNV-1a. **Wire-locked** — see the type docs.
    pub fn write_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            self.state ^= b as u64;
            self.state = self.state.wrapping_mul(FNV1A_PRIME);
        }
        self
    }

    /// Fold a whole 64-bit word in one step. For numeric fields, feed
    /// `x.to_bits()` (floats) or the value directly (ints) — this is the idiom
    /// for "hash, don't serialize" structured cache keys.
    // TODO(multiplayer): deferred — singleplayer focus for now, RBAC disabled for
    // ease of debugging. Nothing at the type level stops a caller from building a
    // wire-identity (tier-2) hash via this endianness-dependent path instead of
    // `write_bytes`; on a big-endian peer such ids would silently diverge.
    // Deferred fix: a `WireKey` newtype populatable only via `write_bytes`.
    // Revisit before multiplayer hardening
    // (INDEPENDENT-REVIEW-2026-07-19_agy.md HASH-1).
    pub fn write_u64(&mut self, x: u64) -> &mut Self {
        self.state ^= x;
        self.state = self.state.wrapping_mul(FNV1A_PRIME);
        self
    }

    /// The 64-bit digest of everything written so far.
    pub fn finish(&self) -> u64 {
        self.state
    }
}

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

/// Byte-wise FNV-1a 64-bit of `bytes` in one call — the canonical, wire-locked
/// fast hash. Equivalent to `Fnv1a::new().write_bytes(bytes).finish()`.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = Fnv1a::new();
    h.write_bytes(bytes);
    h.finish()
}

/// Content-addressing tier — CIDv1 (`raw` codec `0x55` + sha2-256 multihash),
/// i.e. exactly what `ipfs add --raw-leaves --cid-version 1 <file>` produces for
/// a single-block file. This is the **cross-peer, persisted** address of a
/// blob's bytes: the same content yields the same [`Cid`] on every peer with zero
/// coordination, and `ipfs get`/`pin add` resolve it. Use for on-disk precompute
/// cache entries and on-wire asset transfer — never as a per-frame change key
/// (that's the fast tier above).
#[cfg(feature = "cid")]
pub mod content {
    use multihash_codetable::{Code, MultihashDigest};

    /// Re-exported so consumers address content without depending on the `cid`
    /// crate directly.
    pub use cid::{Cid, Version};

    /// IPLD codec for a raw byte block (`--raw-leaves` single-block identity).
    pub const RAW_CODEC: u64 = 0x55;

    /// Build the CIDv1 (`raw` + sha2-256) content address of `bytes`.
    pub fn cid(bytes: &[u8]) -> Cid {
        Cid::new_v1(RAW_CODEC, Code::Sha2_256.digest(bytes))
    }

    /// Parse canonical CID bytes (`Cid::to_bytes()`, as carried on the wire /
    /// stored in a manifest) back into a [`Cid`]. `None` on malformed input —
    /// callers treat a bad CID as "unknown content", never a panic.
    pub fn cid_from_bytes(bytes: &[u8]) -> Option<Cid> {
        Cid::try_from(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_offset_basis() {
        assert_eq!(fnv1a64(b""), FNV1A_OFFSET_BASIS);
    }

    #[test]
    fn known_vector_is_stable() {
        // FNV-1a 64 of "a" = basis ^ 'a' then × prime. Locks the constants +
        // byte-wise order so a refactor can't silently shift the wire identity.
        let expected = (FNV1A_OFFSET_BASIS ^ (b'a' as u64)).wrapping_mul(FNV1A_PRIME);
        assert_eq!(fnv1a64(b"a"), expected);
    }

    #[test]
    fn streaming_matches_oneshot() {
        let mut h = Fnv1a::new();
        h.write_bytes(b"lun").write_bytes(b"co");
        assert_eq!(h.finish(), fnv1a64(b"lunco"));
    }

    #[test]
    fn write_u64_is_xor_multiply_fold() {
        // The word-wise fold terrain's `cache_key` relies on: xor the whole word,
        // then multiply. Distinct from `write_bytes(&x.to_ne_bytes())`.
        let mut h = Fnv1a::new();
        h.write_u64(42);
        assert_eq!(
            h.finish(),
            (FNV1A_OFFSET_BASIS ^ 42).wrapping_mul(FNV1A_PRIME)
        );
    }

    #[cfg(feature = "cid")]
    #[test]
    fn cid_is_deterministic_and_round_trips() {
        let a = content::cid(b"hello");
        let b = content::cid(b"hello");
        assert_eq!(a, b);
        assert_eq!(content::cid_from_bytes(&a.to_bytes()), Some(a));
        assert_ne!(content::cid(b"hello"), content::cid(b"world"));
    }
}
