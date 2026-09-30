//! Renderer-neutral presentation state shared by workbench UI adapters.

use bevy::prelude::Resource;
use egui::Rect;
use std::collections::HashMap;

use crate::{PanelCtx, PanelId};

/// Screen-space rectangles of named UI landmarks, refreshed by their renderers.
///
/// Guided overlays and other presentation adapters use these stable keys to
/// point at real widgets without owning or duplicating their layout.
#[derive(Resource, Default, Debug, Clone)]
pub struct HelpAnchors {
    rects: HashMap<String, Rect>,
    static_rects: HashMap<&'static str, Rect>,
    panel_rects: HashMap<&'static str, Rect>,
}

impl HelpAnchors {
    /// Publish a widget's screen rectangle under `key`.
    pub fn set(&mut self, key: impl Into<String>, rect: Rect) {
        self.rects.insert(key.into(), rect);
    }

    /// Publish an anchor with a compile-time key without allocating a key
    /// string on every UI pass.
    pub fn set_static(&mut self, key: &'static str, rect: Rect) {
        self.static_rects.insert(key, rect);
    }

    /// Publish a panel landmark using its stable registered identity.
    pub fn set_panel(&mut self, panel: PanelId, rect: Rect) {
        self.panel_rects.insert(panel.as_str(), rect);
    }

    /// Read the most recent rectangle under `key`, if any.
    pub fn get(&self, key: &str) -> Option<Rect> {
        self.static_rects
            .get(key)
            .or_else(|| self.rects.get(key))
            .copied()
            .or_else(|| {
                key.strip_prefix("panel.")
                    .and_then(|panel| self.panel_rects.get(panel).copied())
            })
    }

    /// Drop all recorded rectangles at the start of a new UI frame.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.static_rects.clear();
        self.panel_rects.clear();
    }
}

/// One typed application action shown in the viewport's empty state.
#[derive(Clone)]
pub struct ViewportPlaceholderAction {
    /// Human-readable button label.
    pub label: String,
    /// Short explanation shown alongside the action in the empty state.
    pub description: String,
    /// Whether this is the primary action in the empty state.
    pub primary: bool,
    /// Queue the owning application's typed command after the UI pass.
    pub activate: for<'w> fn(&mut PanelCtx<'w>),
}

/// Optional empty-state presentation drawn over the viewport region.
#[derive(Resource, Default)]
pub struct ViewportPlaceholder {
    /// Text to show, or `None` to draw nothing.
    pub message: Option<String>,
    /// Supporting guidance displayed below the current empty-state message.
    pub guidance: Option<String>,
    /// Optional typed actions contributed by the host application.
    pub actions: Vec<ViewportPlaceholderAction>,
}
