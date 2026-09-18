//! Renderer-independent dock layout state for the workbench shell.
//!
//! The concrete egui renderer remains in `lunco-workbench`; this package owns
//! the larger, reusable state machine that materializes perspectives, restores
//! dock snapshots, and tracks panel placement. It depends only on workbench
//! contracts/state plus egui-dock's data model, so changes to menus, viewport
//! synchronization, and Bevy rendering do not rebuild this layout owner.

pub mod layout;
mod perspective;

pub use layout::{WorkbenchLayout, WorkbenchLayoutStateProvider};
pub use perspective::sync_scene_interaction_mode;

/// First leaf node in a dock surface.
pub fn first_leaf(
    surface: &mut egui_dock::Tree<lunco_workbench_core::TabId>,
) -> Option<egui_dock::NodeIndex> {
    for (index, node) in surface.iter_mut().enumerate() {
        if node.is_leaf() {
            return Some(egui_dock::NodeIndex(index));
        }
    }
    None
}

/// First leaf containing a tab accepted by `pred`.
pub fn find_leaf_matching<F>(
    surface: &mut egui_dock::Tree<lunco_workbench_core::TabId>,
    pred: F,
) -> Option<egui_dock::NodeIndex>
where
    F: Fn(&lunco_workbench_core::TabId) -> bool,
{
    for (index, node) in surface.iter_mut().enumerate() {
        if node.is_leaf() {
            if let Some(tabs) = node.tabs() {
                if tabs.iter().any(&pred) {
                    return Some(egui_dock::NodeIndex(index));
                }
            }
        }
    }
    None
}

/// Heal non-finite values serialized by egui-dock as JSON nulls.
pub fn heal_non_finite_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                match (key.as_str(), value.is_null()) {
                    ("fraction", true) => *value = serde_json::json!(0.5),
                    ("x" | "y", true) => *value = serde_json::json!(0.0),
                    _ => heal_non_finite_nulls(value),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(heal_non_finite_nulls),
        _ => {}
    }
}

/// Clamp or repair every split fraction before a dock is rendered.
pub fn sanitize_dock_fractions(dock: &mut egui_dock::DockState<lunco_workbench_core::TabId>) {
    for (_surface, node) in dock.iter_all_nodes_mut() {
        if let egui_dock::Node::Horizontal(split) | egui_dock::Node::Vertical(split) = node {
            split.fraction = if split.fraction.is_finite() {
                split.fraction.clamp(0.01, 0.99)
            } else {
                0.5
            };
        }
    }
}
