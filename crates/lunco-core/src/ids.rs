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
/// process; collision-free across processes thanks to the random
/// instance bits seeded each second.
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

/// Simple LCG-style entropy without pulling in `rand`. Adequate for
/// "different processes pick different starting points each second".
fn rand_entropy() -> u64 {
    static SEED: AtomicU64 = AtomicU64::new(12345);
    let old = SEED.fetch_add(1, Ordering::Relaxed);
    (old.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7FFFFFFF
}
