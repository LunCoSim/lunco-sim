//! Measured viewport geometry shared by rendered UI consumers.

use std::collections::HashMap;

use bevy::prelude::{Resource, UVec2};
use egui::{Context, Rect, Ui};

use crate::PanelId;

/// Stable id for the main workbench viewport panel.
pub const VIEWPORT_PANEL_ID: PanelId = PanelId("workbench::viewport");

/// Per-panel screen-space rect, in physical pixels.
///
/// The concrete workbench records these measurements while rendering its dock
/// panels. Consumers use them for camera/image sizing and screen-space
/// overlays without accessing the concrete dock tree.
#[derive(Resource, Default, Debug)]
pub struct PanelRects {
    rects: HashMap<PanelId, PanelRect>,
    instance_rects: HashMap<(PanelId, u64), PanelRect>,
}

/// One panel's footprint inside the window, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct PanelRect {
    /// Top-left of the panel rect inside the window framebuffer.
    pub origin: UVec2,
    /// Width × height of the rect. It is never zero.
    pub size: UVec2,
}

impl PanelRect {
    /// Convert this physical-pixel footprint to egui's logical point space.
    pub fn to_egui_rect(self, ctx: &Context) -> Rect {
        let ppp = ctx.pixels_per_point().max(f32::EPSILON);
        Rect::from_min_max(
            egui::pos2(self.origin.x as f32 / ppp, self.origin.y as f32 / ppp),
            egui::pos2(
                self.origin.x.saturating_add(self.size.x) as f32 / ppp,
                self.origin.y.saturating_add(self.size.y) as f32 / ppp,
            ),
        )
    }
}

impl PanelRects {
    /// Drop every recorded rect before the next egui pass repopulates them.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.instance_rects.clear();
    }

    /// Compute a physical-pixel rect from an egui UI without touching the
    /// world. The origin is floored and the far edge is ceiled so the result
    /// fully covers the logical panel at non-integer device pixel ratios.
    pub fn panel_rect_from_ui(ui: &Ui) -> PanelRect {
        let rect = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        let origin = UVec2::new(
            (rect.min.x.max(0.0) * ppp).floor() as u32,
            (rect.min.y.max(0.0) * ppp).floor() as u32,
        );
        let end = UVec2::new(
            (rect.max.x.max(0.0) * ppp).ceil() as u32,
            (rect.max.y.max(0.0) * ppp).ceil() as u32,
        );
        let size = UVec2::new(
            end.x.saturating_sub(origin.x).max(1),
            end.y.saturating_sub(origin.y).max(1),
        );
        PanelRect { origin, size }
    }

    /// Record a precomputed singleton panel rect.
    pub fn record(&mut self, panel: PanelId, rect: PanelRect) {
        self.rects.insert(panel, rect);
    }

    /// Record the rect of one multi-instance panel tab.
    pub fn record_instance(&mut self, panel: PanelId, instance: u64, rect: PanelRect) {
        self.instance_rects.insert((panel, instance), rect);
    }

    /// Look up a singleton panel's most recently recorded rect.
    pub fn get(&self, panel: PanelId) -> Option<PanelRect> {
        self.rects.get(&panel).copied()
    }

    /// Look up one multi-instance panel tab's most recently recorded rect.
    pub fn get_instance(&self, panel: PanelId, instance: u64) -> Option<PanelRect> {
        self.instance_rects.get(&(panel, instance)).copied()
    }

    /// Look up a singleton panel footprint directly in egui point space.
    pub fn egui_rect(&self, panel: PanelId, ctx: &Context) -> Option<Rect> {
        self.get(panel).map(|rect| rect.to_egui_rect(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USD_PREVIEW: PanelId = PanelId("usd::viewport");

    #[test]
    fn instance_panel_rects_are_isolated_from_singletons_and_each_other() {
        let mut rects = PanelRects::default();
        let singleton = PanelRect {
            origin: UVec2::new(1, 2),
            size: UVec2::new(3, 4),
        };
        let first = PanelRect {
            origin: UVec2::new(5, 6),
            size: UVec2::new(7, 8),
        };
        let second = PanelRect {
            origin: UVec2::new(9, 10),
            size: UVec2::new(11, 12),
        };

        rects.record(USD_PREVIEW, singleton);
        rects.record_instance(USD_PREVIEW, 1, first);
        rects.record_instance(USD_PREVIEW, 2, second);

        assert_eq!(
            rects.get(USD_PREVIEW).map(|rect| (rect.origin, rect.size)),
            Some((singleton.origin, singleton.size))
        );
        assert_eq!(
            rects
                .get_instance(USD_PREVIEW, 1)
                .map(|rect| (rect.origin, rect.size)),
            Some((first.origin, first.size))
        );
        assert_eq!(
            rects
                .get_instance(USD_PREVIEW, 2)
                .map(|rect| (rect.origin, rect.size)),
            Some((second.origin, second.size))
        );
        assert!(rects.get_instance(USD_PREVIEW, 3).is_none());

        rects.clear();
        assert!(rects.get(USD_PREVIEW).is_none());
        assert!(rects.get_instance(USD_PREVIEW, 1).is_none());
    }
}
