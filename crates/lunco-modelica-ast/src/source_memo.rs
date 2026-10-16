//! One revision for projections derived from Modelica source.
//!
//! Engine icon metadata uses [`SourceMemo`], including negative results.
//! The UI also uses [`source_epoch`] to invalidate shared asset bytes, decoded
//! images and textures together. Source or library changes call
//! [`invalidate_source_memos`]; consumers refresh lazily at the new revision.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bumped whenever the Modelica source or library changes. Process-global because
/// the memos it governs are (an egui texture cache cannot live in the ECS).
static SOURCE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Declare that the source or library changed: every [`SourceMemo`] is now stale.
///
/// Cheap — one atomic increment. The memos clear themselves lazily on next access,
/// so an invalidation that is never followed by a read costs nothing at all.
pub fn invalidate_source_memos() {
    SOURCE_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// The epoch a memo's contents belong to.
pub fn source_epoch() -> u64 {
    SOURCE_EPOCH.load(Ordering::Relaxed)
}

/// A `name → Option<V>` memo of something derived from Modelica source, **including
/// negative results**, that self-invalidates when the source changes.
///
/// `Option<V>` is the value, not a lookup outcome: `Some(None)` means "we looked and
/// there is genuinely no icon" and is a cache *hit*.
pub struct SourceMemo<V> {
    map: HashMap<String, Option<V>>,
    epoch: u64,
}

impl<V> Default for SourceMemo<V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            epoch: source_epoch(),
        }
    }
}

impl<V: Clone> SourceMemo<V> {
    /// Look up `key`, dropping the whole memo first if the source moved on.
    ///
    /// `None` → miss, compute it and call [`SourceMemo::insert`].
    /// `Some(v)` → hit, where `v` may itself be `None` (a remembered negative).
    pub fn peek(&mut self, key: &str) -> Option<Option<V>> {
        self.refresh();
        self.map.get(key).cloned()
    }

    /// Record a result — a `None` value is a remembered negative, and is exactly as
    /// valuable as a positive (it is what stops a missing asset being re-probed every
    /// frame). It stays valid until the next [`invalidate_source_memos`].
    pub fn insert(&mut self, key: &str, value: Option<V>) {
        self.refresh();
        self.map.insert(key.to_string(), value);
    }

    /// Drop everything if we are holding results from a previous epoch.
    fn refresh(&mut self) {
        let now = source_epoch();
        if now != self.epoch {
            self.map.clear();
            self.epoch = now;
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the epoch exists for: a memo filled before an invalidation must
    /// not serve its contents after one.
    #[test]
    fn invalidation_drops_a_filled_memo() {
        let mut memo: SourceMemo<u32> = SourceMemo::default();
        memo.insert("Modelica.Blocks.Add", Some(7));
        assert_eq!(memo.peek("Modelica.Blocks.Add"), Some(Some(7)));

        invalidate_source_memos();
        assert_eq!(
            memo.peek("Modelica.Blocks.Add"),
            None,
            "stale epoch must miss"
        );
        assert!(memo.is_empty());
    }

    /// THE BUG THIS MODULE EXISTS FOR. A remembered *negative* must also clear —
    /// otherwise an asset that was missing (every source library bitmap on wasm today) is
    /// remembered as missing forever, and never re-probed once it lands.
    #[test]
    fn invalidation_drops_a_remembered_negative() {
        let mut memo: SourceMemo<u32> = SourceMemo::default();
        memo.insert("Missing.Icon.png", None);
        assert_eq!(
            memo.peek("Missing.Icon.png"),
            Some(None),
            "a negative is a hit"
        );

        invalidate_source_memos();
        assert_eq!(
            memo.peek("Missing.Icon.png"),
            None,
            "the negative must be retried after the source changes, not cached forever"
        );
    }

    /// Independent memos share the one signal — that is the whole point. A memo
    /// created in another module, after the fact, is invalidated by the same call
    /// without the invalidation site ever naming it.
    #[test]
    fn one_signal_invalidates_every_memo() {
        let mut icons: SourceMemo<u32> = SourceMemo::default();
        let mut textures: SourceMemo<String> = SourceMemo::default();
        icons.insert("A", Some(1));
        textures.insert("a.png", Some("tex".to_string()));

        invalidate_source_memos();

        assert_eq!(icons.peek("A"), None);
        assert_eq!(textures.peek("a.png"), None);
    }

    /// …and without an invalidation, a memo keeps serving. A steady scene must not
    /// re-run the ~80 ms inheritance walk.
    #[test]
    fn a_memo_serves_until_invalidated() {
        let mut memo: SourceMemo<u32> = SourceMemo::default();
        memo.insert("Stable", Some(3));
        assert_eq!(memo.peek("Stable"), Some(Some(3)));
        assert_eq!(memo.peek("Stable"), Some(Some(3)));
    }
}
