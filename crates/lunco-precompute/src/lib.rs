//! Content-addressed precompute disk cache — Substrate B.
//!
//! One place expensive **pure** derivations get computed once and reused: run
//! `bake(input) -> output`, persist the output under a key that is a hash of the
//! *content + parameters*, and on every later run — or every peer — load it back
//! instead of recomputing. This is the tier-3 rung of the derived-data ladder
//! (RAM memo → [`lunco_hash::Fnv1a`] change-compiled resource → **this**).
//!
//! ## Contract
//!
//! - **Pure input → pure output.** The key must capture *everything* the bake
//!   reads. If two inputs hash equal, their outputs must be interchangeable —
//!   that is what makes the entry shareable across runs and peers with zero
//!   coordination (byte-identical bake ⇒ byte-identical key ⇒ cache hit).
//! - **Determinism firewall.** NEVER cache stateful integrator / solver output
//!   or anything clock-dependent — that content isn't a pure function of a
//!   hashable input, so a "hit" would serve stale physics. This tier is for
//!   *structure* (meshes, textures, AO/normal layers, flattened stages,
//!   colliders), never *state*.
//! - **Best-effort persistence.** A cache write that fails only costs a rebake
//!   next time; [`bake_or_load`] never fails because of I/O.
//!
//! ## Keys
//!
//! The key is a `u64` from the fast tier ([`lunco_hash::Fnv1a`]) — cheap enough
//! to fold large buffers (DEM heights, vertex arrays) every call. Fold a
//! **format version first** so a change to the bake math or on-disk layout
//! invalidates every stale entry (content-addressed → no explicit purge). For a
//! blob that must be addressed *cross-peer* (streamed over the wire), also take
//! its [`blob_cid`] (feature `cid`) — the fast key is process-local, the CID is
//! the IPFS-interop identity. See `docs/architecture/hashing-substrate.md`.
//!
//! ## Layout
//!
//! Entries live at `<root>/<namespace>/<key-hex>/…`, where `root` is the app's
//! `cache://` dir (`lunco_assets::cache_dir()`, passed in so this crate stays
//! runtime-agnostic) and `namespace` is a `/`-separated subpath like
//! `"terrain/derived"`. An entry may hold one or several named blobs.

use std::path::{Path, PathBuf};

pub use lunco_hash::{fnv1a64, Fnv1a};
pub use lunco_storage::{StorageError, StorageResult};

/// Cross-peer content address of a baked blob (CIDv1 raw + sha2-256). Use for an
/// entry that leaves this process — a scenario-distributed asset — so peers dedup
/// and verify by identity. A purely local cache never needs it (the fast key
/// suffices), which is why it is behind the `cid` feature.
#[cfg(feature = "cid")]
pub use lunco_hash::content::{cid as blob_cid, Cid};

/// A cacheable precompute: a pure `bake` plus how to persist/restore its output.
///
/// Implement this for the *thing being baked* (capturing its input by value or
/// reference), then call [`bake_or_load`]. The three "how to cache" methods are
/// deliberately explicit so a multi-blob artifact (e.g. terrain's surface +
/// normal maps) and a single-blob one share the same orchestration without the
/// crate guessing a serialization format.
pub trait Bake {
    /// The baked artifact returned to the caller (hit or miss alike).
    type Output;

    /// Cache subdir, `/`-separated, e.g. `"terrain/derived"`. Groups an entry
    /// family so they can be reasoned about / evicted together.
    const NAMESPACE: &'static str;

    /// Fold the content + parameters into the entry key via [`Fnv1a`]. MUST
    /// include everything the bake reads, and should fold a format version first
    /// so a bake/layout change invalidates old entries. Cheap by design — it
    /// runs on every call, hit or miss.
    fn key(&self) -> u64;

    /// The expensive pure computation. Runs **only on a cache miss**.
    fn bake(&self) -> Self::Output;

    /// Persist `out` into the entry directory `dir` (already namespaced + keyed)
    /// through the Storage API — typically one or more [`store_blob`] calls.
    /// Best-effort: the returned error is logged/ignored by the orchestrator.
    fn store(dir: &Path, out: &Self::Output) -> StorageResult<()>;

    /// Restore a previously-stored output from `dir`, **validating integrity**
    /// (sizes/shape) so a truncated or partial write is treated as a miss.
    /// `None` → miss/corrupt → the orchestrator rebakes.
    fn load(dir: &Path) -> Option<Self::Output>;
}

