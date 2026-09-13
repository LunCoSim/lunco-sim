//! Typed USD document mutation commands.
//!
//! These are the shared command contracts for applying authored USD operations.
//! The runtime observer lives in `lunco-usd`; keeping the data types here lets
//! scene and authoring packages submit USD edits without depending on the
//! aggregate runtime package.

use crate::document::UsdOp;
use bevy::ecs::reflect::ReflectEvent;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;
use lunco_doc::DocumentId;

/// Apply one [`UsdOp`] to a document through the typed command bus.
///
/// The `lunco-usd` runtime observes this command and routes it through the
/// document registry so undo/redo, change notification, and read-only
/// enforcement remain centralized there.
#[Command(default)]
pub struct ApplyUsdOp {
    /// Target document.
    pub doc_id: DocumentId,
    /// Generation the caller edited from. When present, the operation is
    /// rejected if the document advanced before it arrived.
    pub parent_gen: Option<u64>,
    /// Operation to apply.
    pub op: UsdOp,
}

/// Apply one authored intent consisting of several USD operations.
///
/// The `lunco-usd` runtime journals this list as one undo unit and observes it
/// only after the document reaches its complete shape.
#[Command(default)]
pub struct ApplyUsdOps {
    /// Target document.
    pub doc_id: DocumentId,
    /// Generation the caller edited from. When present, the complete compound
    /// edit is rejected if the document advanced before it arrived.
    pub parent_gen: Option<u64>,
    /// Human-readable undo/journal label.
    pub label: String,
    /// Ordered primitive USD operations comprising the one intent.
    pub ops: Vec<UsdOp>,
}
