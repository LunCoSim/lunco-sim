//! Typed scene-command payloads shared by producers and runtime handlers.
//!
//! Editor packages depend on these stable event contracts without depending
//! on the scene mutation handlers. `lunco-scene-commands` owns observer
//! behavior and registers these types with the runtime command API.

use bevy::prelude::{Entity, Reflect, ReflectDefault, ReflectEvent};
use lunco_core::{Command, EditIntent, serde};

/// A value for a presentation property registered by its runtime owner.
#[derive(Clone, Debug, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(crate = "lunco_core::serde")]
pub enum SceneViewValue {
    /// UTF-8 presentation text.
    Text { value: String },
    /// Withdraw the runtime value and use the authored presentation again.
    Clear,
}

/// One owner-validated property value.
#[derive(Clone, Debug, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(crate = "lunco_core::serde")]
pub struct SceneViewWrite {
    /// Public property registered by its presentation owner.
    pub property: String,
    /// Typed runtime value.
    pub value: SceneViewValue,
}

/// Presentation writes for one live entity.
#[derive(Clone, Debug, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(crate = "lunco_core::serde")]
pub struct SceneViewBatch {
    /// Live target, resolved through the command's entity authority checks.
    #[reflect(@lunco_core::AuthzTarget)]
    pub target: Entity,
    /// Ordered writes to this entity's registered presentation properties.
    pub writes: Vec<SceneViewWrite>,
}

/// Apply a batch of runtime presentation values to the mounted scene.
///
/// The entire batch is validated before publication. Repeated writes to one
/// property on one entity use the last value in the batch.
#[Command]
pub struct SetSceneView {
    /// Batches for distinct live entities, in submission order.
    pub batches: Vec<SceneViewBatch>,
}

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