/// Load `bake`'s artifact from the content-addressed cache under `root`, or bake
/// it (and write it through) on a miss. The heart of Substrate B.
///
/// `root` is the app's cache root (`lunco_assets::cache_dir()`), passed in so
/// this crate needs no bevy/asset dependency. Compute it once before spawning an
/// off-thread bake and move it into the task.
pub fn bake_or_load<B: Bake>(bake: &B, root: &Path) -> B::Output {
    let dir = entry_dir(root, B::NAMESPACE, bake.key());
    if let Some(out) = B::load(&dir) {
        return out;
    }
    let out = bake.bake();
    // Best-effort: a failed write just means we rebake next time — never fail the
    // caller for a cache miss we *did* satisfy by baking.
    let _ = B::store(&dir, &out);
    out
}

/// The 16-hex-digit rendering of a `u64` key — the on-disk entry directory name.
pub fn key_hex(key: u64) -> String {
    format!("{key:016x}")
}

/// Resolve an entry directory: `<root>/<namespace segments…>/<key-hex>`.
pub fn entry_dir(root: &Path, namespace: &str, key: u64) -> PathBuf {
    let mut dir = root.to_path_buf();
    for seg in namespace.split('/').filter(|s| !s.is_empty()) {
        dir.push(seg);
    }
    dir.push(key_hex(key));
    dir
}

/// Write one named blob into an entry directory (through the Storage API, which
/// creates parent dirs). Use from a [`Bake::store`] impl.
///
/// **Native-only tier**: on wasm this is a no-op — `write_file_sync` would
/// route multi-MB blobs into `localStorage`-as-hex (2× size, quota-bound).
/// The wasm cache tier is `lunco_storage::opfs_blob` (async), which consumers
/// integrate at their own async seams; the sync bake APIs can't await OPFS.
pub fn store_blob(dir: &Path, name: &str, bytes: &[u8]) -> StorageResult<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        lunco_storage::write_file_sync(&dir.join(name), bytes)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (dir, name, bytes);
        Ok(())
    }
}

/// Read one named blob from an entry directory. `None` on any miss/error → the
/// caller treats it as a cache miss. Use from a [`Bake::load`] impl.
///
/// **Native-only tier** — always `None` (a miss) on wasm; see [`store_blob`].
pub fn load_blob(dir: &Path, name: &str) -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        lunco_storage::read_file_sync(&dir.join(name)).ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (dir, name);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A trivial bake whose output is `[n; 4]`, counting how many times it
    /// actually ran so we can prove the second call hits disk.
    struct Repeat<'a> {
        n: u64,
        bakes: &'a Cell<u32>,
    }

    impl Bake for Repeat<'_> {
        type Output = Vec<u8>;
        const NAMESPACE: &'static str = "test/repeat";
        fn key(&self) -> u64 {
            // Fold a format version first, then the content — the documented idiom.
            let mut h = Fnv1a::new();
            h.write_u64(1).write_u64(self.n);
            h.finish()
        }
        fn bake(&self) -> Vec<u8> {
            self.bakes.set(self.bakes.get() + 1);
            vec![self.n as u8; 4]
        }
        fn store(dir: &Path, out: &Vec<u8>) -> StorageResult<()> {
            store_blob(dir, "data.bin", out)
        }
        fn load(dir: &Path) -> Option<Vec<u8>> {
            load_blob(dir, "data.bin")
        }
    }

    #[test]
    fn entry_dir_is_namespaced_and_keyed() {
        let d = entry_dir(Path::new("/cache"), "terrain/derived", 0xabcd);
        assert_eq!(d, Path::new("/cache/terrain/derived/000000000000abcd"));
    }

    #[test]
    fn bakes_once_then_hits_cache_across_calls() {
        let root = std::env::temp_dir().join("lunco-precompute-test-repeat");
        let _ = std::fs::remove_dir_all(&root); // clean slate

        let bakes = Cell::new(0);
        let first = bake_or_load(
            &Repeat {
                n: 9,
                bakes: &bakes,
            },
            &root,
        );
        let second = bake_or_load(
            &Repeat {
                n: 9,
                bakes: &bakes,
            },
            &root,
        );

        assert_eq!(first, second);
        assert_eq!(first, vec![9u8; 4]);
        assert_eq!(
            bakes.get(),
            1,
            "second call must load from disk, not rebake"
        );

        // A different input is a distinct entry → bakes again.
        let _other = bake_or_load(
            &Repeat {
                n: 10,
                bakes: &bakes,
            },
            &root,
        );
        assert_eq!(bakes.get(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }
}
