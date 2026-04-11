//! Widget core — WidgetSystem trait, WidgetId, and widget\<T\>() call.
//!
//! Every UI widget is a `SystemParam` struct with a uniform
//! `widget::<T>(world, ui, id)` invocation.
//!
//! ## Why this pattern?
//!
//! LunCoSim will have 1,000s of widgets (graphs, diagrams, inspectors).
//! Naive `world.query()` every frame is O(n) per widget — unacceptable.
//! WidgetSystem caches `SystemState` per `WidgetId`, making each widget
//! O(1) after the first frame.
//!
//! See: <https://github.com/bevyengine/bevy/discussions/5522>

use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;
use bevy_egui::egui;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

// ─── WidgetId ──────────────────────────────────────────────────────────

/// Unique identifier for a widget instance.
/// Namespaced via `.with()` to enable arbitrary nesting without state collisions.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WidgetId(pub u64);

impl WidgetId {
    /// Create a root widget ID from a string key.
    pub fn new(key: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// Create a child widget ID — namespaces state from parent.
    pub fn with(&self, key: impl Hash) -> Self {
        let mut hasher = DefaultHasher::new();
        self.0.hash(&mut hasher);
        key.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// Convert to a string for use as egui widget IDs.
    pub fn to_egui_id(&self) -> String {
        format!("widget_{:x}", self.0)
    }
}

impl std::fmt::Debug for WidgetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WidgetId({:x})", self.0)
    }
}

// ─── WidgetSystem Trait ────────────────────────────────────────────────

/// The trait every UI widget implements.
///
/// This is the core abstraction — a uniform signature for all UI components
/// that gives them full ECS access while maintaining O(1) performance via
/// SystemState caching.
///
/// ## Usage
///
/// ```ignore
/// #[derive(SystemParam)]
/// struct TimeSeriesWidget<'w, 's> {
///     channels: Res<'w, ModelicaChannels>,
///     plotted:  Res<'w, PlottedVariables>,
/// }
///
/// impl WidgetSystem for TimeSeriesWidget<'_, '_> {
///     fn run(world: &mut World, state: &mut SystemState<Self>, ui: &mut egui::Ui, id: WidgetId) {
///         let mut params = state.get_mut(world);
///         // Render egui_plot with params.channels, params.plotted
///     }
/// }
/// ```
///
/// ## Performance
///
/// - **First frame**: O(n) SystemState initialization
/// - **Subsequent frames**: O(1) — state is cached by WidgetId
/// - **2,000 widgets @ 60fps**: ~12ms CPU/sec vs 6 sec/sec with naive queries
pub trait WidgetSystem: SystemParam {
    /// Render this widget's UI.
    ///
    /// # Arguments
    /// * `world` — full ECS access for queries and commands
    /// * `state` — cached SystemState (preserves Local\<T\> across frames)
    /// * `ui` — egui UI handle for rendering
    /// * `id` — unique widget identity (for state isolation and child namespacing)
    fn run(world: &mut World, state: &mut SystemState<Self>, ui: &mut egui::Ui, id: WidgetId);
}

// ─── Widget Cache ──────────────────────────────────────────────────────

/// Cached SystemState instances per WidgetId.
///
/// This is the performance optimization: each widget's SystemState is
/// initialized once and reused for the lifetime of the app.
#[derive(Resource)]
pub struct WidgetCache<T: SystemParam + 'static> {
    states: std::collections::HashMap<WidgetId, SystemState<T>>,
}

impl<T: SystemParam + 'static> Default for WidgetCache<T> {
    fn default() -> Self {
        Self {
            states: std::collections::HashMap::default(),
        }
    }
}

// ─── widget\<T\>() Call ────────────────────────────────────────────────

/// Universal widget invocation — same signature for ALL widgets.
///
/// # Performance
/// First call: O(n) SystemState initialization
/// Subsequent calls: O(1) — state is cached by WidgetId
pub fn widget<W: WidgetSystem + 'static>(world: &mut World, ui: &mut egui::Ui, id: WidgetId) {
    // Ensure the cache resource exists
    if world.get_resource::<WidgetCache<W>>().is_none() {
        world.insert_resource(WidgetCache::<W>::default());
    }

    // SAFETY: We use raw pointers to split the mutable borrow of world.
    // WidgetCache only holds SystemState handles (not ECS data), so there
    // is no overlap between what the cache holds and what the widget queries.
    let cache_ptr = world.resource_mut::<WidgetCache<W>>().as_mut() as *mut WidgetCache<W>;

    // Check if we already have a cached state
    let has_state = unsafe { (*cache_ptr).states.contains_key(&id) };

    if !has_state {
        let state = SystemState::<W>::new(world);
        unsafe { (*cache_ptr).states.insert(id, state) };
    }

    // Get the cached state and run the widget
    let state = unsafe { (*cache_ptr).states.get_mut(&id).unwrap() };
    W::run(world, state, ui, id);
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_widget_id_uniqueness() {
        let id1 = WidgetId::new("panel_a");
        let id2 = WidgetId::new("panel_b");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_widget_id_child_namespacing() {
        let parent = WidgetId::new("mission_control");
        let child_a = parent.with("celestial_bodies");
        let child_b = parent.with("spacecraft_list");
        assert_ne!(parent, child_a);
        assert_ne!(child_a, child_b);
    }

    #[test]
    fn test_widget_id_deterministic() {
        let id1 = WidgetId::new("test_key");
        let id2 = WidgetId::new("test_key");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_widget_id_child_deterministic() {
        let parent = WidgetId::new("panel");
        let child1 = parent.with("child");
        let child2 = WidgetId::new("panel").with("child");
        assert_eq!(child1, child2);
    }

    #[test]
    fn test_widget_id_egui_conversion() {
        let id = WidgetId::new("test");
        let egui_id = id.to_egui_id();
        assert!(egui_id.starts_with("widget_"));
    }
}
