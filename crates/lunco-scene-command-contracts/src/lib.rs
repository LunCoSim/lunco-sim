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
    /// Boolean presentation value.
    Bool { value: bool },
    /// Non-negative linear RGB at the render boundary.
    LinearRgb { value: [f32; 3] },
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
/// authoritative live update or explicit authored edit. Physics bodies use the scene handler's bounded
/// kinematic move contract; the translation remains f64 across this boundary.
#[Command(default)]
pub struct MoveEntity {
    /// API-stable global entity ID from `ListEntities`.
    pub entity_id: u64,
    /// Target translation in the active physics frame, in f64 precision.
    pub translation: [f64; 3],
    /// Live by default; Persistent explicitly authors the accepted translation.
    #[serde(default)]
    #[reflect(default)]
    pub intent: EditIntent,
}

/// Set a scene entity's complete pose in the active physics frame.
///
/// Translation and orientation share one live pose mechanism. Persistent intent
/// additionally journals an authored edit; interactive intent never edits USD.
#[Command(default)]
pub struct TransformEntity {
    /// API-stable global entity ID from `ListEntities`.
    #[reflect(@lunco_core::AuthzTarget)]
    pub entity_id: u64,
    /// Target translation in the active physics frame.
    pub translation: [f64; 3],
    /// Target orientation in the active physics frame, `[x, y, z, w]`.
    pub rotation: [f64; 4],
    /// Interactive (default) changes only the live pose; Persistent authors the accepted pose.
    #[serde(default)]
    #[reflect(default)]
    pub intent: EditIntent,
}

/// Set the live contact participation of an already admitted native collider.
/// This changes runtime physics state only; the authored shape is unchanged.
#[Command]
pub struct SetColliderEnabled {
    /// Exact collider entity, not a source prim that has no native collider.
    #[reflect(@lunco_core::AuthzTarget)]
    pub target: Entity,
    /// Whether the collider participates in live contact detection.
    pub enabled: bool,
}

/// Requested solver and contact participation of one independent body.
#[derive(Clone, Debug, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(crate = "lunco_core::serde")]
pub struct BodyParticipation {
    #[reflect(@lunco_core::AuthzTarget)]
    pub target: Entity,
    pub active: bool,
}

/// Runtime participation of one admitted collider or radio sight-line occluder.
#[derive(Clone, Debug, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(crate = "lunco_core::serde")]
pub struct ParticipationSwitch {
    #[reflect(@lunco_core::AuthzTarget)]
    pub target: Entity,
    pub enabled: bool,
}

/// Apply one live scene transition. All destinations and their owners are
/// validated before publishing any physics, radio or presentation changes.
#[Command(default)]
pub struct SetSceneParticipation {
    /// Independent bodies and all their attached contact shapes.
    #[serde(default)]
    #[reflect(default)]
    pub bodies: Vec<BodyParticipation>,
    /// Individual admitted shapes outside the requested bodies.
    #[serde(default)]
    #[reflect(default)]
    pub colliders: Vec<ParticipationSwitch>,
    /// Admitted sight-line blockers, independent of contact shapes.
    #[serde(default)]
    #[reflect(default)]
    pub occluders: Vec<ParticipationSwitch>,
    /// Presentation writes to publish with this transition.
    #[serde(default)]
    #[reflect(default)]
    pub views: Vec<SceneViewBatch>,
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
    /// `Interactive` (default) is live-only; `Persistent` authors the edit.
    #[serde(default)]
    #[reflect(default)]
    pub intent: EditIntent,
}
