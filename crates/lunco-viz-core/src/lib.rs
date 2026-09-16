//! Render-free visualization identifiers.
//!
//! Rendering, panel hosting, and visualization implementations live in
//! `lunco-viz`. This package keeps the stable identifier usable by headless
//! registries and UI state without pulling the renderer stack.

use serde::{Deserialize, Serialize};

/// Unique identifier for one live visualization instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct VizId(pub u64);

impl VizId {
    /// Allocate the next process-local identifier.
    pub fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Return the serialized identifier.
    pub fn raw(self) -> u64 {
        self.0
    }
}
