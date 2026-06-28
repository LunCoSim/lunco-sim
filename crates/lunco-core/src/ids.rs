//! Shared 53-bit time-sorted id generator.
//!
//! Used by [`crate::GlobalEntityId`] (entity identity) and
//! [`crate::commands::OpId`] (event identity). Both are newtype-distinct
//! so the compiler keeps event-ids and entity-ids from being mixed up;
//! the generator that produces the underlying `u64` is shared.
//!
//! - 32 bits: seconds since the LunCo epoch (2025-01-01 00:00:00 UTC)
//! - 21 bits: random instance id + monotonic sequence within the second
//!
//! The full 53 bits fit losslessly in a JS `Number` so ids can travel
//! through JSON to the MCP / web client without precision loss.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use web_time::{SystemTime, UNIX_EPOCH};

/// LunCo epoch: 2025-01-01 00:00:00 UTC.
const LUNCO_EPOCH_SECS: u64 = 1735689600;

/// Generate a fresh 53-bit time-sorted id. Monotonic within a single
/// process; disjoint across processes thanks to the random instance bits,
/// which are drawn from a per-process stream seeded **once at startup**
/// from real OS/browser entropy (see [`rand_entropy`]). Two processes that
/// mint their first id in the same second therefore pick different instance
/// bits instead of colliding.
pub fn make_id_53() -> u64 {
    static LAST_ID: AtomicU64 = AtomicU64::new(0);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let timestamp = now_secs.saturating_sub(LUNCO_EPOCH_SECS) & 0xFFFFFFFF;
    let id_base = timestamp << 21;

    loop {
        let last = LAST_ID.load(Ordering::Relaxed);
        let last_ts = last >> 21;

        let next = if last_ts == timestamp {
            (last + 1) & 0x1FFFFFFFFFFFFF
        } else {
            id_base | (rand_entropy() & 0x1FFFFF)
        };

        if LAST_ID
            .compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return next;
        }
    }
}

/// Per-process random seed, computed **once** from real entropy. This is the
/// load-bearing fix for cross-process id uniqueness: a fixed constant made two
/// freshly-started processes mint identical first-of-the-second ids, which the
/// networking dedup then dropped as a "duplicate" (silent multiplayer data
/// loss). Distinct processes now start from distinct seeds.
///
/// wasm-safe: `getrandom` 0.2 is built with its `js` feature (workspace dep), so
/// on `wasm32` it draws from `crypto.getRandomValues`; on native it reads OS
/// entropy. If getrandom ever fails we fall back to `SystemTime` nanos XOR the
/// process id — and `process::id()` is gated to native only (it doesn't exist on
/// wasm), where the time source alone still differs per browser-tab/worker.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut buf = [0u8; 8];
        if getrandom_02::getrandom(&mut buf).is_ok() {
            return u64::from_le_bytes(buf);
        }
        // Fallback: time-nanos XOR process id (native) / golden-ratio salt.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        nanos ^ process_id_entropy().rotate_left(32) ^ 0x9E37_79B9_7F4A_7C15
    })
}

#[cfg(not(target_family = "wasm"))]
fn process_id_entropy() -> u64 {
    std::process::id() as u64
}

#[cfg(target_family = "wasm")]
fn process_id_entropy() -> u64 {
    // No process id on wasm; the SystemTime nanos above still differ per tab.
    0
}

/// SplitMix64 stream seeded once from [`process_seed`]. Each call advances the
/// shared atomic and mixes, so the instance bits used by [`make_id_53`] are
/// unique per process (the seed) and per call within a second (the counter).
fn rand_entropy() -> u64 {
    static STATE: OnceLock<AtomicU64> = OnceLock::new();
    let state = STATE.get_or_init(|| AtomicU64::new(process_seed()));
    let mut z = state
        .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
