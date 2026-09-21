//! Shared cursor-interaction contracts between editor producers and runtime consumers.
//!
//! The contracts are independent of the editor implementation: an editor
//! publishes the current drag state and affected-entity marker, while camera,
//! possession, and follow runtimes decide how to stand down.

use bevy::prelude::*;

/// Resource indicating that an entity transform is being dragged.
#[derive(Resource, Default)]
pub struct DragModeActive {
    /// Whether a transform-gizmo drag is active.
    pub active: bool,
}

/// Marker for an entity currently being moved by an editor transform gizmo.
#[derive(Component, Default)]
pub struct GizmoDragging;

/// Button-specific interaction intent authored by a USD prim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PointerInteraction {
    /// The prim is the normal target and blocks lower hits.
    #[default]
    Block,
    /// The prim may receive an event, but does not block geometry behind it.
    PassThrough,
    /// The prim remains a target for a context-menu observer.
    Context,
}

/// USD-authored pointer behavior for a scene prim or its visual mesh.
#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScenePointerPolicy {
    /// Behavior for the primary pointer button.
    pub left: PointerInteraction,
    /// Behavior for the secondary pointer button.
    pub right: PointerInteraction,
}

impl ScenePointerPolicy {
    /// Parse the stable vocabulary used by USD interaction attributes.
    /// Unknown values fail closed to `Block`, so a typo cannot make an object
    /// accidentally click-through.
    pub fn from_usd(left: Option<&str>, right: Option<&str>) -> Option<Self> {
        fn parse(value: Option<&str>) -> Option<PointerInteraction> {
            match value {
                Some("pass_through") => Some(PointerInteraction::PassThrough),
                Some("context") => Some(PointerInteraction::Context),
                Some("block") | Some(_) => Some(PointerInteraction::Block),
                None => None,
            }
        }

        let left = parse(left);
        let right = parse(right);
        (left.is_some() || right.is_some()).then(|| Self {
            left: left.unwrap_or_default(),
            right: right.unwrap_or_default(),
        })
    }
}

/// Marker resource indicating that a click-to-place spawn tool is armed.
#[derive(Resource, Default)]
pub struct SpawnToolActive(pub bool);

/// Which subsystem owns primary scene clicks for the active workbench mode.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneInteractionMode {
    /// Plain scene clicks may claim a controllable endpoint.
    #[default]
    Simulation,
    /// Plain scene clicks belong to editor selection and manipulation.
    Editor,
}

impl SceneInteractionMode {
    /// Stable semantic name exposed to authored interaction policies.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Simulation => "simulation",
            Self::Editor => "editor",
        }
    }

    /// Whether the configured semantic intents assign this click exclusively
    /// to the simulation's ordinary selection/possession gesture.
    pub fn possession_owns_pointer(self, intents: &[String]) -> bool {
        matches!(self, Self::Simulation)
            && intents.len() == 1
            && intents[0] == "selection.replace"
    }
}

/// Marker resource indicating that a terrain-sculpt tool is armed.
#[derive(Resource, Default)]
pub struct TerrainToolActive(pub bool);

/// The script-authored click tool currently armed, by tool name, or `None`.
#[derive(Resource, Default)]
pub struct ArmedScriptTool(pub Option<String>);

impl ArmedScriptTool {
    /// Whether any script tool is armed.
    pub fn armed(&self) -> bool {
        self.0.is_some()
    }

    /// Whether `name` is the armed tool.
    pub fn is(&self, name: &str) -> bool {
        self.0.as_deref() == Some(name)
    }
}

/// Shared gate for cursor-driven editor modes.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CursorModeActive<'w> {
    spawn_tool: Option<Res<'w, SpawnToolActive>>,
    terrain_tool: Option<Res<'w, TerrainToolActive>>,
    script_tool: Option<Res<'w, ArmedScriptTool>>,
}

impl CursorModeActive<'_> {
    /// True while any editor mode is using the cursor.
    pub fn any(&self) -> bool {
        self.spawn_tool.as_ref().is_some_and(|tool| tool.0)
            || self.terrain_tool.as_ref().is_some_and(|tool| tool.0)
            || self.script_tool.as_ref().is_some_and(|tool| tool.armed())
    }
}

#[cfg(test)]
mod tests {
    use super::{PointerInteraction, SceneInteractionMode, ScenePointerPolicy};

    #[test]
    fn scene_pointer_policy_has_fail_safe_usd_semantics() {
        assert_eq!(
            ScenePointerPolicy::from_usd(Some("pass_through"), Some("context")),
            Some(ScenePointerPolicy {
                left: PointerInteraction::PassThrough,
                right: PointerInteraction::Context,
            })
        );
        assert_eq!(ScenePointerPolicy::from_usd(None, None), None);
        assert_eq!(
            ScenePointerPolicy::from_usd(Some("typo"), None),
            Some(ScenePointerPolicy {
                left: PointerInteraction::Block,
                right: PointerInteraction::Block,
            })
        );
    }

    #[test]
    fn possession_requires_the_exclusive_semantic_pointer_intent() {
        let selection = vec!["selection.replace".to_string()];
        let overlapping = vec![
            "selection.replace".to_string(),
            "route.add_point".to_string(),
        ];
        let route = vec!["route.add_point".to_string()];
        assert!(SceneInteractionMode::Simulation.possession_owns_pointer(&selection));
        assert!(!SceneInteractionMode::Simulation.possession_owns_pointer(&overlapping));
        assert!(!SceneInteractionMode::Simulation.possession_owns_pointer(&route));
        assert!(!SceneInteractionMode::Simulation.possession_owns_pointer(&[]));
        assert!(!SceneInteractionMode::Editor.possession_owns_pointer(&selection));
    }
}
