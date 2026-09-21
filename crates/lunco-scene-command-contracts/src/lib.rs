//! Typed scene-command payloads shared by producers and runtime handlers.
//!
//! Editor packages depend on these stable event contracts without depending
//! on the scene mutation handlers. `lunco-scene-commands` owns observer
//! behavior and registers these types with the runtime command API.

use bevy::prelude::{Entity, ReflectDefault, ReflectEvent};
use lunco_core::{Command, EditIntent};

/// Move a scene entity to a translation in the active physics frame.
///
/// The runtime resolves the stable API entity id and applies this as an
/// authoritative scene edit. Physics bodies use the scene handler's bounded
/// kinematic move contract; the translation remains f64 across this boundary.
#[Command(default)]
pub struct MoveEntity {
    /// API-stable global entity ID from `ListEntities`.
    pub entity_id: u64,
    /// Target translation in the active physics frame, in f64 precision.
    pub translation: [f64; 3],
}

/// Set a scene entity's complete pose in the active physics frame.
///
/// Translation and orientation are one authored edit, allowing live seating
/// and document persistence to share one command and change-set boundary.
#[Command(default)]
pub struct TransformEntity {
    /// API-stable global entity ID from `ListEntities`.
    pub entity_id: u64,
    /// Target translation in the active physics frame.
    pub translation: [f64; 3],
    /// Target orientation in the active physics frame, `[x, y, z, w]`.
    pub rotation: [f64; 4],
}

/// Remove a scene entity, optionally persisting the edit to its USD document.
///
/// Persistent intent authors a journaled, undoable runtime-layer edit. A
/// runtime-only prim is removed; a composed or referenced prim is deactivated
/// in the runtime layer so its source layer remains intact. Interactive intent
/// removes only the live entity and leaves the document unchanged.
#[Command]
pub struct DeleteEntity {
    /// Entity to remove.
    pub target: Entity,
    /// `Persistent` (the default) authors the edit; `Interactive` is live-only.
    #[serde(default)]
    #[reflect(default)]
    pub intent: EditIntent,
}
