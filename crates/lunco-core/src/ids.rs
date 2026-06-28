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
use web_time::{SystemTime, UNIX_EPOCH};

/// LunCo epoch: 2025-01-01 00:00:00 UTC.
const LUNCO_EPOCH_SECS: u64 = 1735689600;

/// Generate a fresh 53-bit time-sorted id. Monotonic within a single
/// process; disjoint across processes thanks to the random instance bits,
/// which are drawn fresh from real OS/browser entropy each time the second
/// advances (see [`rand_entropy`]). Two processes that mint their first id in
/// the same second therefore pick different instance bits instead of colliding.
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

/// Fresh instance entropy for [`make_id_53`]. This is the load-bearing fix for
/// cross-process id uniqueness: a fixed constant made two freshly-started
/// processes mint identical first-of-the-second ids, which the networking dedup
/// then dropped as a "duplicate" (silent multiplayer data loss).
///
/// Called at most once per second (only when the timestamp advances — within a
/// second ids come from the monotonic counter), so we read OS/browser entropy
/// directly rather than hand-rolling a userspace PRNG. `getrandom` 0.2 is built
/// with its `js` feature (workspace dep), so this is wasm-safe:
/// `crypto.getRandomValues` on wasm, OS entropy on native. On the rare event
/// that the OS RNG fails we fall back to a salted high-resolution timestamp so
/// distinct processes still differ.
fn rand_entropy() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom_02::getrandom(&mut buf).is_ok() {
        return u64::from_le_bytes(buf);
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (nanos >> 29)
}
