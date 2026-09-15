//! Renderer-neutral presentation state shared by workbench UI adapters.

use bevy::prelude::Resource;
use egui::Rect;
use std::collections::HashMap;

/// Screen-space rectangles of named UI landmarks, refreshed by their renderers.
///
/// Guided overlays and other presentation adapters use these stable keys to
/// point at real widgets without owning or duplicating their layout.
#[derive(Resource, Default, Debug, Clone)]
pub struct HelpAnchors {
    rects: HashMap<String, Rect>,
}

impl HelpAnchors {
    /// Publish a widget's screen rectangle under `key`.
    pub fn set(&mut self, key: impl Into<String>, rect: Rect) {
        self.rects.insert(key.into(), rect);
    }

    /// Read the most recent rectangle under `key`, if any.
    pub fn get(&self, key: &str) -> Option<Rect> {
        self.rects.get(key).copied()
    }

    /// Drop all recorded rectangles at the start of a new UI frame.
    pub fn clear(&mut self) {
        self.rects.clear();
    }
}

/// Optional empty-state text drawn over the viewport presentation region.
#[derive(Resource, Default)]
pub struct ViewportPlaceholder {
    /// Text to show, or `None` to draw nothing.
    pub message: Option<String>,
}
